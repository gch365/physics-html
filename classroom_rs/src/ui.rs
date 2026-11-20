//! 主程序悬浮 UI：侧边条、通知横幅、全屏重要通知、倒计时、点名窗。
//!
//! 与 Python 版同构：所有窗口在主线程，后台线程通过 mpsc 队列发消息，
//! 根窗口 100ms 轮询队列（对应 _poll_queue），30 秒轮询上课状态（_check_class_loop）。

use crate::config::Config;
use crate::gitee::{fetch_notice_text, is_valid_notice};
use crate::history::HistoryManager;
use crate::logging;
use crate::schedule;
use crate::theme::*;
use crate::text::{Anchor, Fonts};
use crate::win::layered;
use crate::win::sys::*;
use chrono::Timelike;
use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tiny_skia::{Color, Paint, Pixmap, Transform};

// ---- 定时器 ID ----
const T_POLL: usize = 1; // 100ms 队列轮询
const T_CLASSCHECK: usize = 2; // 30s 上课状态轮询
const T_MINUTE: usize = 3; // 1s 侧边条时间更新
const T_CD_TICK: usize = 10; // 倒计时 200ms
const T_ROLL: usize = 11; // 16ms 点名动画专用定时器（100ms 轮询会把每步延时取整拉长）

// ---- 右键菜单命令 ----
const CMD_SETTINGS: usize = 101;
const CMD_CLOSE_NOTICE: usize = 103;
const CMD_QUIT: usize = 108;
const CD_CMD_SUB: usize = 111;
const CD_CMD_ADD: usize = 112;
const CD_CMD_CLOSE: usize = 113;
const ROLL_CMD_SETTINGS: usize = 114;
const ROLL_CMD_HISTORY: usize = 115;
const ROLL_CMD_QUIT: usize = 116;
const CMD_RESTORE_NOTICE: usize = 118;

// ---- 消息 ----
pub(crate) enum UiMsg {
    NoticeFetched(Option<String>),
    /// 下课转换 / 17 点后静默过期等时机拉回的通知：无视手动关闭立即显示。
    NoticeFetchedForce(Option<String>),
    /// 创建 / 切换班级后拉回的新班通知：强制立即显示，不等课间、不受手动关闭影响
    ClassNoticeFetched(Option<String>),
    PreloadFetched(Option<String>),
    /// GoEasy 门铃；网页端广播里带的正文作为「秒显预览」，需 Gitee 回拉确认
    GoEasyWake { quick_text: Option<String> },
    /// GoEasy 文件信号（网页网络U盘上传完成）：立即唤醒 Gitee 同步线程下载
    GoEasyFiles,
    SyncNewFiles(Vec<String>),
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Clone)]
struct HitButton {
    rect: (f32, f32, f32, f32),
    cmd: usize,
}

struct CountdownState {
    remaining: i64,
    running: bool,
    start_time: f64,
    shown: String, // 当前显示文本（"MM:SS" 或 "时间到！"）
    ended: bool,
    buttons: Vec<HitButton>,
    pos: (i32, i32),
}

struct RollState {
    pos: (i32, i32),
    buttons: Vec<HitButton>,
}

struct DragState {
    hwnd: HWND,
    start_x: i32,
    start_y: i32,
    win_x: i32,
    win_y: i32,
    active: bool,
    press_time: f64,
    long_press_done: bool,
    is_bar: bool,
}

pub struct App {
    pub cfg: Arc<Mutex<Config>>,
    pub history: Arc<HistoryManager>,
    pub ui_rx: Receiver<UiMsg>,
    pub ui_tx: Sender<UiMsg>,
    pub fonts: Fonts,
    pub root: HWND,

    // ---- 通知横幅 ----
    notice_hwnd: HWND,
    notice_visible: bool,
    notice_text: String, // 组合后的显示文本
    /// 通知横幅滚轮缩放 / 拖动后的待落盘时刻
    notice_dirty: Option<f64>,
    notice_rebuild_at: f64,
    /// 通知横幅拖动：(起点鼠标 x, y, 起点偏移 x, y, 是否已判定为拖动)
    notice_drag: Option<(i32, i32, i32, i32, bool)>,
    schedule_text: String,
    cached_notice: Option<String>, // None=未知 ""=确认空
    /// 后台拉取进行中标志：避免 1 秒 tick 里反复 spawn 网络线程
    fetch_inflight: bool,
    preload_done: bool,
    last_period: Option<(String, String)>,
    /// 课间空闲兜底拉取的上次时刻：课间横幅空/不可见时最多 5 分钟拉一次
    /// （Gitee 上无有效通知时不再 30 秒重拉；新通知靠 GoEasy 门铃事件触发）
    last_idle_fetch_at: Option<f64>,
    /// 上课期间兜底轮询的上次时刻：门铃失联时每 120 秒拉一次
    /// （门铃健康时上课期间零轮询，此字段只被预拉取路径更新）
    last_class_fetch_at: Option<f64>,
    /// 上次门铃健康状态：变化时打日志（健康↔失联切换可追溯）
    doorbell_was_ok: Option<bool>,
    /// 新通知强制弹出的保持期：期间上课分支不收起横幅；手动关闭立即失效
    banner_hold_until: Option<f64>,
    closed_until: Option<chrono::DateTime<chrono::Local>>,
    was_class_time: Option<bool>,
    startup_hide: bool,
    startup_seen: Option<String>,
    board_notice_last: Option<String>,
    last_schedule_refresh: f64,
    /// 半屏居中通知卡窗口（不点「关闭」就一直显示，没有自动超时）
    popup: Option<HWND>,
    /// 卡片右下角「关闭」按钮命中区，窗口过程只认这里
    popup_close_rect: (f32, f32, f32, f32),
    last_popup_text: Option<String>,
    /// Some(秒显开始时刻)：当前屏幕上是未经 Gitee 确认的广播预览。
    /// 期间禁止半屏弹屏与语音播报；30 秒仍未被确认就撤回预览。
    quick_display_at: Option<f64>,

    // ---- 同步新文件提醒 ----
    sync_notice_hwnd: Option<HWND>,
    sync_notice_close_at: Option<f64>,

    // ---- 侧边条 ----
    bars: [Option<HWND>; 2],
    bar_size: Option<(i32, i32)>,
    last_minute: i32,

    // ---- 倒计时 / 点名 ----
    cd: Option<CountdownState>,
    cd_hwnd: Option<HWND>,
    roll: Option<RollState>,
    roll_hwnd: Option<HWND>,
    roll_var: String,
    rolling: bool,
    roll_show_at: Option<f64>,

    // ---- 交互 ----
    drag: Option<DragState>,
    last_touch: f64,
    /// 滚轮调节侧边栏：最后调节时刻（None=空闲）；T_POLL 里限流重建 + 停手后落盘
    bar_dirty: Option<f64>,
    bar_rebuild_at: f64,
    #[allow(dead_code)] // 预留：同步提醒去重（对应 Python sync_seen_files）
    sync_seen_files: bool,
    cfg_mtime: Option<u64>,
    /// 上次检查 config.json mtime 的时刻（秒），用于 250ms 节流
    last_cfg_check: f64,
    /// 班级切换去重：mtime 变化与 class_switch.flag 可能在相邻两拍先后到达，
    /// 3 秒内同一个班只做一次「清缓存 + 回拉 + 强制弹出」。
    last_switch_sync: Option<(String, f64)>,
    /// update_notice 的一次性「忽略上课 / 周末抑制」开关
    force_notice_once: bool,
    /// 新班通知已落盘，poll_queue 末尾写大屏强制弹出标记
    pending_popup_flag: bool,
    /// 上次 (上课中, 课前预备期) 状态：update_minute 每秒比对，
    /// 变化即时触发 check_class_loop（下课弹出 / 上课前30秒收起不再等 30 秒轮询）
    last_class_state: Option<(bool, bool)>,
    /// 下课前 2 分钟预拉取：已拉取过的课节 (start,end)，每节课只拉一次
    preload_period: Option<(String, String)>,
    /// 预拉取触发时刻（秒）：下课瞬间若 3 分钟内拉过，缓存内容直接秒显横幅
    preload_at: Option<f64>,
}

impl App {
    fn cfg(&self) -> Config {
        self.cfg.lock().unwrap().clone()
    }

    fn students(&self) -> Vec<String> {
        let c = self.cfg();
        c.current().map(|x| x.students.clone()).unwrap_or_default()
    }

    // ==================== 通知横幅 ====================

    fn update_schedule_text(&mut self) {
        let c = self.cfg();
        let class_id = c.current().map(|x| x.file_name.clone()).unwrap_or_default();
        let sch = schedule::load_schedule(&class_id);
        self.schedule_text = schedule::get_next_lesson(&sch, &c.work_time).unwrap_or_default();
    }

    /// 后台线程拉取通知，结果经 UiMsg 回 UI 线程——**绝不阻塞消息循环**。
    /// 「横幅点不到 / 关闭迟缓」的根因就是旧版在 UI 线程同步做 2 次网络请求，
    /// 网络一慢整个窗口几秒不响应，点击全部排队或丢失。
    /// fetch_inflight 去重：拉取期间 1 秒 tick 里的重复触发直接忽略。
    fn fetch_async(&mut self, force: bool) {
        if self.fetch_inflight {
            return;
        }
        let c = self.cfg();
        if c.current().is_none() {
            return;
        }
        self.fetch_inflight = true;
        let tx = self.ui_tx.clone();
        let cfg = self.cfg.clone();
        std::thread::spawn(move || {
            let c = cfg.lock().unwrap().clone();
            let r = fetch_notice_text(&c);
            let msg = if force {
                UiMsg::NoticeFetchedForce(r)
            } else {
                UiMsg::NoticeFetched(r)
            };
            let _ = tx.send(msg);
        });
    }

    /// config.json mtime 变化 → 整体重载配置 + 作息 override + 自启注册表。
    /// 设置窗口是独立进程，靠此机制让主进程跟随最新配置。
    fn reload_config_if_changed(&mut self) {
        // 250ms 节流：调用方是 100ms 的 T_POLL，stat 很便宜但不必每帧都做
        let now = now_secs();
        if now - self.last_cfg_check < 0.25 {
            return;
        }
        self.last_cfg_check = now;
        // 创建 / 切换班级时设置窗口会写显式标记：即使两次保存落在同一秒、
        // 或 mtime 精度不够，也能保证「立刻切换」不会漏掉。
        let switched = crate::paths::take_class_switch_flag();
        let mtime = crate::paths::file_mtime_ms(&crate::paths::config_file());
        if self.cfg_mtime.is_none() {
            self.cfg_mtime = Some(mtime);
            if switched.is_none() {
                return;
            }
        }
        if switched.is_none() && (self.cfg_mtime == Some(mtime) || mtime == 0) {
            return; // 未变化 / 文件暂不可读
        }
        self.cfg_mtime = Some(mtime);
        crate::paths::trace("reload: begin");
        // 旧自启设置（用于比较是否真的变化）
        let (old_auto, old_class) = {
            let c = self.cfg.lock().unwrap();
            (c.auto_run, c.current_class.clone())
        };
        let new_cfg = crate::config::Config::load();
        let (new_auto, new_class) = (new_cfg.auto_run, new_cfg.current_class.clone());
        crate::paths::trace("reload: cfg loaded");
        crate::schedule::load_time_overrides();
        crate::paths::trace("reload: overrides loaded");
        {
            let mut c = self.cfg.lock().unwrap();
            *c = new_cfg;
        }
        crate::paths::trace("reload: cfg swapped");
        // 自启注册表：仅变化时更新，且放后台线程——注册表 Run 键写入可能被
        // 安全管控/杀毒软件挂起（实测沙箱内 RegDeleteValueW 阻塞 >100s），
        // 绝不能卡主线程消息泵。
        if old_auto != new_auto {
            let ar = new_auto;
            let _ = std::thread::Builder::new().name("auto-run".into()).spawn(move || {
                crate::win::set_auto_run(ar);
                crate::paths::trace("reload: auto_run done");
            });
        }
        self.bar_size = None; // 触发侧边条按新配置重建
        logging::info("检测到配置变更，已热加载 config.json");
        crate::paths::trace("reload: done");
        self.update_bars(); // 立即按新配置重建侧边条（update_bars 内部按尺寸差异决定销毁重建）
        self.refresh_notice_bar(); // 通知横幅大小/位置按新配置立即生效
        self.resize_floating_windows(); // 点名窗/倒计时窗的大小按新配置立即生效
        if old_class != new_class || switched.is_some() {
            self.on_class_switched(&old_class, &new_class, switched.is_some());
        }
    }

    /// 班级切换（含新建班级）：清旧班缓存 → 立刻回拉新班通知 → 强制显示 → 通知大屏弹出。
    ///
    /// 与 30 秒轮询 / GoEasy 唤醒不同，这是用户显式操作，因此：
    /// 不等课间、不受「上课期间攒着下课再弹」限制、不受之前的手动关闭影响。
    fn on_class_switched(&mut self, old: &str, new: &str, signaled: bool) {
        // mtime 与显式标记可能在相邻两拍先后到达，同一个班 3 秒内只做一次
        if let Some((c, t)) = &self.last_switch_sync {
            if c == new && now_secs() - *t < 3.0 {
                return;
            }
        }
        self.last_switch_sync = Some((new.to_string(), now_secs()));
        logging::info(&format!("班级已切换：{old} → {new}，立即同步并弹出通知"));
        crate::paths::trace(&format!("class-switch: {old} -> {new}, signal={signaled}"));

        // 旧班的一切缓存都作废，否则新班会短暂沿用旧班通知 / 全屏去重
        self.cached_notice = None;
        self.fetch_inflight = false;
        self.last_popup_text = None;
        self.startup_seen = None;
        self.closed_until = None;
        self.startup_hide = false;
        self.update_schedule_text();

        // 先把大屏上旧班通知抹掉：万一新班没通知 / 网络失败，也不能残留旧班内容。
        // poll_queue 末尾会按 cached_notice 再写一次新班正文。
        let _ = std::fs::write(crate::paths::data_dir().join("board_notice.txt"), "");
        self.board_notice_last = None;

        let tx = self.ui_tx.clone();
        let cfg = self.cfg.clone();
        std::thread::spawn(move || {
            let c = cfg.lock().unwrap().clone();
            let t = fetch_notice_text(&c);
            let _ = tx.send(UiMsg::ClassNoticeFetched(t));
        });
    }

    /// 设置里改了「点名窗 / 倒计时窗大小」后，已打开的浮窗立即跟着缩放重绘。
    fn resize_floating_windows(&mut self) {
        let c = self.cfg();
        let (sw, sh) = crate::win::screen_size();
        let max_w = (sw as f64 * WIN_MAX_SCREEN_RATIO) as i32;
        let max_h = (sh as f64 * WIN_MAX_SCREEN_RATIO) as i32;
        if let Some(hwnd) = self.roll_hwnd {
            let scale = roll_win_scale(&c);
            let w = ((ROLL_W_DEF as f64 * scale) as i32).min(max_w);
            let h = ((ROLL_H_DEF as f64 * scale) as i32).min(max_h);
            let (x, y) = self.roll.as_ref().map(|r| r.pos).unwrap_or((0, 0));
            layered::move_to(hwnd, x, y, w, h);
            let (fs, bf) = roll_fonts(&c);
            paint_roll(self, fs, bf);
        }
        if let Some(hwnd) = self.cd_hwnd {
            let (cw_px, h_px) = cd_window_size(&c);
            let w = cw_px.min(max_w);
            let h = h_px;
            let (x, y) = self.cd.as_ref().map(|s| s.pos).unwrap_or((0, 0));
            layered::move_to(hwnd, x, y, w, h);
            paint_countdown(self);
        }
    }

