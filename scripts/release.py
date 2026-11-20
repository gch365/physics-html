#!/usr/bin/env python3
"""版本校验 / 构建 / 发布 / 打 tag / 建仓 / 安全推送。

用法：python scripts/release.py <check|build|tag|push|publish|sync-kejian|bump X.Y.Z|create-repo>
      push 可加 --sync-kejian（推送后顺带同步源码镜像）；sync-kejian / publish 可加 --dry-run

设计约定：
- 根目录 version.json 是唯一发布版本源，必须与 theme.rs、两个 Cargo.toml 一致。
- 源码仓库与自动更新通道分离：publish 才触碰 kejian 的 version.json / dist/app.exe。
- sync-kejian 只写 kejian 的 daima/rust/ 子目录（源码镜像），路径隔离保证
  绝不覆盖根目录 version.json、dist/app.exe、tongzhi/ 通知数据。
- Gitee token 只从 ~/.RollCallData/config.json 读取，绝不写入任何文件或日志。
"""

from __future__ import annotations

import base64
import datetime as dt
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / "dist"
VERSION_FILE = ROOT / "version.json"
MINGW_BIN = Path(r"D:\kejian\toolchain\w64devkit\w64devkit\bin")
TOOLCHAIN = "+stable-x86_64-pc-windows-gnu"

# 自动更新通道（与源码仓库分离，勿改成源码仓库）
UPDATE_OWNER = "gzzxgch"
UPDATE_REPO = "kejian"
UPDATE_BRANCH = "master"
# 源码仓库
SOURCE_OWNER = "gzzxgch"
SOURCE_REPO = "classroom-rollcall-rs"
# kejian（私有课件仓库）里的源码镜像目录
KEJIAN_SRC_PREFIX = "daima/rust"

VERSION_PLACES = [
    ("version.json", None),
    ("classroom_rs/src/theme.rs", r'pub const APP_VERSION: &str = "([^"]+)"'),
    ("classroom_rs/Cargo.toml", r'(?m)^version = "([^"]+)"'),
    ("desktop_board_rs/Cargo.toml", r'(?m)^version = "([^"]+)"'),
]


class ReleaseError(RuntimeError):
    pass


def log(msg: str) -> None:
    print(f"[release] {msg}", flush=True)


def cargo_env() -> dict:
    env = os.environ.copy()
    if MINGW_BIN.is_dir():
        env["PATH"] = str(MINGW_BIN) + os.pathsep + env.get("PATH", "")
    return env


def run(cmd: list[str], cwd: Path, tail: int = 12) -> subprocess.CompletedProcess:
    proc = subprocess.run(
        cmd, cwd=str(cwd), env=cargo_env(), capture_output=True, text=True,
        encoding="utf-8", errors="replace",
    )
    text = (proc.stdout or "") + (proc.stderr or "")
    lines = [l for l in text.splitlines() if l.strip()]
    for line in lines[-tail:]:
        print("   " + line)
    if proc.returncode != 0:
        raise ReleaseError(f"命令失败（exit {proc.returncode}）: {' '.join(cmd[:3])}")
    return proc


def read_version() -> str:
    if not VERSION_FILE.exists():
        raise ReleaseError("缺少 version.json")
    data = json.loads(VERSION_FILE.read_text(encoding="utf-8"))
    v = str(data.get("version", "")).strip().lstrip("v")
    if not re.fullmatch(r"\d+\.\d+\.\d+", v):
        raise ReleaseError(f"version.json 版本号不合法: {v!r}")
    return v


def version_map() -> dict[str, str]:
    out: dict[str, str] = {}
    for rel, pattern in VERSION_PLACES:
        p = ROOT / rel
        if not p.exists():
            raise ReleaseError(f"缺少版本载体: {rel}")
        if rel.endswith(".json"):
            out[rel] = read_version()
            continue
        m = re.search(pattern, p.read_text(encoding="utf-8"))
        if not m:
            raise ReleaseError(f"无法从 {rel} 解析版本号")
        out[rel] = m.group(1)
    return out


