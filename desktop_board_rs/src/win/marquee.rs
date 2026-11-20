//! 通知跑马灯条：独立无边框小窗，白底黑字大字、圆角、四周透明。
//!
//! 与 Python 版同构，但不依赖 Tk 的 transparentcolor（那种做法边缘有锯齿）：
//! 用 32 位 DIB + UpdateLayeredWindow 做逐像素 alpha，圆角与文字都是抗锯齿的。
//! 条子可拖动、滚轮缩放，右端两个 × 分别关闭「通知条」和「今日作业栏」；
//! 关掉通知后条子并不消失，而是收成右侧一个小胶囊，两个按钮始终点得到。

use std::ffi::c_void;
use tiny_skia::Pixmap;

use crate::config::load_board_notice;
use crate::paths;
use crate::render::{line, rrect};
use crate::text::{Anchor, Fonts};
use crate::theme::{rgb, LINE, WHITE};

use super::dib::Dib;
use super::sys::*;

const TIMER_SCROLL: usize = 1;
const TIMER_NOTICE: usize = 2;
const TIMER_STOP: usize = 3;

/// 左边缘 / 下边缘的可拖拽厚度（像素）
const EDGE_W: f32 = 16.0;
const EDGE_H: f32 = 12.0;

/// 横幅默认宽度占「图标位右侧可用宽度」的比例：比原来短 1/3。
/// 用户手动拖过之后以存的 marquee_w 为准，不会被这里反复缩小。
const DEFAULT_WIDTH_RATIO: f64 = 2.0 / 3.0;

/// 纯函数，方便单测：屏幕宽 / 边距 / 图标位 → 默认条宽
fn default_width(sw: i32, pad: i32, gutter: i32) -> i32 {
    let base = (sw - 2 * pad - gutter).max(360);
    ((base as f64 * DEFAULT_WIDTH_RATIO).round() as i32).max(360)
}

/// 上课铃响之前多少秒必须收回横幅，避免跳到上课后
const RETRACT_BEFORE_CLASS_SECS: i32 = 30;
/// 手动关闭横幅后的静默时长（秒）：40 分钟内不自动显示，
/// 到期后按时刻表规则（课间/放学后）自动恢复显示。
const MANUAL_SILENCE_SECS: u64 = 2400;
/// 触屏点击/拖动判定位移阈值（px）：希沃屏手指点按常带几像素抖动，
/// 阈值太小会把「点击关闭」误判成「拖动」，表现为点了没反应、✕ 点不中。
const CLICK_MAX_DRIFT: i32 = 16;

/// 自动显示的收回倒计时（秒）：距下节课 g 秒 → g-30 秒后收回；
/// 无下节课（已放学/晚自习后）→ None = 常显，直到时刻表条件变化。
fn auto_retract_secs(gap: Option<i32>) -> Option<u64> {
    gap.map(|g| (g - RETRACT_BEFORE_CLASS_SECS).max(1) as u64)
}

/// 切班强制弹的收回倒计时：临近上课保底 30 秒可见，其余同自动规则。
fn forced_retract_secs(gap: Option<i32>) -> Option<u64> {
    match gap {
        None => None,
        Some(g) if g > RETRACT_BEFORE_CLASS_SECS => auto_retract_secs(Some(g)),
        Some(_) => Some(30),
    }
}