    fn try_preload(&mut self) {
        let work_mode = self.cfg().work_time.clone();
        let period = schedule::current_class_period(&work_mode);
        match period {
            None => {
                self.preload_done = false;
                self.last_period = None;
            }
            Some((start, end)) => {
                let key = (start.clone(), end.clone());
                if self.last_period != Some(key.clone()) {
                    self.preload_done = false;
                    self.last_period = Some(key);
                }
                if self.preload_done {
                    return;
                }
                let now = chrono::Local::now();
                let end_dt = chrono::NaiveTime::parse_from_str(&end, "%H:%M")
                    .map(|t| {
                        now.date_naive()
                            .and_time(t)
                            .and_local_timezone(chrono::Local)
                            .single()
                            .unwrap_or(now)
                    })
                    .unwrap_or(now);
                let remain_min = (end_dt - now).num_seconds() as f64 / 60.0;
                if remain_min > 0.0 && remain_min <= 5.0 {
                    self.preload_done = true;
                    let tx = self.ui_tx.clone();
                    let cfg = self.cfg.clone();
                    std::thread::spawn(move || {
                        let c = cfg.lock().unwrap().clone();
                        let r = fetch_notice_text(&c).map(|t| {
                            if is_valid_notice(&t) {
                                t
                            } else {
                                String::new()
                            }
                        });
                        let _ = tx.send(UiMsg::PreloadFetched(r));
                    });
                }
            }
        }
    }

    /// 30 秒一轮的上课状态机（对应 _check_class_loop）。
    fn check_class_loop(&mut self) {
        let work_mode = self.cfg().work_time.clone();
        let now_class = schedule::is_class_time(&work_mode);
        // 每天 17:15 之后通知默认弹出（不管晚自习是否上课）
        let force_show = schedule::force_notice_show_now();
        // 周日（周末）：不显示、不预拉取；周六为自习日按平时逻辑
        if schedule::is_weekend() && !force_show {
            self.set_notice_visible(false);
            self.close_popup();
            return;
        }
        if !now_class && !force_show {
            self.try_preload();
        }
        if now_secs() - self.last_schedule_refresh > 300.0 {
            self.update_schedule_text();
            self.last_schedule_refresh = now_secs();
        }
        if now_class && !force_show {
            // 新通知弹出保持期：上课期间收到的新通知横幅不立刻被收掉
            let holding = self
                .banner_hold_until
                .map(|t| now_secs() < t)
                .unwrap_or(false);
            if !holding {
                self.set_notice_visible(false);
            }
            self.close_popup();
            self.closed_until = None;
            // 下课准备：距本节课结束 ≤ 2 分钟时预拉取一次通知（每节课只拉一次），
            // 下课铃一响横幅即可秒显。
            if schedule::is_within_preload_window(&work_mode) {
                let period = schedule::current_class_period(&work_mode);
                if self.preload_period != period {
                    if let Some(p) = period {
                        self.preload_period = Some(p);
                        self.preload_at = Some(now_secs());
                        self.last_class_fetch_at = Some(now_secs());
                        self.sync_fetch_and_show(false);
                        logging::info("下课前 2 分钟预拉取通知（为下课横幅备好内容）");
                    }
                }
            }
            // 上课兜底轮询（门铃健康感知）：桥接页心跳正常 → 完全零轮询；
            // 心跳断档（页挂了/SDK 失败/连接掉线）→ 120 秒低频兜底。
            let doorbell_ok = crate::goeasy::doorbell_healthy();
            if self.doorbell_was_ok != Some(doorbell_ok) {
                logging::info(if doorbell_ok {
                    "GoEasy 门铃恢复（桥接心跳正常），上课期间停止兜底轮询"
                } else {
                    "GoEasy 门铃失联（桥接心跳超时），切换 120 秒兜底轮询"
                });
                self.doorbell_was_ok = Some(doorbell_ok);
            }
            let poll_secs = if doorbell_ok {
                f64::INFINITY
            } else {
                crate::theme::DOORBELL_DOWN_POLL_SECS
            };
            let poll_due = self
                .last_class_fetch_at
                .map(|t| now_secs() - t >= poll_secs)
                .unwrap_or(true);
            if poll_due {
                self.last_class_fetch_at = Some(now_secs());
                self.sync_fetch_and_show(false);
            }
            self.was_class_time = Some(true);
            return;
        }
        // 下课瞬间（上课→下课转换）：解除静默、弹出横幅。
        // 放在主分支之前：转换这一拍主分支看到 notice_visible 已为 true，不再重复拉取。
        if self.was_class_time == Some(true) && !now_class {
            self.closed_until = None;
            // 预拉取内容直接上屏：每节课的 Gitee 拉取只发生在下课前 2 分钟那一次。
            // 预取缺失/过期（如程序中途启动）才补拉一次；2 分钟窗口内的临场改动
            // 由 GoEasy 门铃事件触发拉取，不靠轮询。
            let cached = self.cached_notice.clone().unwrap_or_default();
            let preload_fresh = self
                .preload_at
                .map(|t| now_secs() - t < 180.0)
                .unwrap_or(false);
            if preload_fresh && is_valid_notice(&cached) {
                self.update_notice(&cached);
                logging::info("下课瞬间：预拉取内容秒显（本节课不再重复拉取）");
            } else {
                self.last_idle_fetch_at = Some(now_secs());
                self.sync_fetch_and_show(true);
            }
            self.preload_done = false;
            self.last_period = None;
        }
        let now = chrono::Local::now();
        // 上课前 30 秒（预备期）：横幅与弹屏一律收起并**短路返回**。
        // 旧版只在这里收起，后面同一拍的「课间显示分支」发现横幅不可见又调
        // update_notice 重新弹出——预备期永远收不起来（「上课前没及时关掉」的根因）。
        if !force_show && schedule::is_within_pre_close(&work_mode) {
            self.set_notice_visible(false);
            self.close_popup();
            self.was_class_time = Some(now_class);
            return;
        }
        // 课间（含放学后）：静默期内一律不显示；未静默且横幅空/不可见时
        // 兜底拉取——限流 5 分钟一次（横幅正常显示期间完全不拉，
        // 新通知由 GoEasy 门铃事件触发即时拉取，不靠轮询）。
        if self.silenced(&now) {
            self.set_notice_visible(false);
        } else if self.notice_text.is_empty() || !self.notice_visible {
            // 兜底限流：门铃健康 5 分钟一次；门铃失联收紧到 2 分钟（横幅
            // 空着说明上一条通知无效/被关，新通知只靠轮询到达）
            let idle_secs = if crate::goeasy::doorbell_healthy() {
                300.0
            } else {
                crate::theme::DOORBELL_DOWN_POLL_SECS
            };
            let idle_due = self
                .last_idle_fetch_at
                .map(|t| now_secs() - t >= idle_secs)
                .unwrap_or(true);
            if idle_due {
                self.last_idle_fetch_at = Some(now_secs());
                self.sync_fetch_and_show(true);
            }
        }
        self.was_class_time = Some(now_class);
    }

    fn sync_fetch_and_show(&mut self, force: bool) {
        self.fetch_async(force);
    }

    /// 通知栏更新主入口（对应 update()；已修复 Python 版 display 先引用后定义的顺序问题）。
    /// v1.0.12 起通知横幅统一收敛到大屏白色跑马灯（读 board_notice.txt，新通知随时弹出），
    /// 主程序黑底横幅退役不再显示；半屏重要通知卡（课间/放学后）保持不变。
    fn update_notice(&mut self, notice_text: &str) {
        // 半屏通知卡必须放在所有静默 return 之前判断：是否弹由
        // should_popup_notice 自己按作息决定（只在课间/放学后）。
        self.show_popup_notice(notice_text);
        // 消费强制显示标记（切班等），语义保留但黑横幅不再上屏
        let _force = std::mem::take(&mut self.force_notice_once);
        let _work_mode = self.cfg().work_time.clone();
        self.notice_text.clear();
        self.set_notice_visible(false);
    }

    fn set_notice_visible(&mut self, visible: bool) {
        if self.notice_visible != visible {
            self.notice_visible = visible;
            layered::show(self.notice_hwnd, visible);
        } else {
            layered::show(self.notice_hwnd, visible);
        }
    }

    fn place_notice_bar(&self) {
        let (sw, sh) = crate::win::screen_size();
        let cfg = self.cfg();
        let (w, h) = notice_bar_size(&self.notice_text, &self.fonts, &cfg);
        let base_x = match cfg.notice_position.as_str() {
            "靠左" => 10,
            "靠右" => sw - w - 10,
            _ => (sw - w) / 2,
        };
        // 拖动偏移叠加在默认位置上，并夹在屏幕内，避免窗口被拖到看不见
        let x = (base_x + cfg.notice_offset_x).clamp(0, (sw - w).max(0));
        let y = (10 + cfg.notice_offset_y).clamp(0, (sh - h).max(0));
        layered::move_to(self.notice_hwnd, x, y, w, h);
    }

    /// 缩放/位置变化后立即重绘并重定位（滚轮、拖动、设置页热加载共用）。
    fn refresh_notice_bar(&mut self) {
        if !self.notice_text.is_empty() {
            self.paint_notice_bar();
        }
        self.place_notice_bar();
    }

    fn paint_notice_bar(&mut self) {
        let (w, h) = notice_bar_size(&self.notice_text, &self.fonts, &self.cfg());
        let Some(mut pm) = Pixmap::new(w as u32, h as u32) else { return };
        let cfg = self.cfg();
        fill_solid(&mut pm, 0, 0, 0, 0);
        // 黑底圆角
        let s = notice_scale_of(&cfg) as f32;
        layered::rounded_bg(&mut pm, 16.0 * s, [0, 0, 0, 235]);
        // 文字（最多两行，超出省略）；留白与关闭区随缩放同步变化
        let pad = 30.0_f32 * s;
        let close_w = 90.0_f32 * s;
        let max_tw = (w as f32 - pad * 2.0 - close_w).max(40.0 * s);
        let px = notice_px(&cfg);
        // 文字颜色取设置里的「通知颜色」（旧版硬编码白色，设置项等于没用）
        let nc = crate::theme::rgb(&cfg.notice_color);
        let color = Color::from_rgba8(nc.r, nc.g, nc.b, 255);
        let lines = wrap_lines(&self.fonts, &self.notice_text, max_tw, px, true, 2);
        let line_h = px * 1.3;
        let y0 = h as f32 / 2.0 - (lines.len() as f32 - 1.0) * line_h / 2.0;
        for (i, line) in lines.iter().enumerate() {
            self.fonts.draw(
                &mut pm,
                pad,
                y0 + i as f32 * line_h,
                line,
                px,
                true,
                color,
                Anchor::Left,
                None,
                false,
            );
        }
        // 关闭 ✕
        let cx = w as f32 - close_w / 2.0 - 8.0 * s;
        self.fonts.draw(&mut pm, cx, h as f32 / 2.0, "✕", px * 0.8, true, color, Anchor::Center, None, false);
        layered::present(self.notice_hwnd, &pm);
    }

    // ==================== 半屏重要通知卡 ====================

    /// 通知正文含「通知 / 重要通知」时，课间 / 放学后弹半屏居中卡片 + 语音播报。
    ///
    /// 与旧的满屏通知不同：
    /// - 不再被 `board_is_running()` 抑制（桌面大屏常驻时也必须弹）；
    /// - 不设 30 秒自动关闭；点击卡片任意位置（或 Esc）即关；
    /// - 作息切换（上课 / 课前 30 秒 / 周休）仍由 `check_class_loop` 统一收起。
    fn show_popup_notice(&mut self, notice_text: &str) {
        // 广播正文只是预览：未经 Gitee 回拉确认前，绝不弹屏、绝不语音播报。
        // 这样即使 Common Key 泄露被人推了伪造文字，也不会被朗读到全班。
        if self.quick_display_at.is_some() {
            return;
        }
        let work_mode = self.cfg().work_time.clone();
        if !schedule::should_popup_notice(notice_text, &work_mode) {
            return;
        }
        if self.last_popup_text.as_deref() == Some(notice_text) {
            return; // 同一条不重复弹、不重复播报
        }
        self.last_popup_text = Some(notice_text.to_string());
        speak_notice(notice_text);
        let (x, y, w, h) = popup_geometry(crate::win::screen_size());
        let Some(hwnd) = self
            .popup
            .or_else(|| layered::create_floating("RollHalfNoticeRust", "重要通知", w, h, popup_proc))
        else {
            return;
        };
        self.popup = Some(hwnd);
        layered::move_to(hwnd, x, y, w, h);
        if let Some(mut pm) = Pixmap::new(w as u32, h as u32) {
            let (body, important) = clean_notice_body(notice_text);
            self.popup_close_rect = draw_notice_popup(&self.fonts, &mut pm, w, h, &body, important);
            layered::present(hwnd, &pm);
        }
        layered::show(hwnd, true);
    }

    fn close_popup(&mut self) {
        if let Some(hwnd) = self.popup.take() {
            layered::show(hwnd, false);
        }
        self.popup_close_rect = (-1.0, -1.0, -1.0, -1.0);
    }

    // ==================== 同步新文件提醒 ====================

    /// 蓝底白字提醒窗（对应 Python _show_sync_notice：500×(250+n×30)×DPI，居中，20s 自动关）。
    fn show_sync_notice(&mut self, files: &[String]) {
        if files.is_empty() {
            return;
        }
        let dpi = crate::win::dpi_scale();
        let (sw, sh) = crate::win::screen_size();
        let display: Vec<String> = files.iter().take(5).cloned().collect();
        let more = files.len().saturating_sub(5);
        let w = (500.0 * dpi) as i32;
        let h = ((250.0 + display.len() as f64 * 30.0) * dpi) as i32;
        let x = (sw - w) / 2;
        let y = (sh - h) / 2;
        let Some(hwnd) = self.sync_notice_hwnd.or_else(|| {
            layered::create_floating("RollSyncNoticeRust", "sync", w, h, sync_notice_proc)
        }) else {
            return;
        };
        self.sync_notice_hwnd = Some(hwnd);
        self.sync_notice_close_at = Some(now_secs() + 20.0);
        layered::move_to(hwnd, x, y, w, h);
        if let Some(mut pm) = Pixmap::new(w as u32, h as u32) {
            fill_solid(&mut pm, 0x00, 0x7B, 0xFF, 255);
            let white = Color::from_rgba8(255, 255, 255, 255);
            let px = 20.0 * dpi as f32;
            self.fonts.draw(
                &mut pm, 20.0 * dpi as f32, 60.0 * dpi as f32,
                "📥 新文件已同步", px, true, white, Anchor::Left, None, false,
            );
            let mut yy = 110.0 * dpi as f32;
            for f in &display {
                self.fonts.draw(
                    &mut pm, 30.0 * dpi as f32, yy,
                    &format!("  • {f}"), px, false, white, Anchor::Left, None, false,
                );
                yy += 30.0 * dpi as f32;
            }
            if more > 0 {
                self.fonts.draw(
                    &mut pm, 30.0 * dpi as f32, yy,
                    &format!("  • 还有 {more} 个文件..."), px, false, white, Anchor::Left, None, false,
                );
            }
            layered::present(hwnd, &pm);
        }
        layered::show(hwnd, true);
    }

    fn close_sync_notice(&mut self) {
        if let Some(hwnd) = self.sync_notice_hwnd.take() {
            layered::show(hwnd, false);
            self.sync_notice_close_at = None;
        }
    }

    // ==================== 侧边条 ====================

    fn bar_geometry(&self) -> (i32, i32, i32, i32, i32) {
        let (bw, bh, y, lx, rx, _) = bar_dimensions(&self.cfg());
        (bw, bh, y, lx, rx)
    }

    fn update_bars(&mut self) {
        let (bw, bh, y, left_gap, right_x) = self.bar_geometry();
        crate::paths::trace(&format!(
            "update_bars screen={:?} dpi={:.2} bw={bw} bh={bh} y={y} left_gap={left_gap} right_x={right_x}",
            crate::win::screen_size(),
            crate::win::dpi_scale(),
        ));
        if self.bar_size != Some((bw, bh)) {
            for b in self.bars.iter() {
                if let Some(h) = b {
                    unsafe {
                        DestroyWindow(*h);
                    }
                }
            }
            self.bars = [None, None];
            for i in 0..2 {
                if let Some(hwnd) =
                    layered::create_floating("RollSideBarRust", "bar", bw, bh, sidebar_proc)
                {
                    self.bars[i] = Some(hwnd);
                    paint_sidebar(self, hwnd, bw, bh);
                }
            }
            self.bar_size = Some((bw, bh));
        }
        // 侧边条常驻显示：菜单里的「隐藏 40 分钟 / 24 小时 / 永远隐藏」已移除
        for (hwnd, x) in self.bars.iter().zip([left_gap, right_x]) {
            if let Some(h) = hwnd {
                layered::move_to(*h, x, y, bw, bh);
                layered::show(*h, true);
            }
        }
    }

