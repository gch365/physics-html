//! 今日作业卡片：一层独立的透明悬浮窗口，直接浮在用户任意壁纸上。
//!
//! - 卡片本体、通知条、学科作业、值日全部由这层窗口用 32 位 DIB 逐像素 alpha 画出来，
//!   **不再渲染/替换系统壁纸**（旧版是把整屏合成图设成壁纸，换壁纸就得重画全屏）。
//! - 卡片任意位置拖动 → 改位置；右下角拖动 → 等比缩放（0.7~1.4）；位置/缩放落盘。
//! - 右下角红色 ✕ → 本次隐藏作业栏（弹确认码框，下次启动自动恢复）；✕ 左边铅笔 → 打开三页设置面板；
//!   再左边的电源按钮 → **彻底关闭**：先弹确认框，输入 `123` 并点「确定」才生效
//!   （写入持久标志，重启程序 / 重启大屏都不再打开，只能在主程序设置窗口里重新开启）；
//!   点卡片其它任意位置 → 同样直接打开设置面板（作业 / 班干 / 卫生三页同窗切换）。
//! - 只在桌面处于前台时显示：其它程序在前台时整层收起，避免盖住 PPT / 文档、吞掉点击。
//!
//! 卡片几何与 `render::homework_rect` 共用，分辨率/布局变化后跟着重算。

use tiny_skia::Pixmap;

use crate::board::Board;
use crate::render::{self, homework_rect, icon_pencil, line, rrect};
use crate::theme::rgb;

use super::dib::Dib;
use super::sys::*;

const TIMER_POLL: usize = 1;
/// 通知条滚动帧（120ms ≈ 8fps；内容放得下时 advance_scroll 直接返回，不重绘）
const TIMER_SCROLL: usize = 2;
/// 滚到底 / 回到顶部后的停顿帧数
const HOLD_BOTTOM: u32 = 16;
const HOLD_TOP: u32 = 12;
/// 红色 ✕ 圆的直径
const BADGE: f32 = 40.0;
/// ✕ 圆心到卡片右下角的留白（让圆完整落在卡片内，不贴边）
const BADGE_INSET: f32 = 10.0;
/// 铅笔编辑按钮直径（与 ✕ 同高，排在同一条基线上）
const EDIT_BTN: f32 = 40.0;
/// 编辑按钮与 ✕ 的横向间距
const EDIT_GAP: f32 = 12.0;
/// 「彻底关闭」电源按钮直径，与 ✕ / 铅笔同一条基线
const HARD_BTN: f32 = 40.0;
/// 电源按钮与铅笔按钮的横向间距
const HARD_GAP: f32 = 12.0;
/// 「彻底关闭」确认码（设置窗口里的同名开关也用这个值，两处必须保持一致）。
const HARD_CLOSE_CODE: &str = "123";
/// 右下角可以拖着缩放的方形边长（缩放把手在**左下角**，避开 ✕ / 铅笔）
const SCALE_HIT: f32 = 56.0;
/// 拖动判定阈值（像素），低于它算「点一下」而不是拖动
const DRAG_THRESHOLD: i32 = 3;

/// 纯函数：推进一帧滚动 → (新偏移, 新停顿, 是否待回卷, 是否需要重绘)。
fn next_scroll(
    offset: f32,
    hold: u32,
    wrap: bool,
    travel: f32,
    line_h: f32,
) -> (f32, u32, bool, bool) {
    if wrap {
        return (0.0, HOLD_TOP, false, true);
    }
    if hold > 0 {
        return (offset, hold - 1, false, false);
    }
    let step = (line_h / 18.0).max(1.0);
    let next = (offset + step).min(travel);
    if next >= travel {
        (next, HOLD_BOTTOM, true, true)
    } else {
        (next, 0, false, true)
    }
}

/// 按下鼠标时记下的窗口状态
struct Drag {
    sx: i32,
    sy: i32,
    wx: i32,
    wy: i32,
    h0: i32,
    scale0: f32,
    base_w: f32,
    base_h: f32,
    moved: bool,
    on_badge: bool,
    on_edit: bool,
    on_hard: bool,
    scale_mode: bool,
}

pub struct HomHot {
    pub hwnd: HWND,
    dib: Option<Dib>,
    w: i32,
    h: i32,
    x: i32,
    y: i32,
    hidden: bool,
    /// 当前缩放（相对默认布局尺寸）
    scale: f32,
    /// 缓存的「默认尺寸」卡片底图（不含通知正文），缩放时只重采样，不重画整屏
    base: Option<Pixmap>,
    /// 完整通知文字层；travel > 0（内容超长）时才 Some
    layer: Option<render::NoticeLayer>,
    /// 复用的一帧底图，避免每帧重新分配上兆像素缓冲
    frame: Option<Pixmap>,
    /// 当前滚动偏移（默认尺寸坐标）
    scroll_offset: f32,
    /// 滚到底 / 回顶部后的停顿计数
    scroll_hold: u32,
    /// 到底停完后准备回卷到顶部
    scroll_wrap: bool,
    /// 内容签名：日期 / 配置 mtime / 通知 mtime，变了才重新渲染
    sig: Option<(String, f64, f64, Option<i64>)>,
    drag: Option<Drag>,
    /// 双指捏合起始两指距离（像素）；None = 当前不在捏合
    pinch_dist: Option<f32>,
    /// 捏合开始时的缩放 / 默认尺寸（以此的基准做倍率，避免逐帧累加漂移）
    pinch_scale: f32,
    pinch_base_w: f32,
    pinch_base_h: f32,
}

