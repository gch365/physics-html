# 理化生数字化探究实验手册 · 网站版

把《数字化探究系统实验手册（高中定稿打印版）》转换为静态网站，本次以 **`shiyan/` 子目录**形式部署到你的 GitHub Pages 站点 `physics-html`，最终访问地址：

> **https://gch365.github.io/physics-html/shiyan/index.html**

## 站点内容

- **目录导航页** `index.html`：按物理 / 化学 / 生物三科分色列出全部 71 个实验，带实时搜索框。
- **实验单页** `experiments/expNN.html`：每个实验一个独立 HTML 文件，含正文（目的 / 原理 / 器材 / 操作步骤等）、装置图、表格，以及上一页 / 下一页导航。

> ⚠️ **重要：化学篇、生物篇正文缺失**
> 当前源文档只含有**物理篇完整正文 + 全册目录**。化学篇（实验五十~六十，11 个）和生物篇（实验六十一~七十二，12 个）的正文**不在文件中**。
> 因此这 23 个实验是**占位页**（明确标注「正文未包含在当前源文档」），导航不会 404。提供含化/生正文的完整稿件后重跑生成脚本即可自动补全。

## 文件结构

```
shiyan/
├── index.html              # 目录导航页（首页）
├── experiments.json        # 实验清单（71 条：科目/编号/标题/页码/目标文件）
├── README.md
├── deploy.sh              # 一键部署脚本（在 physics-html 仓库根目录运行）
└── experiments/
    ├── exp01.html … exp71.html   # 71 个实验单页（48 完整 + 23 占位）
    └── img/                      # 抽取的实验装置图（137 张）
```

## 本地预览

```bash
cd shiyan
python3 -m http.server 8000
# 浏览器打开 http://localhost:8000/
```

## 部署到 GitHub Pages（子目录方式）

本目录已按 `shiyan/` 命名，直接整体放入 `physics-html` 仓库根目录（与 `notify/` 同级）即可，
相对链接会自动适配 `…/shiyan/` 子路径，无需改动任何代码。

> 注意：云端构建环境（本 Agent 运行沙箱）**无法连接 GitHub**，因此不能在云端自动推送。
> 请在能访问 GitHub 的机器（你本机）执行以下任一步骤。

**方式 A：一键脚本**

```bash
# 在 physics-html 仓库根目录（shiyan/ 已放入）运行：
bash shiyan/deploy.sh
```

脚本会自动 `git add shiyan` → `commit` → `push`。

**方式 B：手动三步**

```bash
cd physics-html            # 进入仓库根目录
# 确保 shiyan/ 已放在仓库根目录（与 notify/ 同级）
git add shiyan
git commit -m "feat: 添加理化生实验手册子站 shiyan"
git push
```

推送后等待 1~2 分钟，访问：

> **https://gch365.github.io/physics-html/shiyan/index.html**

> 提示：GitHub Pages 路径大小写敏感，目录名请保持小写 `shiyan`。

## 图片提取说明

源 docx 中全部 146 张内嵌图的 **CRC 校验位被置为 0**（某云管线的产物），但图像数据本身完好，
可用 `unzip -o` 忽略校验强制抽出；其中少量 TIFF 已用 PIL 转为 PNG 以适配浏览器。

## 重新生成

补充了化学 / 生物正文后，用生成脚本（`gen_pages.py`，需原始 docx 与 `experiments.json`）重跑即可，
占位页会自动替换为完整页，导航与图片无需改动。
