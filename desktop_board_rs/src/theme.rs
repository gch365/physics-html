//! 配色与常量：与教室端「班级通知」卡片同一套（深蓝 + 浅绿 + 白 + 绿）。

use tiny_skia::Color;

/// 页面底（极淡蓝白）
pub const PAGE: &str = "#F4F8FF";
pub const WHITE: &str = "#FFFFFF";
pub const LINE: &str = "#DDE8F5";
/// 主深蓝（标题 / 大字）
pub const BLUE: &str = "#0A2F70";
/// 次深蓝
#[allow(dead_code)]
pub const BLUE2: &str = "#123C86";
pub const BLUE_SOFT: &str = "#E8F0FF";
/// 状态胶囊 / 按钮
pub const GREEN: &str = "#2EBD72";
/// 左侧主卡片
pub const GREEN_BG: &str = "#E9FEF1";
pub const GREEN_LINE: &str = "#D6EDE0";
pub const TEXT: &str = "#0B0B0B";
pub const SUB: &str = "#4A5B72";
/// 弱化文字（「未安排」等）。v1.0.24 后排看不清：#8493A2 → #64788C 加深一档。
pub const MUTED: &str = "#64788C";
/// 右卡片 / 倒计时卡
pub const CARD: &str = "#FFFFFF";
/// 倒计时数字，给深蓝里加一点活泼
pub const PINK: &str = "#7C5CE0";
/// 描边文字用的深蓝描边色
pub const OUTLINE: &str = "#061B3A";

pub const SUBJECT_COLORS: [&str; 10] = [
    "#2EBD72", "#F0A03C", "#3B8CF0", "#8B6DE3", "#20B2AA", "#E26D8A", "#5B8FF9", "#F6BD16",
    "#5AD8A6", "#9270CA",
];

/// 星期一 … 星期日（下标 = chrono 的 num_days_from_monday）
pub const WEEK_CN: [&str; 7] = [
    "星期一",
    "星期二",
    "星期三",
    "星期四",
    "星期五",
    "星期六",
    "星期日",
];

/// `#RRGGBB` → tiny-skia 颜色（不透明）。
pub fn rgb(hex: &str) -> Color {
    let h = hex.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0);
    Color::from_rgba8(
        ((v >> 16) & 0xFF) as u8,
        ((v >> 8) & 0xFF) as u8,
        (v & 0xFF) as u8,
        255,
    )
}

pub fn rgba(hex: &str, a: u8) -> Color {
    let mut c = rgb(hex);
    c.set_alpha(a as f32 / 255.0);
    c
}