/// ✕ 的矩形：卡片自身右下角内侧（不再伸到卡片外面）。
fn badge_rect_at(w: f32, h: f32) -> (f32, f32, f32, f32) {
    let x1 = w - BADGE_INSET;
    let y1 = h - BADGE_INSET;
    (x1 - BADGE, y1 - BADGE, x1, y1)
}

/// 铅笔编辑按钮：贴在 ✕ 左边，底边与 ✕ 对齐。
fn edit_rect_at(w: f32, h: f32) -> (f32, f32, f32, f32) {
    let (bx0, _by0, _bx1, by1) = badge_rect_at(w, h);
    let x1 = bx0 - EDIT_GAP;
    (x1 - EDIT_BTN, by1 - EDIT_BTN, x1, by1)
}

/// 「彻底关闭」电源按钮：贴在铅笔左边，底边与 ✕ / 铅笔对齐。
fn hard_rect_at(w: f32, h: f32) -> (f32, f32, f32, f32) {
    let (ex0, _ey0, _ex1, ey1) = edit_rect_at(w, h);
    let x1 = ex0 - HARD_GAP;
    (x1 - HARD_BTN, ey1 - HARD_BTN, x1, ey1)
}

/// 左下角的缩放把手：三条浅灰斜线，提示这里可以拖着改大小。
fn draw_grip(pm: &mut Pixmap, h: f32) {
    let c = rgb("#9AA9BE");
    for i in 0..3 {
        let o = 8.0 + i as f32 * 6.0;
        line(pm, 8.0, h - o, 8.0 + o, h - 8.0, c, 1.6);
    }
}

/// 把卡片右下角的三个按钮画进 `pm`：
/// 电源（彻底关闭 / 持久）· 铅笔（编辑）· 红色 ✕（本次隐藏）。
pub fn draw_buttons(pm: &mut Pixmap, w: f32, h: f32) {
    // 「彻底关闭」：电源符号用蓝灰色，与红色 ✕ 明确区分
    let (hx0, hy0, hx1, hy1) = hard_rect_at(w, h);
    let hc = ((hx0 + hx1) / 2.0, (hy0 + hy1) / 2.0);
    let hr = (hx1 - hx0) / 2.0;
    let power = rgb("#455A64");
    rrect(
        pm, hx0, hy0, hx1, hy1, hr, rgb("#FFFFFF"), Some((power, 2.5)),
    );
    let ir = hr * 0.58;
    rrect(
        pm,
        hc.0 - ir,
        hc.1 - ir,
        hc.0 + ir,
        hc.1 + ir,
        ir,
        rgb("#FFFFFF"),
        Some((power, 2.2)),
    );
    line(pm, hc.0, hc.1 - ir * 1.12, hc.0, hc.1 - ir * 0.08, power, 3.2);

    let (ex0, ey0, ex1, ey1) = edit_rect_at(w, h);
    let er = (ex1 - ex0) / 2.0;
    rrect(
        pm,
        ex0,
        ey0,
        ex1,
        ey1,
        er,
        rgb("#FFFFFF"),
        Some((rgb("#1E6FD9"), 2.5)),
    );
    icon_pencil(
        pm,
        (ex0 + ex1) / 2.0,
        (ey0 + ey1) / 2.0,
        er * 1.44,
        rgb("#1E6FD9"),
    );

    let (bx0, by0, bx1, by1) = badge_rect_at(w, h);
    let cx = (bx0 + bx1) / 2.0;
    let cy = (by0 + by1) / 2.0;
    let r = (bx1 - bx0) / 2.0;
    rrect(
        pm,
        bx0,
        by0,
        bx1,
        by1,
        r,
        rgb("#FFFFFF"),
        Some((rgb("#E53935"), 2.5)),
    );
    let s = r * 0.55;
    line(pm, cx - s, cy - s, cx + s, cy + s, rgb("#E53935"), 3.5);
    line(pm, cx + s, cy - s, cx - s, cy + s, rgb("#E53935"), 3.5);
}

