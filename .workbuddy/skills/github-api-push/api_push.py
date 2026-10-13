# -*- coding: utf-8 -*-
"""通过 GitHub Git Data API 复刻本地未推送的提交（绕过 github.com 直连阻断）。
blob -> tree -> commit 全部按本地对象精确复刻，SHA 与本地一致，历史不发散。
"""
import base64, ctypes, datetime, io, json, subprocess, sys, urllib.request, urllib.error

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
REPO = "gch365/physics-html"
API = "https://api.github.com/repos/" + REPO

# ---------- 1. 从 Windows 凭据库读 token ----------
adv = ctypes.WinDLL("advapi32")
cred = ctypes.c_void_p()
if not adv.CredReadW("git:https://github.com", 1, 0, ctypes.byref(cred)):
    sys.exit("CredReadW failed: %d" % ctypes.GetLastError())
class CRED(ctypes.Structure):
    _fields_ = [("Flags",ctypes.c_int),("Type",ctypes.c_int),("TargetName",ctypes.c_wchar_p),
                ("Comment",ctypes.c_wchar_p),("LastWritten",ctypes.c_int64),
                ("CredentialBlobSize",ctypes.c_int),("CredentialBlob",ctypes.c_void_p),
                ("Persist",ctypes.c_int),("AttributeCount",ctypes.c_int),("Attributes",ctypes.c_void_p),
                ("TargetAlias",ctypes.c_wchar_p),("UserName",ctypes.c_wchar_p)]
c = ctypes.cast(cred, ctypes.POINTER(CRED)).contents
blob = ctypes.string_at(c.CredentialBlob, c.CredentialBlobSize)
tok = blob.decode("utf-16-le", errors="replace").replace("\x00", "").strip()
print("token:", tok[:6] + "...", "len", len(tok), "user", c.UserName)

def req(method, url, payload=None):
    data = json.dumps(payload).encode() if payload is not None else None
    r = urllib.request.Request(url, data=data, method=method)
    r.add_header("Authorization", "token " + tok)
    r.add_header("Accept", "application/vnd.github+json")
    r.add_header("User-Agent", "gch365-push")
    try:
        with urllib.request.urlopen(r, timeout=30) as resp:
            body = resp.read()
    except urllib.error.HTTPError as e:
        print("HTTP", e.code, e.read().decode("utf-8", "replace")[:300])
        raise
    return json.loads(body) if body else None

# ---------- 2. 校验远端 master 与本地 origin/master 一致 ----------
local_base = subprocess.run(["git","rev-parse","origin/master"], capture_output=True, text=True).stdout.strip()
remote = req("GET", API + "/git/ref/heads/master")["object"]["sha"]
print("remote master:", remote)
assert remote == local_base, "远端与本地 origin/master 不一致，停止"

# ---------- 3. 逐 commit 复刻 ----------
commits = subprocess.run(["git","rev-list","--reverse","origin/master..HEAD"],
                         capture_output=True, text=True).stdout.split()
print("commits to push:", commits)

parent = remote
for cm in commits:
    # 3a. 上传该 commit 相对父提交变更的 blob
    changed = subprocess.run(["git","diff","--name-status","-z", parent, cm],
                             capture_output=True).stdout.decode("utf-8").split("\0")
    i = 0; paths = []
    while i < len(changed) - 1:
        st, p = changed[i], changed[i+1]
        i += 2
        if st.startswith("R"):  # rename: old path at changed[i+1]
            p = changed[i]; i += 1
        if st.startswith("D"):
            continue
        paths.append(p)
    for p in paths:
        raw = subprocess.run(["git","cat-file","-p", cm + ":" + p], capture_output=True).stdout
        local_sha = subprocess.run(["git","hash-object","--stdin"], input=raw, capture_output=True).stdout.decode().strip()
        r = req("POST", API + "/git/blobs", {"content": base64.b64encode(raw).decode(), "encoding": "base64"})
        assert r["sha"] == local_sha, "blob sha 不一致 %s" % p
        print("  blob ok", local_sha[:10], p)
    # 3b. 创建 tree（全量条目，保证删除的文件不在树中）
    entries = []
    out = subprocess.run(["git","ls-tree","-r","-z", cm], capture_output=True).stdout.decode("utf-8").split("\0")
    for e in out:
        if not e: continue
        meta, path = e.split("\t", 1)
        mode, typ, sha = meta.split()
        entries.append({"path": path, "mode": mode, "type": typ, "sha": sha})
    tr = req("POST", API + "/git/trees", {"tree": entries})
    want_tree = subprocess.run(["git","rev-parse", cm + "^{tree}"], capture_output=True, text=True).stdout.strip()
    assert tr["sha"] == want_tree, "tree sha 不一致 %s: %s != %s" % (cm, tr["sha"], want_tree)
    print("  tree ok", tr["sha"][:10])
    # 3c. 创建 commit（author/committer 精确复刻）
    cat = subprocess.run(["git","cat-file","commit", cm], capture_output=True).stdout.decode("utf-8")
    def field(name):
        line = [l for l in cat.splitlines() if l.startswith(name + " ")][0]
        rest = line[len(name)+1:]
        who, ts, tz = rest.rsplit(" ", 2)[0], int(rest.rsplit(" ", 2)[1]), rest.rsplit(" ", 2)[2]
        nm, em = who.split(" <"); em = em.rstrip(">")
        dt = datetime.datetime.fromtimestamp(ts, datetime.timezone(datetime.timedelta(hours=int(tz)//100)))
        return {"name": nm, "email": em, "date": dt.isoformat()}
    msg = cat.split("\n\n", 1)[1]  # 保留原始字节（含末尾换行），保证 SHA 一致
    cr = req("POST", API + "/git/commits", {
        "message": msg, "tree": tr["sha"], "parents": [parent],
        "author": field("author"), "committer": field("committer")})
    assert cr["sha"] == cm, "commit sha 不一致 %s: %s" % (cm, cr["sha"])
    print("  commit ok", cr["sha"][:10], repr(msg))
    parent = cm

# ---------- 4. 更新远端 master ----------
r = req("PATCH", API + "/git/refs/heads/master", {"sha": parent, "force": False})
print("remote master ->", r["object"]["sha"])
subprocess.run(["git","update-ref","refs/remotes/origin/master", parent])
print("ALL DONE")