def redact(s: str, token: str) -> str:
    return s.replace(token, "***") if token else s


def gitee_token() -> str:
    cfg = Path.home() / ".RollCallData" / "config.json"
    if not cfg.exists():
        raise ReleaseError("找不到 ~/.RollCallData/config.json，无法读取 Gitee token")
    t = json.loads(cfg.read_text(encoding="utf-8")).get("gitee_token", "").strip()
    if not t:
        raise ReleaseError("config.json 里 gitee_token 为空")
    return t


def api(token: str, path: str, data: dict | None = None, method: str = "GET") -> dict:
    url = f"https://gitee.com/api/v5/{path}"
    payload = None
    if data is not None:
        payload = urllib.parse.urlencode({**data, "access_token": token}).encode()
    req = urllib.request.Request(url, data=payload, method=method)
    req.add_header("Content-Type", "application/x-www-form-urlencoded")
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            body = r.read().decode("utf-8", "replace")
            return json.loads(body) if body.strip() else {}
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", "replace")[:400]
        raise ReleaseError(
            f"Gitee API {method} {redact(path, token)} -> HTTP {e.code}: {redact(body, token)}"
        )
    except Exception as e:  # 网络异常统一给出可读信息
        raise ReleaseError(f"Gitee API {method} {path} 失败: {type(e).__name__}: {e}")


# --------------------------------------------------------------------------- check

def cmd_check() -> None:
    if shutil.which("cargo") is None:
        raise ReleaseError("PATH 中找不到 cargo，请先安装 rustup")
    versions = version_map()
    uniq = set(versions.values())
    for rel, v in versions.items():
        log(f"  版本 {rel:<38} = {v}")
    if len(uniq) != 1:
        raise ReleaseError(
            "四处版本号不一致，请执行 python scripts/release.py bump <新版本> 统一："
            + ", ".join(f"{k}={v}" for k, v in versions.items())
        )
    log(f"版本一致：{uniq.pop()}，开始单测")
    for crate in ("desktop_board_rs", "classroom_rs"):
        log(f"cargo test --release [{crate}]")
        run(["cargo", TOOLCHAIN, "test", "--release"], ROOT / crate)
    log("check 通过")


# --------------------------------------------------------------------------- build

EXE_SOURCES = {
    "classroom_rollcall.exe": ROOT / "classroom_rs/target/release/classroom_rollcall.exe",
    # v1.0.24 起单文件模式：app.exe 内嵌全部大屏代码，desktop_board.exe 不再随包分发
    # （升级 bat 与主程序启动时会自动删除教室机上的遗留文件）。
}


def copy_unlocked(src: Path, dst: Path) -> None:
    """运行中的 exe / 刚写入正被 Defender 扫描的 exe 都不能直接覆盖写。

    做法：先复制到一个进程唯一的临时名（全新文件不会被占用），再把固定名
    让给 `*_prev.exe` 并原子替换。备用名本身也可能被占用，所以准备两个名字；
    实在让不出时再退回直接覆盖。
    """
    tmp = dst.with_name(f".{dst.name}.{os.getpid()}.tmp")
    shutil.copy2(src, tmp)
    try:
        if dst.exists():
            for prev in (
                dst.with_name(f"{dst.stem}_prev.exe"),
                dst.with_name(f"{dst.stem}_prev_{os.getpid()}.exe"),
            ):
                try:
                    os.replace(dst, prev)
                    break
                except OSError:
                    continue
        try:
            os.replace(tmp, dst)
        except OSError:
            shutil.copy2(tmp, dst)
    finally:
        try:
            tmp.unlink(missing_ok=True)
        except OSError:
            pass