/// 调试出图：画出真正浮在壁纸上的那张卡片（含三个按钮），不需要窗口、不动壁纸。
/// `offset` 用来查看通知条滚到任意一帧的样子，默认 0（首帧）。
pub fn preview(path: &std::path::Path, offset: f32) -> bool {
    let cfg = crate::config::load_app_config();
    let mut b = crate::board::load(&cfg.raw_name);
    // 调试出图只看版式，不受 ✕ 隐藏 / 彻底关闭状态影响
    b.homework_hidden = false;
    b.homework_hard_hidden = false;
    let (_, _, sw, sh) = super::screen_geometry(&b);
    let sched = crate::schedule::load();
    let courses = crate::config::load_courses(&cfg.current_class);
    let now = crate::clock::now();
    let st = crate::schedule::compute_status(&sched, &cfg.work_mode, &courses, now);
    let notice = crate::config::load_board_notice();
    let Some((mut pm, layer)) =
        render::shared().render_card_parts(&b, &st, &notice, now, sw as u32, sh as u32)
    else {
        return false;
    };
    if let Some(l) = layer.as_ref() {
        render::compose_notice(&mut pm, l, offset);
    }
    let (pw, ph) = (pm.width() as f32, pm.height() as f32);
    draw_buttons(&mut pm, pw, ph);
    draw_grip(&mut pm, ph);
    render::save_png(&pm, path)
}