    fn update_minute(&mut self) {
        // 节次边界即时响应：上课 / 下课 / 课前预备期状态变化 1 秒内触发
        // check_class_loop——下课铃响横幅立刻弹出、上课前 30 秒立刻收起，
        // 不再等最长 30 秒的 T_CLASSCHECK 轮询。纯本地计算，开销可忽略。
        let work_mode = self.cfg().work_time.clone();
        let st = (
            schedule::is_class_time(&work_mode),
            !schedule::force_notice_show_now() && schedule::is_within_pre_close(&work_mode),
        );
        if self.last_class_state != Some(st) {
            self.last_class_state = Some(st);
            self.check_class_loop();
        }
        let now = chrono::Local::now();
        let m = now.minute() as i32;
        if m != self.last_minute {
            self.last_minute = m;
            let (bw, bh, _, _, _) = self.bar_geometry();
            let hwnds: Vec<HWND> = self.bars.iter().flatten().copied().collect();
            for h in hwnds {
                paint_sidebar(self, h, bw, bh);
            }
        }
    }

    // ==================== 倒计时 ====================

    fn show_countdown(&mut self) {
        if let Some(hwnd) = self.cd_hwnd {
            layered::show(hwnd, true);
            return;
        }
        let c = self.cfg();
        // 窗口尺寸随 DPI 与自由缩放同步放大，否则高 DPI 下大数字会压到按钮上
        let (sw, sh) = crate::win::screen_size();
        let (cw_px, h_px) = cd_window_size(&c);
        let w = cw_px.min((sw as f64 * WIN_MAX_SCREEN_RATIO) as i32);
        let h = h_px;
        let Some(hwnd) = layered::create_floating("RollCountdownRust", "倒计时", w, h, countdown_proc)
        else {
            return;
        };
        self.cd_hwnd = Some(hwnd);
        self.cd = Some(CountdownState {
            remaining: c.default_countdown,
            running: true,
            start_time: now_secs(),
            shown: String::new(),
            ended: false,
            buttons: Vec::new(),
            pos: (0, 0),
        });
        // 位置：底部居中（避免与侧边条重叠）
        let x = (sw - w) / 2;
        let y = (sh - h - crate::win::seewo_toolbar_height()).max(0);
        self.cd.as_mut().unwrap().pos = (x, y);
        layered::move_to(hwnd, x, y, w, h);
        unsafe {
            SetTimer(hwnd, T_CD_TICK, 200, std::ptr::null_mut());
        }
        paint_countdown(self);
        layered::show(hwnd, true);
    }

    fn countdown_tick(&mut self) {
        let Some(cd) = self.cd.as_mut() else { return };
        if !cd.running {
            return;
        }
        let elapsed = (now_secs() - cd.start_time) as i64;
        let left = (cd.remaining - elapsed).max(0);
        if left <= 0 {
            cd.shown = "时间到！".into();
            cd.running = false;
            cd.ended = true;
            beep(950, 250);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(200));
                beep(1450, 250);
            });
            // 3 秒后自动关闭（由根窗口处理）
            let root_addr = self.root as usize;
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(3000));
                unsafe {
                    PostMessageW(root_addr as *mut c_void, WM_APP + 1, 0, 0);
                }
            });
            paint_countdown(self);
            return;
        }
        let shown = format!("{:02}:{:02}", left / 60, left % 60);
        if shown != cd.shown {
            cd.shown = shown;
            paint_countdown(self);
        }
    }

    fn close_countdown(&mut self) {
        if let Some(h) = self.cd_hwnd.take() {
            unsafe {
                KillTimer(h, T_CD_TICK);
                DestroyWindow(h);
            }
        }
        self.cd = None;
    }

    // ==================== 点名窗 ====================

    fn open_roll(&mut self) {
        let students = self.students();
        if students.is_empty() {
            let c = self.cfg();
            let msg = match c.current() {
                Some(cls) => format!(
                    "「{}」还没有导入学生名单，无法点名。\n\n请先到「学生名单」页导入名单后再试。",
                    cls.display_name
                ),
                None => "还没有创建班级，无法点名。\n\n请先在设置中创建班级并导入学生名单。".to_string(),
            };
            msg_box(&msg, "无法点名", 0x30);
            return;
        }
        if self.roll_hwnd.is_some() {
            return;
        }
        let c = self.cfg();
        let scale = roll_win_scale(&c);
        let mut w = (ROLL_W_DEF as f64 * scale) as i32;
        let mut h = (ROLL_H_DEF as f64 * scale) as i32;
        let (sw, sh) = crate::win::screen_size();
        w = w.min((sw as f64 * WIN_MAX_SCREEN_RATIO) as i32);
        h = h.min((sh as f64 * WIN_MAX_SCREEN_RATIO) as i32);
        let x = ((sw - w) / 2).max(0);
        let y = ((sh - h) / 2).max(0);
        let Some(hwnd) = layered::create_floating("RollRollRust", "点名", w, h, roll_proc) else {
            return;
        };
        self.roll_hwnd = Some(hwnd);
        self.roll = Some(RollState {
            pos: (x, y),
            buttons: Vec::new(),
        });
        self.roll_var = "准备点名".to_string();
        layered::move_to(hwnd, x, y, w, h);
        self.paint_roll_init();
        layered::show(hwnd, true);
        // 50ms 后开滚（对应 root.after(50, _start_roll)）
        self.start_roll();
    }

    /// 未开滚时也先画一帧「准备点名」。
    fn paint_roll_init(&mut self) {
        let (fs, bf) = roll_fonts(&self.cfg());
        paint_roll(self, fs, bf);
    }

    fn start_roll(&mut self) {
        let students = self.students();
        if self.rolling || students.is_empty() {
            return;
        }
        self.rolling = true;
        // 清掉上一次定格状态：否则新动画会被旧的 roll_show_at 提前关闭
        self.roll_show_at = None;
        ROLL_ANIM_QUEUE.with(|q| q.borrow_mut().clear());
        // 16ms 专用定时器：滚动期间由它消费动画队列，定格到期也由它精确关闭。
        unsafe { SetTimer(self.root, T_ROLL, ROLL_TICK_MS as u32, std::ptr::null_mut()) };
        let steps = 22 + (rand_below(9) as usize);
        animate_roll(self, students, 0, steps);
    }

    fn pick_student(&self) -> String {
        let students = self.students();
        let c = self.cfg();
        let cls_display = c.current().map(|x| x.display_name.clone()).unwrap_or_default();
        let recent: Vec<String> = self.history.recent_names(&cls_display, 6);
        let available: Vec<String> = students
            .iter()
            .filter(|s| !recent.contains(s))
            .cloned()
            .collect();
        let pool = if available.is_empty() { students } else { available };
        if pool.is_empty() {
            return String::new();
        }
        pool[rand_below(pool.len() as u64) as usize].clone()
    }

    fn finish_roll(&mut self, name: String) {
        self.roll_var = name.clone();
        beep(1200, 200);
        let c = self.cfg();
        let cls_display = c.current().map(|x| x.display_name.clone()).unwrap_or_default();
        self.history.add(&name, &cls_display);
        self.roll_show_at = Some(now_secs() + ROLL_HOLD_SECS);
        self.rolling = false;
        let (fs, bf) = roll_fonts(&c);
        paint_roll(self, fs, bf);
    }

    fn close_roll(&mut self) {
        unsafe { KillTimer(self.root, T_ROLL) };
        ROLL_ANIM_QUEUE.with(|q| q.borrow_mut().clear());
        if let Some(h) = self.roll_hwnd.take() {
            unsafe {
                DestroyWindow(h);
            }
        }
        self.roll = None;
        self.rolling = false;
    }

    /// 定格展示到期关闭（由 16ms 定时器驱动，精度 ±16ms）
    fn check_roll_close(&mut self) {
        if let Some(at) = self.roll_show_at {
            if now_secs() >= at {
                self.roll_show_at = None;
                self.close_roll();
            }
        }
    }

    // ==================== 菜单 / 队列 ====================

    fn show_menu(&mut self, hwnd: HWND) {
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            return;
        }
        unsafe {
            append(menu, MF_STRING, CMD_SETTINGS, "⚙ 打开设置");
            // 「关闭桌面大屏」与「关闭通知」原本是两个菜单项，但两者都会停掉大屏线程，
            // 从用户看只是把同一个东西关两遍，已合并；要开回来用下面的「恢复」。
            append(menu, MF_STRING, CMD_CLOSE_NOTICE, "🔕 关闭通知与桌面大屏（作业/值日）");
            append(menu, MF_STRING, CMD_RESTORE_NOTICE, "🔔 恢复通知与今日作业");
            // 「点名栏：默认显示 / 隐藏 40 分钟 / 隐藏 24 小时 / 永远隐藏」四项已移除
            append(menu, MF_SEPARATOR, 0, "");
            append(menu, MF_STRING, CMD_QUIT, "✕ 退出程序");
            // 不可点提示行
            append(menu, MF_STRING | 0x4 /* MF_GRAYED */, 0, "💡 滚轮调大小 / 拖动改位置");
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            let cmd = TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
                pt.x,
                pt.y,
                0,
                hwnd,
                std::ptr::null(),
            );
            DestroyMenu(menu);
            // 菜单命令直接交给 App 处理：WM_COMMAND 只在根窗口 proc 处理，
            // PostMessage 到侧边条窗口会被 DefWindowProc 丢弃（旧版所有菜单项失效）。
            if cmd > 0 {
                self.handle_command(cmd as usize);
            }
        }
    }

    fn handle_command(&mut self, cmd: usize) {
        match cmd {
            CMD_SETTINGS => open_settings_process(),
            CMD_CLOSE_NOTICE => self.close_notice_all(),
            CMD_RESTORE_NOTICE => self.restore_notice_all(),
            CMD_QUIT => confirm_exit(),
            CD_CMD_SUB | CD_CMD_ADD | CD_CMD_CLOSE => {}
            _ => {}
        }
    }

    /// 是否处于手动关闭静默期（closed_until 未到）。
    fn silenced(&self, now: &chrono::DateTime<chrono::Local>) -> bool {
        self.closed_until.map(|u| *now < u).unwrap_or(false)
    }

    fn manual_close_notice(&mut self) {
        // 手动关闭 → 40 分钟静默（MANUAL_SILENCE_SECS）：期间一律不显示，
        // 新通知（GoEasyWake）/ 下课转换 / 菜单「恢复」照常解除。不再区分白天/晚上。
        self.set_notice_visible(false);
        self.closed_until =
            Some(chrono::Local::now() + chrono::Duration::seconds(MANUAL_SILENCE_SECS));
        // 手动关闭同时取消「新通知保持期」，横幅立即消失
        self.banner_hold_until = None;
    }

    /// 一键同步关闭：浮动通知横幅 + 桌面大屏（今日作业 / 值日 / 跑马灯通知）。
    /// 旧版「关闭通知」只关横幅、桌面作业还在，这里一并停掉大屏并还原壁纸。
    fn close_notice_all(&mut self) {
        self.manual_close_notice();
        if crate::win::board_is_running() {
            crate::services::stop_board();
        }
        logging::info("已同步关闭通知横幅与桌面今日作业/值日");
    }

    /// 恢复：解除手动关闭、重新拉起大屏、立即回拉通知。
    fn restore_notice_all(&mut self) {
        self.closed_until = None;
        self.startup_hide = false;
        self.last_popup_text = None;
        // 跑马灯与作业栏各有自己的 ✕，这里一并解除隐藏；
        // 但点过「彻底关闭」的作业栏不受菜单恢复影响，只能在设置窗口重新打开。
        let hard = desktop_board::board::homework_hard_hidden();
        desktop_board::board::patch_keys(&[
            ("marquee_hidden", serde_json::json!(false)),
            // true = 隐藏：彻底关闭时菜单也不能把它恢复成 false
            ("homework_hidden", serde_json::json!(hard)),
        ]);
        let _ = std::fs::remove_file(crate::paths::board_stop_file());
        let tx = self.ui_tx.clone();
        let cfg = self.cfg.clone();
        std::thread::spawn(move || {
            // 刚点关闭时旧大屏线程还在退出（互斥体未释放），等它退完再拉起
            for _ in 0..10 {
                std::thread::sleep(std::time::Duration::from_millis(400));
                if crate::win::board_is_running() {
                    continue;
                }
                let _ = std::fs::remove_file(crate::paths::board_stop_file());
                desktop_board::run_in_thread();
                break;
            }
            let c = cfg.lock().unwrap().clone();
            let t = fetch_notice_text(&c);
            let _ = tx.send(UiMsg::NoticeFetched(t));
        });
        logging::info("已恢复通知与今日作业");
    }

    /// 100ms 队列轮询（对应 _poll_queue + 板载通知落盘）。
    fn poll_queue(&mut self) {
        // 退出确认小窗写了 exit.flag（5 分钟内有效）→ 真正退出
        if crate::paths::take_exit_flag() {
            logging::info("收到退出确认（123），开始退出流程");
            quit_app();
            return;
        }
        while let Ok(msg) = self.ui_rx.try_recv() {
            match msg {
                UiMsg::NoticeFetched(text) => {
                    self.fetch_inflight = false;
                    // 广播预览的权威校正：只有真从 Gitee 拉到内容才解除抑制，
                    // 拉空 / 断网时保留预览但不弹屏不播报，等下一次回拉。
                    if self.quick_display_at.is_some() {
                        let verified = text
                            .as_deref()
                            .map(is_valid_notice)
                            .unwrap_or(false);
                        if !verified {
                            logging::warn("GoEasy 广播预览暂未获 Gitee 确认，保留预览但不弹屏播报");
                            continue;
                        }
                        self.quick_display_at = None;
                    }
                    self.update_schedule_text();
                    // 新通知检测：拉到有效且与缓存不同的内容 → 强制弹横幅
                    // （含晚自习上课期间），并保持 3 分钟不被上课分支收起；
                    // 内容没变（轮询兜底拉到相同内容）则完全不扰民。
                    let changed = matches!(&text, Some(t) if is_valid_notice(t) && Some(t) != self.cached_notice.as_ref());
                    self.cached_notice = text;
                    if changed {
                        self.force_notice_once = true;
                        self.banner_hold_until = Some(now_secs() + crate::theme::BANNER_HOLD_SECS);
                        logging::info("检测到新通知：强制弹出横幅（上课期间也弹）");
                    }
                    let work_mode = self.cfg().work_time.clone();
                    let now = chrono::Local::now();
                    // 17:15 之后一律按课间逻辑立即显示（不管晚自习是否上课）
                    if changed
                        || !schedule::is_class_time(&work_mode)
                        || schedule::force_notice_show_now()
                    {
                        let notice_text = self.cached_notice.clone().unwrap_or_default();
                        let ok = changed || !self.silenced(&now);
                        if ok {
                            self.update_notice(&notice_text);
                        }
                    }
                }
                UiMsg::NoticeFetchedForce(text) => {
                    // 下课转换 / 静默过期后的拉取：无视手动关闭与作息，立即显示。
                    self.fetch_inflight = false;
                    self.update_schedule_text();
                    self.cached_notice = text;
                    self.force_notice_once = true;
                    let notice_text = self.cached_notice.clone().unwrap_or_default();
                    self.update_notice(&notice_text);
                }
                UiMsg::ClassNoticeFetched(text) => {
                    self.update_schedule_text();
                    self.cached_notice = text;
                    let notice_text = self.cached_notice.clone().unwrap_or_default();
                    // 用户显式切班：不看作息、不看静默，立刻把新班通知拉起来
                    self.force_notice_once = true;
                    self.update_notice(&notice_text);
                    // board_notice.txt 写完后再通知大屏弹出，保证横幅读到的是新内容
                    self.pending_popup_flag = true;
                    logging::info("班级切换后通知已拉取，立即显示并通知大屏弹出");
                }
                UiMsg::PreloadFetched(text) => {
                    if let Some(t) = text {
                        self.cached_notice = Some(t);
                    }
                    self.preload_done = true;
                }
                UiMsg::GoEasyWake { quick_text } => {
                    // 重新发布 = 新通知：解除静默，重置弹屏去重，立即回拉
                    self.closed_until = None;
                    self.last_popup_text = None;
                    let allow_quick = self.cfg().goeasy_quick_display;
                    if let Some(t) = quick_text.filter(|_| allow_quick) {
                        // 先把广播正文上屏（横幅 + board_notice.txt 给大屏），
                        // 省掉一次 Gitee 往返；权威文本由下面的回拉结果覆盖。
                        // 新通知随时弹（含晚自习上课期间），并保持 3 分钟。
                        self.force_notice_once = true;
                        self.banner_hold_until = Some(now_secs() + crate::theme::BANNER_HOLD_SECS);
                        self.quick_display_at = Some(now_secs());
                        self.cached_notice = Some(t.clone());
                        self.update_notice(&t);
                        logging::info("GoEasy 广播正文已秒显（待 Gitee 确认后才弹屏播报）");
                    }
                    let tx = self.ui_tx.clone();
                    let cfg = self.cfg.clone();
                    std::thread::spawn(move || {
                        let c = cfg.lock().unwrap().clone();
                        let r = fetch_notice_text(&c).map(|t| {
                            if is_valid_notice(&t) {
                                t
                            } else {
                                String::new()
                            }
                        });
                        let _ = tx.send(UiMsg::NoticeFetched(r));
                    });
                }
                UiMsg::GoEasyFiles => {
                    // 网页网络U盘上传完成：不影响通知弹屏，只让同步线程立即跑一轮
                    crate::services::request_sync_now();
                    logging::info("GoEasy 文件信号：已唤醒 Gitee 同步线程");
                }
                UiMsg::SyncNewFiles(files) => self.show_sync_notice(&files),
            }
        }
        // 落盘 board_notice.txt 供桌面大屏跑马灯 / 作业卡通知条显示
        let txt = self.cached_notice.clone();
        if txt != self.board_notice_last {
            self.board_notice_last = txt.clone();
            let p = crate::paths::data_dir().join("board_notice.txt");
            let _ = std::fs::write(&p, txt.unwrap_or_default());
        }
        // 切班的新通知已经写盘，再让大屏横幅立刻弹一次（顺序不能反）
        if self.pending_popup_flag {
            self.pending_popup_flag = false;
            crate::paths::write_notice_popup_flag();
        }
        // 点名窗展示到期关闭（16ms 定时器里也检查，此处为兜底）
        self.check_roll_close();
        // 同步新文件提醒 20 秒自动关
        if let Some(at) = self.sync_notice_close_at {
            if now_secs() >= at {
                self.close_sync_notice();
            }
        }
        // 广播预览 30 秒仍未被 Gitee 确认：撤回，避免未经校验的文字长期上屏
        if let Some(at) = self.quick_display_at {
            if now_secs() - at >= 30.0 {
                self.quick_display_at = None;
                self.cached_notice = None;
                self.notice_text.clear();
                self.set_notice_visible(false);
                logging::warn("GoEasy 广播预览 30 秒未获 Gitee 确认，已撤回");
            }
        }
        // 半屏通知卡没有自动关闭：只有用户点右下角「关闭」才收起，
        // 或由 check_class_loop 在上课 / 课前 30 秒 / 周休时按作息收起。
        // 倒计时「时间到」3 秒后自动关
        if let Some(cd) = self.cd.as_ref() {
            if cd.ended && !cd.running {
                // 由 WM_APP+1 处理
            }
        }
    }
}

