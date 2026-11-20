# 班级桌面大屏 · Rust 版

`classroom_rs` 的 path-dependency，也可独立运行。**不修改系统壁纸**：所有内容都是
独立的 32 位 DIB 分层透明窗口（`WS_EX_LAYERED` + `UpdateLayeredWindow`），叠加在
用户任意壁纸上。

> 项目总览、构建、发布、配置与凭据说明见仓库根目录 [`README.md`](../README.md)。

## 当前架构（v1.0.2 起）

| 窗口 | 文件 | 说明 |
|---|---|---|
| 作业卡悬浮窗 | `src/win/homework.rs` | 可拖动、拖边缘缩放、隐藏/恢复；右下角铅笔编辑 + ✕；顶部通知条支持超长文本滚动 |
| 通知跑马灯 | `src/win/marquee.rs` | 默认宽度是可用宽度的 2/3；课间来新通知临时置顶弹出，上课前 30 秒收回 |
| 大屏编辑器 | `src/win/editor.rs` | 只保留「编辑今日作业 / 班干部值周 / 卫生值日 / 关闭」，预览不改壁纸 |
| 服务循环 | `src/serve.rs` | 不再渲染/替换壁纸；仅在检测到旧版遗留壁纸时自动还原一次 |

`src/render.rs` 里的整屏渲染只用于 `--render-only` 预览与编辑器示意，正常运行时
只用 `render_card_parts()` 生成透明作业卡。

## 常用命令

```bash
desktop_board.exe                                 # 作业卡 + 跑马灯
desktop_board.exe --render-only out.png           # 只出整屏预览图，不改壁纸
desktop_board.exe --hw-preview out.png [offset]   # 只出作业卡透明 PNG（offset 看滚动帧）
desktop_board.exe --marquee-only                  # 只跑横幅
desktop_board.exe --stop                          # 让已运行实例退出
```

## 数据来源（与主程序共用 `~/.RollCallData/`，不复制）

| 文件 | 用途 |
|---|---|
| `desktop_board.json` | 今日作业、值日、横幅/作业卡显隐与位置 |
| `board_notice.txt` | 主程序写入的通知正文（只读） |
| `schedules/class_*.json` | 各班课表 |
| `config.json` | 当前班级、学生名单、作息 |
| `*.flag` | 跨进程协作标记（切班强制弹出、下课补弹等，带 TTL，消费即删） |
| `desktop_board.stop` | 退出信号 |

## 关键实现约束

- 分层窗口像素是**预乘 alpha**；渐隐时必须同时缩放 rgb 与 alpha，否则淡出的字偏白。
- `WS_EX_NOACTIVATE` 窗口收不到普通光标复位与滚轮消息：类 `hCursor` 必须设
  `IDC_ARROW` 并处理 `WM_SETCURSOR`，缩放改为拖边缘/拖把手。
- 作业卡只在桌面前台时显示（`desktop_in_front()`），避免浮在 WPS 等应用上吞点击。
- 编辑器与大屏使用**不同互斥体**，否则点作业卡打不开编辑器。
- `board::patch_keys()` 只改指定键，避免整份 `Board::load()` 默认值回填覆盖用户内容。

## 编译

见根 `README.md`；本机用 `cargo +stable-x86_64-pc-windows-gnu build --release`，
PATH 需包含 w64devkit 的 `bin`。改动本 crate 后**必须重新编译主程序**。

`cargo test --release` 覆盖作息判定、横幅弹出时长、通知滚动状态机、竖排标签、
按钮命中区、值日字号等。