impl HomHot {
    fn state(hwnd: HWND) -> Option<&'static mut HomHot> {
        let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut HomHot;
        if p.is_null() {
            None
        } else {
            Some(unsafe { &mut *p })
        }
    }

    /// 目标矩形 (x, y, w, h) 与缩放。手动拖过就用存下来的位置 / 缩放，否则用默认布局。
    fn target() -> (i32, i32, i32, i32, f32) {
        let b = crate::board::load("");
        let (sx, sy, sw, sh) = super::screen_geometry(&b);
        let (x0, y0, x1, y1) = homework_rect(sw as f32, sh as f32, &b);
        let cw = (x1 - x0).round();
        let ch = (y1 - y0).round();
        let scale = if b.card_scale.is_finite() {
            b.card_scale.clamp(0.70, 1.40)
        } else {
            1.0
        };
        let w = (cw * scale).round().max(160.0) as i32;
        let h = (ch * scale).round().max(120.0) as i32;
        let dx = sx + x0.round() as i32;
        let dy = sy + y0.round() as i32;
        let x = if b.card_x >= 0 { b.card_x } else { dx };
        let y = if b.card_y >= 0 { b.card_y } else { dy };
        // 换过分辨率 / 拖过头时夹回屏幕内，保证右下角按钮点得到
        let x = x.clamp(sx, (sx + sw - w).max(sx));
        let y = y.clamp(sy, (sy + sh - h).max(sy));
        (x, y, w, h, scale)
    }

    fn badge_rect(&self) -> (f32, f32, f32, f32) {
        badge_rect_at(self.w as f32, self.h as f32)
    }

    fn edit_rect(&self) -> (f32, f32, f32, f32) {
        edit_rect_at(self.w as f32, self.h as f32)
    }

    fn hard_rect(&self) -> (f32, f32, f32, f32) {
        hard_rect_at(self.w as f32, self.h as f32)
    }

    /// 重新渲染卡片内容（只在内容签名变化时才调）。
    /// 拆成「静态底图 + 通知文字层」，之后滚动每帧只做一次局部合成。
    fn refresh_card(&mut self, b: &Board) {
        let cfg = crate::config::load_app_config();
        let sched = crate::schedule::load();
        let courses = crate::config::load_courses(&cfg.current_class);
        let (_, _, sw, sh) = super::screen_geometry(b);
        let now = crate::clock::now();
        let st = crate::schedule::compute_status(&sched, &cfg.work_mode, &courses, now);
        let notice = crate::reminder::effective_notice(now, &crate::config::load_board_notice());
        self.base = None;
        self.layer = None;
        if let Some((base, layer)) =
            render::shared().render_card_parts(b, &st, &notice, now, sw as u32, sh as u32)
        {
            self.base = Some(base);
            self.layer = layer;
        }
        // 换了内容就从顶部重新滚，先停一拍让人看清开头
        self.scroll_offset = 0.0;
        self.scroll_hold = HOLD_TOP;
        self.scroll_wrap = false;
        self.frame = None;
    }

    /// 通知超长时推进一帧；内容放得下就什么都不做（定时器照常触发但零重绘）。
    fn advance_scroll(&mut self) {
        // 窗口藏着、或用户正在拖动 / 缩放时别刷帧，省 CPU 也避免拖动过程中跳动
        if self.hidden || self.drag.is_some() {
            return;
        }
        let (travel, line_h) = match self.layer.as_ref() {
            Some(l) if l.travel > 0.0 => (l.travel, l.line_h),
            _ => return,
        };
        let (off, hold, wrap, dirty) = next_scroll(
            self.scroll_offset,
            self.scroll_hold,
            self.scroll_wrap,
            travel,
            line_h,
        );
        self.scroll_offset = off;
        self.scroll_hold = hold;
        self.scroll_wrap = wrap;
        if dirty {
            self.paint();
        }
    }

    fn paint(&mut self) {
        let (w, h) = (self.w, self.h);
        let Some(mut pm) = Pixmap::new(w as u32, h as u32) else {
            return;
        };
        pm.fill(tiny_skia::Color::TRANSPARENT);
        if let Some(base) = self.base.as_ref() {
            let (bw, bh) = (base.width(), base.height());
            let offset = self.scroll_offset;
            let layer = self.layer.as_ref();
            if self.frame.as_ref().map(|f| (f.width(), f.height())) != Some((bw, bh)) {
                self.frame = Pixmap::new(bw, bh);
            }
            let mut ready = false;
            if let Some(frame) = self.frame.as_mut() {
                frame.data_mut().copy_from_slice(base.data());
                if let Some(l) = layer {
                    render::compose_notice(frame, l, offset);
                }
                ready = true;
            }
            if ready {
                let frame = self.frame.as_ref().unwrap();
                if w as u32 == bw && h as u32 == bh {
                    pm.data_mut().copy_from_slice(frame.data());
                } else if let Some(scaled) = render::scale_pixmap(frame, w as u32, h as u32) {
                    // 底是全透明，预乘 alpha 直接覆盖即可（等价于 source-over）
                    pm.data_mut().copy_from_slice(scaled.data());
                }
            }
        }
        draw_buttons(&mut pm, w as f32, h as f32);
        draw_grip(&mut pm, h as f32);

        let Some(dib) = self.dib.as_mut() else {
            return;
        };
        dib.blit(&pm);
        self.present();
    }

    /// 只把现有 DIB 重新贴到当前位置（拖动时位图没变，不必重新渲染/重采样）。
    fn present(&mut self) {
        let Some(dib) = self.dib.as_mut() else {
            return;
        };
        let size = SIZE { cx: self.w, cy: self.h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: 0,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA,
        };
        let dst = POINT { x: self.x, y: self.y };
        unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                std::ptr::null_mut(),
                &dst,
                &size,
                dib.hdc,
                &src,
                0,
                &blend,
                ULW_ALPHA,
            );
        }
    }

    /// 400ms 轮询：内容变化、隐藏开关、桌面是否在前台、布局变化。
    ///
    /// 内容刷新与显示控制解耦：无论卡片是否隐藏、桌面是否在前台，
    /// 新通知 / 新作业一落盘（board_notice.txt / desktop_board.json mtime 变化）
    /// 就立即重渲染——隐藏只是不显示，后台内容始终是最新，
    /// 重新显示的那一刻直接就是新内容。
    fn poll(&mut self) {
        let b = crate::board::load("");
        // ---- 内容后台刷新（不受隐藏 / 前台影响，只在内容签名变化时渲染一次）----
        // 交替相位：每周定期提醒（reminder.rs）与普通通知交替显示时，
        // 相位每 ALT_SECS 秒变一次 → 签名变化 → 触发一次重渲染完成切换；
        // 不在提醒时段恒为 None，不产生任何额外重渲染。
        let now = crate::clock::now();
        let sig = (
            crate::clock::now().format("%Y%m%d").to_string(),
            crate::board::file_mtime(&crate::paths::board_file()),
            crate::board::file_mtime(&crate::paths::notice_file()),
            crate::reminder::alternation_phase(now),
        );
        let dirty = self.base.is_none() || self.sig.as_ref() != Some(&sig);
        if dirty {
            self.refresh_card(&b);
            self.sig = Some(sig);
        }
        // ---- 显示控制 ----
        // 「彻底关闭」优先级最高：即使别处把 homework_hidden 改回 false 也不显示，
        // 只有主程序设置窗口清掉 homework_hard_hidden 后才能恢复。
        // z 序策略：窗口沉底（HWND_BOTTOM），普通程序窗口会盖住它。压底只在
        // 「显示切换 / MoveWindow 后」做，绝不在轮询里每 400ms 反复压——后者会让
        // 分层窗口反复重合成，是桌面作业栏屏闪的根因。
        let visible = !crate::board::homework_effectively_hidden(&b);
        let mut moved = false;
        if !visible {
            if !self.hidden {
                self.hidden = true;
                unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            }
        } else {
            if self.hidden {
                self.hidden = false;
                unsafe { ShowWindow(self.hwnd, SW_SHOW) };
                // 显示瞬间压回底层即可（SW_SHOW 可能把窗口提到前面）
                self.send_to_bottom();
            }
            // 拖动中别拿配置里的旧位置把窗口拽回去
            if self.drag.is_none() {
                let (x, y, w, h, scale) = Self::target();
                moved = (x, y, w, h) != (self.x, self.y, self.w, self.h);
                if moved {
                    self.x = x;
                    self.y = y;
                    self.w = w;
                    self.h = h;
                    self.scale = scale;
                    self.dib = Dib::new(w, h);
                    unsafe { MoveWindow(self.hwnd, x, y, w, h, 1) };
                    // MoveWindow 只改位置也会重排 z 序，这里补压一次
                    self.send_to_bottom();
                }
            }
        }
        // 隐藏期间也画一帧：DIB 始终持最新内容，显示瞬间不会闪旧图；
        // 只在内容 / 位置变化时才 blit，平时零开销。
        if dirty || moved {
            self.paint();
        }
    }

    /// 把卡片压回桌面底层（不抢焦点）。
    /// 只在「显示切换 / 尺寸变化」等状态变化时调用；绝不在 400ms 轮询里无条件调用——
    /// 分层窗口(WS_EX_LAYERED)反复改 z 序会触发桌面反复重合成，肉眼看到的就是屏闪。
    fn send_to_bottom(&self) {
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        };
    }

    /// 落盘手动位置与缩放，只 patch 这几个键，不整份重写配置。
    fn persist(&self) {
        crate::board::patch_keys(&[
            ("card_x", serde_json::json!(self.x)),
            ("card_y", serde_json::json!(self.y)),
            ("card_scale", serde_json::json!(self.scale)),
        ]);
        crate::paths::trace(&format!(
            "作业卡片：位置 ({},{}) {}x{} 缩放 {:.2}",
            self.x, self.y, self.w, self.h, self.scale
        ));
    }

    /// 双指捏合缩放：dist = 当前两指屏幕距离（像素）；None = 触点不足，本次捏合结束。
    /// 倍率以「捏合开始时」为基准，避免逐帧累加漂移；落盘只在结束时做一次。
    fn on_pinch(&mut self, hwnd: HWND, dist: Option<f32>) {
        let Some(dist) = dist else {
            if self.pinch_dist.take().is_some() {
                self.persist();
                crate::paths::trace("作业卡片：双指捏合缩放结束，已保存");
            }
            return;
        };
        if self.pinch_dist.is_none() {
            // 新一次捏合：记下起始距离与当时的缩放/默认尺寸
            self.pinch_dist = Some(dist);
            self.pinch_scale = self.scale;
            self.pinch_base_w = self.w as f32 / self.scale;
            self.pinch_base_h = self.h as f32 / self.scale;
            return;
        }
        let base = self.pinch_dist.unwrap();
        let ns = (self.pinch_scale * super::touch::pinch_ratio(base, dist)).clamp(0.70, 1.40);
        let nw = (self.pinch_base_w * ns).round().max(160.0) as i32;
        let nh = (self.pinch_base_h * ns).round().max(120.0) as i32;
        if nw != self.w || nh != self.h {
            self.w = nw;
            self.h = nh;
            self.scale = ns;
            self.dib = Dib::new(nw, nh);
            unsafe { MoveWindow(hwnd, self.x, self.y, nw, nh, 1) };
            self.paint();
        }
    }
}