// ==================== 绘制 ====================

fn fill_solid(pm: &mut Pixmap, r: u8, g: u8, b: u8, a: u8) {
    for px in pm.pixels_mut() {
        *px = tiny_skia::PremultipliedColorU8::from_rgba(r, g, b, a).unwrap();
    }
}

/// 通知横幅缩放：自由系数 notice_scale 优先（滚轮/设置页滑杆），0 = 回落字号档位原尺寸。
fn notice_scale_of(cfg: &Config) -> f64 {
    if cfg.notice_scale > 0.0 {
        cfg.notice_scale.clamp(NOTICE_SCALE_MIN, NOTICE_SCALE_MAX)
    } else {
        1.0
    }
}

fn notice_px(cfg: &Config) -> f32 {
    let mut fs = font_size_map(&cfg.notice_font_size) as f32;
    let (sw, _) = crate::win::screen_size();
    if sw >= 3840 {
        fs *= 1.2;
    }
    fs * 96.0 / 72.0 * crate::win::dpi_scale() as f32 * notice_scale_of(cfg) as f32
}

/// Tk 字号（磅）→ 像素。
fn tk_px(pt: f32) -> f32 {
    pt * 96.0 / 72.0 * crate::win::dpi_scale() as f32
}

fn notice_bar_size(text: &str, fonts: &Fonts, cfg: &Config) -> (i32, i32) {
    let px = notice_px(cfg);
    let s = notice_scale_of(cfg) as f32;
    let max_w = 1200.0_f32 * s;
    let lines = wrap_lines(fonts, text, max_w, px, true, 3);
    let tw = lines
        .iter()
        .map(|l| fonts.measure(l, px, true))
        .fold(0.0_f32, f32::max);
    let w = (tw + (60.0 + 100.0) * s).ceil() as i32;
    let h = (lines.len() as f32 * px * 1.3 + 30.0 * s).ceil() as i32;
    (w.max((200.0 * s) as i32), h.max((70.0 * s) as i32))
}

/// 简单按宽度折行（CJK 逐字、空格分段）。
fn wrap_lines(fonts: &Fonts, text: &str, max_w: f32, px: f32, bold: bool, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for ch in para.chars() {
            let cand = format!("{}{}", cur, ch);
            if !cur.is_empty() && fonts.measure(&cand, px, bold) > max_w {
                lines.push(std::mem::take(&mut cur));
                cur.push(ch);
            } else {
                cur = cand;
            }
        }
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        let last = lines.last_mut().unwrap();
        *last = format!("{}…", last.trim_end_matches('…'));
        let mut t = last.clone();
        t.pop();
        while t.chars().count() > 1 && fonts.measure(&format!("{}…", t), px, bold) > max_w {
            t.pop();
        }
        *last = format!("{}…", t);
    }
    lines
}

fn paint_sidebar(app: &mut App, hwnd: HWND, bw: i32, bh: i32) {
    let Some(mut pm) = Pixmap::new(bw as u32, bh as u32) else { return };
    draw_sidebar(&mut pm, &app.fonts, &app.cfg().work_time, &chrono::Local::now());
    layered::present(hwnd, &pm);
}

/// 侧边栏绘制核心（纯函数，不依赖窗口句柄，便于单测渲染 PNG 验证布局）。
/// 2026-11-17 恢复 v1.0.13 双色版式：上半浅蓝渐变 + 时钟，下半浅绿渐变 + 距下课。
fn draw_sidebar(pm: &mut Pixmap, fonts: &Fonts, work_mode: &str, now: &chrono::DateTime<chrono::Local>) {
    let w = pm.width() as f32;
    let h = pm.height() as f32;
    let cx = w / 2.0;
    let mid = h / 2.0;

    // ① 上下两段渐变铺满，再按圆角抹掉四个角（层窗口逐像素 alpha，圆角外透明）
    gradient(pm, 0.0, 0.0, w, mid, rgb(crate::theme::BAR_TOP_START), rgb(crate::theme::BAR_TOP_END));
    gradient(pm, 0.0, mid, w, h, rgb(crate::theme::BAR_BOTTOM_START), rgb(crate::theme::BAR_BOTTOM_END));
    round_corners(pm, (w * 0.32).clamp(8.0, 26.0));

    let white = Color::from_rgba8(255, 255, 255, 255);

    // ② 上半：秒表图标 + 时 / 分（都留在蓝色区里）
    let icon_size = (w / 2.0).max(20.0);
    draw_stopwatch(pm, cx, mid * 0.20, icon_size, rgb(crate::theme::BAR_TOP_START));
    let time_size = (w / 2.5).max(25.0);
    text_center(fonts, pm, cx, mid * 0.47, &format!("{:02}", now.hour()), time_size, white);
    text_center(fonts, pm, cx, mid * 0.72, &format!("{:02}", now.minute()), time_size, white);

    // ③ 下半：靶心图标 + 竖排「距 下 课 X分」（按作息表实时计算）
    draw_target(pm, cx, mid + mid * 0.13, icon_size, rgb(crate::theme::BAR_BOTTOM_START));
    let (label, mins, _progress, _urgent, off) = sidebar_status(work_mode, now);
    let lines: Vec<String> = if off {
        vec!["已".into(), "放".into(), "学".into()]
    } else {
        label
            .chars()
            .map(|c| c.to_string())
            .chain(std::iter::once(format!("{}分", mins)))
            .collect()
    };
    let block_y0 = mid + mid * 0.30;
    let block_y1 = h - mid * 0.10;
    let line_h = ((block_y1 - block_y0) / lines.len() as f32).max(10.0);
    let px = (w * 0.40).max(13.0).min(line_h * 0.80);
    for (i, line) in lines.iter().enumerate() {
        // 一行放不下就缩字号（例如「120分」这种三位数）
        let max_w = w * 0.86;
        let tw = fonts.measure(line, px, true);
        let this_px = if tw > max_w { px * max_w / tw } else { px };
        let cy = block_y0 + line_h * (i as f32 + 0.5);
        text_center(fonts, pm, cx, cy, line, this_px, white);
    }
}

/// 以 (cx, cy) 为文字视觉中心绘制（draw 的 y 是文字框顶边，这里换算一次）。
fn text_center(fonts: &Fonts, pm: &mut Pixmap, cx: f32, cy: f32, text: &str, px: f32, color: Color) {
    fonts.draw(pm, cx, cy - px * 0.36, text, px, true, color, Anchor::Center, None, false);
}

/// 侧边栏下半区结构化状态（方案二用）。
/// 返回 (标签, 剩余分钟, 本节进度0~1, 是否≤5分钟, 是否放学/周末)。
/// 进度定义：上课中=本节已过比例；课间=课间已过比例；放学=0（不画进度条）。
fn sidebar_status<'a>(work_mode: &str, now: &chrono::DateTime<chrono::Local>) -> (&'a str, i64, f32, bool, bool) {
    if schedule::is_weekend_at(now) {
        return ("已放学", 0, 0.0, false, true);
    }
    // 上课中：距下课 + 本节进度
    if let Some((start, end)) = schedule::current_class_period_at(work_mode, now) {
        let s = schedule::hm(&start);
        let e = schedule::hm(&end);
        let secs = (e - now.time()).num_seconds().max(0);
        let mins = (secs + 59) / 60;
        let total = (e - s).num_seconds().max(1);
        let elapsed = (now.time() - s).num_seconds().max(0);
        let progress = elapsed as f32 / total as f32;
        return ("距下课", mins, progress, secs <= 300, false);
    }
    // 课间 / 放学前：找下一节课
    let t = now.time();
    let rows = schedule::time_rows(work_mode);
    for (start, _) in &rows {
        let s = schedule::hm(start);
        if s > t {
            let secs = (s - t).num_seconds().max(0);
            let mins = (secs + 59) / 60;
            // 课间进度：从上一节结束到下一节开始的区间，已过比例
            let prev_end = rows
                .iter()
                .take_while(|(rs, _)| schedule::hm(rs) < s)
                .filter_map(|(_, re)| {
                    let e = schedule::hm(re);
                    if e <= t { Some(e) } else { None }
                })
                .max();
            let progress = if let Some(pe) = prev_end {
                let total = (s - pe).num_seconds().max(1);
                (t - pe).num_seconds().max(0) as f32 / total as f32
            } else {
                0.0
            };
            return ("距上课", mins, progress, secs <= 300, false);
        }
    }
    ("已放学", 0, 0.0, false, true)
}

/// 侧边栏文字（测试用）：基于 sidebar_status 生成逐字行，保证与绘制逻辑一致。
#[allow(dead_code)]
fn sidebar_hint_at(work_mode: &str, now: &chrono::DateTime<chrono::Local>) -> Vec<String> {
    let (label, mins, _, _, off) = sidebar_status(work_mode, now);
    if off {
        return label.chars().map(|c| c.to_string()).collect();
    }
    let mut v: Vec<String> = label.chars().map(|c| c.to_string()).collect();
    v.push(format!("{}分", mins));
    v
}

/// 把矩形角落切成圆角：角落像素按到圆心的距离做 1px 抗锯齿淡出。
/// 层窗口的像素是预乘 alpha 的，所以 RGBA 四个通道一起乘覆盖率即可。
fn round_corners(pm: &mut Pixmap, radius: f32) {
    let w = pm.width() as f32;
    let h = pm.height() as f32;
    let r = radius.min(w / 2.0).min(h / 2.0);
    if r <= 0.5 {
        return;
    }
    let nw = pm.width() as usize;
    let nh = pm.height() as usize;
    let stride = nw * 4;
    let data = pm.data_mut();
    for y in 0..nh {
        let py = y as f32 + 0.5;
        let qy = if py < r {
            r - py
        } else if py > h - r {
            py - (h - r)
        } else {
            0.0
        };
        if qy == 0.0 {
            continue;
        }
        for x in 0..nw {
            let px_ = x as f32 + 0.5;
            let qx = if px_ < r {
                r - px_
            } else if px_ > w - r {
                px_ - (w - r)
            } else {
                0.0
            };
            if qx == 0.0 {
                continue;
            }
            let cov = (r - (qx * qx + qy * qy).sqrt()).clamp(0.0, 1.0);
            let i = y * stride + x * 4;
            for k in 0..4 {
                data[i + k] = (data[i + k] as f32 * cov) as u8;
            }
        }
    }
}

fn solid_paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn round_stroke(c: Color, width: f32) -> (Paint<'static>, tiny_skia::Stroke) {
    let mut s = tiny_skia::Stroke::default();
    s.width = width;
    s.line_cap = tiny_skia::LineCap::Round;
    s.line_join = tiny_skia::LineJoin::Round;
    (solid_paint(c), s)
}

fn fill_path_color(pm: &mut Pixmap, path: &tiny_skia::Path, c: Color) {
    pm.fill_path(path, &solid_paint(c), tiny_skia::FillRule::Winding, Transform::identity(), None);
}

fn stroke_path_color(pm: &mut Pixmap, path: &tiny_skia::Path, c: Color, width: f32) {
    let (p, s) = round_stroke(c, width);
    pm.stroke_path(path, &p, &s, Transform::identity(), None);
}

#[allow(dead_code)]
fn gradient(pm: &mut Pixmap, x1: f32, y1: f32, x2: f32, y2: f32, c1: crate::theme::Rgb, c2: crate::theme::Rgb) {
    let steps = 30;
    let region_h = (y2 - y1).max(1.0);
    let step_h = region_h / steps as f32;
    for i in 0..steps {
        let ratio = i as f32 / (steps - 1) as f32;
        let r = (c1.r as f32 + (c2.r as f32 - c1.r as f32) * ratio) as u8;
        let g = (c1.g as f32 + (c2.g as f32 - c1.g as f32) * ratio) as u8;
        let b = (c1.b as f32 + (c2.b as f32 - c1.b as f32) * ratio) as u8;
        let ly1 = y1 + step_h * i as f32;
        let ly2 = (y1 + step_h * (i + 1) as f32 + 1.0).min(y2);
        let mut pb = tiny_skia::PathBuilder::new();
        pb.push_rect(tiny_skia::Rect::from_xywh(x1, ly1, x2 - x1, ly2 - ly1).unwrap());
        if let Some(path) = pb.finish() {
            fill_path_color(pm, &path, Color::from_rgba8(r, g, b, 255));
        }
    }
}

/// ⏱ 的矢量近似：圆 + 指针 + 顶部小柄。
fn draw_stopwatch(pm: &mut Pixmap, cx: f32, cy: f32, size: f32, c: crate::theme::Rgb) {
    let col = Color::from_rgba8(c.r, c.g, c.b, 255);
    let r = size / 2.0;
    let mut pb = tiny_skia::PathBuilder::new();
    pb.push_circle(cx, cy, r * 0.82);
    if let Some(path) = pb.finish() {
        stroke_path_color(pm, &path, col, size * 0.10);
    }
    // 指针
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(cx, cy);
    pb.line_to(cx + r * 0.5, cy - r * 0.45);
    pb.move_to(cx, cy);
    pb.line_to(cx - r * 0.15, cy + r * 0.55);
    if let Some(path) = pb.finish() {
        stroke_path_color(pm, &path, col, size * 0.09);
    }
    // 顶部小柄
    let mut pb = tiny_skia::PathBuilder::new();
    pb.push_rect(tiny_skia::Rect::from_xywh(cx - size * 0.10, cy - r * 1.05, size * 0.20, size * 0.16).unwrap());
    if let Some(path) = pb.finish() {
        fill_path_color(pm, &path, col);
    }
}

