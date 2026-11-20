# 验证脚本

两个辅助脚本，用于核对 Rust 版渲染与 Python 原版是否一致。它们不参与编译。

## render_py_ref.py —— 用 Python 原版出图

```bash
python render_py_ref.py "2026-11-16 10:05" py_class.png
```

第一个参数是锁定的时刻（不传则用当前时间），第二个参数是输出 PNG。
内部 `importlib` 加载原版 `desktop_board.py`（路径写在脚本顶部 `SRC`），
调用其 `load_board / load_schedule_api / compute_status / render_board_image`，
按 1920×1080 出图。

## 与 Rust 版对比

```bash
# Rust 版同一时刻出图
BOARD_NOW="2026-11-16 10:05" ./target/release/desktop_board.exe --render-only rs_class.png

# 逐像素差分
python -c "
from PIL import Image, ImageChops, ImageStat
a=Image.open('py_class.png').convert('RGB'); b=Image.open('rs_class.png').convert('RGB')
d=ImageChops.difference(a,b); print('mean', [round(x,2) for x in ImageStat.Stat(d).mean])
d.point(lambda v: min(255, v*4)).save('diff.png')
"
```

实测各状态平均像素差 2.6~2.9（满分 255），差异全部落在字形边缘：
PIL/FreeType 与 tiny-skia 的抗锯齿策略不同，版式坐标与颜色一致。

## drive_editor.py —— 自动点名编辑器对话框并截图

```bash
./target/release/desktop_board.exe --editor &     # 先启动
python drive_editor.py <PID>                       # 依次截四个对话框 + 主窗口
python drive_editor.py <PID> settings duty         # 只截指定几个
```

靠 `PostMessage(WM_COMMAND, 801..804)` 触发「设置 / 今日作业 / 班干部值周 / 卫生值日」，
截完再 `WM_CLOSE` 关掉，不会保存任何改动。