/// 打开单窗口三页设置面板。同目录 desktop_board.exe 优先（大屏可能跑在主程序线程里）；
/// 没有则回退当前 exe 的 --board-editor 模式（主程序已内嵌编辑器）。
fn open_editor() {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let sibling = dir.join("desktop_board.exe");
    let target = if sibling.exists() && sibling != exe {
        sibling
    } else if exe
        .file_name()
        .map(|n| n.to_string_lossy().starts_with("desktop_board"))
        .unwrap_or(false)
    {
        exe
    } else {
        // 主程序线程里跑大屏且没有独立 desktop_board.exe：用主程序自带的编辑器模式
        exe
    };
    let arg = if target.file_name().map(|n| n.to_string_lossy().starts_with("desktop_board")).unwrap_or(false) {
        "--editor"
    } else {
        "--board-editor"
    };
    match std::process::Command::new(&target).arg(arg).spawn() {
        Ok(_) => crate::paths::trace(&format!("作业卡片：已启动大屏编辑器（{} {}）", target.display(), arg)),
        Err(e) => crate::paths::trace(&format!("作业卡片：启动编辑器失败 {e}")),
    }
}

fn inside(r: (f32, f32, f32, f32), x: i32, y: i32) -> bool {
    (x as f32) >= r.0 && (x as f32) <= r.2 && (y as f32) >= r.1 && (y as f32) <= r.3
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        // 光标：强制显示系统箭头，避免层窗口沿用上一个窗口的忙碌圆环
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16));
            return 1;
        }
        WM_TIMER => {
            if let Some(st) = HomHot::state(hwnd) {
                if wp == TIMER_POLL {
                    st.poll();
                } else if wp == TIMER_SCROLL {
                    st.advance_scroll();
                }
            }
            0
        }
        // 双指捏合缩放（单指仍走下面的 WM_LBUTTON* 拖动/点击）
        WM_TOUCH => {
            if let Some(st) = HomHot::state(hwnd) {
                let dist = super::touch::read_touch_distance(wp, lp);
                if dist.is_some() {
                    // 触摸期间取消鼠标拖动，避免两套手势互相打架
                    st.drag = None;
                    unsafe { ReleaseCapture() };
                }
                st.on_pinch(hwnd, dist);
            }
            0
        }
        WM_LBUTTONDOWN => {
            let x = (lp & 0xFFFF) as u16 as i32;
            let y = ((lp >> 16) & 0xFFFF) as u16 as i16 as i32;
            if let Some(st) = HomHot::state(hwnd) {
                let mut pt = POINT { x: 0, y: 0 };
                let mut r = RECT::default();
                unsafe {
                    GetCursorPos(&mut pt);
                    GetWindowRect(hwnd, &mut r);
                    SetCapture(hwnd);
                }
                let on_badge = inside(st.badge_rect(), x, y);
                let on_edit = !on_badge && inside(st.edit_rect(), x, y);
                let on_hard = !on_badge && !on_edit && inside(st.hard_rect(), x, y);
                let scale_mode = !on_badge
                    && !on_edit
                    && !on_hard
                    && (x as f32) <= SCALE_HIT
                    && (y as f32) >= st.h as f32 - SCALE_HIT;
                st.drag = Some(Drag {
                    sx: pt.x,
                    sy: pt.y,
                    wx: r.left,
                    wy: r.top,
                    h0: st.h,
                    scale0: st.scale,
                    base_w: st.w as f32 / st.scale,
                    base_h: st.h as f32 / st.scale,
                    moved: false,
                    on_badge,
                    on_edit,
                    on_hard,
                    scale_mode,
                });
            }
            0
        }
        WM_MOUSEMOVE => {
            if let Some(st) = HomHot::state(hwnd) {
                let Some(d) = st.drag.as_ref() else { return 0 };
                let (
                    sx,
                    sy,
                    wx,
                    wy,
                    h0,
                    scale0,
                    base_w,
                    base_h,
                    moved,
                    on_badge,
                    on_edit,
                    on_hard,
                    scale_mode,
                ) = (
                    d.sx,
                    d.sy,
                    d.wx,
                    d.wy,
                    d.h0,
                    d.scale0,
                    d.base_w,
                    d.base_h,
                    d.moved,
                    d.on_badge,
                    d.on_edit,
                    d.on_hard,
                    d.scale_mode,
                );
                let mut pt = POINT { x: 0, y: 0 };
                unsafe { GetCursorPos(&mut pt) };
                let (dx, dy) = (pt.x - sx, pt.y - sy);
                if !moved && dx.abs() <= DRAG_THRESHOLD && dy.abs() <= DRAG_THRESHOLD {
                    return 0;
                }
                // 按在按钮上就只当点击，不拖动
                if on_badge || on_edit || on_hard {
                    return 0;
                }
                st.drag.as_mut().unwrap().moved = true;
                if scale_mode {
                    // 左下角把手：往左 / 往下拖变大，往右 / 往上拖变小
                    let delta = (dy - dx) as f32;
                    let ns = (scale0 * (h0 as f32 + delta) / h0 as f32).clamp(0.70, 1.40);
                    let nw = (base_w * ns).round().max(160.0) as i32;
                    let nh = (base_h * ns).round().max(120.0) as i32;
                    if nw != st.w || nh != st.h {
                        st.w = nw;
                        st.h = nh;
                        st.scale = ns;
                        st.dib = Dib::new(nw, nh);
                        unsafe { MoveWindow(hwnd, wx, wy, nw, nh, 1) };
                        st.paint();
                    }
                } else {
                    let (sw, sh) = unsafe {
                        (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
                    };
                    let nx = (wx + dx).clamp(0, (sw - st.w).max(0));
                    let ny = (wy + dy).clamp(0, (sh - st.h).max(0));
                    if (nx, ny) != (st.x, st.y) {
                        st.x = nx;
                        st.y = ny;
                        unsafe { MoveWindow(hwnd, nx, ny, st.w, st.h, 1) };
                        // 只挪窗口，位图不用重画，把现有 DIB 贴到新位置即可
                        st.present();
                    }
                }
            }
            0
        }
        WM_LBUTTONUP => {
            // 确认框会跑自己的模态消息循环：必须先结束对 HomHot 的
            // &mut 借用，否则循环分发 WM_TIMER 时同一内存会同时存在两个 &mut。
            let mut hard_click = false;
            let mut badge_click = false;
            if let Some(st) = HomHot::state(hwnd) {
                unsafe { ReleaseCapture() };
                let Some(d) = st.drag.take() else { return 0 };
                if d.moved {
                    st.persist();
                    return 0;
                }
                if d.on_hard {
                    hard_click = true;
                } else if d.on_badge {
                    badge_click = true;
                } else {
                    if d.on_edit {
                        crate::paths::trace("作业卡片：点铅笔按钮打开编辑器");
                    }
                    open_editor();
                }
            }
            if hard_click {
                if super::confirm::confirm_code(hwnd, HARD_CLOSE_CODE) {
                    // 持久标志 + 本次隐藏一起写：主程序启动 / 右键恢复都会尊重这个标志，
                    // 只有设置窗口清掉 homework_hard_hidden 才能重新打开。
                    crate::board::patch_keys(&[
                        ("homework_hard_hidden", serde_json::json!(true)),
                        ("homework_hidden", serde_json::json!(true)),
                    ]);
                    if let Some(st) = HomHot::state(hwnd) {
                        st.hidden = true;
                    }
                    unsafe { ShowWindow(hwnd, SW_HIDE) };
                    crate::paths::trace(
                        "作业卡片：输入确认码并点确定，彻底关闭作业栏（重启不自动打开，需在主程序设置里重新开启）",
                    );
                } else {
                    crate::paths::trace("作业卡片：彻底关闭已取消（确认码不正确或点了取消）");
                }
            }
            if badge_click {
                // 红色 ✕：只隐藏本次（下次启动自动恢复），但同样需要确认码，防止学生误点/乱点。
                if super::confirm::confirm_code_ex(
                    hwnd,
                    HARD_CLOSE_CODE,
                    "确认隐藏作业栏",
                    "此操作只隐藏本次作业栏：下次启动大屏会自动恢复。\r\n\r\n\
                     请输入确认码 123 后点「确定」继续：",
                ) {
                    crate::board::patch_keys(&[("homework_hidden", serde_json::json!(true))]);
                    if let Some(st) = HomHot::state(hwnd) {
                        st.hidden = true;
                    }
                    unsafe { ShowWindow(hwnd, SW_HIDE) };
                    crate::paths::trace("作业卡片：输入确认码并点确定，✕ 隐藏作业栏（下次启动自动恢复）");
                } else {
                    crate::paths::trace("作业卡片：✕ 隐藏已取消（确认码不正确或点了取消）");
                }
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                KillTimer(hwnd, TIMER_POLL);
                let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut HomHot;
                if !p.is_null() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(p));
                }
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// 创建作业卡片窗口（与跑马灯同线程，共用消息泵）。失败返回空指针。
pub fn create() -> HWND {
    let mut b = crate::board::load("");
    // 即使当前是隐藏 / 彻底关闭状态也要把窗口建好（只是不显示），
    // 否则设置窗口重新打开作业栏时无窗可显示。
    let initially_hidden = crate::board::homework_effectively_hidden(&b);
    b.homework_hidden = false;
    b.homework_hard_hidden = false; // 位图始终画完整卡片，隐藏只影响 ShowWindow
    let (x, y, w, h, scale) = HomHot::target();
    if w == 0 || h == 0 {
        return std::ptr::null_mut();
    }
    let inst = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = wide("BoardHomeworkRust");
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
        let hwnd = CreateWindowExW(
            // 注意：不要加 WS_EX_TOPMOST。作业栏要求永远沉在所有程序窗口之下，
            // 只浮在壁纸上方——z 序由 poll() 里的 HWND_BOTTOM 每 400ms 校正。
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            wide("今日作业栏").as_ptr(),
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
            return std::ptr::null_mut();
        }
        // 注册触摸：让双指捏合收到 WM_TOUCH（单指仍会模拟成鼠标消息走拖动/点击）。
        // NOACTIVATE 层窗口收不到手势识别(WM_GESTURE)，所以用原始触点自行算距离。
        RegisterTouchWindow(hwnd, TWF_NONE);
        let me = Box::new(HomHot {
            hwnd,
            dib: Dib::new(w, h),
            w,
            h,
            x,
            y,
            hidden: initially_hidden,
            scale,
            base: None,
            layer: None,
            frame: None,
            scroll_offset: 0.0,
            scroll_hold: HOLD_TOP,
            scroll_wrap: false,
            sig: None,
            drag: None,
            pinch_dist: None,
            pinch_scale: 1.0,
            pinch_base_w: 0.0,
            pinch_base_h: 0.0,
        });
        let raw = Box::into_raw(me);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        // 第一次就把内容画好，避免出现「空窗口闪一下再出卡片」
        (*raw).refresh_card(&b);
        (*raw).paint();
        if initially_hidden {
            // 建好但先藏起来；内容已经画好，恢复时可以立刻显示
            (*raw).hidden = true;
        } else {
            // 沉底窗口不会遮挡任何程序，直接显示即可；建好立即压底，
            // 不等首个 poll（400ms），避免启动瞬间在顶层闪一下
            ShowWindow(hwnd, SW_SHOW);
            SetWindowPos(
                hwnd,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        SetTimer(hwnd, TIMER_POLL, 400, std::ptr::null_mut());
        SetTimer(hwnd, TIMER_SCROLL, 120, std::ptr::null_mut());
        hwnd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_scroll_wraps_and_pauses() {
        let (travel, line_h) = (80.0, 40.0);
        // 顶部停顿：只消耗计数，不重绘
        let (o, h, w, d) = next_scroll(0.0, HOLD_TOP, false, travel, line_h);
        assert_eq!((o, h, w, d), (0.0, HOLD_TOP - 1, false, false));
        // 正常推进：往下走且要重绘
        let (o, h, w, d) = next_scroll(0.0, 0, false, travel, line_h);
        assert!(o > 0.0 && o < travel, "offset={o}");
        assert_eq!((h, w, d), (0, false, true));
        // 滚到底：夹在 travel 上，进入底部停顿并准备回卷
        let (o, h, w, d) = next_scroll(travel - 1.0, 0, false, travel, line_h);
        assert_eq!(o, travel);
        assert_eq!(h, HOLD_BOTTOM);
        assert!(w && d);
        // 回卷到开头重新滚
        let (o, h, w, d) = next_scroll(travel, HOLD_BOTTOM, true, travel, line_h);
        assert_eq!((o, h, w, d), (0.0, HOLD_TOP, false, true));
    }

    #[test]
    fn zero_travel_never_moves_past_end() {
        // travel <= 0 时窗口层根本不会启用滚动；纯函数也要保证偏移不越界
        for i in 0..5 {
            let (o, _, _, _) = next_scroll(i as f32, 0, false, 0.0, 40.0);
            assert_eq!(o, 0.0, "travel=0 时偏移应恒为 0");
        }
    }

    /// 卡片实际尺寸（1920x1080 下实测 403x790）
    const CW: f32 = 403.0;
    const CH: f32 = 790.0;

    #[test]
    fn edit_button_sits_left_of_badge() {
        let (bx0, _by0, bx1, by1) = badge_rect_at(CW, CH);
        let (ex0, _ey0, ex1, ey1) = edit_rect_at(CW, CH);
        assert!(ex1 < bx0, "铅笔按钮压到 ✕ 了：{ex1} >= {bx0}");
        assert!((bx0 - ex1 - EDIT_GAP).abs() < 0.01, "间距应为 {EDIT_GAP}");
        assert!((ey1 - by1).abs() < 0.01);
        assert!((ex1 - ex0 - (bx1 - bx0)).abs() < 0.01);
        assert!(ex0 > 0.0 && bx0 > 0.0);
        assert!(by1 <= CH && bx1 <= CW);
    }

    #[test]
    fn hard_button_sits_left_of_edit() {
        let (ex0, _ey0, _ex1, ey1) = edit_rect_at(CW, CH);
        let (hx0, _hy0, hx1, hy1) = hard_rect_at(CW, CH);
        assert!(hx1 < ex0, "电源按钮压到铅笔了：{hx1} >= {ex0}");
        assert!((ex0 - hx1 - HARD_GAP).abs() < 0.01, "间距应为 {HARD_GAP}");
        assert_eq!(hy1, ey1, "三个按钮底边要对齐");
        assert_eq!(hx1 - hx0, HARD_BTN);
        assert!(hx0 > 0.0 && hy1 <= CH);
    }

    #[test]
    fn hard_flag_keeps_card_hidden_even_if_normal_flag_is_cleared() {
        let mut b = crate::board::Board::default();
        assert!(!b.homework_hard_hidden, "默认不能是彻底关闭");
        assert!(!crate::board::homework_effectively_hidden(&b));
        b.homework_hard_hidden = true;
        b.homework_hidden = false; // 模拟启动逻辑 / 菜单误把普通开关改回 false
        assert!(
            crate::board::homework_effectively_hidden(&b),
            "彻底关闭标志必须压过普通隐藏开关"
        );
    }

    #[test]
    fn buttons_draw_inside_their_rects_only() {
        let w = CW as u32;
        let h = CH as u32;
        let mut pm = Pixmap::new(w, h).unwrap();
        pm.fill(tiny_skia::Color::TRANSPARENT);
        draw_buttons(&mut pm, CW, CH);
        let (hx0, hy0, hx1, hy1) = hard_rect_at(CW, CH);
        let (ex0, ey0, ex1, ey1) = edit_rect_at(CW, CH);
        let (bx0, by0, bx1, by1) = badge_rect_at(CW, CH);
        let mut in_hard = 0;
        let mut in_edit = 0;
        let mut in_badge = 0;
        let mut outside = 0;
        for y in 0..h {
            for x in 0..w {
                let px = pm.pixel(x, y).unwrap();
                if px.alpha() == 0 {
                    continue;
                }
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                // 描边以路径为中心，会向外溢 1.25px，留 2px 容差
                let e = 2.0;
                let in_h = fx >= hx0 - e && fx <= hx1 + e && fy >= hy0 - e && fy <= hy1 + e;
                let in_e = fx >= ex0 - e && fx <= ex1 + e && fy >= ey0 - e && fy <= ey1 + e;
                let in_b = fx >= bx0 - e && fx <= bx1 + e && fy >= by0 - e && fy <= by1 + e;
                if in_h {
                    in_hard += 1;
                } else if in_e {
                    in_edit += 1;
                } else if in_b {
                    in_badge += 1;
                } else {
                    outside += 1;
                }
            }
        }
        assert!(in_hard > 300, "电源按钮几乎没画出来：{in_hard} 像素");
        assert!(in_edit > 500, "铅笔按钮几乎没画出来：{in_edit} 像素");
        assert!(in_badge > 500, "✕ 几乎没画出来：{in_badge} 像素");
        assert_eq!(outside, 0, "有 {outside} 个像素画到按钮矩形外面了");
    }

    #[test]
    fn scale_hotspot_does_not_cover_buttons() {
        // 缩放把手在左下角，右下角的三个按钮都不在它的范围内
        let (bx0, _, _, _) = badge_rect_at(CW, CH);
        let (ex0, _, _, _) = edit_rect_at(CW, CH);
        let (hx0, _, _, _) = hard_rect_at(CW, CH);
        assert!(ex0 < CW - SCALE_HIT, "铅笔按钮应在右下，缩放区碰不到");
        assert!(bx0 > SCALE_HIT, "✕ 应在右下，缩放区碰不到");
        assert!(hx0 > SCALE_HIT, "电源按钮应在右下，缩放区碰不到");
    }
}