/// 🎯 的矢量近似：三重同心圆。
#[allow(dead_code)]
fn draw_target(pm: &mut Pixmap, cx: f32, cy: f32, size: f32, c: crate::theme::Rgb) {
    let col = Color::from_rgba8(c.r, c.g, c.b, 255);
    for k in [0.78, 0.48] {
        let mut pb = tiny_skia::PathBuilder::new();
        pb.push_circle(cx, cy, size / 2.0 * k);
        if let Some(path) = pb.finish() {
            stroke_path_color(pm, &path, col, size * 0.10);
        }
    }
    let mut pb = tiny_skia::PathBuilder::new();
    pb.push_circle(cx, cy, size * 0.10);
    if let Some(path) = pb.finish() {
        fill_path_color(pm, &path, col);
    }
}

// ==================== 全屏通知卡片 ====================

/// 去掉「重要通知 / 通知：」前缀，返回 (正文, 是否重要)。
pub fn clean_notice_body(notice_text: &str) -> (String, bool) {
    let raw = notice_text.replace('\r', "").trim().to_string();
    let important = raw.contains("重要通知") || raw.contains("重要消息");
    let mut body = raw.clone();
    for prefix in ["重要通知", "重要消息", "通知"] {
        if body.starts_with(prefix) {
            body = body[prefix.len()..]
                .trim_start_matches(|c: char| "：: 、,，—-　\n\t".contains(c))
                .to_string();
            break;
        }
    }
    ((if body.is_empty() { raw } else { body }), important)
}

/// 自动折行 + 挑字号（对应 _fit_notice_lines）。
fn fit_notice_lines(
    fonts: &Fonts,
    text: &str,
    max_w: f32,
    max_h: f32,
    max_px: f32,
    min_px: f32,
) -> (Vec<String>, f32) {
    const TAIL_PUNCT: &str = "，。！？；：、,.!?;:）」』】》…";
    const BREAK_PUNCT: &str = "，。！？；：、";
    let paras: Vec<&str> = {
        let t = text.trim();
        let v: Vec<&str> = t.split('\n').collect();
        if v.is_empty() {
            vec![""]
        } else {
            v
        }
    };
    let mut lines: Vec<String> = vec![String::new()];
    let mut chosen_px = max_px;
    let mut px = max_px;
    while px >= min_px {
        let mut cur_lines: Vec<String> = Vec::new();
        for para in &paras {
            if para.is_empty() {
                cur_lines.push(String::new());
                continue;
            }
            let mut cur = String::new();
            for ch in para.chars() {
                let cand = format!("{}{}", cur, ch);
                if !cur.is_empty() && fonts.measure(&cand, px, true) > max_w {
                    if TAIL_PUNCT.contains(ch) {
                        cur.push(ch); // 标点跟随上一行
                        continue;
                    }
                    let cut = BREAK_PUNCT
                        .chars()
                        .filter_map(|p| cur.rfind(p))
                        .max()
                        .unwrap_or(usize::MAX);
                    let half = cur.chars().count() / 2;
                    let cut_len = cur.chars().count();
                    if cut != usize::MAX && cut >= half.max(1) && cut < cut_len {
                        let head: String = cur.chars().take(cut + 1).collect();
                        let tail: String = cur.chars().skip(cut + 1).collect();
                        cur_lines.push(head);
                        cur = tail;
                        cur.push(ch);
                    } else {
                        cur_lines.push(std::mem::take(&mut cur));
                        cur.push(ch);
                    }
                } else {
                    cur = cand;
                }
            }
            cur_lines.push(cur);
        }
        lines = cur_lines;
        chosen_px = px;
        if lines.len() as f32 * px * 1.36 <= max_h {
            break;
        }
        px -= 2.0;
    }
    let max_lines = ((max_h / (chosen_px * 1.36)).floor() as usize).max(1);
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        let last = lines.last_mut().unwrap();
        if last.is_empty() {
            *last = "…".to_string();
        } else {
            let mut t: String = last.chars().take(last.chars().count() - 1).collect();
            t.push('…');
            *last = t;
        }
    }
    (lines, chosen_px)
}

/// 半屏弹屏的窗口位置和大小：宽约 62%、高约 56%，屏幕正中。
/// 抽成纯函数便于单测；下限保证低分辨率下正文仍有可读空间。
fn popup_geometry(screen: (i32, i32)) -> (i32, i32, i32, i32) {
    let (sw, sh) = screen;
    let w = ((sw as f32 * 0.62) as i32).max(640).min(sw.max(1));
    let h = ((sh as f32 * 0.56) as i32).max(420).min(sh.max(1));
    ((sw - w) / 2, (sh - h) / 2, w, h)
}

/// 「班级通知」半屏居中卡片绘制；返回右下角「关闭」按钮命中区。
/// 窗口本身就是半屏大小，因此沿用原满屏卡片的等比版式即可自动缩放。
fn draw_notice_popup(
    fonts: &Fonts,
    pm: &mut Pixmap,
    sw: i32,
    sh: i32,
    body: &str,
    important: bool,
) -> (f32, f32, f32, f32) {
    let (sw, sh) = (sw as f32, sh as f32);
    let s = sw.min(sh);
    let pad = (s * 0.014).max(6.0);
    let bar_w = (sw * 0.112).max(56.0);
    let bar_x0 = pad;
    let bar_x1 = pad + bar_w;
    let top_h = (sh * 0.105).max(42.0);
    let slant = top_h * 0.62;
    let tag_x2 = (sw - pad).min(sw * 0.29);

    // 白底
    fill_solid(pm, 255, 255, 255, 255);

    // 左侧深蓝竖条
    fill_rect(pm, bar_x0, pad, bar_x1, sh - pad, "#0A2F70");
    // 顶部斜切标题块
    let pts = [
        (bar_x0, pad),
        (tag_x2, pad),
        (tag_x2 - slant, pad + top_h),
        (bar_x0, pad + top_h),
    ];
    fill_poly(pm, &pts, "#123C86");
    let title = if important { "重要通知" } else { "班级通知" };
    fonts.draw(
        pm,
        bar_x0 + (bar_w * 0.22).max(8.0),
        pad + top_h * 0.5,
        title,
        (top_h * 0.40).max(16.0).max(16.0),
        true,
        Color::from_rgba8(255, 255, 255, 255),
        Anchor::Left,
        None,
        false,
    );
    // 竖排四字
    let vert: Vec<&str> = if important {
        vec!["重", "要", "通", "知"]
    } else {
        vec!["班", "级", "通", "知"]
    };
    let vpx = (bar_w * 0.40).max(20.0);
    let cx = (bar_x0 + bar_x1) / 2.0;
    let total_h = vert.len() as f32 * vpx * 1.25;
    for (i, t) in vert.iter().enumerate() {
        fonts.draw(
            pm,
            cx,
            sh * 0.5 - total_h / 2.0 + vpx * 0.62 + i as f32 * vpx * 1.25,
            t,
            vpx,
            true,
            Color::from_rgba8(255, 255, 255, 255),
            Anchor::Center,
            None,
            false,
        );
    }

    // 右侧主卡片
    let cx0 = bar_x1;
    let cy0 = (pad + top_h + pad * 0.4).max(sh * 0.135);
    let cx1 = sw - pad;
    let cy1 = sh - pad;
    let r = (s * 0.024).max(10.0);
    rounded_rect(pm, cx0 - 8.0, cy0 - 6.0, cx1 + 8.0, cy1 + 10.0, r + 8.0, "#D7E5DC");
    rounded_rect(pm, cx0 - 4.0, cy0 - 3.0, cx1 + 4.0, cy1 + 5.0, r + 4.0, "#FFFFFF");
    rounded_rect(pm, cx0, cy0, cx1, cy1, r, "#E9FEF1");

    // 状态小标签
    let tag_h = (sh * 0.062).max(24.0);
    let tag_w = (sw * 0.155).max(140.0);
    let tx = cx0 + (sw * 0.042).max(18.0);
    let ty = cy0 + (sh * 0.032).max(14.0);
    rounded_rect(pm, tx, ty, tx + tag_w, ty + tag_h, tag_h * 0.30, "#FFFFFF");
    fonts.draw(
        pm,
        tx + tag_h * 0.45,
        ty + tag_h * 0.52,
        "教室通知 · 请查看",
        (tag_h * 0.40).max(11.0),
        false,
        Color::from_rgba8(0x8C, 0x9A, 0x93, 255),
        Anchor::Left,
        None,
        false,
    );

    // 居中超大正文
    let ax1 = cx0 + (sw * 0.055).max(30.0);
    let ax2 = cx1 - (sw * 0.055).max(30.0);
    let ay1 = ty + tag_h + (sh * 0.015).max(12.0);
    let ay2 = cy1 - (sh * 0.13).max(48.0);
    let max_px = ((ax2 - ax1) / 11.0).min(sh * 0.15).max(28.0);
    let min_px = (sh * 0.030).max(20.0);
    let (lines, px) = fit_notice_lines(fonts, body, ax2 - ax1, ay2 - ay1, max_px, min_px);
    let line_h = px * 1.36;
    let y0 = (ay1 + ay2) / 2.0 - (lines.len() as f32 - 1.0) * line_h / 2.0;
    for (i, line) in lines.iter().enumerate() {
        fonts.draw(
            pm,
            (ax1 + ax2) / 2.0,
            y0 + i as f32 * line_h,
            line,
            px,
            true,
            Color::from_rgba8(0x0B, 0x0B, 0x0B, 255),
            Anchor::Center,
            None,
            false,
        );
    }

    // 左下角发送时间
    let now = chrono::Local::now();
    fonts.draw(
        pm,
        cx0 + (sw * 0.042).max(18.0),
        cy1 - (sh * 0.085).max(30.0),
        &format!("发送时间：今天 {}", now.format("%H:%M")),
        (sh * 0.032).max(12.0),
        false,
        Color::from_rgba8(0x7C, 0x8A, 0x82, 255),
        Anchor::Left,
        None,
        false,
    );

    // 右下角「关闭」按钮：唯一能关掉弹屏的地方
    let bw = (sw * 0.16).max(120.0);
    let bh = (sh * 0.10).max(46.0);
    let bx2 = cx1 - (sw * 0.018).max(14.0);
    let by2 = cy1 - (sh * 0.032).max(14.0);
    let bx1 = bx2 - bw;
    let by1 = by2 - bh;
    rounded_rect(pm, bx1, by1, bx2, by2, bh * 0.26, "#B3261E");
    fonts.draw(
        pm,
        (bx1 + bx2) / 2.0,
        by1 + bh * 0.52,
        "关闭",
        (bh * 0.40).max(16.0),
        true,
        Color::from_rgba8(255, 255, 255, 255),
        Anchor::Center,
        None,
        false,
    );
    fonts.draw(
        pm,
        cx0 + (sw * 0.042).max(18.0),
        cy1 - (sh * 0.032).max(14.0),
        "未点「关闭」前本通知会一直显示",
        (sh * 0.030).max(13.0),
        false,
        Color::from_rgba8(0xB0, 0x3A, 0x2E, 255),
        Anchor::Left,
        None,
        false,
    );
    (bx1, by1, bx2, by2)
}

fn fill_rect(pm: &mut Pixmap, x1: f32, y1: f32, x2: f32, y2: f32, color: &str) {
    let mut pb = tiny_skia::PathBuilder::new();
    pb.push_rect(tiny_skia::Rect::from_xywh(x1, y1, x2 - x1, y2 - y1).unwrap());
    if let Some(path) = pb.finish() {
        let c = crate::theme::rgb(color);
        fill_path_color(pm, &path, Color::from_rgba8(c.r, c.g, c.b, 255));
    }
}

fn fill_poly(pm: &mut Pixmap, pts: &[(f32, f32)], color: &str) {
    let mut pb = tiny_skia::PathBuilder::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        if i == 0 {
            pb.move_to(*x, *y);
        } else {
            pb.line_to(*x, *y);
        }
    }
    pb.close();
    if let Some(path) = pb.finish() {
        let c = crate::theme::rgb(color);
        fill_path_color(pm, &path, Color::from_rgba8(c.r, c.g, c.b, 255));
    }
}

fn rounded_rect(pm: &mut Pixmap, x1: f32, y1: f32, x2: f32, y2: f32, radius: f32, color: &str) {
    let mut pb = tiny_skia::PathBuilder::new();
    let r = radius.max(1.0).min((x2 - x1) / 2.0).min((y2 - y1) / 2.0);
    layered::push_round_rect(&mut pb, x1, y1, x2 - x1, y2 - y1, r);
    if let Some(path) = pb.finish() {
        let c = crate::theme::rgb(color);
        fill_path_color(pm, &path, Color::from_rgba8(c.r, c.g, c.b, 255));
    }
}

