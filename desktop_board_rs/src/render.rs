//! 整屏渲染：把「壁纸 + 白卡片 + 文字」画成一张位图（1:1 复刻 Python 版版式）。
//!
//! 版式口径完全沿用原程序：
//! 顶部仅留白（日期 / 星期 / 时钟白条已移除，通知由独立跑马灯条显示）、
//! 左侧状态胶囊 + 大字 + 倒计时、右侧「今日作业」+
//! 最后一科下方的班干值日 / 卫生值日。

use chrono::{DateTime, Datelike, Local};
use std::sync::{Mutex, OnceLock};
use tiny_skia::{
    Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform,
};

use crate::board::Board;
use crate::paths;
use crate::schedule::Status;
use crate::text::{Anchor, Fonts};
use crate::theme::*;

/// 画布尺寸 + 底图缓存键。
#[derive(PartialEq, Clone, Copy)]
struct BgKey {
    w: u32,
    h: u32,
    frost: bool,
    src_mtime: f64,
}

fn bg_cache() -> &'static Mutex<Option<(BgKey, Pixmap)>> {
    static C: OnceLock<Mutex<Option<(BgKey, Pixmap)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// 壁纸底图：等比铺满后居中裁剪（等价 PIL 的 cover），不做任何调色。
fn background(w: u32, h: u32, frost: bool) -> Option<Pixmap> {
    let src = paths::wall_src();
    let key = BgKey {
        w,
        h,
        frost,
        src_mtime: crate::board::file_mtime(&src),
    };
    {
        let guard = bg_cache().lock().unwrap();
        if let Some((k, pm)) = guard.as_ref() {
            if *k == key {
                return Some(pm.clone());
            }
        }
    }

    let img = image::open(&src).ok()?.to_rgba8();
    let (sw0, sh0) = img.dimensions();
    let scale = (w as f32 / sw0 as f32).max(h as f32 / sh0 as f32);
    let nw = ((sw0 as f32 * scale).round() as u32).max(w);
    let nh = ((sh0 as f32 * scale).round() as u32).max(h);
    let resized = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Lanczos3);
    let x0 = (nw - w) / 2;
    let y0 = (nh - h) / 2;
    let cropped = image::imageops::crop_imm(&resized, x0, y0, w, h).to_image();

    let mut pm = Pixmap::new(w, h)?;
    {
        let dst = pm.data_mut();
        for (i, px) in cropped.pixels().enumerate() {
            let a = px[3] as u32;
            dst[i * 4] = ((px[0] as u32 * a) / 255) as u8;
            dst[i * 4 + 1] = ((px[1] as u32 * a) / 255) as u8;
            dst[i * 4 + 2] = ((px[2] as u32 * a) / 255) as u8;
            dst[i * 4 + 3] = a as u8;
        }
    }
    if frost {
        // 白雾磨砂，让壁纸更通透柔和
        let mut paint = Paint::default();
        paint.set_color(rgba(WHITE, 0x40));
        paint.anti_alias = false;
        pm.fill_rect(
            Rect::from_xywh(0.0, 0.0, w as f32, h as f32).unwrap(),
            &paint,
            Transform::identity(),
            None,
        );
    }
    *bg_cache().lock().unwrap() = Some((key, pm.clone()));
    Some(pm)
}

pub fn round_path(x1: f32, y1: f32, x2: f32, y2: f32, r: f32) -> Option<tiny_skia::Path> {
    let (x1, x2) = if x2 > x1 { (x1, x2) } else { (x2, x1) };
    let (y1, y2) = if y2 > y1 { (y1, y2) } else { (y2, y1) };
    let r = r.max(1.0).min((x2 - x1) / 2.0).min((y2 - y1) / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x1 + r, y1);
    pb.line_to(x2 - r, y1);
    pb.quad_to(x2, y1, x2, y1 + r);
    pb.line_to(x2, y2 - r);
    pb.quad_to(x2, y2, x2 - r, y2);
    pb.line_to(x1 + r, y2);
    pb.quad_to(x1, y2, x1, y2 - r);
    pb.line_to(x1, y1 + r);
    pb.quad_to(x1, y1, x1 + r, y1);
    pb.close();
    pb.finish()
}

