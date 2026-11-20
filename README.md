# 教室随机点名系统 · Rust 版

Python 版 `main202609245.py` 的全量 Rust 移植。纯 Win32 FFI + 手写 32 位 DIB
分层窗口，无 Qt / Tk 运行时依赖，单文件 exe 直接运行。

> **源码仓库**：Gitee 私有仓库 `gzzxgch/classroom-rollcall-rs`（唯一构建来源）
> **源码镜像**：同一份源码同步到私有课件仓库 `gzzxgch/kejian` 的 `daima/rust/`
> **发布通道**（version.json + 更新用 exe）：Gitee `gzzxgch/kejian`，与源码仓库分离

---

## 组成

| 目录 | 说明 | 产物 |
|---|---|---|
| `classroom_rs/` | 主程序：悬浮侧边条、通知横幅、半屏通知卡、点名/倒计时窗、设置窗口(egui)、Gitee 同步、GoEasy 实时推送（订阅+发布）、下课守护 | `classroom_rollcall.exe` |
| `desktop_board_rs/` | 班级桌面大屏：透明悬浮作业卡 + 顶部通知跑马灯 + 单窗口三页设置面板；同时作为 lib 被主程序 path-dependency 引用 | `desktop_board.exe` / `desktop_board` lib |
| `dist/` | 本地发布目录（**不入库**，由脚本生成） | 固定名 exe + 时间戳副本 |
| `assets/` | 随包运行时资源：`goeasy-lite.min.js`（GoEasy SDK，构建时复制进 dist） | — |
| `scripts/` | 版本校验、构建、发布、打 tag、建仓、安全推送 | `release.py` |
| `version.json` | **唯一发布版本源** + 自动更新清单 | — |
| `CHANGELOG.md` | 版本变更记录 | — |

`classroom_rs` 通过 `path = "../desktop_board_rs"` 依赖大屏 lib；**改过大屏源码后
必须重新编译主程序**，两者共用同一份 `Cargo.lock` 之外的依赖解析。

---

## 环境要求

- Windows 10/11，Rust `stable-x86_64-pc-windows-gnu`
- MinGW-w64 工具链（本机为 `C:\Users\gchui\w64devkit\w64devkit\bin`，需在 PATH 中）
- Python 3.8+（仅发布脚本用，运行时不依赖）

```bash
export PATH="/c/Users/gchui/w64devkit/w64devkit/bin:$PATH"

# 单测（两个 crate）
cd desktop_board_rs && cargo +stable-x86_64-pc-windows-gnu test --release && cd ..
cd classroom_rs  && cargo +stable-x86_64-pc-windows-gnu test --release && cd ..

# 构建
cd desktop_board_rs && cargo +stable-x86_64-pc-windows-gnu build --release && cd ..
cd classroom_rs  && cargo +stable-x86_64-pc-windows-gnu build --release && cd ..
```

产物在各自 `target/release/`。日常直接用发布脚本，见下一节。

> 若 cargo 因时间戳异常一直显示 `Fresh`，用 `touch` 把改动的源码时间调到当前之后
> 再构建（本机曾出现产物 mtime 超前于系统时间的情况）。

---

## 一键发布脚本

```bash
python scripts/release.py check          # 校验四处版本号一致 + 两个 crate 单测
python scripts/release.py build          # check + release 构建 + 同步 dist/ + 生成更新包
python scripts/release.py tag            # 按 version.json 打 vX.Y.Z tag（先 commit）
python scripts/release.py push           # 推送源码到远端，token 不写入 .git/config
python scripts/release.py push --sync-kejian # 推送后顺带同步源码镜像到 kejian
python scripts/release.py sync-kejian --dry-run  # 只核对镜像差异，不写远端
python scripts/release.py sync-kejian   # 增量同步源码到 kejian 的 daima/rust/
python scripts/release.py publish --dry-run  # 只核对发布通道，不写远端、不触发升级
python scripts/release.py publish        # build + 上传 version.json / exe 到 kejian
python scripts/release.py bump 1.0.3     # 同步修改四处版本号
python scripts/release.py create-repo    # 一次性：调用 Gitee API 新建私有仓库
```

日常推送请用 `release.py push`（脚本从 `config.json` 读 token 并用一次性 URL，
不把凭据写进 `.git/config`）；直接 `git push` 需要自己配置 Gitee 凭据。