pub struct Marquee {
    pub hwnd: HWND,
    dib: Option<Dib>,
    /// 当前窗口宽（收起时是小胶囊宽）
    w: i32,
    h: i32,
    /// 展开态宽度（用户滚轮缩放的就是它，落盘也用它）
    full_w: i32,
    px: f32,
    inset: f32,
    step: f32,
    tx: f32,
    notice: String,
    fonts: Fonts,
    /// 右侧两个 × 按钮的宽度（「通知」「作业」）
    btn_w: f32,
    /// 拖动状态：(按下时鼠标x, y, 窗口x, y, 按下时宽, 按下时高, 是否已拖动)
    drag_start: Option<(i32, i32, i32, i32, i32, i32, bool)>,
    /// 拖动模式：0=整条移动，1=左边缘改宽，2=下边缘改高
    drag_mode: u8,
    /// 位置/尺寸改动后的落盘时刻（防抖）
    save_due: Option<std::time::Instant>,
    /// 用户点了条子（或 ✕）隐藏通知条（手动关闭，40 分钟静默后自动恢复）
    hidden: bool,
    /// 手动关闭的时刻：静默 MANUAL_SILENCE_SECS 到期后按时刻表恢复显示
    hidden_at: Option<std::time::Instant>,
    /// 时刻表驱动的自动显示中（课间 / 放学后），到上课前 30 秒收回
    auto_active: bool,
    /// 自动显示的收回时刻（None = 常显，直到时刻表条件变化才收）
    popup_deadline: Option<std::time::Instant>,
    /// 上一拍是否处于上课时段：检测「上课→课间」的下课铃转换，
    /// 转换瞬间强制解除手动静默，保证每个课间都照常弹出
    was_in_class: Option<bool>,
    /// 双指捏合起始两指距离（像素）；None = 当前不在捏合
    pinch_dist: Option<f32>,
    /// 捏合开始时的宽/高（以此倍率，避免逐帧累加漂移）
    pinch_w: i32,
    pinch_h: i32,
}

impl Marquee {
    /// 按屏幕矩形与左侧图标位算出条子的位置和字号，创建窗口后把状态交给窗口。
    fn create(
        sx: i32,
        sy: i32,
        sw: i32,
        sh: i32,
        gutter: i32,
        board: crate::board::Board,
    ) -> Option<HWND> {
        let pad = ((sw as f32 * 0.02) as i32).max(14);
        let mut w = default_width(sw, pad, gutter);
        let mut h = ((sh as f32 * 0.085) as i32).max(60);
        let mut x = sx + pad + gutter;
        // 壁纸顶部的日期/时钟白条已移除，跑马灯上移占据屏幕顶部，
        // 屏幕上方只保留这一条白条。
        let mut y = sy + ((sh as f32 * 0.012) as i32).max(8);
        // 用户手动拖动/缩放过的位置优先
        if board.marquee_w > 0 {
            w = board.marquee_w;
        }
        if board.marquee_h > 0 {
            h = board.marquee_h;
        }
        if board.marquee_x >= 0 {
            x = board.marquee_x;
        }
        if board.marquee_y >= 0 {
            y = board.marquee_y;
        }
        // 存下来的位置可能越界（比如换过分辨率/拖过头），这里夹回屏幕内，
        // 否则右端的 ✕ 会落在可视区外，点不到也看不到。
        if w > sw {
            w = sw;
        }
        x = x.clamp(0, (sw - w).max(0));
        y = y.clamp(0, (sh - h).max(0));
        // 字号跟条子高度走，缩放后文字才不会挤出
        let px = (h as f32 * 0.50).max(18.0);
        let btn_w = (h as f32 * 1.25).max(56.0);

        let inst = unsafe { GetModuleHandleW(std::ptr::null()) };
        let class = wide("BoardMarqueeRust");
        unsafe {
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: inst,
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);

            // 去掉 WS_EX_TRANSPARENT：条子要能接收鼠标，才能拖动/缩放/点 ×
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                wide("大屏通知").as_ptr(),
                WS_POPUP,
                x,
                y,
                w,
                h,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                inst,
                std::ptr::null_mut(),
            );
            if hwnd.is_null() {
                println!("创建跑马灯窗口失败");
                return None;
            }
            // 手动关闭的 40 分钟静默：重启后依然有效；无时间戳的旧记录
            // 按已过期处理（升级后第一次启动自动恢复按时刻表显示）
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let silence_ms = MANUAL_SILENCE_SECS * 1000;
            let elapsed = now_ms.saturating_sub(board.marquee_hidden_ms);
            let (hidden, hidden_at) = if board.marquee_hidden
                && board.marquee_hidden_ms > 0
                && elapsed < silence_ms
            {
                (
                    true,
                    Some(std::time::Instant::now() - std::time::Duration::from_millis(elapsed)),
                )
            } else {
                (false, None)
            };
            let me = Box::new(Self {
                hwnd,
                dib: Dib::new(w, h),
                w,
                h,
                full_w: w,
                px,
                inset: ((w as f32 * 0.018) as i32).max(14) as f32,
                step: (px / 6.0).max(4.0),
                tx: w as f32,
                notice: load_board_notice(),
                fonts: Fonts::load(),
                btn_w,
                drag_start: None,
                drag_mode: 0,
                save_due: None,
                hidden,
                hidden_at,
                auto_active: false,
                popup_deadline: None,
                was_in_class: None,
                pinch_dist: None,
                pinch_w: w,
                pinch_h: h,
            });
            let raw = Box::into_raw(me);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
            // 注册触摸：希沃大屏双指捏合缩放走 WM_TOUCH 原始触点
            // （横幅是 NOACTIVATE 层窗口，手势识别 WM_GESTURE 依赖激活状态，不可靠）。
            RegisterTouchWindow(hwnd, TWF_NONE);
            let st = &mut *raw;
            st.tx = st.w as f32 - st.inset;
            st.refresh_layout();
            SetTimer(hwnd, TIMER_SCROLL, 60, std::ptr::null_mut());
            SetTimer(hwnd, TIMER_NOTICE, 2000, std::ptr::null_mut());
            SetTimer(hwnd, TIMER_STOP, 250, std::ptr::null_mut());
            Some(hwnd)
        }
    }
}