pub fn rrect(
    pm: &mut Pixmap,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    r: f32,
    fill: Color,
    outline: Option<(Color, f32)>,
) {
    let Some(path) = round_path(x1, y1, x2, y2, r) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(fill);
    paint.anti_alias = true;
    pm.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    if let Some((col, w)) = outline {
        let mut paint = Paint::default();
        paint.set_color(col);
        paint.anti_alias = true;
        let stroke = Stroke {
            width: w,
            ..Default::default()
        };
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

pub fn line(pm: &mut Pixmap, x1: f32, y1: f32, x2: f32, y2: f32, col: Color, width: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x1, y1);
    pb.line_to(x2, y2);
    let Some(path) = pb.finish() else { return };
    let mut paint = Paint::default();
    paint.set_color(col);
    paint.anti_alias = true;
    let stroke = Stroke {
        width,
        ..Default::default()
    };
    pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// 铅笔图标：笔尖朝右上 45°，中心 (cx, cy)，总长 `size`。
///
/// 字体里没有可靠的铅笔字形（U+270E / U+1F4DD 都缺），所以用路径直接画，
/// 保证任何机器上都画得出来。
pub fn icon_pencil(pm: &mut Pixmap, cx: f32, cy: f32, size: f32, col: Color) {
    let d = std::f32::consts::FRAC_1_SQRT_2;
    let (ux, uy) = (d, -d); // 轴向：左下 → 右上
    let (nx, ny) = (d, d); // 法向
    let half = size * 0.5;
    let hw = size * 0.125; // 笔身半宽
    let shoulder = -half + size * 0.36; // 笔尖与笔身交界
    let at = |t: f32, s: f32| (cx + ux * t + nx * s, cy + uy * t + ny * s);

    let tip = at(-half, 0.0);
    let s1 = at(shoulder, -hw);
    let s2 = at(half, -hw);
    let s3 = at(half, hw);
    let s4 = at(shoulder, hw);
    let mut pb = PathBuilder::new();
    pb.move_to(tip.0, tip.1);
    pb.line_to(s1.0, s1.1);
    pb.line_to(s2.0, s2.1);
    pb.line_to(s3.0, s3.1);
    pb.line_to(s4.0, s4.1);
    pb.close();
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(col);
        paint.anti_alias = true;
        pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
    // 笔尖分界线 + 笔尾封头，让人一眼认出是铅笔而不是一根斜杠
    line(pm, s1.0, s1.1, s4.0, s4.1, rgb("#FFFFFF"), 1.6);
    let e0 = at(half - size * 0.11, -hw);
    let e1 = at(half - size * 0.11, hw);
    line(pm, e0.0, e0.1, e1.0, e1.1, rgb("#FFFFFF"), 1.6);
}

/// 渲染参数：字体 + 是否画「编辑」入口提示。
pub struct Renderer {
    pub fonts: Fonts,
}

/// 今日作业卡片的矩形（物理像素）：(x0, y0, x1, y1)。
///
/// 壁纸渲染与覆盖在卡片上的悬浮热区共用这一份几何，改布局只需改这里。
pub fn homework_rect(sw: f32, sh: f32, _board: &Board) -> (f32, f32, f32, f32) {
    let pad = (sw * 0.02).max(14.0);
    let top_h = (sh * 0.100).max(84.0);
    let body_y0 = top_h + (sh * 0.018).max(10.0);
    let body_y1 = sh * 0.850;
    let right_x1 = sw - pad;
    // 今日作业宽度缩小 1/3（原宽 × 2/3）并靠右
    let full_right_w = right_x1 - (sw * 0.645 + pad);
    let right_x0 = right_x1 - full_right_w * 2.0 / 3.0;
    (right_x0, body_y0, right_x1, body_y1)
}

/// 通知条滚动层：完整正文单独画进一张长图，卡片底图里正文区域留空。
/// 坐标是「裁切后的作业卡」坐标系，由 `render_card_parts` 从整屏坐标换算过来。
pub struct NoticeLayer {
    text: Option<Pixmap>,
    /// 可见窗口左上角（卡片坐标）
    vx: f32,
    vy: f32,
    /// 可见窗口宽高
    vw: f32,
    vh: f32,
    /// 文字图里第 0 行视觉中心的 y（顶部留白，防止首行字形被裁）
    src_top: f32,
    /// 行高
    pub line_h: f32,
    /// 上下渐隐带高度：文字滚入 / 滚出窗口时淡入淡出，不硬切半行
    fade: f32,
    /// 滚动总行程（0 = 内容放得下，不用滚）
    pub travel: f32,
}

impl NoticeLayer {
    fn empty() -> Self {
        Self {
            text: None,
            vx: 0.0,
            vy: 0.0,
            vw: 0.0,
            vh: 0.0,
            src_top: 0.0,
            line_h: 0.0,
            fade: 0.0,
            travel: 0.0,
        }
    }
}

/// 把文字长图的 `[src_top + offset, +vh]` 窗口以 source-over 合成到卡片上。
/// 两张图都是预乘 alpha：`out = src + dst * (1 - a)`。
pub fn compose_notice(pm: &mut Pixmap, layer: &NoticeLayer, offset: f32) {
    let Some(src) = layer.text.as_ref() else {
        return;
    };
    let (dx0, dy0) = (layer.vx.round() as i32, layer.vy.round() as i32);
    let (vw, vh) = (layer.vw.round() as i32, layer.vh.round() as i32);
    let sy0 = (layer.src_top + offset).round() as i32;
    let (dw, dh) = (pm.width() as i32, pm.height() as i32);
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let fade = layer.fade.max(0.0);
    let s = src.data();
    let d = pm.data_mut();
    for y in 0..vh {
        let dy = dy0 + y;
        let sy = sy0 + y;
        if dy < 0 || dy >= dh || sy < 0 || sy >= sh {
            continue;
        }
        // 窗口上下各留一条渐隐带：整行滚进来时由淡到实，出去时由实到淡
        let f = if fade > 0.0 {
            (y as f32 / fade).min((vh - y) as f32 / fade).min(1.0).max(0.0)
        } else {
            1.0
        };
        if f <= 0.0 {
            continue;
        }
        for x in 0..vw {
            let dx = dx0 + x;
            if dx < 0 || dx >= dw || x >= sw {
                continue;
            }
            let si = ((sy * sw + x) * 4) as usize;
            let di = ((dy * dw + dx) * 4) as usize;
            let sa = s[si + 3] as u32;
            if sa == 0 {
                continue;
            }
            // 源整体乘 f（包括 rgb），保持预乘 alpha 一致，否则淡出的字会偏白
            let a = (sa as f32 * f) as u32;
            let inv = 255 - a;
            for c in 0..4 {
                let sc = (s[si + c] as f32 * f) as u32;
                let dc = d[di + c] as u32;
                let out = if c == 3 {
                    a + dc * inv / 255
                } else {
                    sc + dc * inv / 255
                };
                d[di + c] = out.min(255) as u8;
            }
        }
    }
}

/// 全局共享的渲染器（字体只读一次，避免每次渲染重读 20MB 字体）。
pub fn shared() -> &'static Renderer {
    static R: OnceLock<Renderer> = OnceLock::new();
    R.get_or_init(Renderer::new)
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            fonts: Fonts::load(),
        }
    }

    /// 按老版 Canvas 布局渲染整屏大图。
    /// 整屏预览图（编辑器预览 / --render-only 调试出图用）。
    /// 注意：这只是画一张图，**不会**把它设成系统壁纸。
    pub fn render(
        &self,
        board: &Board,
        st: &Status,
        notice: &str,
        now: DateTime<Local>,
        w: u32,
        h: u32,
    ) -> Pixmap {
        self.render_impl(board, st, notice, now, w, h, false, None)
    }

    /// 只产出作业卡的位图（滚动首帧，调试出图 / 编辑器预览用）。
    ///
    /// 卡片圆角以外自然是透明的，拼进 layered 窗口后四角不会带出一块方形底色。
    /// 现在系统壁纸不再被替换，作业栏就靠这张图浮在任意壁纸上。
    pub fn render_card(
        &self,
        board: &Board,
        st: &Status,
        notice: &str,
        now: DateTime<Local>,
        w: u32,
        h: u32,
    ) -> Option<Pixmap> {
        let (mut base, layer) = self.render_card_parts(board, st, notice, now, w, h)?;
        if let Some(l) = layer.as_ref() {
            compose_notice(&mut base, l, 0.0);
        }
        Some(base)
    }

    /// 拆成「不含通知正文的卡片底图」+「可滚动的完整通知文字层」。
    /// 滚动时每帧只需在底图上重新合成文字层，不必重画六科作业 / 值日 / 标签。
    pub fn render_card_parts(
        &self,
        board: &Board,
        st: &Status,
        notice: &str,
        now: DateTime<Local>,
        w: u32,
        h: u32,
    ) -> Option<(Pixmap, Option<NoticeLayer>)> {
        let mut layer = NoticeLayer::empty();
        let full = self.render_impl(board, st, notice, now, w, h, true, Some(&mut layer));
        let (sw, sh) = (w as f32, h as f32);
        let (x0, y0, x1, y1) = homework_rect(sw, sh, board);
        let mut base = crop_pixmap(
            &full,
            x0.round() as i32,
            y0.round() as i32,
            x1.round() as i32,
            y1.round() as i32,
        )?;
        // 文字层坐标从整屏换算到裁切后的卡片坐标系
        layer.vx -= x0.round();
        layer.vy -= y0.round();
        if layer.text.is_none() {
            return Some((base, None));
        }
        if layer.travel <= 0.0 {
            // 内容放得下：把首帧文字直接并进底图，之后窗口不必再合成、也不做渐隐
            compose_notice(&mut base, &layer, 0.0);
            return Some((base, None));
        }
        Some((base, Some(layer)))
    }

    fn render_impl(
        &self,
        board: &Board,
        st: &Status,
        notice: &str,
        now: DateTime<Local>,
        w: u32,
        h: u32,
        transparent_bg: bool,
        mut notice_layer: Option<&mut NoticeLayer>,
    ) -> Pixmap {
        let (sw, sh) = (w as f32, h as f32);
        let mut pm = if transparent_bg {
            // 全透明底：只保留后面画上去的卡片，四角与左主区都不带底色
            let mut p = Pixmap::new(w.max(1), h.max(1)).expect("透明底 Pixmap");
            p.fill(Color::TRANSPARENT);
            p
        } else if board.wall_bg {
            match background(w, h, board.wall_frost) {
                Some(p) => p,
                None => {
                    println!(
                        "壁纸文件不存在，退回纯色底：{}",
                        paths::wall_src().display()
                    );
                    solid(sw, sh)
                }
            }
        } else {
            solid(sw, sh)
        };

        let glass = board.glass_cards;
        let pad = (sw * 0.02).max(14.0);
        let top_h = (sh * 0.100).max(84.0);
        let body_y0 = top_h + (sh * 0.018).max(10.0);
        let body_y1 = sh * 0.850;
        let gutter = if board.icon_gutter {
            (sw * 0.11).max(150.0)
        } else {
            0.0
        };
        let top_x0 = pad + gutter;
        let left_x0 = top_x0;
        // 今日作业卡片的几何与 `homework_rect` 共用一份，保证悬浮层与壁纸对得上
        let (right_x0, _, right_x1, _) = homework_rect(sw, sh, board);
        // 作业栏被关闭（含「彻底关闭」）时左主区占满整行
        let left_x1 = if crate::board::homework_effectively_hidden(board) {
            right_x1
        } else {
            right_x0 - pad
        };

        // 顶部「日期 / 星期 / 时钟」白条已按需求移除：屏幕上方只保留唯一的
        // 通知跑马灯白条，不再出现两条并列白条。顶部留白保留给跑马灯窗口。
        let _ = (notice, board.motto.as_str(), top_x0);
        let wd = now.weekday().num_days_from_monday() as usize;

        // ===== 左主区：状态胶囊 + 大字状态 + 倒计时 =====
        if !glass {
            rrect(
                &mut pm,
                left_x0,
                body_y0,
                left_x1,
                body_y1,
                (sh * 0.022).max(14.0),
                rgb(CARD),
                Some((rgb(LINE), 2.0)),
            );
        }
        let mid = (left_x0 + left_x1) / 2.0;
        let cap_w = (sw * 0.072).max(90.0);
        let cap_h = (sh * 0.042).max(30.0);
        let cap_y = body_y0 + sh * 0.035;
        let big = (sh * 0.155).min((left_x1 - left_x0) * 0.16);
        let by = cap_y + cap_h + sh * 0.105;

        // 状态为空（周末等）时中间整块都不画，避免残留空胶囊/空卡片
        if !st.state.trim().is_empty() {
            rrect(
                &mut pm,
                mid - cap_w / 2.0,
                cap_y,
                mid + cap_w / 2.0,
                cap_y + cap_h,
                cap_h / 2.0,
                rgb(GREEN),
                None,
            );
            self.text(
                &mut pm,
                mid,
                cap_y + cap_h / 2.0,
                &st.state,
                cap_h * 0.48,
                true,
                rgb(WHITE),
                Anchor::Center,
            );
            self.glass_text(&mut pm, mid, by, &st.state, big, true, rgb(BLUE), glass);
            if !st.range_text.trim().is_empty() {
                self.glass_text(
                    &mut pm,
                    mid,
                    by + big * 0.72,
                    &st.range_text,
                    (sh * 0.026).max(15.0),
                    false,
                    rgb(SUB),
                    glass,
                );
            }
        }

        if !st.cd_text.trim().is_empty() {
            let cd_w = ((left_x1 - left_x0) * 0.52).min(sw * 0.36);
            let cd_y0 = by + big * 0.95;
            let cd_y1 = cd_y0 + sh * 0.205;
            if !glass {
                rrect(
                    &mut pm,
                    mid - cd_w / 2.0,
                    cd_y0,
                    mid + cd_w / 2.0,
                    cd_y1,
                    (sh * 0.025).max(14.0),
                    rgb(GREEN_BG),
                    Some((rgb(GREEN_LINE), 2.0)),
                );
            }
            if !st.cd_label.trim().is_empty() {
                self.glass_text(
                    &mut pm,
                    mid,
                    cd_y0 + sh * 0.055,
                    &st.cd_label,
                    (sh * 0.018).max(12.0),
                    false,
                    rgb(MUTED),
                    glass,
                );
            }
            self.glass_text(
                &mut pm,
                mid,
                cd_y0 + sh * 0.132,
                &st.cd_text,
                (sh * 0.058).max(30.0),
                true,
                rgb(PINK),
                glass,
            );
            if !st.next_text.trim().is_empty() {
                self.glass_text(
                    &mut pm,
                    mid,
                    (cd_y1 + sh * 0.052).min(body_y1 - sh * 0.035),
                    &st.next_text,
                    (sh * 0.026).max(15.0),
                    true,
                    rgb(BLUE),
                    glass,
                );
            }
        }

        // 作业栏关闭（跑马灯上的 × 作业，或电源按钮的彻底关闭）：整块不绘制
        if !crate::board::homework_effectively_hidden(board) {
        // ===== 右侧今日作业 =====
        // v1.0.24 后排看不清：glass 卡底从 #E4EFFD 提亮到近白，与深色壁纸对比更强
        let hw_bg = if glass { "#F1F6FE" } else { CARD };
        let hw_line = if glass { "#D3E3F8" } else { LINE };
        rrect(
            &mut pm,
            right_x0,
            body_y0,
            right_x1,
            body_y1,
            (sh * 0.022).max(14.0),
            rgb(hw_bg),
            Some((rgb(hw_line), 2.0)),
        );
        // 标题「今日作业」已按需求移除，作业列表直接从卡片顶部开始
        // ===== 卡片顶部：常驻通知条 =====
        // 固定至多 4 行的可见窗口；完整通知折成 N 行单独画进文字层，
        // 超长时由窗口垂直滚动展示，不再用「…」把后面的内容吃掉。
        let mut list_top = body_y0 + sh * 0.03;
        let notice_txt = notice.trim();
        if !notice_txt.is_empty() {
            let pad_x = sw * 0.014;
            let bx0 = right_x0 + pad_x;
            let bx1 = right_x1 - pad_x;
            let pad = (sh * 0.011).max(8.0);
            let inner_top = body_y0 + sh * 0.014;
            const VISIBLE_LINES: usize = 4;

            // 固定的「通知」胶囊标签：「通」「知」上下竖排，宽度只占一个字，
            // 省下来的横向空间全部让给通知正文。
            let chip_px = (sh * 0.0235).max(15.0);
            let one_w = self.fonts.measure("通", chip_px, true);
            let (chip_w, chip_h, chip_line_h) = notice_chip_metrics(chip_px, one_w);
            let chip_x0 = bx0 + pad;
            let text_x = chip_x0 + chip_w + pad * 0.9;
            let text_max_w = bx1 - pad - text_x;

            // 能滚动就不必缩小字号：固定用大号，完整折行。
            // wrap 的 max_lines 给足，就不会在末行补省略号。
            // v1.0.24 后排看不清：通知正文 0.0245→0.0285（1080p 约 26.5→30.8px）。
            let npx = (sh * 0.0285).max(20.0);
            let line_h = npx * 1.42;
            let lines = self.fonts.wrap(notice_txt, text_max_w, npx, true, 512);
            let shown = lines.len().min(VISIBLE_LINES);
            let text_h = shown as f32 * line_h;
            let strip_h = (text_h + pad * 2.0).max(chip_h + pad * 2.0);
            let text_y = inner_top + (strip_h - text_h) / 2.0;
            // 标签跟随第一行文字，但竖排标签比单行高，必须限幅防止顶出通知条
            let chip_y0 = (text_y + (line_h - chip_h) / 2.0)
                .max(inner_top + pad * 0.5)
                .min(inner_top + strip_h - chip_h - pad * 0.5);

            let bg = if glass { "#E7F1FD" } else { "#EAF2FE" };
            rrect(
                &mut pm,
                bx0,
                inner_top,
                bx1,
                inner_top + strip_h,
                (sh * 0.010).max(8.0),
                rgb(bg),
                None,
            );
            // 「通知」标签：蓝底白字，竖向胶囊（圆角按宽度取，避免画成大椭圆）
            rrect(
                &mut pm,
                chip_x0,
                chip_y0,
                chip_x0 + chip_w,
                chip_y0 + chip_h,
                chip_w * 0.5,
                rgb("#2F6FD0"),
                None,
            );
            // 「通」「知」上下两行，各自水平居中。
            // Fonts::draw 的 baseline = y + (asc+desc)/2，雅黑汉字相对 y 上下近似对称，
            // 实测 y 就是字形视觉中心（不要再套别的区块用的 `- px*0.36`，那会把字顶出胶囊）。
            // 两字中心相距 chip_line_h，整体居中于胶囊 ⇒ 第一个字中心在 (chip_h - chip_line_h)/2。
            let cy0 = chip_y0 + (chip_h - chip_line_h) / 2.0;
            for (i, ch) in ["通", "知"].iter().enumerate() {
                self.text(
                    &mut pm,
                    chip_x0 + chip_w / 2.0,
                    cy0 + i as f32 * chip_line_h,
                    ch,
                    chip_px,
                    true,
                    rgb(WHITE),
                    Anchor::Center,
                );
            }
            if let Some(layer) = notice_layer.take() {
                // 文字图上下各留一点 bleed，首行 / 末行的字形才不会被裁掉
                let bleed = npx * 0.75;
                let tw = text_max_w.ceil().max(1.0) as u32;
                let th = (bleed + lines.len() as f32 * line_h + bleed)
                    .ceil()
                    .max(1.0) as u32;
                if let Some(mut tp) = Pixmap::new(tw, th) {
                    tp.fill(Color::TRANSPARENT);
                    for (i, ln) in lines.iter().enumerate() {
                        self.text(
                            &mut tp,
                            0.0,
                            bleed + line_h * 0.5 + i as f32 * line_h,
                            ln,
                            npx,
                            true,
                            rgb("#17335C"),
                            Anchor::Left,
                        );
                    }
                    layer.text = Some(tp);
                    layer.vx = text_x;
                    layer.vy = text_y;
                    layer.vw = text_max_w;
                    layer.vh = text_h;
                    layer.src_top = bleed;
                    layer.line_h = line_h;
                    layer.travel = (lines.len() as f32 - shown as f32) * line_h;
                    if layer.travel < 1.0 {
                        layer.travel = 0.0;
                    }
                    // 只有真的要滚才渐隐；短通知直接静态显示，别把首尾行弄淡
                    layer.fade = if layer.travel > 0.0 {
                        (npx * 0.55).max(6.0)
                    } else {
                        0.0
                    };
                }
            } else {
                // 静态预览 / 整屏渲染：只画可见窗口那几行
                for (i, ln) in lines.iter().take(shown).enumerate() {
                    self.text(
                        &mut pm,
                        text_x,
                        text_y + line_h * 0.5 + i as f32 * line_h,
                        ln,
                        npx,
                        true,
                        rgb("#17335C"),
                        Anchor::Left,
                    );
                }
            }
            list_top = inner_top + strip_h + sh * 0.014;
        }
        let duty_h = (sh * 0.145).max(96.0); // 值日区高度（放在最后一科作业下方）
        let rows: Vec<_> = board.homework.iter().take(8).collect();
        if !rows.is_empty() {
            let top = list_top;
            let bottom = body_y1 - duty_h - sh * 0.01;
            let row_h = (bottom - top) / rows.len() as f32;
            for (i, item) in rows.iter().enumerate() {
                let ry = top + i as f32 * row_h;
                let color = SUBJECT_COLORS[i % SUBJECT_COLORS.len()];
                let sx0 = right_x0 + sw * 0.018;
                // v1.0.24 后排看不清：字号整体上调——
                // 内容 0.021→0.027（1080p 约 22.7→29.2px）、备注 0.015→0.018；
                // 学科白字与下方内容同大（0.017→cpx），胶囊按文字实测宽度自适应，
                // 长科目名（如「信息技术」）也不会溢出色块。
                let cpx = (sh * 0.027).max(16.0);
                let npx = (sh * 0.018).max(12.0);
                let subj: String = item.subject.chars().take(4).collect();
                let spx = cpx;
                let subj_w = self.fonts.measure(&subj, spx, true);
                let sw2 = (subj_w + spx * 1.0).max(spx * 3.2);
                let sh2 = (cpx * 1.30).max(28.0);
                let has_note = !item.note.trim().is_empty();
                // 内容块整体在行内垂直居中：有备注时「内容 + 8px 呼吸 + 备注」算一块，
                // 这样行距收紧后备注不会黏在内容上。
                let gap = 8.0;
                let blk = if has_note {
                    cpx + gap + npx * 1.28
                } else {
                    cpx * 1.20
                };
                let by = ry + (row_h - blk) / 2.0;
                // 学科标签对齐内容这一行（不是整块中心，否则备注一长标签就下滑）
                let my = by + cpx * 0.45;
                rrect(
                    &mut pm,
                    sx0,
                    my - sh2 / 2.0,
                    sx0 + sw2,
                    my + sh2 / 2.0,
                    sh2 * 0.30,
                    rgb(color),
                    None,
                );
                // y 就是字形视觉中心（同「通知」胶囊实测结论），正对胶囊中心即可；
                // 不要减 0.36*px——字号改大后那会把白字顶出色块上沿。
                self.text(
                    &mut pm,
                    sx0 + sw2 / 2.0,
                    my,
                    &subj,
                    spx,
                    true,
                    rgb(WHITE),
                    Anchor::Center,
                );
                let cx = sx0 + sw2 + sw * 0.012;
                let maxw = right_x1 - cx - sw * 0.012;
                let content = self.fonts.ellipsis(&item.content, maxw, cpx, true);
                self.text(
                    &mut pm,
                    cx,
                    by,
                    &content,
                    cpx,
                    true,
                    rgb(TEXT),
                    Anchor::Left,
                );
                if has_note {
                    let note = self.fonts.ellipsis(&item.note, maxw, npx, true);
                    self.text(
                        &mut pm,
                        cx,
                        by + cpx + gap,
                        &format!("· {}", note),
                        npx,
                        true,
                        rgb("#D32F2F"), // 交作业时间等备注用红色醒目标注
                        Anchor::Left,
                    );
                }
                if i < rows.len() - 1 {
                    line(
                        &mut pm,
                        right_x0 + sw * 0.015,
                        ry + row_h * 0.93,
                        right_x1 - sw * 0.015,
                        ry + row_h * 0.93,
                        rgb("#DFE9F4"), // v1.0.24 分隔线加深一档，行与行分得更开
                        1.0,
                    );
                }
            }
        }

        // ===== 值日：直接放在最后一科作业下方（原底部白色长条已移除）=====
        let duty_top = body_y1 - duty_h;
        line(
            &mut pm,
            right_x0 + sw * 0.015,
            duty_top,
            right_x1 - sw * 0.015,
            duty_top,
            rgb(LINE),
            1.5,
        );
        // 值日两行的字号：v1.0.24 后排看不清，从约 19px 提到约 22.7px（1080p），
        // 标签和姓名胶囊都跟着放大。
        let dpx = duty_font_px(sh);
        let d_chip_h = duty_h * 0.28;
        let day = WEEK_CN[wd];
        self.chips(
            &mut pm,
            right_x0 + sw * 0.014,
            right_x1 - sw * 0.014,
            duty_top + duty_h * 0.30,
            d_chip_h,
            sw,
            dpx,
            "班干值日",
            &board.duty_of(day),
        );
        self.chips(
            &mut pm,
            right_x0 + sw * 0.014,
            right_x1 - sw * 0.014,
            duty_top + duty_h * 0.72,
            d_chip_h,
            sw,
            dpx,
            "卫生值日",
            &board.tasks_of(day),
        );
        }
        pm
    }

    fn text(
        &self,
        pm: &mut Pixmap,
        x: f32,
        y: f32,
        t: &str,
        px: f32,
        bold: bool,
        col: Color,
        anchor: Anchor,
    ) {
        self.fonts
            .draw(pm, x, y, t, px, bold, col, anchor, None, false);
    }

    /// glass 模式：文字白 + 深色描边直接浮在壁纸上；小字改成单层阴影。
    fn glass_text(
        &self,
        pm: &mut Pixmap,
        x: f32,
        y: f32,
        t: &str,
        px: f32,
        bold: bool,
        dark: Color,
        glass: bool,
    ) {
        if glass {
            if px < 22.0 {
                self.fonts.draw(
                    pm,
                    x,
                    y,
                    t,
                    px,
                    bold,
                    rgb(WHITE),
                    Anchor::Center,
                    None,
                    true,
                );
            } else {
                let width = if px < 40.0 { 2.0 } else { 3.0 };
                self.fonts.draw(
                    pm,
                    x,
                    y,
                    t,
                    px,
                    bold,
                    rgb(WHITE),
                    Anchor::Center,
                    Some((rgb(OUTLINE), width)),
                    false,
                );
            }
        } else {
            self.fonts
                .draw(pm, x, y, t, px, bold, dark, Anchor::Center, None, false);
        }
    }

    /// 「标签： 姓名 姓名 …」，放不下即停；返回终点 x。
    #[allow(clippy::too_many_arguments)]
    fn chips(
        &self,
        pm: &mut Pixmap,
        x: f32,
        x_limit: f32,
        cy: f32,
        chip_h: f32,
        sw: f32,
        fpx: f32,
        label: &str,
        names: &[String],
    ) -> f32 {
        let head = format!("{}：", label);
        let hw = self.fonts.measure(&head, fpx, true);
        if x + hw > x_limit {
            return x_limit;
        }
        // self.text 的 y 是文字框顶边，减 0.36*px 才能让字在胶囊里视觉居中
        let ty = cy - fpx * 0.36;
        self.text(pm, x, ty, &head, fpx, true, rgb(BLUE), Anchor::Left);
        let mut x = x + hw + sw * 0.004;
        if names.is_empty() {
            self.text(pm, x, ty, "未安排", fpx, false, rgb(MUTED), Anchor::Left);
            return x + self.fonts.measure("未安排", fpx, false) + sw * 0.010;
        }
        for (i, name) in names.iter().enumerate() {
            let w2 = self.fonts.measure(name, fpx, false) + sw * 0.022;
            if x + w2 > x_limit {
                break;
            }
            let bg = if i % 2 == 0 { GREEN_BG } else { BLUE_SOFT };
            rrect(
                pm,
                x,
                cy - chip_h / 2.0,
                x + w2,
                cy + chip_h / 2.0,
                chip_h * 0.35,
                rgb(bg),
                None,
            );
            let col = SUBJECT_COLORS[i % SUBJECT_COLORS.len()];
            self.text(
                pm,
                x + sw * 0.011,
                ty,
                name,
                fpx,
                false,
                rgb(col),
                Anchor::Left,
            );
            x += w2 + sw * 0.008;
        }
        x
    }
}