/// SAPI 语音播报（PowerShell 后台执行，同一条播三遍）。
fn speak_notice(text: &str) {
    let clean = text
        .replace("重要通知", "")
        .replace("重要消息", "")
        .trim()
        .to_string();
    if clean.is_empty() {
        return;
    }
    let b64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(clean.as_bytes())
    };
    let ps = format!(
        "Add-Type -AssemblyName System.Speech; \
         $s=New-Object System.Speech.Synthesis.SpeechSynthesizer; \
         $s.Rate=-1; \
         $t=[System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String('{}')); \
         $s.Speak($t); Start-Sleep -Milliseconds 700; \
         $s.Speak($t); Start-Sleep -Milliseconds 700; \
         $s.Speak($t)",
        b64
    );
    std::thread::spawn(move || {
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

// ==================== 点名动画 ====================

// 动画队列条目：(到期时间, step, steps, 名单)。
thread_local! {
    static ROLL_ANIM_QUEUE: std::cell::RefCell<Vec<(f64, usize, usize, Vec<String>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// 点名定格展示时长（秒）：滚动结束后停这么久让人看清名字，再关窗。
/// roll_seconds 是「滚动 + 定格」的总时长，不再额外叠加。
const ROLL_HOLD_SECS: f64 = 0.6;

/// 点名动画定时器周期(ms)：每步延时都对齐到这个网格，
/// 配合 16ms 定时器驱动，让"总时长"精确贴合设定值（旧版 100ms 轮询会向上取整拉长）。
const ROLL_TICK_MS: f64 = 16.0;

/// 给定滚动动画总时长(ms)与步数，返回每步延时(ms)，全部为 ROLL_TICK_MS 的整数倍，
/// 且 Σdelay 在网格意义上严格等于 total_ms（误差 ≤ 1 tick）。
/// 逐步放慢：delay(s) = a + b*(s/n)^p；用离散和精确反解 b，再逐项取整并补差。
fn roll_step_delays(total_ms: f64, n: usize) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let total_ticks = (total_ms / ROLL_TICK_MS).round().max(1.0) as i64;
    let a = 35.0 / ROLL_TICK_MS; // 起步间隔 ≈ 2 tick
    let p = 2.5; // 放慢曲线（与旧版手感一致）
    let shape: f64 = (0..n).map(|s| (s as f64 / n as f64).powf(p)).sum();
    let b = if shape > 0.0 {
        ((total_ticks as f64 - a * n as f64) / shape).max(0.0)
    } else {
        0.0
    };
    let mut ticks: Vec<i64> = (0..n)
        .map(|s| ((a + b * (s as f64 / n as f64).powf(p)).round() as i64).max(1))
        .collect();
    // 逐项取整后补差：从后往前分摊剩余 tick（后期延时本就长，改动最不显眼）
    let mut diff = total_ticks - ticks.iter().sum::<i64>();
    let mut i = n;
    while diff != 0 && i > 0 {
        i -= 1;
        let d = if diff > 0 { 1 } else { -1 };
        if ticks[i] + d >= 1 {
            ticks[i] += d;
            diff -= d;
        }
    }
    ticks.into_iter().map(|t| t as f64 * ROLL_TICK_MS).collect()
}

fn animate_roll(app: &mut App, students: Vec<String>, step: usize, steps: usize) {
    if app.roll_hwnd.is_none() {
        app.rolling = false;
        return;
    }
    if step >= steps {
        let name = app.pick_student();
        app.finish_roll(name);
        return;
    }
    if !students.is_empty() {
        app.roll_var = students[rand_below(students.len() as u64) as usize].clone();
    }
    let (fs, bf) = roll_fonts(&app.cfg());
    paint_roll(app, fs, bf);
    // roll_seconds 是「滚动 + 定格」总时长：扣掉定格后才是滚动动画预算。
    // 每步延时已对齐 ROLL_TICK_MS(16ms) 网格，Σdelay 精确等于滚动预算。
    let total_ms = app.cfg().roll_seconds.clamp(2.0, 5.0) * 1000.0;
    let scroll_ms = total_ms - ROLL_HOLD_SECS * 1000.0;
    let delay = roll_step_delays(scroll_ms, steps)
        .get(step)
        .copied()
        .unwrap_or(ROLL_TICK_MS);
    let due = now_secs() + delay / 1000.0;
    ROLL_ANIM_QUEUE.with(|q| {
        q.borrow_mut().push((due, step + 1, steps, students));
    });
}

// ==================== 各窗口过程 ====================

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    // try_borrow_mut：窗口过程可能重入（模态消息框、DestroyWindow 同步派发 WM_DESTROY 等），
    // 已借用时跳过本次处理，等下一个定时器周期重试，避免 RefCell already borrowed panic。
    APP.with(|a| match a.try_borrow_mut() {
        Ok(mut b) => b.as_mut().map(|app| f(app)),
        Err(_) => None,
    })
}

thread_local! {
    static APP: std::cell::RefCell<Option<App>> = const { std::cell::RefCell::new(None) };
}

/// 根窗口：收定时器与菜单命令。
unsafe extern "system" fn root_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    if msg == WM_TIMER {
        match wp {
            T_POLL => {
                with_app(|app| {
                    // 点名动画由 T_ROLL(16ms) 驱动，避免 100ms 网格把每步延时取整拉长
                    // 长按 800ms 未抬起且未拖动 → 弹菜单（对应 _do_long_press）
                    let long_press_hwnd = {
                        let d = app.drag.as_ref();
                        match d {
                            Some(d) if !d.active && !d.long_press_done && d.is_bar
                                && now_secs() - d.press_time
                                    >= crate::theme::LONG_PRESS_MS as f64 / 1000.0 =>
                            {
                                Some(d.hwnd)
                            }
                            _ => None,
                        }
                    };
                    if let Some(h) = long_press_hwnd {
                        if let Some(d) = app.drag.as_mut() {
                            d.long_press_done = true;
                        }
                        app.show_menu(h);
                    }
                    // 配置热加载放在 100ms 轮询里（内部 250ms 节流），
                    // 设置页改动约 0.3 秒即可生效，不再等 30 秒的 T_CLASSCHECK。
                    app.reload_config_if_changed();
                    app.poll_queue();
                    // 滚轮调节侧边栏：150ms 限流重建，停手 600ms 后落盘
                    if let Some(t) = app.bar_dirty {
                        let now = now_secs();
                        if now - app.bar_rebuild_at >= 0.15 {
                            app.bar_rebuild_at = now;
                            app.update_bars(); // 尺寸未变时内部只 move_to，开销小
                        }
                        if now - t >= 0.6 {
                            app.cfg.lock().unwrap().save();
                            // 标记 mtime，避免自触发热加载
                            app.cfg_mtime =
                                Some(crate::paths::file_mtime_ms(&crate::paths::config_file()));
                            app.bar_dirty = None;
                            app.bar_rebuild_at = 0.0;
                        }
                    }
                    // 滚轮调节通知横幅：150ms 限流重绘，停手 600ms 后落盘
                    if let Some(t) = app.notice_dirty {
                        let now = now_secs();
                        if now - app.notice_rebuild_at >= 0.15 {
                            app.notice_rebuild_at = now;
                            app.refresh_notice_bar();
                        }
                        if now - t >= 0.6 {
                            app.cfg.lock().unwrap().save();
                            app.cfg_mtime =
                                Some(crate::paths::file_mtime_ms(&crate::paths::config_file()));
                            app.notice_dirty = None;
                            app.notice_rebuild_at = 0.0;
                        }
                    }
                });
                return 0;
            }
            T_ROLL => {
                with_app(|app| {
                    // 16ms 网格消费点名动画队列：到期才步进，总时长精确贴合设定
                    let now_t = now_secs();
                    let next = ROLL_ANIM_QUEUE.with(|q| {
                        let mut b = q.borrow_mut();
                        if let Some(front) = b.first() {
                            if front.0 <= now_t {
                                return Some(b.remove(0));
                            }
                        }
                        None
                    });
                    if let Some((_, step, steps, students)) = next {
                        animate_roll(app, students, step, steps);
                    }
                    // 定格到期也在这里检查（100ms 的 poll_queue 兜底）
                    app.check_roll_close();
                });
                return 0;
            }
            T_CLASSCHECK => {
                with_app(|app| app.check_class_loop());
                return 0;
            }
            T_MINUTE => {
                with_app(|app| app.update_minute());
                return 0;
            }
            _ => {}
        }
    }
    if msg == WM_COMMAND {
        with_app(|app| app.handle_command(wp));
        return 0;
    }
    if msg == WM_APP + 1 {
        with_app(|app| {
            if let Some(cd) = app.cd.as_ref() {
                if cd.ended {
                    app.close_countdown();
                }
            }
        });
        return 0;
    }
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return 0;
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// 通知横幅窗口：单击关闭 / 滚轮自由缩放 / 按住拖动改位置 / 右键弹菜单。
unsafe extern "system" fn notice_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_SETCURSOR => return force_arrow_cursor(),
        WM_LBUTTONDOWN => {
            with_app(|app| {
                let c = app.cfg();
                app.notice_drag = Some((cursor_x(), cursor_y(), c.notice_offset_x, c.notice_offset_y, false));
            });
            SetCapture(hwnd);
            return 0;
        }
        WM_MOUSEMOVE => {
            with_app(|app| {
                let Some((sx, sy, ox, oy, active)) = app.notice_drag.as_ref().copied() else { return };
                let dx = cursor_x() - sx;
                let dy = cursor_y() - sy;
                if !active
                    && dx.abs() <= NOTICE_DRAG_THRESHOLD
                    && dy.abs() <= NOTICE_DRAG_THRESHOLD
                {
                    return;
                }
                if !active {
                    if let Some(d) = app.notice_drag.as_mut() {
                        d.4 = true;
                    }
                }
                {
                    let mut c = app.cfg.lock().unwrap();
                    c.notice_offset_x = ox + dx;
                    c.notice_offset_y = oy + dy;
                }
                app.place_notice_bar();
            });
            return 0;
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            with_app(|app| {
                let dragged = app.notice_drag.take().map(|d| d.4).unwrap_or(false);
                if dragged {
                    // 拖动结束落盘（刷新 mtime，避免自触发热加载）
                    app.cfg.lock().unwrap().save();
                    app.cfg_mtime = Some(crate::paths::file_mtime_ms(&crate::paths::config_file()));
                } else {
                    app.manual_close_notice();
                }
            });
            return 0;
        }
        WM_RBUTTONUP => {
            with_app(|app| app.show_menu(hwnd));
            return 0;
        }
        // 滚轮：自由调节通知横幅大小（0.5×~2.5×）
        WM_MOUSEWHEEL => {
            let delta = ((wp >> 16) & 0xFFFF) as u16 as i16 as i32;
            if delta != 0 {
                with_app(|app| {
                    let steps = (delta as f64 / 120.0) * NOTICE_SCALE_STEP;
                    let mut c = app.cfg.lock().unwrap();
                    let cur = notice_scale_of(&c);
                    let next = (cur + steps).clamp(NOTICE_SCALE_MIN, NOTICE_SCALE_MAX);
                    if (next - cur).abs() >= f64::EPSILON {
                        c.notice_scale = next;
                        drop(c);
                        app.notice_dirty = Some(now_secs());
                    }
                });
            }
            return 0;
        }
        WM_DESTROY => return 0,
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// 半屏通知卡窗口过程：点击卡片任意位置（或按 Esc）即关闭。
/// 无超时自动关，不点就一直显示。
unsafe extern "system" fn popup_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_SETCURSOR => return force_arrow_cursor(),
        WM_LBUTTONUP => {
            with_app(|app| app.close_popup());
            return 0;
        }
        WM_KEYDOWN if wp == 0x1B => {
            // Esc 是「关闭」的键盘等价操作
            with_app(|app| app.close_popup());
            return 0;
        }
        WM_DESTROY => return 0,
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// 悬浮窗光标：统一强制系统箭头。
///
/// 层窗口（WS_EX_LAYERED|WS_EX_NOACTIVATE）在某些情况下不会触发类光标复位，
/// 会把上一个窗口留下的系统忙碌圆环（蓝色 O）一直粘在鼠标上，横幅上最明显。
fn force_arrow_cursor() -> isize {
    unsafe {
        SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16));
    }
    1
}

/// 同步新文件提醒窗：点击即关（Python 版无点击处理，此处小幅增强）。
unsafe extern "system" fn sync_notice_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_SETCURSOR => return force_arrow_cursor(),
        WM_LBUTTONUP => {
            with_app(|app| app.close_sync_notice());
            return 0;
        }
        WM_DESTROY => return 0,
        _ => {}
    }
    let _ = (wp, lp);
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// 侧边条窗口：拖动 / 单击 / 长按 / 右键。
unsafe extern "system" fn sidebar_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_SETCURSOR => return force_arrow_cursor(),
        WM_LBUTTONDOWN => {
            let x = (lp & 0xFFFF) as u16 as i32;
            let y = ((lp >> 16) & 0xFFFF) as u16 as i32;
            with_app(|app| {
                let mut rect = RECT::default();
                GetWindowRect(hwnd, &mut rect);
                app.drag = Some(DragState {
                    hwnd,
                    start_x: cursor_x(),
                    start_y: cursor_y(),
                    win_x: rect.left,
                    win_y: rect.top,
                    active: false,
                    press_time: now_secs(),
                    long_press_done: false,
                    is_bar: true,
                });
                SetCapture(hwnd);
                let _ = (x, y);
            });
            return 0;
        }
        WM_MOUSEMOVE => {
            with_app(|app| {
                let Some((dhwnd, sx, sy, wx, wy)) = app
                    .drag
                    .as_ref()
                    .map(|d| (d.hwnd, d.start_x, d.start_y, d.win_x, d.win_y))
                else { return };
                if dhwnd != hwnd {
                    return;
                }
                let dx = cursor_x() - sx;
                let dy = cursor_y() - sy;
                let was_active = app.drag.as_ref().map(|d| d.active).unwrap_or(false);
                if !was_active && dx.abs() <= DRAG_THRESHOLD && dy.abs() <= DRAG_THRESHOLD {
                    return;
                }
                if let Some(d) = app.drag.as_mut() {
                    d.active = true;
                }
                // x / y 都可自由拖动：不再只改上下偏移、把两条钉死在左右贴边
                let (sw, sh) = crate::win::screen_size();
                let (bw, bh, ..) = app.bar_geometry();
                let nx = (wx + dx).clamp(0, (sw - bw).max(0));
                let ny = (wy + dy).clamp(0, (sh - bh).max(0));
                let is_left = app.bars[0] == Some(hwnd);
                {
                    let mut c = app.cfg.lock().unwrap();
                    if is_left {
                        c.bar_left_x = nx;
                    } else {
                        c.bar_right_x = nx;
                    }
                    c.bar_top_offset = ny - (sh - bh) / 2;
                }
                app.update_bars();
            });
            return 0;
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            with_app(|app| {
                let Some(d) = app.drag.take() else { return };
                if d.hwnd != hwnd {
                    return;
                }
                if d.long_press_done {
                    return;
                }
                if d.active {
                    // 拖动结束：保存偏移（标记 mtime，避免自触发热加载）
                    app.cfg.lock().unwrap().save();
                    app.cfg_mtime = Some(crate::paths::file_mtime_ms(&crate::paths::config_file()));
                    beep(80, 50);
                    return;
                }
                // 防抖
                if now_secs() - app.last_touch < TOUCH_DEBOUNCE as f64 / 1000.0 {
                    return;
                }
                app.last_touch = now_secs();
                let y = ((lp >> 16) & 0xFFFF) as u16 as i32;
                let mut rect = RECT::default();
                GetClientRect(hwnd, &mut rect);
                if y < rect.bottom / 2 {
                    app.show_countdown();
                } else {
                    app.open_roll();
                }
            });
            return 0;
        }
        WM_RBUTTONUP => {
            with_app(|app| app.show_menu(hwnd));
            return 0;
        }
        // 滚轮：自由调节侧边栏大小（0.5×~2.5×）
        WM_MOUSEWHEEL => {
            // delta 在 wParam 高字（GET_WHEEL_DELTA_WPARAM），低字是修饰键
            let delta = ((wp >> 16) & 0xFFFF) as u16 as i16 as i32; // 每格 ±120
            with_app(|app| {
                if delta == 0 {
                    return;
                }
                let steps = (delta as f64 / 120.0) * crate::theme::BAR_SCALE_STEP;
                let mut c = app.cfg.lock().unwrap();
                let cur = if c.bar_scale > 0.0 { c.bar_scale } else { bar_scale(&c.bar_size) };
                let next = (cur + steps).clamp(crate::theme::BAR_SCALE_MIN, crate::theme::BAR_SCALE_MAX);
                if (next - cur).abs() < f64::EPSILON {
                    return;
                }
                c.bar_scale = next;
                app.bar_dirty = Some(now_secs());
            });
            return 0;
        }
        WM_DESTROY => return 0,
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn cursor_x() -> i32 {
    let mut pt = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut pt);
    }
    pt.x
}

fn cursor_y() -> i32 {
    let mut pt = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut pt);
    }
    pt.y
}