// 状态挂在窗口的 GWLP_USERDATA 上：创建时 Box::into_raw，销毁时 Box::from_raw 回收。
impl Marquee {
    fn state(hwnd: HWND) -> Option<&'static mut Marquee> {
        let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Marquee;
        if p.is_null() {
            None
        } else {
            Some(unsafe { &mut *p })
        }
    }

    /// 文字可用右边界（扣掉右端的 ✕ 关闭区）
    fn text_right(&self) -> f32 {
        (self.w as f32 - self.inset - self.btn_w).max(60.0)
    }

    /// 展开态左边界（窗口右端固定，由右端反推）
    fn full_left(&self) -> i32 {
        let mut r = RECT::default();
        unsafe { GetWindowRect(self.hwnd, &mut r) };
        r.right - self.full_w
    }

    /// 进入自动显示：课间 / 放学后常显，上课前 30 秒收回。
    /// `forced=true`（切班等用户主动操作）同时解除手动静默。
    fn begin_auto_show(&mut self, forced: bool, why: &str) {
        if forced {
            self.hidden = false;
            self.hidden_at = None;
        }
        let work_mode = crate::config::load_app_config().work_mode;
        let gap = crate::schedule::secs_until_next_class(&work_mode);
        let retract = if forced {
            forced_retract_secs(gap)
        } else {
            auto_retract_secs(gap)
        };
        self.auto_active = true;
        self.popup_deadline = retract
            .map(|d| std::time::Instant::now() + std::time::Duration::from_secs(d));
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        let mode = match retract {
            Some(_) => "常显至下节课前 30 秒",
            None => "常显（无下节课）",
        };
        if forced {
            self.persist();
        }
        paths::trace(&format!("跑马灯：{}，{}", why, mode));
    }

    /// 退出自动显示（上课铃临近 / 已上课 / 周末 / 收回时刻到）。
    fn end_auto_show(&mut self, why: &str) {
        self.auto_active = false;
        self.popup_deadline = None;
        paths::trace(&format!("跑马灯：{}", why));
    }

    /// 按当前宽度重算并决定显隐：关掉或没有通知时整条隐藏。
    fn apply_layout(&mut self) {
        if self.w != self.full_w {
            let mut r = RECT::default();
            unsafe { GetWindowRect(self.hwnd, &mut r) };
            self.w = self.full_w;
            self.inset = ((self.w as f32 * 0.018) as i32).max(14) as f32;
            self.dib = Dib::new(self.w, self.h);
            unsafe { MoveWindow(self.hwnd, r.right - self.w, r.top, self.w, self.h, 1) };
        }
        let show = self.auto_active && !self.notice.trim().is_empty();
        unsafe { ShowWindow(self.hwnd, if show { SW_SHOW } else { SW_HIDE }) };
        if show {
            self.tx = self.text_right();
            self.paint();
        }
    }

    /// 兼容旧调用名。
    fn refresh_layout(&mut self) {
        self.apply_layout();
    }

    /// 落盘位置/尺寸/开关：只 patch 这几个键，绝不整份重写（避免默认值回填覆盖用户内容）。
    fn persist(&mut self) {
        use serde_json::json;
        let (x, y) = (self.full_left(), self.cur_y());
        let hidden_ms = if self.hidden_at.is_some() {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        } else {
            0
        };
        crate::board::patch_keys(&[
            ("marquee_x", json!(x)),
            ("marquee_y", json!(y)),
            ("marquee_w", json!(self.full_w)),
            ("marquee_h", json!(self.h)),
            ("marquee_hidden", json!(self.hidden)),
            ("marquee_hidden_ms", json!(hidden_ms)),
        ]);
    }

    /// 双指捏合缩放：dist = 当前两指屏幕距离（像素）；None = 触点不足，本次捏合结束。
    /// 宽高同步等比缩放，字号/按钮宽/步长/内边距跟着重算；落盘交给 save_due 防抖。
    fn on_pinch(&mut self, dist: Option<f32>) {
        let Some(dist) = dist else {
            if self.pinch_dist.take().is_some() {
                self.save_due = Some(std::time::Instant::now());
            }
            return;
        };
        if self.pinch_dist.is_none() {
            self.pinch_dist = Some(dist);
            self.pinch_w = self.full_w;
            self.pinch_h = self.h;
            return;
        }
        let base = self.pinch_dist.unwrap();
        let ratio = super::touch::pinch_ratio(base, dist);
        let nw = ((self.pinch_w as f32 * ratio) as i32).clamp(400, 3200);
        let nh = ((self.pinch_h as f32 * ratio) as i32).clamp(44, 260);
        if nw != self.full_w || nh != self.h {
            self.full_w = nw;
            self.h = nh;
            self.px = (nh as f32 * 0.50).max(18.0);
            self.btn_w = (nh as f32 * 1.25).max(56.0);
            self.step = (self.px / 6.0).max(4.0);
            self.inset = ((nw as f32 * 0.018) as i32).max(14) as f32;
            self.w = -1; // 强制 apply_layout 重算宽度与 DIB
            self.apply_layout();
        }
    }

    fn cur_y(&self) -> i32 {
        let mut r = RECT::default();
        unsafe { GetWindowRect(self.hwnd, &mut r) };
        r.top
    }

    fn paint(&mut self) {
        let (w, h) = (self.w, self.h);
        let Some(mut pm) = Pixmap::new(w as u32, h as u32) else {
            return;
        };
        // 圆角白条（底色透明，只有条子本身可见）
        rrect(
            &mut pm,
            0.5,
            0.5,
            w as f32 - 0.5,
            h as f32 - 0.5,
            h as f32 * 0.30,
            rgb(WHITE),
            Some((rgb(LINE), 1.0)),
        );
        let text_right = self.text_right();
        if !self.notice.trim().is_empty() {
            let tw = self.fonts.measure(&self.notice, self.px, true);
            let cy = h as f32 / 2.0;
            if tw <= text_right - self.inset {
                self.fonts.draw(
                    &mut pm,
                    (self.inset + text_right) / 2.0,
                    cy,
                    &self.notice,
                    self.px,
                    true,
                    rgb("#111111"),
                    Anchor::Center,
                    None,
                    false,
                );
            } else {
                self.fonts.draw(
                    &mut pm,
                    self.tx,
                    cy,
                    &self.notice,
                    self.px,
                    true,
                    rgb("#111111"),
                    Anchor::Left,
                    None,
                    false,
                );
            }
        }
        // 右端一个红色 ✕：点它（或点条子任意位置）隐藏通知条。
        // 用线条画而不是字形——字体里没有 U+2715，直接画字会空白。
        let bh = h as f32 * 0.62;
        let cx = w as f32 - self.inset * 0.5 - bh * 0.5;
        let cy = h as f32 / 2.0;
        let r = bh * 0.5;
        rrect(
            &mut pm,
            cx - r,
            cy - r,
            cx + r,
            cy + r,
            r,
            rgb("#FDECEA"),
            None,
        );
        let s = r * 0.42;
        line(&mut pm, cx - s, cy - s, cx + s, cy + s, rgb("#E53935"), 3.5);
        line(&mut pm, cx + s, cy - s, cx - s, cy + s, rgb("#E53935"), 3.5);
        let Some(dib) = self.dib.as_mut() else {
            return;
        };
        dib.blit(&pm);
        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: 0,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA,
        };
        unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
                dib.hdc,
                &src,
                0,
                &blend,
                ULW_ALPHA,
            );
        }
    }

    fn tick(&mut self) {
        if self.notice.trim().is_empty() {
            return;
        }
        let tw = self.fonts.measure(&self.notice, self.px, true);
        if tw <= self.text_right() - self.inset {
            return; // 放得下：居中不滚
        }
        self.tx -= self.step;
        if self.tx < -tw - 12.0 {
            self.tx = self.text_right();
        }
        self.paint();
    }

    fn poll_notice(&mut self) {
        let mut dirty = false;
        let fresh = load_board_notice();
        if fresh != self.notice {
            self.notice = fresh;
            dirty = true;
        }
        // 主程序菜单的「恢复通知」会写 marquee_hidden=false，这里同步解除静默
        let b = crate::board::load("");
        if !b.marquee_hidden && self.hidden {
            self.hidden = false;
            self.hidden_at = None;
            self.persist();
            dirty = true;
        }
        // 守护拉起标记：主程序关机期间来过新通知。现在课间由时刻表状态机
        // 自动把最新内容显示出来，这里只需消费掉标记文件，避免残留。
        let flag = crate::paths::break_popup_flag();
        if flag.exists() {
            let _ = std::fs::remove_file(&flag);
        }
        // 创建 / 切换班级：用户主动操作，无视静默立即弹出。标记带毫秒时间戳，
        // 过期丢弃，避免大屏当时没开、隔天启动时突然补弹旧通知。
        let pf = paths::notice_popup_flag();
        if pf.exists() {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let fresh = std::fs::read_to_string(&pf)
                .ok()
                .map(|s| paths::notice_popup_fresh(&s, now_ms))
                .unwrap_or(false);
            let _ = std::fs::remove_file(&pf);
            if fresh && !self.notice.trim().is_empty() {
                self.begin_auto_show(true, "创建/切换班级，立即弹出新班通知");
                dirty = true;
            }
        }
        // ---- 时刻表驱动的显隐状态机（TIMER_NOTICE 每 2 秒走到这里）----
        // 规则：上学日 + 非白天上课时段（课间/午休/17:15 后/晚自习）→ 常显不自动收回；
        // 上课前 30 秒收回；上课期间与周末不显示；手动关闭 40 分钟静默后按本规则恢复。
        // 1) 手动静默到期 → 解除，交给下面的状态机自然恢复
        if self.hidden {
            if let Some(t) = self.hidden_at {
                if t.elapsed().as_secs() >= MANUAL_SILENCE_SECS {
                    self.hidden = false;
                    self.hidden_at = None;
                    self.persist();
                    dirty = true;
                    paths::trace("跑马灯：手动关闭静默 40 分钟到期，恢复时刻表显示");
                }
            }
        }
        // 2) 判断当前是否该显示
        let work_mode = crate::config::load_app_config().work_mode;
        let in_class = crate::schedule::in_class_now(&work_mode);
        // 下课铃响（上课→非上课转换）：强制解除手动静默——每个课间都照常弹出，
        // 防止误触关闭后通知长时间消失；40 分钟静默只兜底放学后等无转换时段。
        if !in_class && self.was_in_class == Some(true) && self.hidden {
            self.hidden = false;
            self.hidden_at = None;
            self.persist();
            dirty = true;
            paths::trace("跑马灯：下课铃响，强制解除手动静默");
        }
        self.was_in_class = Some(in_class);
        let gap = crate::schedule::secs_until_next_class(&work_mode);
        let near_class = gap.map(|g| g <= RETRACT_BEFORE_CLASS_SECS).unwrap_or(false);
        let want_show = crate::schedule::is_school_day_now()
            && !in_class
            && !near_class
            && !self.notice.trim().is_empty();
        // 3) 该收就收：上课铃临近 / 已上课 / 周末
        if self.auto_active && !want_show {
            let why = if in_class {
                "上课了，通知条自动收回"
            } else if near_class {
                "距上课不足 30 秒，通知条自动收回"
            } else {
                "周末，通知条自动收回"
            };
            self.end_auto_show(why);
            dirty = true;
        }
        // 4) 该弹就弹：下课铃响转入课间 / 午休 / 17:15 放学后，每节课都会走到
        if want_show && !self.auto_active && !self.hidden {
            let why = if gap.is_some() {
                "下课转入课间，通知条自动弹出"
            } else {
                "放学后通知条自动弹出"
            };
            self.begin_auto_show(false, why);
            dirty = true;
        }
        // 5) 收回时刻到（兜底：正常由 want_show=false 触发）
        if self.auto_active {
            if let Some(t) = self.popup_deadline {
                if std::time::Instant::now() >= t {
                    self.end_auto_show("通知条收回时刻到");
                    dirty = true;
                }
            }
        }
        if dirty {
            self.refresh_layout();
        }
    }

    fn poll_stop(&mut self) -> bool {
        let stop = paths::stop_file();
        if stop.exists() {
            // 不删 stop 文件：壁纸线程也要靠它退出（由 serve 统一清理）
            STOP_SEEN.store(true, std::sync::atomic::Ordering::Relaxed);
            paths::trace("跑马灯收到退出信号");
            unsafe {
                DestroyWindow(self.hwnd);
            }
            return true;
        }
        false
    }
}