/// 班干值日 / 卫生值日字号：约屏幕高度的 1.8%（1080p ≈ 19px），小屏不低于 13px。
/// 通知标签（竖排「通」「知」两字）尺寸：(宽, 高, 行距)。
/// 宽度只占一个字，把横向空间让给通知正文；高度容纳上下两行。
fn notice_chip_metrics(chip_px: f32, one_w: f32) -> (f32, f32, f32) {
    let line_h = chip_px * 1.18;
    let w = one_w + chip_px * 0.72;
    let h = line_h * 2.0 + chip_px * 0.46;
    (w, h, line_h)
}

fn duty_font_px(sh: f32) -> f32 {
    (sh * 0.021).max(15.0)
}

fn solid(sw: f32, sh: f32) -> Pixmap {
    let mut pm = Pixmap::new(sw as u32, sh as u32).unwrap_or_else(|| Pixmap::new(8, 8).unwrap());
    pm.fill(rgb(PAGE));
    pm
}

/// 保存为 PNG（壁纸 / 预览图）。
pub fn save_png(pm: &Pixmap, path: &std::path::Path) -> bool {
    match pm.encode_png() {
        Ok(bytes) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(path, bytes).is_ok()
        }
        Err(e) => {
            println!("PNG 编码失败: {e}");
            false
        }
    }
}