/// 倒计时窗口。
unsafe extern "system" fn countdown_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_SETCURSOR => return force_arrow_cursor(),
        WM_TIMER if wp == T_CD_TICK => {
            with_app(|app| app.countdown_tick());
            return 0;
        }
        WM_LBUTTONDOWN => {
            with_app(|app| {
                let x = (lp & 0xFFFF) as u16 as i32;
                let y = ((lp >> 16) & 0xFFFF) as u16 as i32;
                if let Some(cd) = app.cd.as_mut() {
                    // 按钮命中优先
                    for b in &cd.buttons {
                        if x as f32 >= b.rect.0
                            && x as f32 <= b.rect.2
                            && y as f32 >= b.rect.1
                            && y as f32 <= b.rect.3
                        {
                            match b.cmd {
                                CD_CMD_ADD => {
                                    cd.remaining += 60;
                                    paint_countdown(app);
                                }
                                CD_CMD_SUB => {
                                    if cd.remaining > 60 {
                                        cd.remaining -= 60;
                                        cd.start_time = now_secs();
                                        paint_countdown(app);
                                    }
                                }
                                CD_CMD_CLOSE => app.close_countdown(),
                                _ => {}
                            }
                            return;
                        }
                    }
                }
                // 其余区域拖动
                let mut rect = RECT::default();
                GetWindowRect(hwnd, &mut rect);
                app.drag = Some(DragState {
                    hwnd,
                    start_x: cursor_x(),
                    start_y: cursor_y(),
                    win_x: rect.left,
                    win_y: rect.top,
                    active: false,
                    press_time: now_secs(),
                    long_press_done: false,
                    is_bar: false,
                });
                SetCapture(hwnd);
            });
            return 0;
        }
        WM_MOUSEMOVE => {
            with_app(|app| {
                let Some(d) = app.drag.as_mut() else { return };
                if d.hwnd != hwnd {
                    return;
                }
                let dx = cursor_x() - d.start_x;
                let dy = cursor_y() - d.start_y;
                if !d.active && (dx.abs() > DRAG_THRESHOLD || dy.abs() > DRAG_THRESHOLD) {
                    d.active = true;
                }
                if d.active {
                    let nx = d.win_x + dx;
                    let ny = d.win_y + dy;
                    if let Some(cd) = app.cd.as_mut() {
                        cd.pos = (nx, ny);
                    }
                    let (w, h) = cd_window_size(&app.cfg());
                    layered::move_to(hwnd, nx, ny, w, h);
                }
            });
            return 0;
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            with_app(|app| {
                app.drag = None;
            });
            return 0;
        }
        WM_DESTROY => return 0,
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn paint_countdown(app: &mut App) {
    let Some(hwnd) = app.cd_hwnd else { return };
    let c = app.cfg();
    let (_, _, fs_pt, bf_pt, bp) = cd_metrics(&c);
    let k = crate::win::dpi_scale() as f32;
    let (sw, _sh) = crate::win::screen_size();
    let (cw_px, h_px) = cd_window_size(&c);
    let w = cw_px.min((sw as f64 * WIN_MAX_SCREEN_RATIO) as i32);
    let h = h_px;
    let Some(cd) = app.cd.as_mut() else { return };
    // 显示文本兜底
    if cd.shown.is_empty() {
        cd.shown = format!("{:02}:{:02}", cd.remaining / 60, cd.remaining % 60);
    }
    let shown = cd.shown.clone();
    let mut buttons_state = cd.buttons.clone();
    let ended = cd.ended;
    let Some(mut pm) = Pixmap::new(w as u32, h as u32) else { return };
    fill_solid(&mut pm, 0, 0, 0, 0);
    layered::rounded_bg(&mut pm, 6.0 * k, [255, 255, 255, 247]);
    stroke_round(&mut pm, 6.0 * k, "#dddddd");
    let fs = tk_px(fs_pt);
    let bf = tk_px(bf_pt);
    // 兜底：任何 DPI / 尺寸档位下都保证大数字底部不会压到按钮行
    let btn_h_pre = bf * 2.2;
    let by_pre = h as f32 - btn_h_pre - bp as f32 * k - 8.0 * k;
    let digit_cap = ((by_pre - h as f32 * 0.38) * 1.6).max(fs * 0.5);
    let tpx = if ended { fs * 0.6 } else { fs.min(digit_cap) };
    app.fonts.draw(
        &mut pm,
        w as f32 / 2.0,
        h as f32 * 0.38,
        &shown,
        tpx,
        true,
        Color::from_rgba8(0x00, 0x67, 0xB8, 255),
        Anchor::Center,
        None,
        false,
    );
    // 按钮排布：所有留白随 DPI 等比放大，确保按钮始终在数字下方不重叠
    let btn_w = (w as f32 - 48.0 * k) / 3.0;
    let btn_h = bf * 2.2;
    let by = h as f32 - btn_h - bp as f32 * k - 8.0 * k;
    let defs = [
        ("−1分钟", BTN_MINUS_BG, CD_CMD_SUB),
        ("+1分钟", BTN_PLUS_BG, CD_CMD_ADD),
        ("关闭", BTN_CLOSE_BG, CD_CMD_CLOSE),
    ];
    buttons_state.clear();
    for (i, (label, color, cmd)) in defs.iter().enumerate() {
        let bx = 16.0 * k + i as f32 * (btn_w + 8.0 * k);
        rounded_rect(&mut pm, bx, by, bx + btn_w, by + btn_h, 8.0 * k, color);
        app.fonts.draw(
            &mut pm,
            bx + btn_w / 2.0,
            by + btn_h / 2.0,
            label,
            bf,
            true,
            Color::from_rgba8(0x33, 0x33, 0x33, 255),
            Anchor::Center,
            None,
            false,
        );
        buttons_state.push(HitButton {
            rect: (bx, by, bx + btn_w, by + btn_h),
            cmd: *cmd,
        });
    }
    if let Some(cd) = app.cd.as_mut() {
        cd.buttons = buttons_state;
    }
    layered::present(hwnd, &pm);
}

fn stroke_round(pm: &mut Pixmap, radius: f32, color: &str) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let mut pb = tiny_skia::PathBuilder::new();
    layered::push_round_rect(&mut pb, 1.0, 1.0, w - 2.0, h - 2.0, radius);
    if let Some(path) = pb.finish() {
        let c = crate::theme::rgb(color);
        stroke_path_color(pm, &path, Color::from_rgba8(c.r, c.g, c.b, 255), 1.0);
    }
}

/// 点名窗。
unsafe extern "system" fn roll_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_SETCURSOR => return force_arrow_cursor(),
        WM_LBUTTONDOWN => {
            with_app(|app| {
                let x = (lp & 0xFFFF) as u16 as i32;
                let y = ((lp >> 16) & 0xFFFF) as u16 as i32;
                if let Some(roll) = app.roll.as_mut() {
                    for b in &roll.buttons {
                        if x as f32 >= b.rect.0
                            && x as f32 <= b.rect.2
                            && y as f32 >= b.rect.1
                            && y as f32 <= b.rect.3
                        {
                            match b.cmd {
                                ROLL_CMD_SETTINGS => open_settings_process(),
                                ROLL_CMD_HISTORY => open_history_process(),
                                ROLL_CMD_QUIT => confirm_exit(),
                                _ => {}
                            }
                            return;
                        }
                    }
                }
                let mut rect = RECT::default();
                GetWindowRect(hwnd, &mut rect);
                app.drag = Some(DragState {
                    hwnd,
                    start_x: cursor_x(),
                    start_y: cursor_y(),
                    win_x: rect.left,
                    win_y: rect.top,
                    active: false,
                    press_time: now_secs(),
                    long_press_done: false,
                    is_bar: false,
                });
                SetCapture(hwnd);
            });
            return 0;
        }
        WM_MOUSEMOVE => {
            with_app(|app| {
                let Some(d) = app.drag.as_mut() else { return };
                if d.hwnd != hwnd {
                    return;
                }
                let dx = cursor_x() - d.start_x;
                let dy = cursor_y() - d.start_y;
                if !d.active && (dx.abs() > DRAG_THRESHOLD || dy.abs() > DRAG_THRESHOLD) {
                    d.active = true;
                }
                if d.active {
                    let nx = d.win_x + dx;
                    let ny = d.win_y + dy;
                    if let Some(roll) = app.roll.as_mut() {
                        roll.pos = (nx, ny);
                    }
                    let c = app.cfg();
                    let scale = roll_win_scale(&c);
                    let (sw, sh) = crate::win::screen_size();
                    let w = ((ROLL_W_DEF as f64 * scale) as i32).min((sw as f64 * WIN_MAX_SCREEN_RATIO) as i32);
                    let h = ((ROLL_H_DEF as f64 * scale) as i32).min((sh as f64 * WIN_MAX_SCREEN_RATIO) as i32);
                    layered::move_to(hwnd, nx, ny, w, h);
                }
            });
            return 0;
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            with_app(|app| {
                app.drag = None;
            });
            return 0;
        }
        WM_DESTROY => {
            with_app(|app| {
                app.roll_hwnd = None;
                app.roll = None;
                app.rolling = false;
            });
            return 0;
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn paint_roll(app: &mut App, fs: f32, bf: f32) {
    let Some(hwnd) = app.roll_hwnd else { return };
    if app.roll.is_none() {
        return;
    }
    let text = app.roll_var.clone();
    let c = app.cfg();
    let scale = roll_win_scale(&c);
    let (sw, _sh) = crate::win::screen_size();
    let w = ((ROLL_W_DEF as f64 * scale) as i32).min((sw as f64 * WIN_MAX_SCREEN_RATIO) as i32);
    let h = ((ROLL_H_DEF as f64 * scale) as i32).min((_sh as f64 * WIN_MAX_SCREEN_RATIO) as i32);
    let Some(mut pm) = Pixmap::new(w as u32, h as u32) else { return };
    fill_solid(&mut pm, 0, 0, 0, 0);
    layered::rounded_bg(&mut pm, 10.0, [255, 255, 255, 250]);
    // 大字（超宽缩小字号）
    let mut px = fs;
    let max_w = w as f32 * 0.9;
    while px > 30.0 && app.fonts.measure(&text, px, true) > max_w {
        px -= 4.0;
    }
    let shown = app.fonts.ellipsis(&text, max_w, px, true);
    app.fonts.draw(
        &mut pm,
        w as f32 / 2.0,
        h as f32 * 0.42,
        &shown,
        px,
        true,
        Color::from_rgba8(0x11, 0x11, 0x11, 255),
        Anchor::Center,
        None,
        false,
    );
    // 按钮行：设置 记录 | 退出程序
    let bh = bf * 2.4;
    let by = h as f32 - bh - 12.0;
    let mut buttons = Vec::new();
    let defs: [(&str, &str, usize, bool); 3] = [
        ("设置", BTN_ROLL_BG, ROLL_CMD_SETTINGS, false),
        ("记录", BTN_ROLL_BG, ROLL_CMD_HISTORY, false),
        ("退出程序", BTN_EXIT_BG, ROLL_CMD_QUIT, true),
    ];
    let bw = bf * 7.0;
    let gap = 10.0;
    let left_x = 12.0;
    let mut right_x = w as f32 - 12.0 - bw;
    for (i, (label, color, cmd, right)) in defs.iter().enumerate() {
        let bx = if *right { right_x } else { left_x + i as f32 * (bw + gap) };
        rounded_rect(&mut pm, bx, by, bx + bw, by + bh, 8.0, color);
        let fg = if *right { TEXT_EXIT_RED } else { "#333333" };
        let fc = crate::theme::rgb(fg);
        app.fonts.draw(
            &mut pm,
            bx + bw / 2.0,
            by + bh / 2.0,
            label,
            bf,
            true,
            Color::from_rgba8(fc.r, fc.g, fc.b, 255),
            Anchor::Center,
            None,
            false,
        );
        if *right {
            right_x -= bw + gap;
        }
        buttons.push(HitButton {
            rect: (bx, by, bx + bw, by + bh),
            cmd: *cmd,
        });
    }
    if let Some(roll) = app.roll.as_mut() {
        roll.buttons = buttons;
    }
    layered::present(hwnd, &pm);
}

// ==================== 启动 / 命令 ====================

/// 点名窗自由缩放（0 = 未设置 → 1.0，即档位原尺寸）
fn roll_scale_of(c: &Config) -> f64 {
    if c.roll_scale > 0.0 {
        c.roll_scale.clamp(WIN_SCALE_MIN, WIN_SCALE_MAX)
    } else {
        1.0
    }
}

/// 倒计时窗自由缩放
fn cd_scale_of(c: &Config) -> f64 {
    if c.countdown_scale > 0.0 {
        c.countdown_scale.clamp(WIN_SCALE_MIN, WIN_SCALE_MAX)
    } else {
        1.0
    }
}

/// 点名窗整体缩放倍数（档位倍数 × 自由缩放）
fn roll_win_scale(c: &Config) -> f64 {
    let (s, _, _) = roll_font(&c.roll_win_size);
    s * roll_scale_of(c)
}

fn roll_fonts(c: &Config) -> (f32, f32) {
    let (_, fs, bf) = roll_font(&c.roll_win_size);
    let k = roll_scale_of(c) as f32;
    (tk_px(fs as f32) * k, tk_px(bf as f32) * k)
}

/// 倒计时窗实际像素尺寸（已含自由缩放与 DPI）
fn cd_window_size(c: &Config) -> (i32, i32) {
    let (cw, h, ..) = cd_metrics(c);
    let d = crate::win::dpi_scale() as f32;
    ((cw as f32 * d) as i32, (h as f32 * d) as i32)
}

/// 倒计时窗 (宽, 高, 数字字号pt, 按钮字号pt, 按钮内边距)，已乘自由缩放
fn cd_metrics(c: &Config) -> (i32, i32, f32, f32, i32) {
    let (cw, h, fs, bf, bp) = countdown_size(&c.countdown_win_size);
    let k = cd_scale_of(c) as f32;
    (
        (cw as f32 * k) as i32,
        (h as f32 * k) as i32,
        fs as f32 * k,
        bf as f32 * k,
        (bp as f32 * k) as i32,
    )
}

fn bar_scale(size: &str) -> f64 {
    match size {
        "小" => 1.1,
        "中" => 1.4,
        _ => 1.8,
    }
}

/// 侧边栏缩放：自由系数 bar_scale 优先（滚轮/设置页滑杆），未设置回落三档。
fn bar_scale_of(c: &Config) -> f64 {
    if c.bar_scale > 0.0 {
        c.bar_scale.clamp(BAR_SCALE_MIN, BAR_SCALE_MAX)
    } else {
        bar_scale(&c.bar_size)
    }
}

/// 侧边条几何的纯函数版（主 UI 与设置页共用）。
/// 返回 (宽, 高, 当前 y, 左条 x, 右条 x, 可移动的最大 y)。
pub(crate) fn bar_dimensions(c: &Config) -> (i32, i32, i32, i32, i32, i32) {
    let (sw, sh) = crate::win::screen_size();
    let dpi = crate::win::dpi_scale();
    let (w_base, h_base) = if sw >= 3840 {
        ((40.0 * dpi) as i32, (420.0 * dpi) as i32)
    } else if sw >= 2560 {
        ((40.0 * dpi) as i32, (280.0 * dpi) as i32)
    } else {
        ((36.0 * dpi) as i32, (260.0 * dpi) as i32)
    };
    let scale = bar_scale_of(c);
    let bw = (w_base as f64 * scale) as i32;
    let bh = (h_base as f64 * scale) as i32;
    let max_y = (sh - bh).max(0);
    let y = ((sh - bh) / 2 + c.bar_top_offset).clamp(0, max_y);
    let max_x = (sw - bw).max(0);
    let gap = c.bar_left_gap;
    let left_x = if c.bar_left_x >= 0 {
        c.bar_left_x.clamp(0, max_x)
    } else {
        gap.clamp(0, max_x)
    };
    let right_x = if c.bar_right_x >= 0 {
        c.bar_right_x.clamp(0, max_x)
    } else {
        (sw - bw - gap).clamp(0, max_x)
    };
    (bw, bh, y, left_x, right_x, max_y)
}

fn append(menu: HMENU, flags: u32, id: usize, text: &str) {
    unsafe {
        AppendMenuW(menu, flags, id, wide(text).as_ptr());
    }
}

fn open_settings_process() {
    let exe = crate::win::exe_path();
    match std::process::Command::new(exe).arg("--settings").spawn() {
        Ok(_) => logging::info("已启动设置窗口进程"),
        Err(e) => logging::error(&format!("打开设置失败: {}", e)),
    }
}

fn open_history_process() {
    let exe = crate::win::exe_path();
    let _ = std::process::Command::new(exe)
        .arg("--history")
        .spawn()
        .map_err(|e| logging::error(&format!("打开历史失败: {}", e)));
}

/// 退出前先弹确认小窗（--confirm-exit 子进程）：输入 123 才写 exit.flag，
/// 主程序轮询到标记后才真正 quit_app——防止学生/误触一键退出整个系统。
fn confirm_exit() {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            logging::error(&format!("取当前 exe 失败，跳过退出确认: {e}"));
            return;
        }
    };
    if let Err(e) = std::process::Command::new(&exe).arg("--confirm-exit").spawn() {
        logging::error(&format!("启动退出确认窗口失败: {e}"));
    }
}

