#!/usr/bin/env python3
"""把学校名单/课表按班拆成小 JSON，上传到独立的私有数据仓库。

设计要点（与「给其他教室用的版本」配套）：
- 源码仓库 / kejian 发布通道 / 本脚本自身 **绝不含学生姓名**；数据只进私有仓库
  `gzzxgch/classroom-data` 的 `banji/` 目录。
- 一个班一个文件：`banji/{学校}/{入学年份}/{NN}班.json`，程序创建班级时按班号
  只拉那一份，教室电脑磁盘上从此只留本班名单，不再随包分发全校 1530 人。
- 增量上传：用 Git blob SHA-1 与远端 `sha` 比对，一致就跳过，重复跑幂等。
- 上传用的 token 来自 `~/.RollCallData/config.json`（或 ROLLCALL_GITEE_TOKEN）。
  分发给教室时请在设置里填**只读令牌**，不要把这个可读写令牌发出去。

用法：
  python scripts/upload_class_data.py --dry-run          # 只看会上传哪些文件
  python scripts/upload_class_data.py --create-repo      # 首次：建私有仓库
  python scripts/upload_class_data.py --create-repo --apply
  python scripts/upload_class_data.py --apply           # 后续换年级时重跑即可
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

API = "https://gitee.com/api/v5"
DEFAULT_OWNER = "gzzxgch"
DEFAULT_REPO = "classroom-data"
DEFAULT_DIR = "banji"
BRANCH = "master"
DEFAULT_SEED = Path(
    os.environ.get("ROLLCALL_SEED_FILE")
    or (Path.home() / ".RollCallData" / "seed" / "seed.json")
)


class UploadError(RuntimeError):
    pass


def log(msg: str) -> None:
    print(f"[class-data] {msg}", flush=True)


def token() -> str:
    t = os.environ.get("ROLLCALL_GITEE_TOKEN", "").strip()
    if t:
        return t
    cfg = Path.home() / ".RollCallData" / "config.json"
    if cfg.exists():
        t = json.loads(cfg.read_text(encoding="utf-8")).get("gitee_token", "").strip()
    if not t:
        raise UploadError("找不到 Gitee token（config.json / ROLLCALL_GITEE_TOKEN）")
    return t


def redact(s: str, tok: str) -> str:
    return s.replace(tok, "***") if tok else s


def api(tok: str, path: str, data: dict | None = None, method: str = "GET") -> dict:
    url = f"{API}/{path}"
    if method == "GET" and data is None:
        sep = "&" if "?" in url else "?"
        url += f"{sep}access_token={urllib.parse.quote(tok, safe='')}"
    payload = None
    if data is not None:
        payload = urllib.parse.urlencode({**data, "access_token": tok}).encode()
    req = urllib.request.Request(url, data=payload, method=method)
    req.add_header("Content-Type", "application/x-www-form-urlencoded")
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            body = r.read().decode("utf-8", "replace")
            return json.loads(body) if body.strip() else {}
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", "replace")[:300]
        raise UploadError(
            f"HTTP {e.code} {method} {redact(path, tok)}: {redact(body, tok)}"
        ) from None
    except Exception as e:
        raise UploadError(f"{method} {path} 失败: {type(e).__name__}: {e}") from None


def contents_path(owner: str, repo: str, remote: str) -> str:
    return f"repos/{owner}/{repo}/contents/{urllib.parse.quote(remote, safe='/')}"


def blob_sha(data: bytes) -> str:
    h = hashlib.sha1()
    h.update(b"blob " + str(len(data)).encode() + b"\0")
    h.update(data)
    return h.hexdigest()


def create_repo(tok: str, owner: str, repo: str) -> None:
    try:
        d = api(tok, "user/repos", {
            "name": repo,
            "description": "教室点名系统：班级名单与课表（私有，按班分片）",
            "private": "true", "has_issues": "false", "has_wiki": "false",
            "auto_init": "true",
        }, "POST")
        log(f"私有仓库已创建：{d.get('full_name')} (private={d.get('private')})")
    except UploadError as e:
        if "HTTP 409" in str(e) or "已存在" in str(e) or "already" in str(e).lower():
            log("私有仓库已存在，跳过创建")
        else:
            raise


def class_file_name(raw: str) -> str:
    """班号补零到两位；非数字原样保留（与 Rust normalize_class_num 一致）。"""
    t = str(raw).strip()
    try:
        return f"{int(t):02d}"
    except ValueError:
        return t


def split_seed(seed: dict, remote_dir: str) -> list[tuple[str, bytes]]:
    """seed.json → [(远端路径, 文件字节)]，每个班一个文件。"""
    school = str(seed.get("school", "")).strip()
    year = str(seed.get("year", "")).strip()
    classes = seed.get("classes") or {}
    if not school or not year:
        raise UploadError("seed.json 缺少 school / year，无法生成云端路径")
    if not classes:
        raise UploadError("seed.json 里没有班级数据")
    out: list[tuple[str, bytes]] = []
    for num, item in sorted(classes.items(), key=lambda kv: class_file_name(kv[0])):
        payload = {
            "version": 1,
            "school": school,
            "year": year,
            "class_num": str(num).strip(),
            "students": item.get("students") or [],
            "schedule": item.get("schedule") or [],
        }
        data = (json.dumps(payload, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
        remote = f"{remote_dir}/{school}/{year}/{class_file_name(num)}.json"
        out.append((remote, data))
    return out


def upload_all(tok: str, owner: str, repo: str, files: list[tuple[str, bytes]],
               dry_run: bool, apply: bool) -> tuple[int, int]:
    added = same = 0
    for remote, data in files:
        path = contents_path(owner, repo, remote)
        cur = None
        try:
            cur = api(tok, f"{path}?ref={BRANCH}")
        except UploadError as e:
            if "HTTP 404" not in str(e):
                raise
        if cur and cur.get("sha") == blob_sha(data):
            same += 1
            continue
        if dry_run or not apply:
            log(f"  将{'更新' if cur else '新增'} {remote} ({len(data) // 1024} KB)")
            continue
        body = {
            "content": base64.b64encode(data).decode(),
            "message": f"class data: {'更新' if cur else '新增'} {remote}",
            "branch": BRANCH,
        }
        method = "PUT" if cur else "POST"
        if cur:
            body["sha"] = cur["sha"]
        api(tok, path, body, method)
        if cur:
            added += 1
        else:
            added += 1
        time.sleep(0.12)  # 轻微限流
    return added, same


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--seed", type=Path, default=DEFAULT_SEED, help="源 seed.json")
    ap.add_argument("--owner", default=DEFAULT_OWNER)
    ap.add_argument("--repo", default=DEFAULT_REPO)
    ap.add_argument("--dir", default=DEFAULT_DIR, help="仓库内的数据根目录")
    ap.add_argument("--create-repo", action="store_true", help="首次创建私有仓库")
    ap.add_argument("--apply", action="store_true", help="真正写入远端；不加则只预演")
    args = ap.parse_args()

    try:
        if not args.seed.exists():
            raise UploadError(f"找不到 {args.seed}，先跑 scripts/build_seed.py 生成")
        seed = json.loads(args.seed.read_text(encoding="utf-8"))
        files = split_seed(seed, args.dir.strip("/"))
        tok = token()
        log(f"数据源 {args.seed}：{len(files)} 个班 / "
            f"{sum(len(c.get('students') or []) for c in seed['classes'].values())} 名学生")
        log(f"目标 {args.owner}/{args.repo}/{args.dir}/（私有仓库，{'实际写入' if args.apply else '预演'}）")
        if args.create_repo and args.apply:
            create_repo(tok, args.owner, args.repo)
        written, same = upload_all(tok, args.owner, args.repo, files,
                                   dry_run=not args.apply, apply=args.apply)
        if args.apply:
            log(f"完成：写入 {written}，已一致 {same}")
        else:
            log(f"预演完成：待写 {len(files) - same}，已一致 {same}；未写入任何远端文件"
                "（加 --apply 才真正上传）")
        return 0
    except UploadError as e:
        print(f"[class-data] 失败: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