/// 从整屏位图裁出一块（保留预乘 alpha），越界的地方留透明。
pub fn crop_pixmap(src: &Pixmap, x0: i32, y0: i32, x1: i32, y1: i32) -> Option<Pixmap> {
    let cw = (x1 - x0).max(1) as u32;
    let ch = (y1 - y0).max(1) as u32;
    let mut out = Pixmap::new(cw, ch)?;
    out.fill(Color::TRANSPARENT);
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let s = src.data();
    let d = out.data_mut();
    for y in 0..ch as i32 {
        let sy = y0 + y;
        if sy < 0 || sy >= sh {
            continue;
        }
        for x in 0..cw as i32 {
            let sx = x0 + x;
            if sx < 0 || sx >= sw {
                continue;
            }
            let si = ((sy * sw + sx) * 4) as usize;
            let di = ((y * cw as i32 + x) * 4) as usize;
            d[di..di + 4].copy_from_slice(&s[si..si + 4]);
        }
    }
    Some(out)
}

/// 等比缩放位图（卡片拖右下角缩放时用，像素是预乘 alpha，直接重采样即可）。
pub fn scale_pixmap(src: &Pixmap, w: u32, h: u32) -> Option<Pixmap> {
    if w == 0 || h == 0 {
        return None;
    }
    if w == src.width() && h == src.height() {
        return Some(src.clone());
    }
    let img = image::RgbaImage::from_raw(src.width(), src.height(), src.data().to_vec())?;
    let scaled = image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3);
    let mut out = Pixmap::new(w, h)?;
    out.data_mut().copy_from_slice(scaled.as_raw());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{duty_font_px, notice_chip_metrics};

    #[test]
    fn duty_text_is_bigger_than_before_but_bounded() {
        // 1080p：v1.0.14 约 14px → v1.0.23 约 19.4px → v1.0.24 后排看不清提到约 22.7px
        assert!((duty_font_px(1080.0) - 22.68).abs() < 0.01);
        assert!(duty_font_px(1080.0) > 1080.0 * 0.013 * 1.3);
        // 小屏有下限，不会缩到看不清
        assert!(duty_font_px(600.0) >= 15.0);
        assert!(duty_font_px(1080.0) < duty_font_px(2160.0));
    }

    #[test]
    fn notice_chip_is_vertical_and_narrow() {
        let px = 22.0;
        let (w, h, line_h) = notice_chip_metrics(px, px);
        // 竖排标签比横排两字窄，且明显是「竖」的
        assert!(w < px * 2.0, "标签宽度不应超过两个字的字宽: {w}");
        assert!(h > w, "竖排标签高度必须大于宽度: {w}x{h}");
        assert!(h >= line_h * 2.0, "高度要放得下上下两行: {h}");
        // 小字号下仍然保持竖排比例
        let (w2, h2, _) = notice_chip_metrics(13.0, 13.0);
        assert!(h2 > w2);
    }
}