`sync-kejian` 把 **git HEAD 中已跟踪的文件**按原样镜像到私有课件仓库
`gzzxgch/kejian` 的 `daima/rust/` 下（读 `git cat-file blob` 取规范 LF 内容，
避免 Windows CRLF 造成整文件 diff）。用 Git blob SHA-1 与 Gitee 返回的 `sha`
比对做增量同步，已一致的文件不重复上传。**路径隔离是硬约束**：只允许写
`daima/rust/` 子目录，绝不触碰 kejian 根目录的 `version.json`、`dist/app.exe`
与 `tongzhi/` 通知数据，因此同步源码不会触发客户端自升级。先跑
`--dry-run` 看清单，确认无误再实际写入；`push --sync-kejian` 适合日常一条命令
同时更新两个仓库。仓库里的 0 字节占位文件（如 `desktop_board_rs/src/.keep`）会写成
一个换行，因为 Gitee Contents API 拒绝空 `content`；比对与上传统一处理，重复同步
不会反复重写。同步时还会把镜像目录里**本地已删除的文件一并清理**（本轮删掉的
`classroom_rs/build.rs` 就是这样移除的），保证 `daima/rust/` 严格等于 git HEAD；
`--dry-run` 会先列出「将删除」清单。该目录是自动生成区，请勿手动往里放文件。

`build` 会生成 `dist/classroom_rollcall.exe`、`dist/desktop_board.exe`、
`dist/app.exe`（自动更新用的主程序副本）、`dist/version.json`，以及带时间戳的副本。

`publish` **必须显式调用**，默认任何操作都不会触碰自动更新通道，避免误触发
客户端自替换。

---

## 运行方式

```
classroom_rollcall.exe                 主程序（悬浮条 + 通知 + 点名窗）
classroom_rollcall.exe --settings      设置窗口（独立进程，经 config.json 与主程序同步）
classroom_rollcall.exe --history       历史记录窗口
classroom_rollcall.exe --guard         无窗口下课守护（主程序退出后仍常驻）
classroom_rollcall.exe --selftest      打印配置 / 作息 / 版本自检，不出窗口
desktop_board.exe                      独立运行桌面大屏
desktop_board.exe --render-only out.png 只出整屏预览图，不改系统壁纸
desktop_board.exe --hw-preview out.png [offset]  只出作业卡透明 PNG
```

设置窗口与主程序是**不同进程**：改完配置通过 `config.json` 的毫秒级 mtime 热加载
（约 0.3 秒生效），创建/切换班级另写 `class_switch.flag` 保证立即同步通知。

作业栏右下角三个按钮从左到右：**电源（彻底关闭）** · 铅笔（编辑作业 / 值日） ·
红色 ✕（本次隐藏，下次启动自动恢复）。点电源会**先弹出确认框**，必须输入确认码
`123` 并点「确定」才会写入持久标志 `homework_hard_hidden=true`；取消 / 关闭窗口 /
ESC / 输错都不会关闭作业栏。写入后重启主程序、重启大屏、右键「恢复通知与今日作业」
都不会把它打开；**只有设置窗口「基本 → 桌面作业栏」里的「显示桌面作业栏」开关能重新开启**，
取消勾选该开关同样要输入 `123` 二次确认，重新勾选则立即生效、无需确认。

**点击作业栏任意位置**（含铅笔按钮）直接打开「班级桌面大屏设置」面板：一个窗口里
只有三块内容，顶部切换「今日作业 / 班干部值周 / 卫生值日」，底部统一「保存 / 关闭」。
不再有壁纸预览、中间选择界面和子窗口；预览渲染链路已整体删除（既不占 exe 体积，
也不再分配 1120×630 的运行时位图）。改完点保存，大屏靠 400ms 轮询读 `desktop_board.json`
自动刷新。

**本机想回到 0 班**：设置窗口「基本 → 班级管理」有「🧹 清空所有班级（回到 0 班）」，
点一次进入确认态、再点「确定清空」即写入空班级列表；执行前自动把 `config.json`
备份成 `config.json.bak-clear-<时间戳>`。主程序靠配置热加载在约 0.3 秒内同步。

---

## 通知秒级推送与半屏弹屏

