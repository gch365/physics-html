//! 配色与常量（与 Python 版 Theme / 全局常量逐项对应）。

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// "#rrggbb" → Rgb；异常回落白色。
pub fn rgb(s: &str) -> Rgb {
    let s = s.trim_start_matches('#');
    if s.len() == 6 {
        if let Ok(v) = u32::from_str_radix(s, 16) {
            return Rgb {
                r: (v >> 16) as u8,
                g: (v >> 8) as u8,
                b: v as u8,
            };
        }
    }
    Rgb {
        r: 255,
        g: 255,
        b: 255,
    }
}


pub const BG: &str = "#f8f9fa";
pub const BAR_COLOR_TOP: &str = "#d4e6f1";
pub const BAR_COLOR_BOTTOM: &str = "#fdebd0";
pub const BAR_TOP_START: &str = "#85c1e9";
pub const BAR_TOP_END: &str = "#d6eaf8";
pub const BAR_BOTTOM_START: &str = "#82e0aa";
pub const BAR_BOTTOM_END: &str = "#d5f5e3";
// 方案二：深蓝灰胶囊侧边栏
pub const BAR_GLASS_BG: &str = "#20344f";       // 胶囊底
pub const BAR_GLASS_ACCENT: &str = "#8fb4da";   // 时钟图标/冒号（浅蓝）
pub const BAR_GLASS_MUTED: &str = "#9fb3c8";    // 次要文字（标签/单位）
pub const BAR_GLASS_NUM: &str = "#f2f6fa";      // 倒计时大数字（白，与上方时钟一致）
pub const BAR_GLASS_URGENT: &str = "#e24b4a";   // ≤5 分钟警示（红，不闪）
pub const BAR_GLASS_PROGRESS: &str = "#f5a623"; // 进度条填充
pub const BAR_GLASS_TRACK: &str = "#3a5170";    // 进度条底槽
pub const BAR_GLASS_DIVIDER: &str = "#3a5170";  // 上下分区细线
pub const BAR_GLASS_OFF: &str = "#8393a8";      // 放学/周末文字（灰蓝）
pub const BAR_TEXT_TOP: &str = "#2874a6";
pub const BAR_TEXT_BOTTOM: &str = "#1e8449";
pub const BAR_CORNER_RADIUS: i32 = 12;
pub const BAR_TOP_EDGE: &str = "#6ba3d1";
pub const BAR_BOTTOM_EDGE: &str = "#62b080";
pub const COUNTDOWN_BLUE: &str = "#0067b8";
pub const BTN_MINUS_BG: &str = "#f8d7da";
pub const BTN_PLUS_BG: &str = "#d4edda";
pub const BTN_CLOSE_BG: &str = "#e2e3e5";
pub const BTN_ROLL_BG: &str = "#f1f3f5";
pub const BTN_EXIT_BG: &str = "#ffeaea";
pub const BTN_DANGER_BG: &str = "#f8d7da";
pub const TEXT_EXIT_RED: &str = "#c0392b";
pub const LINE_GRAY: &str = "#bdc3c7";
pub const HISTORY_SELECT_BG: &str = "#d4edda";
pub const WHITE: &str = "#FFFFFF";
pub const BLACK: &str = "#000000";
pub const RED: &str = "#FF0000";
pub const BLUE: &str = "#0066ff";
pub const PURPLE: &str = "#9933cc";

/// "白色/红色/蓝色/紫色" → 颜色值（配置里的中文颜色名）。
pub fn color_text_map(name: &str) -> &'static str {
    match name {
        "红色" => RED,
        "蓝色" => BLUE,
        "紫色" => PURPLE,
        _ => WHITE,
    }
}

// ---- 全局常量 ----
pub const ROLL_W_DEF: i32 = 520;
pub const ROLL_H_DEF: i32 = 320;
pub const WIN_MAX_SCREEN_RATIO: f64 = 0.9;

pub const TIME2MIN: i64 = 120;
pub const TIME3MIN: i64 = 180;
pub const TIME5MIN: i64 = 300;

pub const TOUCH_DEBOUNCE: u32 = 300;

