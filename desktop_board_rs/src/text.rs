//! 文字渲染：从系统字体（微软雅黑）里取字形轮廓，用 tiny-skia 填充 + 描边。
//!
//! 原 Python 版用 Tk Canvas 的 8 向偏移 / PIL 的 stroke_width 做「白字 + 深蓝描边」。
//! 这里直接对字形轮廓先 stroke 再 fill，效果更干净，也不再需要 8 次重复绘制。

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};
use ttf_parser::{Face, OutlineBuilder};

use crate::win::sys::*;

pub enum Anchor {
    Left,
    Center,
    Right,
}

/// 字体字节：优先只读内存映射，失败时退回一次性读入。
///
/// 映射的好处是这 36MB 走的是**文件页**（可被系统回收、不计入私有内存），
/// 而不是进程私有堆；三处 `Fonts` 又共用同一份 `Arc`，整机只占一份。
///
/// 映射句柄故意不在 Drop 里释放：字体是进程级单例，进程退出由系统回收，
/// 这样任何持有 `&[u8]` 的地方都不会踩到悬空指针。
pub struct FontBlob {
    ptr: *const u8,
    len: usize,
    /// 映射失败时的兜底缓冲（此时 bytes() 直接用它，不用 ptr）
    owned: Option<Vec<u8>>,
}

// 指向只读文件映射 / 只读堆缓冲，跨线程共享是安全的
unsafe impl Send for FontBlob {}
unsafe impl Sync for FontBlob {}

impl FontBlob {
    pub fn bytes(&self) -> &[u8] {
        if let Some(v) = &self.owned {
            return v.as_slice();
        }
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    fn open(path: &Path) -> Option<Arc<Self>> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            let hfile = CreateFileW(
                wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if hfile == INVALID_HANDLE_VALUE {
                return Self::read_owned(path);
            }
            let size = GetFileSize(hfile, std::ptr::null_mut());
            if size == 0 || size == u32::MAX {
                CloseHandle(hfile);
                return Self::read_owned(path);
            }
            let hmap = CreateFileMappingW(
                hfile,
                std::ptr::null_mut(),
                PAGE_READONLY,
                0,
                0,
                std::ptr::null(),
            );
            if hmap.is_null() {
                CloseHandle(hfile);
                return Self::read_owned(path);
            }
            let view = MapViewOfFile(hmap, FILE_MAP_READ, 0, 0, 0);
            if view.is_null() {
                CloseHandle(hmap);
                CloseHandle(hfile);
                return Self::read_owned(path);
            }
            Some(Arc::new(FontBlob {
                ptr: view as *const u8,
                len: size as usize,
                owned: None,
            }))
        }
    }

    /// 兜底：直接读进堆（老路子），映射不可用时保证功能不退化
    fn read_owned(path: &Path) -> Option<Arc<Self>> {
        let data = std::fs::read(path).ok().filter(|d| !d.is_empty())?;
        Some(Arc::new(FontBlob {
            ptr: std::ptr::null(),
            len: data.len(),
            owned: Some(data),
        }))
    }
}

/// 一段文字的水平位置（垂直方向一律按字面框居中）。
///
/// 字体字节用 `Arc` 共享：主程序 UI、壁纸渲染、跑马灯三处各有一个 `Fonts`，
/// 但底层只映射一次文件（雅黑 19MB + 粗体 17MB），否则三份就是 110MB。
pub struct Fonts {
    regular: Option<Arc<FontBlob>>,
    bold: Option<Arc<FontBlob>>,
}

const FONT_DIR: &str = r"C:\Windows\Fonts";

fn open_font(names: &[&str]) -> Option<Arc<FontBlob>> {
    for n in names {
        if let Some(b) = FontBlob::open(&Path::new(FONT_DIR).join(n)) {
            return Some(b);
        }
    }
    None
}

/// 进程内共享的字体字节（常规体, 粗体）。
///
/// 只映射一次文件，之后所有 `Fonts` 拿同一份 `Arc`。粗体缺失时回落到常规体。
pub fn shared_font_data() -> (Option<Arc<FontBlob>>, Option<Arc<FontBlob>>) {
    static SHARED: OnceLock<(Option<Arc<FontBlob>>, Option<Arc<FontBlob>>)> = OnceLock::new();
    SHARED
        .get_or_init(|| {
            let regular = open_font(&[
                "msyh.ttc",
                "msyh.ttf",
                "simhei.ttf",
                "simsun.ttc",
                "deng.ttf",
            ]);
            let bold = open_font(&["msyhbd.ttc", "msyhbd.ttf", "simhei.ttf"])
                .or_else(|| regular.clone());
            if regular.is_none() {
                println!("警告：未找到中文字体（msyh.ttc），文字将无法绘制");
            }
            (regular, bold)
        })
        .clone()
}

impl Fonts {
    pub fn load() -> Self {
        let (regular, bold) = shared_font_data();
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

    /// 按最大宽度折行，最多 `max_lines` 行，放不下时末行加省略号。
    ///
    /// 只解析一次字体面（逐字符量宽度），比反复调 `measure` 快得多。
    pub fn wrap(&self, text: &str, max_w: f32, px: f32, bold: bool, max_lines: usize) -> Vec<String> {
        let flat: String = text
            .chars()
            .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
            .collect();
        let flat = flat.trim();
        if flat.is_empty() || max_lines == 0 {
            return Vec::new();
        }
        let Some(data) = self.data(bold) else {
            return vec![flat.to_string()];
        };
        let Ok(face) = Face::parse(data, 0) else {
            return vec![flat.to_string()];
        };
        let s = px / face.units_per_em() as f32;
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut cur_w = 0.0f32;
        // 本行因悬挂标点已经超出的宽度（不计入换行判断，避免死循环）
        let mut overflow = 0.0f32;
        for ch in flat.chars() {
            let w = match face.glyph_index(ch) {
                Some(gid) => face.glyph_hor_advance(gid).unwrap_or(0) as f32 * s,
                None => px * 0.5,
            };
            if cur_w + w > max_w && !cur.is_empty() {
                // 中文禁则：这些标点不能落在行首，允许它悬挂在本行末尾（最多溢出 1.25 字）
                let no_start = matches!(
                    ch,
                    '、' | '，'
                        | '。'
                        | '；'
                        | '：'
                        | '？'
                        | '！'
                        | '）'
                        | '】'
                        | '》'
                        | '」'
                        | '』'
                        | '”'
                        | '’'
                        | '…'
                        | '·'
                        | '%'
                );
                if !(no_start && overflow + w <= px * 1.25) {
                    if lines.len() + 1 == max_lines {
                        // 已经是最后一行：剩下的内容折成省略号
                        lines.push(format!("{}…", cur.trim_end()));
                        return lines;
                    }
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0.0;
                    overflow = 0.0;
                } else {
                    overflow += w;
                }
            }
            cur.push(ch);
            cur_w += w;
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        lines
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
            build(pm, 1.0, 1.0, crate::theme::rgb(crate::theme::OUTLINE), None);
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