/// 跑马灯已收到退出信号（供 serve 通知壁纸线程）。
pub static STOP_SEEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        // 光标：强制显示系统箭头。
        // 只靠类光标不够——层窗口（WS_EX_LAYERED + NOACTIVATE）在鼠标进入时不一定会
        // 触发 DefWindowProc 的类光标复位，结果是上一个窗口留下的"忙碌圆环"粘在横幅上。
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16));
            return 1;
        }
        WM_TIMER => {
            if let Some(st) = Marquee::state(hwnd) {
                match wp {
                    TIMER_SCROLL => st.tick(),
                    TIMER_NOTICE => {
                        st.poll_notice();
                        // 滚轮缩放的防抖落盘
                        if let Some(t) = st.save_due {
                            if t.elapsed().as_millis() >= 600 {
                                st.save_due = None;
                                st.persist();
                            }
                        }
                    }
                    TIMER_STOP => {
                        if st.poll_stop() {
                            return 0;
                        }
                    }
                    _ => {}
                }
            }
            0
        }
        // 双指捏合缩放（单指仍走下面的鼠标拖动/点击）
        WM_TOUCH => {
            if let Some(st) = Marquee::state(hwnd) {
                let dist = super::touch::read_touch_distance(wp, lp);
                if dist.is_some() {
                    // 触摸期间取消鼠标拖动，避免两套手势互相打架
                    st.drag_start = None;
                    st.drag_mode = 0;
                    unsafe { ReleaseCapture() };
                }
                st.on_pinch(dist);
            }
            0
        }
        // ---- 鼠标：整条拖动、拖左/下边缘缩放、点一下隐藏 ----
        WM_LBUTTONDOWN => {
            let x = (lp & 0xFFFF) as u16 as i32;
            let y = ((lp >> 16) & 0xFFFF) as u16 as i16 as i32;
            if let Some(st) = Marquee::state(hwnd) {
                let mut r = RECT::default();
                let mut pt = POINT { x: 0, y: 0 };
                unsafe {
                    GetWindowRect(hwnd, &mut r);
                    GetCursorPos(&mut pt);
                    SetCapture(hwnd);
                }
                // 左边缘改宽度（右端固定），下边缘改高度（上端固定），其余整条拖动。
                // 按下后原地松开 = 点一下，在 LBUTTONUP 里隐藏通知条。
                st.drag_mode = if (x as f32) <= EDGE_W {
                    1
                } else if y as f32 >= st.h as f32 - EDGE_H {
                    2
                } else {
                    0
                };
                st.drag_start = Some((pt.x, pt.y, r.left, r.top, st.full_w, st.h, false));
            }
            0
        }
        WM_MOUSEMOVE => {
            if let Some(st) = Marquee::state(hwnd) {
                if let Some((sx, sy, wx, wy, w0, h0, was)) = st.drag_start {
                    let mut pt = POINT { x: 0, y: 0 };
                    unsafe { GetCursorPos(&mut pt) };
                    let (dx, dy) = (pt.x - sx, pt.y - sy);
                    let active = was || dx.abs() > CLICK_MAX_DRIFT || dy.abs() > CLICK_MAX_DRIFT;
                    if active {
                        st.drag_start = Some((sx, sy, wx, wy, w0, h0, true));
                        match st.drag_mode {
                            0 => {
                                // 整条留在屏幕内：右端的 ✕ 不能被挤出可视区
                                let (sw, sh) = unsafe {
                                    (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
                                };
                                let max_x = (sw - st.w).max(0);
                                let nx = if st.w <= sw {
                                    (wx + dx).clamp(0, max_x)
                                } else {
                                    (wx + dx).clamp(max_x, 0)
                                };
                                let ny = (wy + dy).clamp(0, (sh - st.h).max(0));
                                unsafe { MoveWindow(hwnd, nx, ny, st.w, st.h, 1) };
                            }
                            mode => {
                                // 一律以「按下时」的宽高为基准，避免逐帧累加导致越拖越小
                                let (_, sh) = unsafe {
                                    (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
                                };
                                let nw = if mode == 1 {
                                    // 右端固定，宽度不能顶出屏幕（否则右端的 ✕ 会跑到屏幕外）
                                    (w0 - dx).clamp(360, (wx + w0).clamp(360, 3200))
                                } else {
                                    st.full_w
                                };
                                let nh = if mode == 2 {
                                    (h0 + dy).clamp(44, (sh - wy).clamp(44, 260))
                                } else {
                                    st.h
                                };
                                if nw != st.full_w || nh != st.h {
                                    st.full_w = nw;
                                    st.h = nh;
                                    st.px = (nh as f32 * 0.50).max(18.0);
                                    st.btn_w = (nh as f32 * 1.25).max(56.0);
                                    st.step = (st.px / 6.0).max(4.0);
                                    st.inset = ((nw as f32 * 0.018) as i32).max(14) as f32;
                                    st.w = nw;
                                    st.dib = Dib::new(nw, nh);
                                    // 右端固定：宽度变化时左边界反推
                                    let nx = wx + (w0 - nw);
                                    unsafe { MoveWindow(hwnd, nx, wy, nw, nh, 1) };
                                    st.refresh_layout();
                                }
                            }
                        }
                    }
                }
            }
            0
        }
        WM_LBUTTONUP => {
            if let Some(st) = Marquee::state(hwnd) {
                unsafe { ReleaseCapture() };
                let dragged = st.drag_start.map(|d| d.6).unwrap_or(false);
                st.drag_start = None;
                st.drag_mode = 0;
                if dragged {
                    st.persist();
                } else {
                    // 原地按下松开 = 点一下条子 → 手动关闭（40 分钟静默，
                    // 到期后按时刻表课间/放学后自动恢复；菜单「恢复通知」可立即打开）
                    st.hidden = true;
                    st.hidden_at = Some(std::time::Instant::now());
                    st.auto_active = false;
                    st.popup_deadline = None;
                    st.persist();
                    st.apply_layout();
                    paths::trace("跑马灯：点击手动关闭通知条（40 分钟静默，下课铃强制解除）");
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            // 层窗口是 NOACTIVATE，通常收不到滚轮；这里作为兜底保留
            let delta = ((wp >> 16) & 0xFFFF) as u16 as i16 as i32;
            if delta != 0 {
                if let Some(st) = Marquee::state(hwnd) {
                    let k = 1.0 + (delta as f32 / 120.0) * 0.06;
                    let nw = ((st.full_w as f32 * k) as i32).clamp(400, 3200);
                    let nh = ((st.h as f32 * k) as i32).clamp(44, 260);
                    if nw != st.full_w || nh != st.h {
                        st.full_w = nw;
                        st.h = nh;
                        st.px = (nh as f32 * 0.50).max(18.0);
                        st.btn_w = (nh as f32 * 1.25).max(56.0);
                        st.step = (st.px / 6.0).max(4.0);
                        st.w = -1; // 强制 apply_layout 重算宽度与 DIB
                        st.apply_layout();
                        st.save_due = Some(std::time::Instant::now());
                    }
                }
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                KillTimer(hwnd, TIMER_SCROLL);
                KillTimer(hwnd, TIMER_NOTICE);
                KillTimer(hwnd, TIMER_STOP);
                let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Marquee;
                if !p.is_null() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(p));
                }
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// 跑马灯专用消息循环（主线程），直到收到退出信号。
pub fn run(sx: i32, sy: i32, sw: i32, sh: i32, gutter: i32, board: crate::board::Board) -> bool {
    if Marquee::create(sx, sy, sw, sh, gutter, board).is_none() {
        return false;
    }
    // 作业卡片热区同线程创建，共用下面这个消息泵
    super::homework::create();
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, std::ptr::null_mut() as *mut c_void, 0, 0) };
        if r <= 0 {
            break;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{auto_retract_secs, default_width, forced_retract_secs, MANUAL_SILENCE_SECS};

    #[test]
    fn banner_default_is_one_third_shorter() {
        // 1920 屏：pad=38，图标位=211，可用宽 1633 → 2/3 ≈ 1089
        let full = (1920 - 2 * 38 - 211).max(360);
        let short = default_width(1920, 38, 211);
        assert_eq!(short, 1089);
        assert!((short as f64 / full as f64 - 2.0 / 3.0).abs() < 0.002);
        // 小屏也不会被压到点不到 ✕
        assert!(default_width(800, 16, 150) >= 360);
    }

    #[test]
    fn auto_show_no_time_limit_but_retracts_before_class() {
        // 已放学 / 晚自习后：常显（None = 无自动收回时刻）
        assert_eq!(auto_retract_secs(None), None);
        // 刚下课（离下节课 10 分钟）：常显 570 秒后收回 = 上课前 30 秒
        assert_eq!(auto_retract_secs(Some(600)), Some(570));
        // 离上课 100 秒：70 秒后收回（上课前 30 秒）
        assert_eq!(auto_retract_secs(Some(100)), Some(70));
        // 极端值不出现 0/负数
        assert_eq!(auto_retract_secs(Some(31)), Some(1));
    }

    #[test]
    fn forced_show_near_class_gets_30s_floor() {
        // 放学后切班：常显
        assert_eq!(forced_retract_secs(None), None);
        // 课间充足：同自动规则，上课前 30 秒收回
        assert_eq!(forced_retract_secs(Some(600)), Some(570));
        assert_eq!(forced_retract_secs(Some(100)), Some(70));
        // 临近上课（≤30 秒）也保底可见 30 秒，不会刚弹就收
        assert_eq!(forced_retract_secs(Some(30)), Some(30));
        assert_eq!(forced_retract_secs(Some(10)), Some(30));
    }

    #[test]
    fn manual_silence_is_40_minutes() {
        assert_eq!(MANUAL_SILENCE_SECS, 2400);
    }
}
