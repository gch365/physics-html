#!/usr/bin/env bash
# ============================================================
#  理化生实验手册 子站 一键部署脚本
#  用法：把本脚本放在 physics-html 仓库的 shiyan/ 目录内，
#        在能访问 GitHub 的机器上运行：
#            bash shiyan/deploy.sh
#  前提：你已 clone 了 https://github.com/gch365/physics-html.git
#        且本 shiyan/ 目录已在仓库根目录（与 notify/ 同级）
# ============================================================
set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"   # .../physics-html/shiyan
REPO_ROOT="$(dirname "$SCRIPT_DIR")"           # .../physics-html
cd "$REPO_ROOT"

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "❌ 当前目录不是 git 仓库，请先 clone physics-html 并放入 shiyan/ 后重试。"
  exit 1
fi

git add shiyan
if git diff --cached --quiet; then
  echo "ℹ️  没有新变更，无需提交。"
else
  git commit -m "feat: 添加理化生数字化探究实验手册子站（shiyan）"
fi
git push
echo "✅ 已推送。稍候 1~2 分钟，访问："
echo "   https://gch365.github.io/physics-html/shiyan/index.html"
