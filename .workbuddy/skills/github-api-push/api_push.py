# -*- coding: utf-8 -*-
"""通过 GitHub Git Data API 增量推送本地 HEAD 到远端 master（绕过 github.com 直连阻断）。

做法：以远端当前 master 的 tree 为 base_tree，只提交相对远端的「变更文件」
（新增 / 更新 / 删除），避免一次性提交数百条 tree 条目触发 GitHub 422 超时；
最终远端 tree 与本地 HEAD 的 tree 完全一致（assert 校验）。

优点：
- 不依赖本地 origin/master 引用与远端是否分叉（始终以真实远端 SHA 为 base）。
- 无变更时直接退出，绝不会把远端 master 回退。
"""
import base64, ctypes, datetime, json, subprocess, sys, time, urllib.request, urllib.error

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
REPO = "gch365/physics-html"
API = "https://api.github.com/repos/" + REPO


def req(method, url, payload=None, retry=6):
    data = json.dumps(payload).encode() if payload is not None else None
    last = None
    for i in range(retry):
        try:
            r = urllib.request.Request(url, data=data, method=method)
            r.add_header("Authorization", "token " + tok)
            r.add_header("Accept", "application/vnd.github+json")
            r.add_header("User-Agent", "gch365-push")
            with urllib.request.urlopen(r, timeout=120) as resp:
                b = resp.read()
            return json.loads(b) if b else None
        except Exception as e:
            last = e
            print("  retry", i, type(e).__name__, str(e)[:90])
            time.sleep(1.5 * (i + 1))
    raise last


# ---------- 1. 从 Windows 凭据库读 token ----------
adv = ctypes.WinDLL("advapi32")
cred = ctypes.c_void_p()
if not adv.CredReadW("git:https://github.com", 1, 0, ctypes.byref(cred)):
    sys.exit("CredReadW failed: %d" % ctypes.GetLastError())
class CRED(ctypes.Structure):
    _fields_ = [("Flags", ctypes.c_int), ("Type", ctypes.c_int), ("TargetName", ctypes.c_wchar_p),
                ("Comment", ctypes.c_wchar_p), ("LastWritten", ctypes.c_int64),
                ("CredentialBlobSize", ctypes.c_int), ("CredentialBlob", ctypes.c_void_p),
                ("Persist", ctypes.c_int), ("AttributeCount", ctypes.c_int), ("Attributes", ctypes.c_void_p),
                ("TargetAlias", ctypes.c_wchar_p), ("UserName", ctypes.c_wchar_p)]
c = ctypes.cast(cred, ctypes.POINTER(CRED)).contents
tok = ctypes.string_at(c.CredentialBlob, c.CredentialBlobSize).decode("utf-16-le", errors="replace").replace("\x00", "").strip()
print("token:", tok[:6] + "...", "len", len(tok), "user", c.UserName)


def tree_map_from_api(tree_sha):
    tr = req("GET", API + "/git/trees/" + tree_sha + "?recursive=1")
    return {e["path"]: (e["mode"], e["sha"]) for e in tr["tree"] if e["type"] == "blob"}


def tree_map_local(ref):
    out = subprocess.run(["git", "ls-tree", "-r", "-z", ref], capture_output=True).stdout.decode("utf-8").split("\0")
    m = {}
    for e in out:
        if not e:
            continue
        meta, path = e.split("\t", 1)
        mode, typ, sha = meta.split()
        m[path] = (mode, sha)
    return m


# ---------- 2. 远端 base ----------
remote_master = req("GET", API + "/git/ref/heads/master")["object"]["sha"]
remote_commit = req("GET", API + "/git/commits/" + remote_master)
base_tree = remote_commit["tree"]["sha"]
print("remote master:", remote_master)
print("remote base_tree:", base_tree)
remote = tree_map_from_api(base_tree)

# ---------- 3. 本地 HEAD ----------
local = tree_map_local("HEAD")

changed = []  # (kind, path, mode, sha)
for path, (mode, sha) in local.items():
    if path not in remote or remote[path][1] != sha:
        changed.append(("add/update", path, mode, sha))
for path in remote:
    if path not in local:
        changed.append(("delete", path, None, None))

if not changed:
    print("远端已是最新（内容一致），无需推送，不修改远端引用。")
    subprocess.run(["git", "update-ref", "refs/remotes/origin/master", remote_master])
    print("ALL DONE (no-op)")
    sys.exit(0)

print("changed entries:", len(changed))
for kind, path, *_ in changed:
    print("   ", kind, path)

# ---------- 4. 增量建 tree ----------
entries = []
for kind, path, mode, sha in changed:
    if kind == "delete":
        entries.append({"path": path, "mode": "0"})  # 在 base_tree 上删除该文件
    else:
        raw = subprocess.run(["git", "cat-file", "-p", sha], capture_output=True).stdout
        r = req("POST", API + "/git/blobs", {"content": base64.b64encode(raw).decode(), "encoding": "base64"})
        if r.get("sha") != sha:
            print("  note blob sha diff", r.get("sha", "?")[:10], sha[:10], path[:40])
        entries.append({"path": path, "mode": mode, "type": "blob", "sha": sha})

tr2 = req("POST", API + "/git/trees", {"base_tree": base_tree, "tree": entries})
want = subprocess.run(["git", "rev-parse", "HEAD^{tree}"], capture_output=True, text=True).stdout.strip()
assert tr2["sha"] == want, "tree sha 不一致 %s != %s" % (tr2["sha"], want)
print("tree ok", tr2["sha"][:10])

# ---------- 5. 创建 commit ----------
cat = subprocess.run(["git", "cat-file", "commit", "HEAD"], capture_output=True).stdout.decode("utf-8")
def field(name):
    line = [l for l in cat.splitlines() if l.startswith(name + " ")][0]
    rest = line[len(name) + 1:]
    who, ts, tz = rest.rsplit(" ", 2)
    nm, em = who.split(" <"); em = em.rstrip(">")
    dt = datetime.datetime.fromtimestamp(int(ts), datetime.timezone(datetime.timedelta(hours=int(tz) // 100)))
    return {"name": nm, "email": em, "date": dt.isoformat()}
msg = cat.split("\n\n", 1)[1]  # 含末尾换行，保证内容一致
cr = req("POST", API + "/git/commits", {
    "message": msg, "tree": tr2["sha"], "parents": [remote_master],
    "author": field("author"), "committer": field("committer")})
print("commit created", cr["sha"][:10])

# ---------- 6. 更新远端 master ----------
r = req("PATCH", API + "/git/refs/heads/master", {"sha": cr["sha"], "force": True})
print("remote master ->", r["object"]["sha"])
subprocess.run(["git", "update-ref", "refs/remotes/origin/master", "HEAD"])
print("ALL DONE")