fn quit_app() {
    // 先让大屏（通知条 + 今日作业/值日）跟着退出：写 stop 文件，等它自己收尾、
    // 还原壁纸后，主程序再退。否则主窗口没了，作业栏还留在壁纸上。
    crate::services::stop_board();
    for _ in 0..40 {
        if !crate::win::board_is_running() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // 兜底：大屏没来得及还原壁纸时，这里补上
    let backup = desktop_board::paths::wallpaper_backup_file();
    if let Ok(prev) = std::fs::read_to_string(&backup) {
        desktop_board::win::wallpaper::restore_wallpaper(prev.trim());
        let _ = std::fs::remove_file(&backup);
    }
    with_app(|app| {
        app.history.flush();
        let cfg = app.cfg.lock().unwrap();
        cfg.save();
    });
    unsafe {
        PostQuitMessage(0);
    }
}

// ==================== 入口 ====================

pub fn run(cfg: Arc<Mutex<Config>>, history: Arc<HistoryManager>) {
    let (tx, rx) = std::sync::mpsc::channel::<UiMsg>();
    let (sw, sh) = crate::win::screen_size();
    let _ = (sw, sh);

    // 根窗口（隐藏）
    let inst = unsafe { GetModuleHandleW(std::ptr::null()) };
    // 类名 buffer 必须活过 RegisterClassW（WNDCLASSW 只存指针）
    let cls_buf = wide("RollCallRootRust");
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(root_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: inst,
        hIcon: std::ptr::null_mut(),
        hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16) },
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: cls_buf.as_ptr(),
    };
    unsafe {
        RegisterClassW(&wc);
        let root = CreateWindowExW(
            0,
            cls_buf.as_ptr(),
            wide("").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            inst,
            std::ptr::null_mut(),
        );
        if root.is_null() {
            logging::error("创建根窗口失败");
            return;
        }

        let notice_hwnd = layered::create_floating("RollNoticeBarRust", "通知", 400, 80, notice_proc)
            .unwrap_or(std::ptr::null_mut());

        // 先建 App（所有窗口过程都通过 thread_local 访问它）
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                cfg: cfg.clone(),
                history: history.clone(),
                ui_rx: rx,
                ui_tx: tx.clone(),
                fonts: Fonts::load(),
                root,
                notice_hwnd,
                notice_visible: false,
                notice_text: String::new(),
                notice_dirty: None,
                notice_rebuild_at: 0.0,
                notice_drag: None,
                schedule_text: String::new(),
                cached_notice: None,
                fetch_inflight: false,
                preload_done: false,
                last_period: None,
                last_idle_fetch_at: None,
                last_class_fetch_at: None,
                doorbell_was_ok: None,
                banner_hold_until: None,
                closed_until: None,
                was_class_time: None,
                startup_hide: true,
                startup_seen: None,
                board_notice_last: None,
                last_schedule_refresh: 0.0,
                popup: None,
                popup_close_rect: (-1.0, -1.0, -1.0, -1.0),
                last_popup_text: None,
                quick_display_at: None,
                sync_notice_hwnd: None,
                sync_notice_close_at: None,
                bars: [None, None],
                bar_size: None,
                last_minute: -1,
                cd: None,
                cd_hwnd: None,
                roll: None,
                roll_hwnd: None,
                roll_var: "准备点名".to_string(),
                rolling: false,
                roll_show_at: None,
                drag: None,
                last_touch: 0.0,
                bar_dirty: None,
                bar_rebuild_at: 0.0,
                sync_seen_files: false,
                cfg_mtime: None,
                last_cfg_check: 0.0,
                last_switch_sync: None,
                force_notice_once: false,
                pending_popup_flag: false,
                last_class_state: None,
                preload_period: None,
                preload_at: None,
            });
        });

        // 启动时序（对应 root.after 系列）
        SetTimer(root, T_POLL, 100, std::ptr::null_mut());
        SetTimer(root, T_MINUTE, 1000, std::ptr::null_mut());
        SetTimer(root, T_CLASSCHECK, 30000, std::ptr::null_mut());
        with_app(|app| {
            app.update_bars();
            app.update_schedule_text();
        });

        // 后台服务
        let cfg_bg = cfg.clone();
        let sync = Arc::new(crate::services::GiteeSyncManager::new());
        let sync2 = sync.clone();
        let tx2 = tx.clone();
        {
            let mut cb = sync.on_new_files.lock().unwrap();
            *cb = Box::new(move |files| {
                let _ = tx2.send(UiMsg::SyncNewFiles(files));
            });
        }
        std::thread::spawn(move || {
            // 启动后 3.5 秒开始同步（对应 after(3500)）
            std::thread::sleep(std::time::Duration::from_millis(3500));
            sync2.start(cfg_bg);
        });
        let hb = Arc::new(crate::services::HeartbeatService::new());
        let hb2 = hb.clone();
        let cfg_hb = cfg.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(3500));
            hb2.start(cfg_hb);
        });
        let sn = Arc::new(crate::services::ScheduledNoticeService::new());
        let sn2 = sn.clone();
        let cfg_sn = cfg.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(9000));
            seed_builtin_notices(&cfg_sn);
            sn2.start(cfg_sn);
        });
        // GoEasy 实时推送桥（启动 5 秒后；绑定唤醒端口 + 无头订阅页，失败降级为轮询）
        let cfg_ge = cfg.clone();
        let tx_ge = tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(5000));
            crate::goeasy::start(tx_ge, cfg_ge);
        });
        // 自动更新检查（启动 8 秒后，静默；发现新版本全自动下载自替换）
        let cfg_up = cfg.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(8000));
            crate::update::run_silent_check(cfg_up);
        });
        // 拉起桌面大屏：启动后 2 秒就自动打开作业栏 / 通知栏，
        // 原来等 12 秒看上去像「没自动开」。大屏是独立线程，不阻塞消息泵。
        let cfg_board = cfg.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(2000));
            crate::services::maybe_start_board(&cfg_board);
        });

        // 消息循环
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if r == 0 {
                break; // WM_QUIT
            }
            if r == -1 {
                // GetMessage 出错：记录后继续，避免静默退出
                crate::paths::trace(&format!(
                    "GETMESSAGE_ERR gle={} msg={:?}",
                    GetLastError(),
                    msg.message
                ));
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        crate::paths::trace("LOOP_EXIT");
        hb.stop();
        sync.stop();
        sn.stop();
        APP.with(|a| *a.borrow_mut() = None);
    }
}

fn rand_below(n: u64) -> u64 {
    let t = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    if n == 0 {
        return 0;
    }
    (t ^ (t >> 17) ^ (std::process::id() as u64)) % n
}

/// 启动播种：内置定时通知按内容去重合并进 Gitee 共享源。
fn seed_builtin_notices(cfg: &Arc<Mutex<Config>>) {
    let c = cfg.lock().unwrap().clone();
    if !(c.gitee_enabled && !c.gitee_token.is_empty()) {
        return;
    }
    let api = crate::gitee::GiteeApi::new(&c.gitee_owner, &c.gitee_repo, &c.gitee_token);
    let existing: Vec<crate::config::ScheduledNotice> =
        match api.download_file_content(&format!("{}/{}", c.gitee_tongzhi_dir, crate::services::SCHEDULED_NOTICES_PATH)) {
            Ok(raw) => serde_json::from_slice(&raw).unwrap_or_default(),
            Err(_) => c.scheduled_notices.clone(),
        };
    let builtin = [
        ("周四", 4usize, "14:00", "21:40", "请班长及时提交周五晚上留宿名单"),
        ("周五", 5, "14:00", "21:40", "温馨提醒:大家回家注意安全，骑车带好头盔，周末注意防溺水，防诈骗等"),
    ];
    let mut list = existing.clone();
    for (_, wd, s, e, content) in builtin {
        if list.iter().any(|n| n.content.trim() == content) {
            continue;
        }
        list.push(crate::config::ScheduledNotice {
            enabled: true,
            name: String::new(),
            content: content.to_string(),
            weekdays: vec![serde_json::json!(wd)],
            start: s.to_string(),
            end: e.to_string(),
            extra: Default::default(),
        });
    }
    if list.len() != existing.len() {
        let json = serde_json::to_string_pretty(&list).unwrap_or_default();
        let _ = api.create_text_file(&json, &format!("{}/{}", c.gitee_tongzhi_dir, crate::services::SCHEDULED_NOTICES_PATH), "master");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn roll_total_duration_exact() {
        // 总时长 = Σ每步延时（滚动） + 定格，必须精确落在目标 2~5 秒。
        // 每步延时已对齐 16ms 网格；模拟定时器按网格消费，实际总时长误差 ≤0.1s。
        for total in [2.0_f64, 3.0, 5.0] {
            let scroll_ms = total * 1000.0 - ROLL_HOLD_SECS * 1000.0;
            for steps in [22usize, 26, 30] {
                let delays = roll_step_delays(scroll_ms, steps);
                assert_eq!(delays.len(), steps);
                // 每步延时必须是网格整数倍
                for d in &delays {
                    assert!((d % ROLL_TICK_MS).abs() < 0.001, "延时 {d}ms 未对齐网格");
                }
                let sum: f64 = delays.iter().sum();
                assert!(
                    (sum - scroll_ms).abs() <= ROLL_TICK_MS,
                    "Σdelay={sum} 应≈滚动预算 {scroll_ms} (total={total}, steps={steps})"
                );
                // 模拟 16ms 定时器：每步实际耗时向上取整到网格（本已对齐，误差≈0）
                let rolled_ms: f64 = delays
                    .iter()
                    .map(|d| (d / ROLL_TICK_MS).ceil() * ROLL_TICK_MS)
                    .sum();
                let hold_ms = (ROLL_HOLD_SECS * 1000.0 / ROLL_TICK_MS).ceil() * ROLL_TICK_MS;
                let total_secs = (rolled_ms + hold_ms) / 1000.0;
                assert!(
                    (total_secs - total).abs() < 0.1,
                    "点名总时长 {total_secs} 应≈{total}（16ms 网格）"
                );
                // 逐步放慢：最后一步延时必须大于第一步
                assert!(delays[steps - 1] > delays[0]);
            }
        }
    }

    fn at(y: i32, m: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(y, m, d, h, mi, 0)
            .single()
            .unwrap()
    }

    /// 上课中：显示「距下课 X 分」，X 取该节结束时间与当前时间之差（进位到分钟）。
    #[test]
    fn sidebar_countdown_in_class() {
        // 2026-11-16 周一；08:10 处于第 1 节 08:00~08:40
        let l = sidebar_hint_at("summer", &at(2026, 11, 16, 8, 10));
        assert_eq!(l, vec!["距", "下", "课", "30分"]);
    }

    /// 课间：改为「距上课 X 分」。
    #[test]
    fn sidebar_countdown_between_classes() {
        let l = sidebar_hint_at("summer", &at(2026, 11, 16, 7, 30));
        assert_eq!(&l[..3], ["距", "上", "课"]);
        assert!(l[3].ends_with('分'), "末行应为分钟数：{:?}", l);
    }

    /// 周末：整块显示「已放学」。
    #[test]
    fn sidebar_countdown_weekend() {
        let l = sidebar_hint_at("summer", &at(2026, 11, 15, 10, 0)); // 周日
        assert_eq!(l, vec!["已", "放", "学"]);
    }

    /// 上课中进度条：08:20 处于第 1 节 08:00~08:40，已过一半。
    #[test]
    fn sidebar_status_in_class_progress() {
        let (label, mins, progress, urgent, off) = sidebar_status("summer", &at(2026, 11, 16, 8, 20));
        assert_eq!(label, "距下课");
        assert_eq!(mins, 20, "距 08:40 还有 20 分");
        assert!((progress - 0.5).abs() < 0.01, "本节已过一半，实际 {progress}");
        assert!(!urgent && !off);
    }

    /// ≤5 分钟转红：08:37 距下课 3 分。
    #[test]
    fn sidebar_status_urgent_before_end() {
        let (label, mins, _, urgent, off) = sidebar_status("summer", &at(2026, 11, 16, 8, 37));
        assert_eq!(label, "距下课");
        assert_eq!(mins, 3);
        assert!(urgent, "剩余 3 分应触发紧急色");
        assert!(!off);
    }

    /// 课间进度条：08:42 处于第 1 节结束(08:40)与第 2 节开始(08:45)之间。
    #[test]
    fn sidebar_status_break_progress() {
        let (label, mins, progress, _, off) = sidebar_status("summer", &at(2026, 11, 16, 8, 42));
        assert_eq!(label, "距上课");
        assert_eq!(mins, 3, "距 08:45 上课还有 3 分");
        assert!((progress - 0.4).abs() < 0.01, "课间 5 分已过 2 分=0.4，实际 {progress}");
        assert!(!off);
    }

    /// 放学后（晚自习 21:40 之后）：显示已放学，无进度。
    #[test]
    fn sidebar_status_after_school() {
        let (label, _, progress, _, off) = sidebar_status("summer", &at(2026, 11, 16, 22, 0));
        assert_eq!(label, "已放学");
        assert!(off);
        assert_eq!(progress, 0.0);
    }

    /// 圆角蒙版：四角被抹成全透明，中心像素不受影响。
    #[test]
    fn round_corners_clip() {
        let mut pm = Pixmap::new(60, 200).unwrap();
        pm.fill(Color::from_rgba8(10, 20, 30, 255));
        round_corners(&mut pm, 18.0);
        let d = pm.data();
        let alpha = |x: usize, y: usize| d[(y * 60 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0, "左上角应透明");
        assert_eq!(alpha(59, 0), 0, "右上角应透明");
        assert_eq!(alpha(0, 199), 0, "左下角应透明");
        assert_eq!(alpha(59, 199), 0, "右下角应透明");
        assert_eq!(alpha(30, 100), 255, "中心应完全不透明");
        // 边中点不能被削掉（说明只切了四个角）
        assert_eq!(alpha(30, 0), 255);
        assert_eq!(alpha(0, 100), 255);
    }

    /// 半屏弹屏：宽高约为屏幕 62% / 56%，且严格居中。
    #[test]
    fn popup_geometry_is_half_screen_and_centered() {
        let (x, y, w, h) = popup_geometry((1920, 1080));
        assert_eq!(w, (1920.0 * 0.62) as i32);
        assert_eq!(h, (1080.0 * 0.56) as i32);
        assert!(w < 1920 && h < 1080, "必须是半屏而非满屏");
        assert_eq!(x, (1920 - w) / 2);
        assert_eq!(y, (1080 - h) / 2);
        // 小屏不能越界
        let (_, _, w2, h2) = popup_geometry((800, 600));
        assert!(w2 <= 800 && h2 <= 600);
    }

    /// 手动冒烟：真的渲染一张半屏通知卡 PNG，检查关闭按钮命中区在卡片内。
    /// 运行：cargo test --release popup_preview -- --ignored --nocapture
    #[test]
    #[ignore = "手动图片冒烟，会写 notice_popup_preview.png"]
    fn popup_preview_writes_png() {
        let (_, _, w, h) = popup_geometry((1920, 1080));
        let mut pm = Pixmap::new(w as u32, h as u32).unwrap();
        let fonts = Fonts::load();
        let (body, important) = clean_notice_body("重要通知：测试测试2 测试美女");
        let (x1, y1, x2, y2) = draw_notice_popup(&fonts, &mut pm, w, h, &body, important);
        assert!(x2 > x1 && y2 > y1, "关闭按钮必须有面积");
        assert!(x2 <= w as f32 && y2 <= h as f32, "关闭按钮必须在窗口内");
        let png = pm.encode_png().unwrap();
        let out = std::path::PathBuf::from("..").join("notice_popup_preview.png");
        std::fs::write(&out, &png).unwrap();
        println!("预览已写出：{} ({w}x{h})，关闭区 ({x1},{y1})-({x2},{y2})", out.display());
    }

    /// 手动冒烟：渲染四种状态的侧边栏并排合成一张 PNG，肉眼检查布局。
    /// 运行：cargo test --release sidebar_preview -- --ignored --nocapture
    #[test]
    #[ignore = "手动图片冒烟，会写 sidebar_preview.png"]
    fn sidebar_preview_writes_png() {
        let fonts = Fonts::load();
        let bw: u32 = 72;
        let bh: u32 = 420; // 4K 屏基准尺寸，放大便于观察
        let gap = 30usize;
        let cases = [
            ("上课中 08:20", "summer", at(2026, 11, 16, 8, 20)),
            ("临近下课 08:37", "summer", at(2026, 11, 16, 8, 37)),
            ("课间 08:42", "summer", at(2026, 11, 16, 8, 42)),
            ("周末", "summer", at(2026, 11, 15, 10, 0)),
        ];
        let total_w = cases.len() as u32 * bw + (cases.len() as u32 - 1) * gap as u32;
        let mut big = Pixmap::new(total_w, bh).unwrap();
        big.fill(Color::from_rgba8(255, 255, 255, 255));
        let big_stride = big.width() as usize * 4;
        let src_stride = bw as usize * 4;
        for (i, (_, mode, now)) in cases.iter().enumerate() {
            let mut pm = Pixmap::new(bw, bh).unwrap();
            draw_sidebar(&mut pm, &fonts, mode, now);
            let x0 = i * (bw as usize + gap);
            let dst = big.data_mut();
            let src = pm.data();
            for y in 0..bh as usize {
                let dst_off = y * big_stride + x0 * 4;
                let src_off = y * src_stride;
                dst[dst_off..dst_off + src_stride].copy_from_slice(&src[src_off..src_off + src_stride]);
            }
        }
        let out = std::path::PathBuf::from("..").join("sidebar_preview.png");
        std::fs::write(&out, big.encode_png().unwrap()).unwrap();
        println!("侧边栏预览：{} ({total_w}x{bh})", out.display());
    }
}