**数据源始终是 Gitee，GoEasy 只当门铃。** 通知正文写在 `tongzhi/{班级文件名}`；
发布方写完正文后广播一条 `{f: 班级文件名, a: 动作, text: 正文, t: 时间戳}`，
各教室的无头订阅页按 `f` 过滤后 fetch 本机 `127.0.0.1:18765/wake` → 主程序
`UiMsg::GoEasyWake` → **1~2 秒内**重新拉取 Gitee 通知。门铃丢了还有 60/90 秒轮询兜底。

**广播正文秒显（默认开）**：网页端已经在广播里带了 `text`，订阅页会把它一起送到
`/wake`，主程序先上屏（横幅 + `board_notice.txt` 给桌面大屏），省掉一次 Gitee 往返；
随后异步回拉 Gitee 用权威文本覆盖。安全约束：秒显期间**不弹半屏、不语音播报**，
只有 Gitee 回拉确认后才允许；30 秒未确认自动撤回预览。正文限 600 字、剔除控制字符，
别班广播直接丢弃。这样即使 Common Key 泄露被人推了伪造文字，也不会被朗读到全班。
在「Web 与仓库 → GoEasy 实时推送」取消勾选「⚡ 广播正文秒显」即可退回
「严格以 Gitee 为准」（多等一次网络往返）。

**主发布端一直在网页里（不在本仓库）**：手机/浏览器打开的
`https://gch365.github.io/physics-html/notify/` 内置 Common Key，用 GoEasy JS SDK
`goEasy.pubsub.publish({channel: "gzzx_class_notice_v1", message: ...})` 在
「保存通知 / 批量通报 / 删除班级 / 发测试」成功后广播 `{f, a, text, t}`；
教室端订阅页按 `f` 与本机 `current_class` 比对后唤醒。**秒级链路本来就是通的**，
条件是网页端「⚡ 实时推送」处于已连接（`pushReady=true`）；未连接时页面
`publishNotice()` 会静默降级为轮询，这才是「感觉不秒级」最常见的原因。
本仓库 Rust 源码只有订阅端，先前仅凭本地 grep 得出「项目没有发布端」的判断
不准确，在此更正。

**本次新增的是 PC 端第三个发布入口**：设置窗口「定时通知」页的
「📣 立即发布通知 / 🔔 仅广播 / 🧹 清空本班通知」，用 REST publish
`https://rest-hz.goeasy.io/v2/pubsub/publish`（新加坡区域
`rest-singapore.goeasy.io`）广播同一种 `{f,a}` 门铃，好处是管理员不打开网页
也能发通知并立即唤醒教室。与网页端同频道、同格式，可同时使用、互不冲突。

**发布密钥只对 PC 入口是必需的**：教室端用 `BS-` Subscribe Key 订阅是**正确用法**
（只收不发、权限最小），不需要也不应该换成 Common Key；网页端用自己内置的
Common Key 建立连接后发布，也不受本机 `BS-` 限制。本机实测 `BS-` 直接调
REST publish 两个区域都返回 `400 appkey is invalid`，所以 PC 入口需要管理员在
「Web 与仓库 → GoEasy 实时推送」把 `BC-` Common Key / Rest Key 填到
`goeasy_publish_key`（或环境变量 `ROLLCALL_GOEASY_PUBLISH_KEY`）。
**不填也不影响日常使用**：通知照常写入 Gitee，教室靠网页端广播或轮询收到，
只是 PC 端那一次广播会跳过并在状态栏给出提示。「🧪 测试发布密钥」只发到
`{频道}__selftest__`，不会影响任何教室。

⚠️ 网页端的 Common Key 已内置在公开页面（做了简单混淆）。若以后自建发布页，
应由服务端代理调用 REST publish，不要把 Common Key 明文写进静态页面：
任何拿到它的人都能向全校频道群发通知。

**半屏弹屏**：通知正文含「通知」「重要通知」「重要消息」且在**课间 / 放学后**
（上课、周休、课前 30 秒不弹）时，屏幕正中弹出约 62%×56% 的卡片，
同时用 Windows 自带 SAPI 语音播报三遍。卡片**没有超时自动关闭**，点其它位置
也不会关，只有右下角「✕ 关闭」按钮（或 Esc）能收起；不点就一直显示。
桌面大屏 / 作业栏常驻不再抑制弹屏；作息切换时由 30 秒轮询统一收起。

---

## 配置与凭据