def cmd_build() -> None:
    cmd_check()
    for crate in ("desktop_board_rs", "classroom_rs"):
        log(f"cargo build --release [{crate}]")
        run(["cargo", TOOLCHAIN, "build", "--release"], ROOT / crate)
    DIST.mkdir(exist_ok=True)
    # 名单与课表不再随发布包分发：程序创建班级时从私有仓库按班拉取，
    # 见 scripts/upload_class_data.py 与 classroom_rs/src/seed.rs。
    for name, src in EXE_SOURCES.items():
        if not src.exists():
            raise ReleaseError(f"构建产物缺失: {src}")
        copy_unlocked(src, DIST / name)
        log(f"同步 dist/{name} ({src.stat().st_size // 1024} KB)")
    # 自动更新用的固定文件名。刚写入的 exe 常被 Defender / 索引服务以
    # DELETE 共享方式短暂占用（不能直接覆盖，但能 rename），因此同样走
    # copy_unlocked 先让出固定名再复制，避免 build 在这里整个失败。
    copy_unlocked(EXE_SOURCES["classroom_rollcall.exe"], DIST / "app.exe")
    # GoEasy SDK 随发布包自带，避免离线 / 内网环境回退线上加载失败
    sdk = ROOT / "assets/goeasy-lite.min.js"
    if not sdk.exists():
        raise ReleaseError("缺少 assets/goeasy-lite.min.js，请从数据目录补回")
    shutil.copy2(sdk, DIST / sdk.name)
    v = read_version()
    stamp = dt.datetime.now().strftime("%Y%m%d_%H%M")
    for name in EXE_SOURCES:
        shutil.copy2(DIST / name, DIST / f"{Path(name).stem}_{stamp}.exe")
    data = json.loads(VERSION_FILE.read_text(encoding="utf-8"))
    (DIST / "version.json").write_text(
        json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    log(f"build 完成 -> dist/ (版本 {v}, 时间戳 {stamp}, 已生成 app.exe / version.json)")


# --------------------------------------------------------------------------- tag / push

def cmd_tag() -> None:
    v = read_version()
    tag = f"v{v}"
    exists = subprocess.run(
        ["git", "tag", "--list", tag], cwd=str(ROOT), capture_output=True, text=True
    ).stdout.strip()
    if exists:
        log(f"tag {tag} 已存在，跳过")
        return
    run(["git", "tag", "-a", tag, "-m", f"release {tag}"], ROOT, tail=4)
    log(f"已打 tag {tag}")


def cmd_push(sync_kejian: bool = False) -> None:
    token = gitee_token()
    clean = f"https://gitee.com/{SOURCE_OWNER}/{SOURCE_REPO}.git"
    remotes = subprocess.run(
        ["git", "remote"], cwd=str(ROOT), capture_output=True, text=True
    ).stdout.split()
    if "origin" not in remotes:
        run(["git", "remote", "add", "origin", clean], ROOT, tail=4)
    else:
        run(["git", "remote", "set-url", "origin", clean], ROOT, tail=4)
    push_url = f"https://{SOURCE_OWNER}:{token}@gitee.com/{SOURCE_OWNER}/{SOURCE_REPO}.git"
    proc = subprocess.run(
        # URL 已带 token，禁用本机失效的 credential helper，避免噪音与交互卡住
        ["git", "-c", "credential.helper=", "push", push_url, "HEAD:master", "--tags"],
        cwd=str(ROOT), capture_output=True, text=True, encoding="utf-8", errors="replace",
    )
    out = redact((proc.stdout or "") + (proc.stderr or ""), token)
    for line in [l for l in out.splitlines() if l.strip()][-12:]:
        print("   " + line)
    if proc.returncode != 0:
        raise ReleaseError(f"push 失败（exit {proc.returncode}）")
    log("remote 已保持为干净 URL，token 未写入 .git/config")
    if sync_kejian:
        cmd_sync_kejian()


# --------------------------------------------------------------------------- publish

def put_file(token: str, repo_path: str, local: Path, message: str) -> None:
    if not local.exists():
        raise ReleaseError(f"待上传文件不存在: {local}")
    content = base64.b64encode(local.read_bytes()).decode()
    quoted = urllib.parse.quote(repo_path, safe="/")
    sha = None
    try:
        cur = api(token, f"repos/{UPDATE_OWNER}/{UPDATE_REPO}/contents/{quoted}"
                  f"?access_token={token}&ref={UPDATE_BRANCH}")
        sha = cur.get("sha")
    except ReleaseError:
        pass  # 文件还不存在 -> 走创建分支
    body = {
        "content": content, "message": message, "branch": UPDATE_BRANCH,
    }
    if sha:
        body["sha"] = sha
        method = "PUT"
    else:
        method = "POST"
    api(token, f"repos/{UPDATE_OWNER}/{UPDATE_REPO}/contents/{quoted}", body, method)
    log(f"已上传 {repo_path} ({local.stat().st_size // 1024} KB, {method})")


def cmd_publish(dry_run: bool = False) -> None:
    token = gitee_token()
    v = read_version()
    if dry_run:
        # 只核对鉴权、远端路径与本地发布包，绝不写远端，避免误触发客户端自升级
        for repo_path in ("version.json", "dist/app.exe"):
            quoted = urllib.parse.quote(repo_path, safe="/")
            d = api(token, f"repos/{UPDATE_OWNER}/{UPDATE_REPO}/contents/{quoted}"
                    f"?access_token={token}&ref={UPDATE_BRANCH}")
            log(f"  远端可读 {repo_path} ({d.get('size', 0) // 1024} KB, sha={str(d.get('sha'))[:8]})")
        for local in (DIST / "version.json", DIST / "app.exe"):
            if not local.exists():
                raise ReleaseError(f"缺少本地发布包 {local}，请先执行 build")
            log(f"  本地就绪 {local.name} ({local.stat().st_size // 1024} KB)")
        log(f"dry-run 通过：v{v} 发布通道可用，未写入任何远端文件")
        return
    cmd_build()
    put_file(token, "version.json", DIST / "version.json", f"release v{v}: 更新版本清单")
    # Gitee Contents API 对超大 base64 可能 413；失败时给出明确的人工降级路径
    try:
        put_file(token, "dist/app.exe", DIST / "app.exe", f"release v{v}: 发布主程序")
    except ReleaseError as e:
        raise ReleaseError(
            f"exe 上传失败：{e}\n"
            "  降级方案：把 dist/app.exe 通过 git push 更新到 kejian 仓库的 dist/ 目录，"
            "version.json 已上传成功。"
        )
    log(f"publish 完成：kejian 已是 v{v}，客户端下次启动会自动升级")


# --------------------------------------------------------------------------- sync-kejian

def git_blob_sha(data: bytes) -> str:
    """Git blob SHA-1，可直接与 Gitee Contents API 返回的 sha 比对做增量同步。"""
    h = hashlib.sha1()
    h.update(b"blob " + str(len(data)).encode() + b"\0")
    h.update(data)
    return h.hexdigest()


def mirror_bytes(data: bytes) -> bytes:
    """Gitee Contents API 拒绝 content 为空的文件（HTTP 400 content is empty）。
    仓库里的 `.keep` 这类占位文件用一个换行代替：它本来就没有内容，语义不变。
    比较与上传都统一走这里，保证下一次同步 sha 仍一致、不会反复重写。"""
    return data if data else b"\n"


def tracked_blobs() -> list[tuple[str, bytes]]:
    """读 HEAD 的规范内容（LF），不用工作区文件（可能被 autocrlf 转成 CRLF）。"""
    names = subprocess.run(
        ["git", "ls-files", "-z"], cwd=str(ROOT), capture_output=True, check=True
    ).stdout.decode("utf-8").split("\0")
    out: list[tuple[str, bytes]] = []
    for rel in filter(None, names):
        p = subprocess.run(
            ["git", "cat-file", "blob", f"HEAD:{rel}"],
            cwd=str(ROOT), capture_output=True, check=True,
        )
        out.append((rel, p.stdout))
    return out


def api_retry(token: str, path: str, data: dict | None = None,
              method: str = "GET", tries: int = 3):
    for i in range(tries):
        try:
            return api(token, path, data, method)
        except ReleaseError as e:
            msg = str(e)
            retryable = any(k in msg for k in ("HTTP 429", "HTTP 403", "timed out"))
            if retryable and i + 1 < tries:
                time.sleep(1.5 * (i + 1))
                continue
            raise
    raise ReleaseError("api_retry 意外退出")


def remote_mirror_files(token: str) -> list[tuple[str, str]]:
    """递归列出 kejian/daima/rust/ 下所有文件 (path, sha)。

    同步是「增量上传」，本地删掉的文件原本会永远留在镜像里（例如本轮删掉的
    build.rs，会让人误以为名单还编进 exe）。这里把远端清单拿出来做差集清理。
    """
    out: list[tuple[str, str]] = []

    def walk(rel: str) -> None:
        quoted = urllib.parse.quote(rel, safe="/")
        try:
            d = api_retry(
                token,
                f"repos/{UPDATE_OWNER}/{UPDATE_REPO}/contents/{quoted}"
                f"?access_token={token}&ref={UPDATE_BRANCH}",
            )
        except ReleaseError as e:
            if "HTTP 404" in str(e):
                return
            raise
        if not isinstance(d, list):
            return
        for it in d:
            p = str(it.get("path", ""))
            if not p.startswith(KEJIAN_SRC_PREFIX + "/"):
                continue
            if it.get("type") == "dir":
                walk(p)
            elif it.get("type") == "file":
                out.append((p, str(it.get("sha", ""))))

    walk(KEJIAN_SRC_PREFIX)
    return out


def cmd_sync_kejian(dry_run: bool = False) -> None:
    token = gitee_token()
    files = tracked_blobs()
    if not files:
        raise ReleaseError("git 里没有已跟踪文件，请先 commit")
    log(f"源码镜像 -> {UPDATE_OWNER}/{UPDATE_REPO}/{KEJIAN_SRC_PREFIX}/ "
        f"({len(files)} 个文件, {'dry-run' if dry_run else '实际写入'})")
    added = updated = same = to_write = 0
    for rel, data in files:
        remote = f"{KEJIAN_SRC_PREFIX}/{rel}"
        # 路径隔离：任何越出镜像目录的路径直接拒绝，保护根 version.json / dist / tongzhi
        parts = rel.replace("\\", "/").split("/")
        if (
            rel.startswith("/")
            or "\\" in rel
            or any(p in ("", ".", "..") for p in parts)
            or not remote.startswith(KEJIAN_SRC_PREFIX + "/")
        ):
            raise ReleaseError(f"拒绝写入镜像目录之外的路径: {remote}")
        quoted = urllib.parse.quote(remote, safe="/")
        cpath = f"repos/{UPDATE_OWNER}/{UPDATE_REPO}/contents/{quoted}"
        payload = mirror_bytes(data)
        cur = None
        try:
            cur = api_retry(token, f"{cpath}?access_token={token}&ref={UPDATE_BRANCH}")
        except ReleaseError as e:
            if "HTTP 404" not in str(e):
                raise  # 401 / 限流等不能误判为「文件不存在」
        if cur and cur.get("sha") == git_blob_sha(payload):
            same += 1
            continue
        action = "更新" if cur else "新增"
        if dry_run:
            to_write += 1
            log(f"  将{action} {remote} ({len(data) // 1024} KB)")
            continue
        body = {
            "content": base64.b64encode(payload).decode(),
            "message": f"sync source: {action} {rel}",
            "branch": UPDATE_BRANCH,
        }
        if cur:
            body["sha"] = cur["sha"]
            method = "PUT"
        else:
            method = "POST"
        api_retry(token, cpath, body, method)
        if cur:
            updated += 1
        else:
            added += 1
        time.sleep(0.12)  # 轻微限流，避免触发 Gitee 频率限制
    # 清理本地已删除、镜像里还留着的文件，保证镜像严格等于 git HEAD。
    # 路径仍限定在 daima/rust/ 内，Root 的 version.json / dist / tongzhi 不受影响。
    expected = {f"{KEJIAN_SRC_PREFIX}/{rel}" for rel, _ in files}
    pruned = 0
    for remote, sha in remote_mirror_files(token):
        if remote in expected:
            continue
        if dry_run:
            log(f"  将删除 {remote}（本地已不存在）")
            pruned += 1
            continue
        quoted = urllib.parse.quote(remote, safe="/")
        cpath = f"repos/{UPDATE_OWNER}/{UPDATE_REPO}/contents/{quoted}"
        api_retry(token, cpath, {
            "sha": sha,
            "message": f"sync source: 删除 {remote}",
            "branch": UPDATE_BRANCH,
        }, "DELETE")
        log(f"  已删除 {remote}（本地已不存在）")
        pruned += 1
        time.sleep(0.12)
    if dry_run:
        log(f"dry-run 完成：待写 {to_write}，已一致 {same}，待清理 {pruned}；未写入任何远端文件")
    else:
        log(f"同步完成：新增 {added}，更新 {updated}，已一致 {same}，清理残留 {pruned}")


# --------------------------------------------------------------------------- bump / create-repo

def cmd_bump(new: str) -> None:
    new = new.strip().lstrip("v")
    if not re.fullmatch(r"\d+\.\d+\.\d+", new):
        raise ReleaseError(f"版本号格式应为 X.Y.Z，收到 {new!r}")
    data = json.loads(VERSION_FILE.read_text(encoding="utf-8"))
    old = str(data["version"])
    data["version"] = new
    VERSION_FILE.write_text(
        json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    for rel, pattern in VERSION_PLACES[1:]:
        p = ROOT / rel
        txt = p.read_text(encoding="utf-8")
        txt2, n = re.subn(pattern, lambda m: m.group(0).replace(old, new), txt, count=1)
        if n != 1:
            raise ReleaseError(f"替换 {rel} 版本号失败")
        p.write_text(txt2, encoding="utf-8")
    log(f"版本 {old} -> {new} 已同步到 version.json / theme.rs / 两个 Cargo.toml")
    log("记得在 CHANGELOG.md 补上该版本条目，并重新 build 让 Cargo.lock 更新")


def cmd_create_repo() -> None:
    token = gitee_token()
    try:
        d = api(token, "user/repos", {
            "name": SOURCE_REPO,
            "description": "教室随机点名系统 Rust 版（源码 / 版本管理）",
            "private": "true", "has_issues": "true", "has_wiki": "false",
            "auto_init": "false",
        }, "POST")
        log(f"仓库已创建：{d.get('full_name')} (private={d.get('private')})")
    except ReleaseError as e:
        if "已存在" in str(e) or "already" in str(e).lower() or "HTTP 409" in str(e):
            log("仓库已存在，跳过创建")
        else:
            raise


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    cmd, *args = sys.argv[1:]
    try:
        if cmd == "check":
            cmd_check()
        elif cmd == "build":
            cmd_build()
        elif cmd == "tag":
            cmd_tag()
        elif cmd == "push":
            cmd_push(sync_kejian="--sync-kejian" in args)
        elif cmd == "publish":
            cmd_publish(dry_run="--dry-run" in args)
        elif cmd == "sync-kejian":
            cmd_sync_kejian(dry_run="--dry-run" in args)
        elif cmd == "bump":
            if not args:
                raise ReleaseError("用法: release.py bump X.Y.Z")
            cmd_bump(args[0])
        elif cmd == "create-repo":
            cmd_create_repo()
        else:
            print(__doc__)
            return 2
    except ReleaseError as e:
        print(f"[release] 失败: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
