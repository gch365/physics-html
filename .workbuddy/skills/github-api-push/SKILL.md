---
name: github-api-push
description: 当 git push 到 github.com 报 "Failed to connect / Connection was reset / Could not connect to server port 443" 时，用 GitHub Git Data API（走 api.github.com，本机畅通）把本地未推送的提交原样复刻到远端，SHA 与本地完全一致，历史不发散。适用于 gch365/physics-html 及同机其他仓库。
---

# 用途

本机网络对 `github.com:443` 直连被阻断（`git push/clone` 失败），但 `api.github.com` 的 HTTPS 小请求畅通。本技能把 `origin/master..HEAD` 的本地提交通过 Git Data API 逐层复刻（blob → tree → commit），提交 SHA 与本地**完全一致**，之后本地 `refs/remotes/origin/master` 同步更新，git 状态保持一致。

# 使用步骤

1. 确认前提（任一不满足则停止）：
   - `curl -s -m 10 https://api.github.com/` 返回 200（api.github.com 通）。
   - `git rev-parse origin/master` 与远端 `GET /repos/{o}/{r}/git/ref/heads/master` 一致（脚本内已断言）。
2. 凭据：脚本自动从 Windows 凭据库读 `git:https://github.com`（GCM 存的 gho 令牌，UTF-16LE blob，用 ctypes CredReadW 读取并按 utf-16-le 解码）。无需设 GH_TOKEN。
3. 运行（在目标仓库根目录）：

   ```
   "C:/Users/gchui/.workbuddy/binaries/python/versions/3.13.12/python.exe" <本技能目录>/api_push.py
   ```

4. 脚本需要改仓库地址时，改文件头 `REPO = "owner/repo"`。
5. 幂等性：blob/tree 是内容寻址，重复创建无害；已推过的提交再跑会因 ref 已更新而正常跳过（rev-list 为空则无操作）。

# 关键经验（踩过的坑）

- **commit SHA 不一致的坑**：git 提交对象 message 末尾**带一个 `\n`**。用 GitHub API 建 commit 时若把末尾换行 rstrip 掉，得到的 commit SHA 会不同。必须保留 message 原始字节（`cat.split("\n\n",1)[1]` 不做 rstrip）。
- tree 创建用**全量递归条目**（`git ls-tree -r`），天然处理删除/改名；改名文件在 `git diff --name-status -z` 解析时要吃掉旧路径。
- author/committer 用 `datetime.fromtimestamp(ts, timezone(offset)).isoformat()` 从原始 unix+tz 还原，可精确复刻。
- 若仓库有 GPG 签名提交，API 复刻无法带上 gpgsig 头，SHA 将不一致——本仓库未启用签名，适用。
- 已在 2026-10-13 用该方法成功推送 physics-html 两个提交（3e4ab8f / 911b372），远端 SHA 校验一致。