运行时数据全部在 `~/.RollCallData/`（**不入库**）：

| 文件 | 用途 |
|---|---|
| `config.json` | 主配置：班级名单、Gitee 仓库、GoEasy、作息、显示选项 |
| `desktop_board.json` | 大屏内容：今日作业、值日、横幅显隐与位置；`homework_hard_hidden` 是「彻底关闭作业栏」的持久标志 |
| `board_notice.txt` | 主程序写给大屏的当前通知正文 |
| `class_switch.flag` / `notice_popup.flag` / `break_popup.flag` | 跨进程协作标记（带 TTL，消费即删） |
| `schedules/class_*.json` | **本班**课表 |
| `app.log` / `board_quit.log` | 运行日志 |

**本机不再保存全校名单。** `dist/seed/` 与 exe 内置名单已全部移除；
教室电脑里只有 `config.json` / `schedules/` 中**已创建班级**的名单。
管理员构建机上的 `seed/seed.json` 只是生成上传源的一次性中间文件，不参与运行。

**源码里不含任何密钥。** 全新部署（无 `config.json`）时从环境变量注入：

| 变量 | 用途 |
|---|---|
| `ROLLCALL_GITEE_TOKEN` | Gitee 私人令牌（通知同步 / 定时通知 / 自动更新 / 发布脚本 / 上传名单） |
| `ROLLCALL_GOEASY_APPKEY` | GoEasy Subscribe Key（`BS-` 开头，教室端订阅唤醒用） |
| `ROLLCALL_GOEASY_PUBLISH_KEY` | GoEasy **发布**密钥（`BC-` Common Key / Rest Key），只配管理员机；留空回退订阅 AppKey |
| `ROLLCALL_CLASS_DATA_TOKEN` | 云端名单仓库的**只读令牌**，发到各教室；留空回退 `gitee_token` |
| `ROLLCALL_SEED_FILE` | **仅管理员上传时**：指定 `upload_class_data.py` 的源 seed.json |

参考 `.env.example`。程序**不会自动加载 `.env`**：请设为系统环境变量，或直接
在设置窗口「Web 与仓库」页填入（会写进 `config.json`）。已有配置文件的用户
无需做任何改动。

---

## 班级名单与课表：私有仓库按班拉取

全校名单**不编进 exe、不随发布包分发、不进源码仓库**，只放在独立私有仓库
`gzzxgch/classroom-data`（`.gitignore` 排除 `*.xls*` 与 `**/seed.json`）：

```
banji/{学校}/{入学年份}/{01..31}.json      # 一个班一个文件
```

每个文件是自校验的 JSON：`version / school / year / class_num / students / schedule`。
程序创建班级时按班号**只请求那一个文件**，拉到后写入本班 `config.json` 与
`schedules/`，本机从此不出现全校 1530 人的整包数据。

### 管理员：生成并上传

```bash
python scripts/build_seed.py --dry-run                  # 只打印统计
python scripts/build_seed.py                           # Excel → ~/.RollCallData/seed/seed.json
python scripts/upload_class_data.py --dry-run           # 预演会写哪些文件
python scripts/upload_class_data.py --create-repo --apply   # 首次：建私有仓库 + 上传
python scripts/upload_class_data.py --apply            # 换年级时重跑（增量幂等，只写变化的文件）
```

`build_seed.py` 依赖 `openpyxl`（.xlsx）与 `xlrd`（.xls），仅离线转换用；
学生名单按班号分组（31 个班 / 1530 人）、允许跨班重名；课表只取周一~周五
节次 1~12，早读不占 period，星期日丢弃。`--apply`（build_seed）是管理员
一次性批量导入工具，会一次建 31 个班，**发布版不要用**。

上传脚本用 Git blob SHA-1 与远端 `sha` 比对，重复执行「写入 0，已一致 31」。
本次已建仓并上传 31 个文件（合计约 173 KB）。

### 教室端：创建班级即拉取

- 点「➕ 创建班级」→ **立刻建空班级并落盘** → 后台按
  `{dir}/{学校}/{届}/{NN}.json` 拉本班 → 回填名单、写课表、自动保存。
- 云端有哪一届的数据就带出哪一届，不再硬编码「只有高二」；
  云端没有该届就保持空班级，自行导入。
