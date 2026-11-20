//! 文字渲染：从系统字体（微软雅黑）里取字形轮廓，用 tiny-skia 填充 + 描边。
//!
//! 原 Python 版用 Tk Canvas 的 8 向偏移 / PIL 的 stroke_width 做「白字 + 深蓝描边」。
//! 这里直接对字形轮廓先 stroke 再 fill，效果更干净，也不再需要 8 次重复绘制。

use std::sync::Arc;

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};
use ttf_parser::{Face, OutlineBuilder};

use desktop_board::text::FontBlob;

pub enum Anchor {
    Left,
    Center,
    #[allow(dead_code)] // 预留：通知靠右档位
    Right,
}

/// 一段文字的水平位置（垂直方向一律按字面框居中）。
///
/// 字体字节直接复用 `desktop_board::text` 的进程级单例（同一个 `Arc<FontBlob>`，
/// 底层是只读文件映射），主程序 UI 与大屏渲染不再各读一遍雅黑。
pub struct Fonts {
    regular: Option<Arc<FontBlob>>,
    bold: Option<Arc<FontBlob>>,
}

impl Fonts {
    pub fn load() -> Self {
        let (regular, bold) = desktop_board::text::shared_font_data();
        Self { regular, bold }
    }

    fn data(&self, bold: bool) -> Option<&[u8]> {
        if bold {
            self.bold
                .as_deref()
                .or(self.regular.as_deref())
                .map(FontBlob::bytes)
        } else {
            self.regular.as_deref().map(FontBlob::bytes)
        }
    }

    fn face<R>(&self, bold: bool, f: impl FnOnce(&Face) -> R) -> Option<R> {
        let data = self.data(bold)?;
        let face = Face::parse(data, 0).ok()?;
        Some(f(&face))
    }

    /// 按像素字号测量文字宽度。
    pub fn measure(&self, text: &str, px: f32, bold: bool) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        self.face(bold, |face| {
            let s = px / face.units_per_em() as f32;
            let mut w = 0.0f32;
            for ch in text.chars() {
                if let Some(gid) = face.glyph_index(ch) {
                    w += face.glyph_hor_advance(gid).unwrap_or(0) as f32 * s;
                } else {
                    // 缺字：按半个字宽留白，避免文字挤在一起
                    w += px * 0.5;
                }
            }
            w
        })
        .unwrap_or(0.0)
    }

    /// 按像素宽度截断并加省略号。
    pub fn ellipsis(&self, text: &str, max_w: f32, px: f32, bold: bool) -> String {
        if self.measure(text, px, bold) <= max_w {
            return text.to_string();
        }
        let mut chars: Vec<char> = text.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let mut cand: String = chars.iter().collect();
            cand.push('…');
            if self.measure(&cand, px, bold) <= max_w {
                return cand;
            }
        }
        "…".to_string()
    }

    /// 画一段文字。
    ///
    /// * `x`/`y`：锚点位置（y 为文字视觉中线）
    /// * `outline`：(描边色, 描边宽度 px)，None = 不描边
    /// * `shadow`：小字用的单层阴影（深色偏移 1px）
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        pm: &mut Pixmap,
        x: f32,
        y: f32,
        text: &str,
        px: f32,
        bold: bool,
        color: Color,
        anchor: Anchor,
        outline: Option<(Color, f32)>,
        shadow: bool,
    ) {
        if text.is_empty() || px <= 0.0 {
            return;
        }
        let Some(width) = self.face(bold, |face| {
            let s = px / face.units_per_em() as f32;
            let mut w = 0.0f32;
            for ch in text.chars() {
                if let Some(gid) = face.glyph_index(ch) {
                    w += face.glyph_hor_advance(gid).unwrap_or(0) as f32 * s;
                } else {
                    w += px * 0.5;
                }
            }
            w
        }) else {
            return;
        };

        let start_x = match anchor {
            Anchor::Left => x,
            Anchor::Center => x - width / 2.0,
            Anchor::Right => x - width,
        };

        let build = |pm: &mut Pixmap, dx: f32, dy: f32, col: Color, thick: Option<f32>| {
            let data = match self.data(bold) {
                Some(d) => d,
                None => return,
            };
            let Ok(face) = Face::parse(data, 0) else {
                return;
            };
            let upem = face.units_per_em() as f32;
            let s = px / upem;
            let asc = face.ascender() as f32 * s;
            let desc = face.descender() as f32 * s;
            let baseline = y + dy + (asc + desc) / 2.0;

            let mut sink = PathSink {
                pb: PathBuilder::new(),
                ox: start_x + dx,
                oy: baseline,
                s,
            };
            let mut cursor = 0.0f32;
            for ch in text.chars() {
                if let Some(gid) = face.glyph_index(ch) {
                    sink.ox = start_x + dx + cursor;
                    let _ = face.outline_glyph(gid, &mut sink);
                    cursor += face.glyph_hor_advance(gid).unwrap_or(0) as f32 * s;
                } else {
                    cursor += px * 0.5;
                }
            }
            let Some(path) = sink.pb.finish() else {
                return;
            };

            if let Some(w) = thick {
                let mut paint = Paint::default();
                paint.set_color(col);
                paint.anti_alias = true;
                let stroke = Stroke {
                    width: w,
                    line_cap: tiny_skia::LineCap::Round,
                    line_join: tiny_skia::LineJoin::Round,
                    ..Default::default()
                };
                pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
            let mut paint = Paint::default();
            paint.set_color(col);
            paint.anti_alias = true;
            pm.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        };

        if let Some((ocol, owidth)) = outline {
            build(pm, 0.0, 0.0, ocol, Some(owidth));
            build(pm, 0.0, 0.0, color, None);
        } else if shadow {
            build(pm, 1.0, 1.0, Color::from_rgba8(190, 190, 190, 255), None);
            build(pm, 0.0, 0.0, color, None);
        } else {
            build(pm, 0.0, 0.0, color, None);
        }
    }
}

struct PathSink {
    pb: PathBuilder,
    ox: f32,
    oy: f32,
    s: f32,
}

impl OutlineBuilder for PathSink {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pb.move_to(self.ox + x * self.s, self.oy - y * self.s);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.pb.line_to(self.ox + x * self.s, self.oy - y * self.s);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.pb.quad_to(
            self.ox + x1 * self.s,
            self.oy - y1 * self.s,
            self.ox + x * self.s,
            self.oy - y * self.s,
        );
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.pb.cubic_to(
            self.ox + x1 * self.s,
            self.oy - y1 * self.s,
            self.ox + x2 * self.s,
            self.oy - y2 * self.s,
            self.ox + x * self.s,
            self.oy - y * self.s,
        );
    }
    fn close(&mut self) {
        self.pb.close();
    }
}
