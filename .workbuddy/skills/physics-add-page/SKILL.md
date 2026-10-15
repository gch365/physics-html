---
name: physics-add-page
description: 给 gch365/physics-html 站点新增/接入一个仿真实验或高考真题页面到 index.html，并补上教室大屏整页缩放控件（phZoom）。当用户要"新增实验、加一个仿真、加高考题、把某 html 接到首页"时使用。
agent_created: true
---

# 给 physics-html 站点接入新页面 + 注入教室缩放控件

本技能用于把一个新的 HTML 仿真/真题页接入 `index.html` 的导航，并确保它在教室 Win7/希沃 大屏上能整页缩放（phZoom 控件）。

## 0. 站点结构（必读）
- 所有实验页放在 `exp/<coll>/<name>.html`。`<coll>` 是分章代号。
- `index.html` 里 `const data = {...}` 是按章节键组织的数组：键如 `b11,b12,...,b32,b33,x1c1,...,x2c3,x3c5,exp,gaokao`。
- `names` 字典把键映射成侧栏章节标题（如 `gaokao`→"高考真题精析"、`b32`→"静电场的能量"）。`coll` 仅供拼 URL，`names` 仅供侧栏标题，**两者独立**。
- 高考真题统一放 `gaokao` 数组，`coll` 用 `202610`（即目录 `exp/202610/`）；福建等个别老题用 `coll:"root"`。

## 1. URL 拼法（openExp，index.html 约 190–199 行）
```
if(coll==='root') url = './' + exp.split('/').map(encodeURIComponent).join('/');
else { const file = (coll==='f01') ? exp : (exp + '.html'); url = 'exp/'+coll+'/'+encodeURIComponent(file); }
```
- `coll='f01'`：`exp` 本身已含 `.html`（如 `（6）运动图像-仿写.html`）。
- `coll='root'`：`exp` 带子路径（如 `高考题/（60）24福建压轴题-美化版.html`），文件在仓库根目录下对应子目录。
- 其它（含 `c01..c07`、`202610`）：`exp` **不带**扩展名，自动补 `.html`，路径 `exp/<coll>/<exp>.html`。

## 2. 接入步骤
1. **放文件**：把新 HTML 放到 `exp/<coll>/` 下（文件名即 `exp` 值；若 coll≠f01/root 则不带 `.html`）。
2. **加条目**：在 `index.html` 对应章节数组里追加一条：
   `{"icon":"🏆","name":"显示名","desc":"一句话说明","coll":"202610","exp":"（NN）文件名"}`
   - 用 Python 字符串替换，锚定该章节已有的某条 `anchor` 字符串后插入（保持 CRLF：插入串用 `\r\n` 连接）。
   - 注意 name 里的全角括号 `（）` 要原样保留。
3. **补缩放控件**（见下）。

## 3. 注入教室缩放控件 phZoom（每个独立/仿真页都要有）
- 控件作用：右下角 A−/100%/A+ 胶囊，比例存 localStorage，教室大屏一次设永久生效；被 index 的 iframe 嵌入时自动隐藏自身按钮、改收父页 `postMessage({phZoom})` 缩放。
- **提取片段**：从任一已注入页（如 `exp/202610/2026年贵州高考压轴题目（无人机投弹模型）.html`）取控件块：
  ```python
  pos = s.index('id="phZoom"')
  start = s.rfind('<div', 0, pos)          # 注意 id 不是首属性，不能硬匹配 <div id="phZoom"
  bi = s.rindex('</body>')
  end = s.rindex('</script>', 0, bi)       # 取 </body> 前的最后一个 script
  snippet = s[start:end+len('</script>')]
  ```
- **注入**（幂等，保留原行尾）：
  ```python
  if 'id="phZoom"' in s: skip
  nl = "\r\n" if "\r\n" in s else "\n"
  snip = snippet.replace("\r\n", nl).replace("\r", nl)
  bi = s.rindex("</body>")
  s = s[:bi] + nl + snip + nl + s[bi:]
  ```
- ⚠️ 新生成页常为 CRLF 且结尾是 `</script>\r\n</body>\r\n</html>\r\n`，**不能假设文件以 `</body>` 裸结尾**，必须先 `rindex('</body>')` 再在它前面插入。
- 注入后抽该页最后一个 `<script>` 块跑 `node --check` 验证语法。

## 4. 校验清单
- 该章节数组条目数 = 预期（Python 平衡括号扫描 `data[键]` 数 `"icon"` 出现次数）。
- 每个新文件 `grep -c 'id="phZoom"' == 1`（无重复注入）。
- 5 个（或更少）新文件各自最后一个 script `node --check` 通过。
- 若改了章节归属，确认原章节不再含该 `exp`、目标章节含之。

## 5. 坑点提醒
- `id="phZoom"` 前的 `<div` 提取法（不是 `<div id="phZoom"`）。
- 注入前判 `</body>` 位置，别假设文件以 `</body>` 结尾。
- 插入 index.html 条目时保持 CRLF，避免行尾混用产生超大 diff。
- 若新文件在子目录（如旧 `ggb20261012/`），先确认是否要移到 `exp/202610/` 根目录与同批题并列；移动前用 `find` 查重，避免重复件。
- 临时脚本用完即删（`.workbuddy/*.py`），别留仓库里。
