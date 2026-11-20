#!/usr/bin/env python3
"""网络 U 盘上传工具：把本地文件上传到 Gitee 仓库 uploads/ 目录（教室端自动拉取）。

为什么需要它：Gitee **网页**拖拽上传单文件限制 10MB；而 contents API 实测
17MB 也能一次传（v1.0.19 的 app.exe 就这么传的）。本脚本走 API，并且
>8MB 自动分块（.chunks/00000.bin + .manifest.json），彻底没有大小上限。

分块格式与教室端（classroom_rs/src/gitee.rs download_chunked）严格对齐：
  uploads/<文件名>.manifest.json   {"chunks": N, "size": 总字节, "originalName": "<文件名>", "sha256": "..."}
  uploads/<文件名>.chunks/00000.bin  第 1 块（编号从 0 开始）
  ...                              依次 00001.bin / 00002.bin ...

用法：
  python scripts/upload_udisk.py 大文件.zip                # 上传单个文件
  python scripts/upload_udisk.py a.mp4 b.pdf 课件/          # 多文件 / 目录
  python scripts/upload_udisk.py --dir uploads/2026-11-18 a.mp4   # 指定远端子目录
  python scripts/upload_udisk.py --list                     # 只看远端 uploads 现状
幂等：远端 manifest 里记录的 sha256 与本地一致就跳过，重复跑不重复传。

令牌来源（优先级）：环境变量 ROLLCALL_GITEE_TOKEN > ~/.RollCallData/config.json 的 giteeToken。
仓库默认 gzzxgch/kejian（网络U盘所在仓库），可用 --owner/--repo 覆盖。
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

API = "https://gitee.com/api/v5"
RAW = "https://gitee.com/{}/{}/raw/master/{}"
DEFAULT_OWNER = "gzzxgch"
DEFAULT_REPO = "kejian"
DEFAULT_DIR = "uploads"
# 单块原始字节数：base64 后约 10.7MB，远低于实测可用上限（17MB 文件整传成功）
CHUNK_SIZE = 8 * 1024 * 1024
BRANCH = "master"


def load_token() -> str:
    tok = os.environ.get("ROLLCALL_GITEE_TOKEN", "").strip()
    if tok:
        return tok
    cfg = Path.home() / ".RollCallData" / "config.json"
    if cfg.is_file():
        try:
            data = json.loads(cfg.read_text(encoding="utf-8"))
            # 教室端 config.json 用下划线 key（gitee_token），网页/脚本习惯用驼峰，两者都兼容
            tok = str(data.get("gitee_token") or data.get("giteeToken") or data.get("token") or "").strip()
            if tok:
                return tok
        except Exception as e:
            print(f"⚠ 读取 {cfg} 失败: {e}")
    sys.exit("✗ 未找到 Gitee 令牌：请设置环境变量 ROLLCALL_GITEE_TOKEN 或写入 ~/.RollCallData/config.json")


def api(token: str, path: str, body: dict | None = None, method: str = "GET") -> dict | list:
    # Gitee API 统一 form-urlencoded：GET 的 token 放 query；PUT/POST 的 token 放表单。
    # 实测 JSON body 里的 access_token 会被忽略（返回 404 Not Found Project，如同匿名）。
    if method == "GET":
        sep = "&" if "?" in path else "?"
        url = f"{API}{path}{sep}access_token={urllib.parse.quote(token)}"
        payload = None
        headers = {}
    else:
        url = f"{API}{path}"
        params = dict(body or {})
        params["access_token"] = token
        payload = urllib.parse.urlencode(params).encode()
        headers = {"Content-Type": "application/x-www-form-urlencoded"}
    req = urllib.request.Request(url, data=payload, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            raw = resp.read()
        return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", errors="replace")[:300]
        raise RuntimeError(f"Gitee API HTTP {e.code}: {detail}") from e


def enc(path: str) -> str:
    return "/".join(urllib.parse.quote(p, safe="") for p in path.split("/"))


def get_sha(token: str, owner: str, repo: str, path: str) -> str:
    """远端文件存在则返回 sha，否则空串。Gitee 对不存在路径返回 200 + []（空数组）。"""
    try:
        info = api(token, f"/repos/{owner}/{repo}/contents/{enc(path)}?ref={BRANCH}")
        if isinstance(info, dict):
            return str(info.get("sha") or "")
        return ""   # [] = 不存在
    except RuntimeError as e:
        if "HTTP 404" in str(e):
            return ""
        raise


def put_file(token: str, owner: str, repo: str, path: str, content_b64: str, message: str) -> None:
    sha = get_sha(token, owner, repo, path)
    body = {"content": content_b64, "message": message, "branch": BRANCH}
    method = "POST"
    if sha:
        body["sha"] = sha
        method = "PUT"
    api(token, f"/repos/{owner}/{repo}/contents/{enc(path)}", body, method)


def sha256_file(p: Path) -> str:
    h = hashlib.sha256()
    with p.open("rb") as f:
        for blk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(blk)
    return h.hexdigest()


def upload_one(token: str, owner: str, repo: str, local: Path, remote_dir: str) -> None:
    remote_dir = remote_dir.strip("/")
    remote_path = f"{remote_dir}/{local.name}" if remote_dir else local.name
    data = local.read_bytes()
    size = len(data)
    digest = sha256_file(local)

    # 幂等：远端 manifest 记录一致则跳过（不存在时 Gitee 返回 200 []，按类型判断）
    try:
        mf = api(token, f"/repos/{owner}/{repo}/contents/{enc(remote_path)}.manifest.json?ref={BRANCH}")
        if isinstance(mf, dict) and mf.get("content"):
            old = json.loads(base64.b64decode(mf["content"]).decode("utf-8"))
            if old.get("sha256") == digest and int(old.get("size", -1)) == size:
                print(f"↷ 跳过（远端已是最新）: {remote_path} ({size} 字节)")
                return
    except Exception:
        pass

    n_chunks = (size + CHUNK_SIZE - 1) // CHUNK_SIZE
    if n_chunks <= 0:
        n_chunks = 1

    msg = f"udisk: {local.name} ({size} 字节)"
    if n_chunks == 1:
        print(f"↑ 上传: {remote_path} ({size} 字节)")
        put_file(token, owner, repo, remote_path, base64.b64encode(data).decode("ascii"), msg)
    else:
        print(f"↑ 上传(分块 {n_chunks} 块): {remote_path} ({size} 字节)")
        for i in range(n_chunks):
            blk = data[i * CHUNK_SIZE:(i + 1) * CHUNK_SIZE]
            chunk_path = f"{remote_path}.chunks/{i:05d}.bin"
            put_file(token, owner, repo, chunk_path,
                     base64.b64encode(blk).decode("ascii"),
                     f"udisk chunk {i + 1}/{n_chunks}: {local.name}")
            print(f"   块 {i + 1}/{n_chunks} ✓ ({len(blk)} 字节)")
        manifest = {
            "chunks": n_chunks,
            "size": size,
            "originalName": local.name,
            "sha256": digest,
        }
        put_file(token, owner, repo, f"{remote_path}.manifest.json",
                 base64.b64encode(json.dumps(manifest).encode("utf-8")).decode("ascii"),
                 f"udisk manifest: {local.name} ({n_chunks} 块)")
    print(f"✓ 完成: {remote_path}")


def list_remote(token: str, owner: str, repo: str, remote_dir: str) -> None:
    try:
        items = api(token, f"/repos/{owner}/{repo}/contents/{enc(remote_dir.strip('/'))}?ref={BRANCH}")
    except RuntimeError as e:
        print(f"(目录 {remote_dir} 读取失败: {e})")
        return
    if not isinstance(items, list):
        print("(空目录)")
        return
    manifests, regular = [], []
    for it in items:
        name, size, typ = it.get("name", ""), it.get("size", 0), it.get("type", "")
        if typ == "dir":
            print(f"[目录] {name}/")
            continue
        if name.endswith(".manifest.json"):
            try:
                m = json.loads(base64.b64decode(it.get("content", "")).decode("utf-8"))
                manifests.append((m.get("originalName", "?"), m.get("size", 0), m.get("chunks", 0)))
            except Exception:
                manifests.append((name, size, "?"))
        elif ".chunks/" in it.get("path", ""):
            continue
        else:
            regular.append((name, size))
    for n, s, c in manifests:
        print(f"[分块] {n}  {s} 字节  ({c} 块)")
    for n, s in regular:
        print(f"[文件] {n}  {s} 字节")


def collect_files(args) -> list[Path]:
    out: list[Path] = []
    for x in args.files:
        p = Path(x)
        if p.is_dir():
            out.extend(sorted(q for q in p.rglob("*") if q.is_file()))
        elif p.is_file():
            out.append(p)
        else:
            print(f"⚠ 跳过（不存在）: {x}")
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description="网络U盘上传（大文件自动分块）")
    ap.add_argument("files", nargs="*", help="要上传的文件或目录")
    ap.add_argument("--dir", default=DEFAULT_DIR, help=f"远端目录（默认 {DEFAULT_DIR}）")
    ap.add_argument("--owner", default=DEFAULT_OWNER)
    ap.add_argument("--repo", default=DEFAULT_REPO)
    ap.add_argument("--list", action="store_true", help="只列出远端现状，不上传")
    args = ap.parse_args()

    token = load_token()
    if args.list:
        list_remote(token, args.owner, args.repo, args.dir)
        return
    files = collect_files(args)
    if not files:
        print("用法: python scripts/upload_udisk.py <文件...> [--dir uploads]")
        return
    ok = fail = 0
    for p in files:
        try:
            upload_one(token, args.owner, args.repo, p, args.dir)
            ok += 1
        except Exception as e:
            print(f"✗ {p.name} 上传失败: {e}")
            fail += 1
    print(f"\n完成: 成功 {ok}，失败 {fail}")


if __name__ == "__main__":
    main()
