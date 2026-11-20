//! 大屏内容模型（今日作业 / 值周 / 卫生值日 / 班级名 / 显示选项）。
//!
//! 与 Python 版 `load_board` 完全对齐：默认值补齐、旧版 `tasks` 迁移到 `tasks_week`、
//! 未填写的作业一律回填默认作业文案。未知字段原样保留（`extra`），保存时不丢。

use chrono::Datelike;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

use crate::paths;
use crate::theme::WEEK_CN;

pub const DEFAULT_HW: &str = "同步练习 第 1 页";
pub const DEFAULT_HW_NOTE: &str = "第 3 节晚自习前交";

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct HomeworkItem {
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub note: String,
}

fn default_homework() -> Vec<HomeworkItem> {
    ["语文", "数学", "英语", "物理", "化学", "生物"]
        .iter()
        .map(|s| HomeworkItem {
            subject: s.to_string(),
            content: DEFAULT_HW.to_string(),
            note: DEFAULT_HW_NOTE.to_string(),
        })
        .collect()
}

/// 兼容数字 / 字符串 / null 的浮点反序列化（旧版 JSON 里可能存成字符串）。
fn de_f64<'de, D>(d: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Option::<Value>::deserialize(d)?;
    Ok(match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(1.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(1.0),
        _ => 1.0,
    })
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Board {
    pub class_name: String,
    pub motto: String,
    /// 【已废弃，仅兼容旧 desktop_board.json】启动时自动 Win+D 已移除：
    /// 置顶横幅 / 跑马灯无需回到桌面，作业卡只在桌面前台时显示。
    pub auto_show_desktop: bool,
    /// 背景使用 Win11 经典壁纸（Bloom）
    pub wall_bg: bool,
    /// false = 原图直出；true = 叠一层白雾磨砂
    pub wall_frost: bool,
    /// true = 删白卡，文字白+描边直接浮在壁纸上
    pub glass_cards: bool,
    /// 左侧留出桌面图标位
    pub icon_gutter: bool,
    /// 顶部通知横幅宽度占可用宽度的比例（0.35~1.0）
    #[serde(deserialize_with = "de_f64")]
    pub banner_width: f64,
    /// 多显示器时选第几块（0=主屏，从左到右排序）
    pub screen_index: i64,
    /// 手动指定 [x,y,w,h] 物理像素，优先级最高
    pub screen_rect: Option<Value>,
    /// 通知跑马灯手动位置/尺寸（x/y = -1、w/h = 0 表示自动）
    pub marquee_x: i32,
    pub marquee_y: i32,
    pub marquee_w: i32,
    pub marquee_h: i32,
    /// 跑马灯上的 × 关闭：「通知条」/「作业栏」
    pub marquee_hidden: bool,
    /// 手动关闭的时刻（Unix 毫秒）：配合 40 分钟静默规则，
    /// 重启大屏后静默期依然有效；0 表示没有/已过期
    #[serde(default)]
    pub marquee_hidden_ms: u64,
    pub homework_hidden: bool,
    /// 「彻底关闭作业栏」：点作业栏上的电源按钮写入，重启程序 / 重新启动大屏
    /// 都不会自动打开，只能在主程序设置窗口里清除。旧配置没有这个键时按 false。
    #[serde(default)]
    pub homework_hard_hidden: bool,
    /// 作业卡悬浮窗的手动位置与等比缩放（-1/-1/1.0 = 用默认布局）
    pub card_x: i32,
    pub card_y: i32,
    pub card_scale: f32,
    /// 值日表：一周填一次，按星期自动显示
    pub duty: BTreeMap<String, String>,
    pub homework: Vec<HomeworkItem>,
    /// 兼容旧版：单份事项、不分星期；新版只用 tasks_week
    pub tasks: Vec<Value>,
    /// 今日事项：一周填一次，按星期自动显示
    pub tasks_week: BTreeMap<String, String>,
    /// 未知字段原样保留
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Board {
    fn default() -> Self {
        Self {
            class_name: String::new(),
            motto: "新的一天，元气满满，一起加油哦！".to_string(),
            auto_show_desktop: true,
            wall_bg: true,
            wall_frost: false,
            glass_cards: true,
            icon_gutter: true,
            banner_width: 1.0,
            screen_index: 0,
            screen_rect: None,
            marquee_x: -1,
            marquee_y: -1,
            marquee_w: 0,
            marquee_h: 0,
            // 通知默认落在作业卡顶部（常驻），顶部横幅平时收起；
            // 课间 / 放学后由跑马灯按时刻表自动弹出，不需要用户手动开
            marquee_hidden: true,
            marquee_hidden_ms: 0,
            homework_hidden: false,
            homework_hard_hidden: false,
            card_x: -1,
            card_y: -1,
            card_scale: 1.0,
            duty: BTreeMap::new(),
            homework: default_homework(),
            tasks: default_tasks(),
            tasks_week: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

/// 程序自带的示例占位事项（用于判断老师是否真的编辑过旧版事项）。
fn default_tasks() -> Vec<Value> {
    [
        ("值日安排", "擦黑板 王小明"),
        ("扫地拖地", "李小红、张小刚"),
        ("倒垃圾", "刘小华"),
        ("整理图书角", "陈小丽"),
    ]
    .iter()
    .map(|(tag, text)| serde_json::json!({ "tag": tag, "text": text }))
    .collect()
}

impl Board {
    /// 横幅宽度比例（0.35~1.0），异常值回落铺满。
    pub fn banner_ratio(&self) -> f64 {
        if self.banner_width.is_nan() {
            return 1.0;
        }
        self.banner_width.clamp(0.35, 1.0)
    }

    /// 手动指定的屏幕矩形
    pub fn manual_rect(&self) -> Option<(i32, i32, i32, i32)> {
        let v = self.screen_rect.as_ref()?;
        let arr = v.as_array()?;
        if arr.len() != 4 {
            return None;
        }
        let mut out = [0i32; 4];
        for (i, item) in arr.iter().enumerate() {
            out[i] = item.as_i64()? as i32;
        }
        Some((out[0], out[1], out[2], out[3]))
    }

    /// 某天的值周名单（按「；」拆开）
    pub fn duty_of(&self, weekday: &str) -> Vec<String> {
        split_items(self.duty.get(weekday).map(String::as_str).unwrap_or(""))
    }

    /// 某天的卫生值日名单
    pub fn tasks_of(&self, weekday: &str) -> Vec<String> {
        split_items(
            self.tasks_week
                .get(weekday)
                .map(String::as_str)
                .unwrap_or(""),
        )
    }
}

/// 按 `；` / `;` / 换行 拆分多项，与 Python 的 re.split 一致。
pub fn split_items(raw: &str) -> Vec<String> {
    raw.split(|c| c == '；' || c == ';' || c == '\n')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 「赣州中学2025级17班」→「高二（17）班」；识别不了就原样返回。
pub fn guess_class_name(raw: &str) -> String {
    let re_full = regex::Regex::new(r"(\d{4})级\s*(\d+)班").unwrap();
    if let Some(c) = re_full.captures(raw) {
        let enroll: i32 = c[1].parse().unwrap_or(0);
        let num = c[2].trim_start_matches('0').to_string();
        let now = chrono::Local::now();
        let years = now.year() - enroll - if now.month() < 9 { 1 } else { 0 };
        let grade = match years {
            0 => "高一",
            1 => "高二",
            2 => "高三",
            _ => "",
        };
        return if grade.is_empty() {
            format!("{}班", num)
        } else {
            format!("{}（{}）班", grade, num)
        };
    }
    let re_short = regex::Regex::new(r"(\d+)班").unwrap();
    if let Some(c) = re_short.captures(raw) {
        return format!("{}班", c[1].trim_start_matches('0'));
    }
    if raw.is_empty() {
        "班级大屏".to_string()
    } else {
        raw.to_string()
    }
}

/// 读大屏内容，缺失字段用默认值补齐。
pub fn load(raw_name: &str) -> Board {
    let mut board = Board::default();
    let saved: Option<Value> = std::fs::read_to_string(paths::board_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    let has_saved_week = saved
        .as_ref()
        .and_then(|v| v.get("tasks_week"))
        .map(|v| v.is_object())
        .unwrap_or(false);

    if let Some(Value::Object(_)) = saved {
        if let Ok(mut parsed) = serde_json::from_value::<Board>(saved.clone().unwrap()) {
            // 保持 Python 的键集合语义：已知字段整体覆盖（缺失 = 默认值）
            parsed.extra.clear();
            board = parsed;
        }
    }

    if !board.banner_width.is_finite() {
        board.banner_width = 1.0;
    }
    board.banner_width = board.banner_width.clamp(0.35, 1.0);

    for day in WEEK_CN {
        board.duty.entry(day.to_string()).or_default();
    }

    // 旧版 tasks 是「不分星期的一份事项」：迁移到 tasks_week，复制满 7 天以免丢信息。
    // 判断依据是「存档里有没有 tasks_week」，不能用默认空表判断。
    if !has_saved_week {
        let old = board.tasks.clone();
        let mut parts: Vec<String> = Vec::new();
        // 程序自带的示例占位事项没有任何信息价值，不必灌进 7 天。
        if old != default_tasks() {
            for t in old.iter() {
                let tag = t.get("tag").and_then(|v| v.as_str()).unwrap_or("").trim();
                let text = t.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
                let s = if text.is_empty() {
                    tag.to_string()
                } else {
                    format!("{}  {}", tag, text).trim().to_string()
                };
                if !s.is_empty() {
                    parts.push(s);
                }
            }
        }
        let merged = parts.join("；");
        board.tasks_week = WEEK_CN
            .iter()
            .map(|d| (d.to_string(), merged.clone()))
            .collect();
    }
    for day in WEEK_CN {
        board.tasks_week.entry(day.to_string()).or_default();
    }

    if board.class_name.trim().is_empty() {
        board.class_name = guess_class_name(raw_name);
    }

    if board.homework.is_empty() {
        board.homework = default_homework();
    } else {
        // 未填写（含空）的科目一律显示默认作业 + 默认交作业时间
        for item in board.homework.iter_mut() {
            let content = item.content.trim();
            if content.is_empty() || content == "点击填写作业" {
                item.content = DEFAULT_HW.to_string();
                if item.note.trim().is_empty() {
                    item.note = DEFAULT_HW_NOTE.to_string();
                }
            }
        }
    }
    board
}

/// 作业栏最终是否隐藏：普通 ✕ 只是本次隐藏（启动会恢复），
/// 「彻底关闭」是持久标志，优先级更高。所有显示判断都必须走这里。
pub fn homework_effectively_hidden(b: &Board) -> bool {
    b.homework_hard_hidden || b.homework_hidden
}

/// 只读「彻底关闭」这一个键：不经过 `Board::load`，避免默认值回填、
/// 也避免任何字段解析失败把真实状态掩盖成默认值。主程序设置窗口 / 启动逻辑用它。
pub fn homework_hard_hidden() -> bool {
    std::fs::read_to_string(paths::board_file())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("homework_hard_hidden").cloned())
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 只改写指定键（跑马灯拖动/缩放/× 开关用），不经过 Board 结构，
/// 避免 `load` 的默认值回填把用户没填的作业/值日内容改掉。
pub fn patch_keys(pairs: &[(&str, Value)]) {
    let target = paths::board_file();
    let mut v: Value = std::fs::read_to_string(&target)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| Value::Object(Map::new()));
    if !v.is_object() {
        v = Value::Object(Map::new());
    }
    if let Some(o) = v.as_object_mut() {
        for (k, val) in pairs {
            o.insert((*k).to_string(), val.clone());
        }
    }
    if let Ok(s) = serde_json::to_string_pretty(&v) {
        write_board(&s);
    }
}

fn write_board(data: &str) {
    let dir = paths::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let target = paths::board_file();
    let tmp = target.with_extension("tmp");
    if std::fs::write(&tmp, data).is_ok() {
        let _ = std::fs::rename(&tmp, &target);
    }
}

// ====================== 异步落盘 ======================
// 实测一次落盘约 44ms（杀软实时扫描），同步写会让人感觉「点了按钮半天没反应」。
// UI 只用内存里的 Board，落盘交给后台线程，最后一次由 flush() 兜底。

struct SaveState {
    pending: Option<String>,
    running: bool,
}

fn save_state() -> &'static std::sync::Mutex<SaveState> {
    static STATE: std::sync::OnceLock<std::sync::Mutex<SaveState>> = std::sync::OnceLock::new();
    STATE.get_or_init(|| {
        std::sync::Mutex::new(SaveState {
            pending: None,
            running: false,
        })
    })
}

pub fn save(board: &Board) {
    let data = match serde_json::to_string_pretty(board) {
        Ok(d) => d,
        Err(e) => {
            println!("保存大屏数据失败: {e}");
            return;
        }
    };
    let mut st = save_state().lock().unwrap();
    st.pending = Some(data);
    if st.running {
        return;
    }
    st.running = true;
    drop(st);
    std::thread::spawn(|| loop {
        let data = {
            let mut st = save_state().lock().unwrap();
            match st.pending.take() {
                Some(d) => d,
                None => {
                    st.running = false;
                    break;
                }
            }
        };
        write_board(&data);
    });
}

/// 退出前把最后一次还没落盘的设置同步写完，避免丢失。
pub fn flush() {
    let data = {
        let mut st = save_state().lock().unwrap();
        st.pending.take()
    };
    if let Some(d) = data {
        write_board(&d);
    }
}

/// 数据文件是否比给定时间更新（服务线程用来判断是否要立刻重绘）。
pub fn file_mtime(p: &Path) -> f64 {
    std::fs::metadata(p)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
