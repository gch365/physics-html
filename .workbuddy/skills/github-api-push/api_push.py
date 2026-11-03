# -*- coding: utf-8 -*-
"""通过 GitHub Git Data API 增量推送本地 HEAD 到远端 master（绕过 github.com 直连阻断）。

做法：以远端当前 master 的 tree 为 base_tree，只提交相对远端的「变更文件」
（新增 / 更新 / 删除），避免一次性提交数百条 tree 条目触发 GitHub 422 超时；
最终远端 tree 与本地 HEAD 的 tree 一致（PROTECT 保护的目录除外）。
PROTECT 中的目录（网页直接写入的 biji/ 与 notes/）永远不会被本脚本删除。

优点：
- 不依赖本地 origin/master 引用与远端是否分叉（始终以真实远端 SHA 为 base）。
- 无变更时直接退出，绝不会把远端 master 回退。
"""
import base64, ctypes, datetime, json, subprocess, sys, time, urllib.request, urllib.error

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
REPO = "gch365/physics-html"
API = "https://api.github.com/repos/" + REPO

# 这些目录由网页（GitHub API）直接写入，本地仓库里没有；
# 推送时既不要删除，也不要用本地旧版本覆盖远端的更新。
PROTECT = ("biji/", "notes/")


def is_protected(path):
    return path.startswith(PROTECT)


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
        if is_protected(path):
            continue        # 保护目录：远端有、本地没有，属正常情况，绝不删除
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
        entries.append({"path": path, "mode": "100644", "sha": None})  # 删除须 sha=null 且带合法 mode
    else:
        raw = subprocess.run(["git", "cat-file", "-p", sha], capture_output=True).stdout
        r = req("POST", API + "/git/blobs", {"content": base64.b64encode(raw).decode(), "encoding": "base64"})
        if r.get("sha") != sha:
            print("  note blob sha diff", r.get("sha", "?")[:10], sha[:10], path[:40])
        entries.append({"path": path, "mode": mode, "type": "blob", "sha": sha})

tr2 = req("POST", API + "/git/trees", {"base_tree": base_tree, "tree": entries})

# 校验：除保护目录外，远端 tree 必须与本地 HEAD 完全一致
tr2_map = tree_map_from_api(tr2["sha"])
local_np = {p: v for p, v in local.items() if not is_protected(p)}
remote_np = {p: v for p, v in tr2_map.items() if not is_protected(p)}
if local_np != remote_np:
    diff = [p for p in set(local_np) | set(remote_np) if local_np.get(p) != remote_np.get(p)]
    raise SystemExit("tree 校验失败，差异 %d 项，示例: %s" % (len(diff), diff[:8]))

kept = sorted(p for p in tr2_map if is_protected(p))
print("tree ok", tr2["sha"][:10])
print("保留保护目录文件 %d 个（不会被本次推送删除）" % len(kept))
for p in kept[:10]:
    print("   保留", p)

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

# ---------- 7. 刷新备份分支（biji/notes 的兜底快照） ----------
# 该分支只由本脚本维护，与 master 同一点位；万一 master 上的 biji/ 或 notes/
# 被别的途径删掉，还能从这里找回。失败不影响本次推送。
try:
    cur = req("GET", API + "/git/ref/heads/biji-backup", retry=1)["object"]["sha"]
    if cur != cr["sha"]:
        req("PATCH", API + "/git/refs/heads/biji-backup", {"sha": cr["sha"], "force": True}, retry=2)
        print("backup biji-backup ->", cr["sha"][:10])
    else:
        print("backup biji-backup already at", cr["sha"][:10])
except Exception as e:
    print("backup skipped:", type(e).__name__, str(e)[:80])

print("ALL DONE")