/// 长按 800ms 弹菜单（对应 Python LONG_PRESS_MS）。
pub const LONG_PRESS_MS: u32 = 800;
pub const DRAG_THRESHOLD: i32 = 8;
/// 通知横幅专用拖动阈值：触摸屏（希沃大屏）点击常有 10~20px 抖动，
/// 8px 阈值会把「点一下关闭」误判成拖动，表现为横幅有时关不掉。
pub const NOTICE_DRAG_THRESHOLD: i32 = 24;

/// 放宽到一屏范围，最终由 bar_geometry 按实际屏幕高度夹取，
/// 避免旧值（±200/500）把侧边条钉死在上下两个位置。
pub const BAR_OFFSET_MIN: i32 = -1200;
pub const BAR_OFFSET_MAX: i32 = 1200;
/// 默认垂直居中（0 = 不加像素偏移）；旧值 180 在放大后的侧边条上会被夹到屏幕底部。
pub const BAR_OFFSET_DEF: i32 = 0;
/// 侧边栏自由缩放范围（滚轮/设置页滑杆共用）
pub const BAR_SCALE_MIN: f64 = 0.5;
pub const BAR_SCALE_MAX: f64 = 2.5;
pub const BAR_SCALE_STEP: f64 = 0.05;
/// 通知横幅自由缩放范围（滚轮 / 设置页滑杆共用）
pub const NOTICE_SCALE_MIN: f64 = 0.5;
pub const NOTICE_SCALE_MAX: f64 = 2.5;
pub const NOTICE_SCALE_STEP: f64 = 0.05;
/// 点名窗 / 倒计时窗自由缩放范围（与侧边栏一致的调节手感）
pub const WIN_SCALE_MIN: f64 = 0.5;
pub const WIN_SCALE_MAX: f64 = 2.5;
pub const BAR_GAP_STEP: i32 = 20;
pub const BAR_GAP_MAX: i32 = 300;
pub const BAR_GAP_DEF: i32 = 10;

pub const APP_INIT_SLEEP: u64 = 1;
pub const APP_MUTEX_NAME: &str = "Local\\PhysicalClassRollCallV7_Mutex";

/// 手动关闭横幅后的静默时长（秒）：40 分钟内不再显示，新通知 / 下课转换照常解除。
pub const MANUAL_SILENCE_SECS: i64 = 2400;

/// 上课期间兜底轮询间隔（秒）：仅在 GoEasy 门铃失联（桥接页心跳超时）时启用；
/// 门铃健康时上课期间完全零轮询（见 goeasy::doorbell_healthy）。
/// 失联时 120 秒一轮 + 心跳 150 秒判定窗 → 新通知最迟约 2.5 分钟到达。
pub const DOORBELL_DOWN_POLL_SECS: f64 = 120.0;

/// 新通知强制弹出后横幅保持显示的时长（秒）：期间上课状态不收起，
/// 手动关闭立即失效。避免刚弹出来就被 check_class_loop 的上课分支收掉。
pub const BANNER_HOLD_SECS: f64 = 180.0;

pub const APP_VERSION: &str = "1.0.28";

/// 尺寸档位 → 点名窗（倍数, 数字字号, 按钮字号）
pub fn roll_font(size: &str) -> (f64, i32, i32) {
    match size {
        "中" => (1.3, 110, 13),
        "大" => (1.6, 130, 14),
        _ => (1.0, 90, 12),
    }
}

/// 尺寸档位 → 倒计时窗（宽, 高, 数字字号, 按钮字号, 按钮内边距）
pub fn countdown_size(size: &str) -> (i32, i32, i32, i32, i32) {
    match size {
        "中" => (470, 200, 67, 13, 10),
        "大" => (520, 220, 76, 15, 12),
        _ => (420, 180, 58, 12, 9),
    }
}

/// 字号档位 → 像素
pub fn font_size_map(size: &str) -> i32 {
    match size {
        "中" => 28,
        "大" => 35,
        "特大" => 45,
        "巨大" => 60,
        _ => 22,
    }
}