- 断网 / 只读令牌失效 / 找不到该班：**班级照常创建**，界面提示原因，
  联网后点「☁ 从云端补全当前班」重试。
- 「基本 → 班级管理」显示拉取状态与重试按钮；「Web 与仓库 → 云端班级名单」
  可配开关、owner / repo / 数据目录 / 只读令牌。

### 权限与令牌（重要）

Gitee 私人令牌只能在网页「设置 → 私人令牌」生成，API 无法代生成：

- **上传**用管理员的可读写令牌（`~/.RollCallData/config.json` 的 `gitee_token`），
  只留在构建机，绝不下发。
- **教室端**请另生成一个只勾读取权限的令牌，填到「Web 与仓库 → 云端班级名单 →
  只读令牌」（或环境变量 `ROLLCALL_CLASS_DATA_TOKEN`）。留空会回退到
  `gitee_token`，仅建议本机调试，**不要把可读写令牌分发到教室**。
- 注意：按班拆文件减少了传输量，但同一个只读令牌技术上仍能读到仓库内其它班文件。
  真正的隔离靠「只读令牌 + 每台机器只创建自己的班」，而不是靠文件名。
- `release.py publish` 只上传 `version.json` 与 `dist/app.exe`，
  **名单永远不会进发布通道 / kejian 镜像**。

### 其他教室的使用规则

- 全新安装 **初始班级为空**（不再预置 9 班）；首次使用在「基本 → 班级管理」创建。
- 每个教室默认 **最多 2 个班级**；已有 2 个班后继续创建，要在设置窗口输入解锁密码
  `123321`（本次设置窗口会话内有效，关窗后需重新输入）。
- 发布 zip 里只有 `classroom_rollcall.exe` / `desktop_board.exe` /
  `goeasy-lite.min.js` / `version.json` / `使用说明.txt`，**没有 seed 目录**。
- 已删除「↩ 恢复默认9班」按钮，避免全新版本里又冒出内置班级。

### 设置窗口控件裁剪

「基本」页已移除 **通知颜色 / 侧边栏水平间距 / 通知位置 / 通知位置微调** 四个控件，
位置与大小一律在横幅、侧边条上直接拖动或用滚轮调节。对应配置字段仍保留，
兼容旧 `config.json`。

---

## 自动更新链路

```
启动时 8s → 读 kejian/version.json（Contents API + token）
          → 与 theme.rs::APP_VERSION 三元组比较
          → 有新版本则取 exe_path（默认 dist/app.exe，raw 403 时降级 blobs API）
          → 写临时 exe + updater.bat → 隐藏窗口启动并退出 → 批处理覆盖并重启
```

- 程序配置 `gitee_owner / gitee_repo / update_branch / update_exe_path` 同时服务于
  **通知数据同步**（仓库 `tongzhi/` 目录），因此**不要改成源码仓库**。
- 源码仓库只负责代码、tag、CHANGELOG；发布产物由 `release.py publish` 推到
  `gzzxgch/kejian`。`kejian` 本身是私有课件仓库，同时存放 `daima/`（旧版 Python
  源码）、`tongzhi/`（通知数据）、`dist/` 与 `version.json`（自动更新）。
  Rust 源码镜像固定写在 `daima/rust/`，与这些既有目录互不干扰。
- 发布前务必确认 `version.json` 的 `exe_path` 与上传到 kejian 的路径一致。

---

## 版本规则

1. 改版本号只用 `python scripts/release.py bump X.Y.Z`，它会同步
   `version.json`、`theme.rs`、两个 `Cargo.toml`，避免手改漏一处。
2. 在 `CHANGELOG.md` 补上该版本条目。
3. `check` → `build` → 提交 → `tag` → `push` →（需要发版时）`publish`。
4. tag 格式 `vX.Y.Z`，与 `version.json` 一致。

---

## 已知限制

- 仅 Windows（Win32 FFI / layered window / 注册表自启）。
- 工具或脚本启动的 GUI 进程在部分会话中会被回收，长期驻留需手动双击 exe。
- `desktop_board_rs` 的 `verify/` 目录是 Python 对照脚本，仅用于出图核对，非运行时依赖。
- 旧版 Python 参考实现 `classroom_rs_ref/` 含历史硬编码凭据，已排除出版本库。
- 大屏作业卡只在桌面前台时显示（`desktop_in_front()`），避免浮在其它应用上吞点击。
