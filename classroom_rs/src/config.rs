//! 配置模型与读写（对应 Python 的 DEFAULT_CONFIG / load_config / save_config）。

use crate::paths;
use crate::theme;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;

/// GoEasy 只读订阅 key（BS- 开头）。按 GoEasy 的设计，Subscribe Key 本就是下发到
/// 客户端浏览器使用的公开只读 key，与 Gitee token / GoEasy 发布 key（BC-）性质不同
/// ——后两者是真正的密钥，绝不写进程序；这个 key 旧版 Python 一直内置在源码里。
///
/// 零密钥改造后改为只从环境变量读，导致「全新安装」或「旧 config 丢字段」的机器
/// key 为空 → 无头桥接在 start() 直接 return → presence 永远不上报 → notify 一直
/// 显示离线（虽然 Gitee 心跳/通知都正常）。这里对空值回填，恢复旧版的自动行为。
pub const DEFAULT_GOEASY_SUBSCRIBE_KEY: &str = "BS-ac5b60f3e15a48bd9d337094147235d0";

pub fn load_json<T: for<'de> Deserialize<'de>>(p: &PathBuf, default: T) -> T {
    match std::fs::read_to_string(p) {
        Ok(s) => serde_json::from_str(&s).unwrap_or(default),
        Err(_) => default,
    }
}

/// 原子写 JSON：临时文件 + os.replace。
pub fn save_json_atomic<T: Serialize>(p: &PathBuf, data: &T) -> bool {
    let v = match serde_json::to_string_pretty(data) {
        Ok(v) => v,
        Err(_) => return false,
    };
    save_text_atomic(p, &v)
}

pub fn save_text_atomic(p: &PathBuf, text: &str) -> bool {
    use std::io::Write;
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = p.with_extension("json.tmp");
    match std::fs::File::create(&tmp) {
        Ok(mut f) => {
            if f.write_all(text.as_bytes()).is_err() {
                return false;
            }
            let _ = f.sync_all();
            drop(f);
            std::fs::rename(&tmp, p).is_ok()
        }
        Err(_) => false,
    }
}

/// 班级条目。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ClassEntry {
    pub school: String,
    pub year: String,
    pub class_num: String,
    pub display_name: String,
    pub file_name: String,
    pub students: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ClassEntry {
    /// 班级文件名：school_年级级NN班.txt（班级号补零两位）。
    pub fn build_file_name(school: &str, year: &str, class_num: &str) -> String {
        let cn = match class_num.trim().parse::<i64>() {
            Ok(n) => format!("{:02}", n),
            Err(_) => class_num.trim().to_string(),
        };
        format!("{}_{}级{}班.txt", school.trim(), year.trim(), cn)
    }

    /// 显示名：school年级+class_num班 → "赣州中学2025级9班"（不补零）。
    pub fn build_display_name(school: &str, year: &str, class_num: &str) -> String {
        let cn = match class_num.trim().parse::<i64>() {
            Ok(n) => n.to_string(),
            Err(_) => class_num.trim().to_string(),
        };
        format!("{}{}级{}班", school.trim(), year.trim(), cn)
    }
}

/// 定时通知条目。weekdays 兼容 int 与 string 两种存法。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ScheduledNotice {
    pub enabled: bool,
    pub name: String,
    pub content: String,
    pub weekdays: Vec<Value>,
    pub start: String,
    pub end: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ScheduledNotice {
    /// 提取星期编号（1=周一 … 7=周日）。
    pub fn weekday_nums(&self) -> Vec<i64> {
        self.weekdays
            .iter()
            .filter_map(|v| {
                if let Some(n) = v.as_i64() {
                    Some(n)
                } else {
                    v.as_str().and_then(|s| s.trim().parse().ok())
                }
            })
            .collect()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub current_class: String,
    pub class_list: Vec<ClassEntry>,
    pub gitee_tongzhi_dir: String,
    pub notice_color: String,
    pub bar_size: String,
    /// 侧边栏自由缩放系数（0.5~2.5；0 = 未设置，回落 bar_size 三档）
    pub bar_scale: f64,
    /// 通知横幅自由缩放系数（0 = 回落 notice_font_size 档位）
    pub notice_scale: f64,
    /// 通知横幅拖动位置偏移（相对默认位置）
    pub notice_offset_x: i32,
    pub notice_offset_y: i32,
    pub notice_font_size: String,
    pub notice_position: String,
    pub default_countdown: i64,
    pub bar_top_offset: i32,
    pub bar_left_gap: i32,
    /// 侧边条手动拖动后的绝对 x（-1 = 未拖动，按 bar_left_gap 自动贴边）
    pub bar_left_x: i32,
    pub bar_right_x: i32,
    pub roll_win_size: String,
    pub countdown_win_size: String,
    /// 点名窗口 / 倒计时窗口自由缩放（1.0 = 档位原尺寸；0 = 未设置回落 1.0）
    pub roll_scale: f64,
    pub countdown_scale: f64,
    /// 点名滚动总时长（秒，2.0~5.0；默认 3.0）
    pub roll_seconds: f64,
    pub auto_run: bool,
    pub work_time: String,
    pub gitee_enabled: bool,
    pub gitee_owner: String,
    pub gitee_repo: String,
    pub gitee_token: String,
    pub gitee_random_max: i64,
    pub log_max_days: i64,
    pub gitee_download_folder: String,
    pub update_enabled: bool,
    pub update_branch: String,
    pub update_exe_path: String,
    pub scheduled_notices: Vec<ScheduledNotice>,
    pub goeasy_enabled: bool,
    pub goeasy_appkey: String,
    pub goeasy_host: String,
    pub goeasy_channel: String,
    pub goeasy_port: u16,
    pub goeasy_headless: bool,
    /// 允许把 GoEasy 广播里带的正文先「秒显」上屏，再由 Gitee 回拉校正。
    /// 关掉则严格以 Gitee 为准（多等一次网络往返，但完全不信任广播内容）。
    pub goeasy_quick_display: bool,
    pub board_autostart: bool,
    /// 云端班级名单（独立私有仓库，按班分片）：创建班级时按需拉取本班数据。
    /// 名单不再编进 exe、不再随发布包分发，本机只保留已创建班级的名单。
    pub class_data_enabled: bool,
    pub class_data_owner: String,
    pub class_data_repo: String,
    /// 只读令牌；留空时回退 gitee_token（便于本机平滑过渡）。
    /// 分发到教室时务必填只读令牌，不要下发可读写令牌。
    pub class_data_token: String,
    /// 仓库内的数据根目录，文件布局 `{dir}/{学校}/{入学年份}/{NN}.json`。
    pub class_data_dir: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn d(s: &str) -> String {
    s.to_string()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            // 全新安装不预置任何班级：由使用老师在设置窗口创建（默认最多 2 个）。
            current_class: String::new(),
            class_list: Vec::new(),
            gitee_tongzhi_dir: d("tongzhi"),
            notice_color: theme::WHITE.to_string(),
            bar_size: d("大"),
            bar_scale: 0.0,
            notice_scale: 0.0,
            notice_offset_x: 0,
            notice_offset_y: 0,
            notice_font_size: d("大"),
            notice_position: d("居中"),
            default_countdown: theme::TIME2MIN,
            bar_top_offset: theme::BAR_OFFSET_DEF,
            bar_left_gap: theme::BAR_GAP_DEF,
            bar_left_x: -1,
            bar_right_x: -1,
            roll_win_size: d("大"),
            countdown_win_size: d("大"),
            roll_scale: 1.0,
            countdown_scale: 1.0,
            roll_seconds: 3.0,
            auto_run: true,
            work_time: d("summer"),
            // v1.0.24 默认不启用「网络U盘/传文件到教室」同步：只有勾选后才轮询
            // Gitee 下载文件；已有 config.json 里显式 true 的用户不受影响
            gitee_enabled: false,
            gitee_owner: d("gzzxgch"),
            gitee_repo: d("kejian"),
            // 凭据不入库：默认从环境变量注入，已有 config.json 的用户完全不受影响
            gitee_token: env_credential("ROLLCALL_GITEE_TOKEN"),
            gitee_random_max: 10,
            log_max_days: 3,
            gitee_download_folder: paths::home().join("wulikejian").to_string_lossy().to_string(),
            update_enabled: true,
            update_branch: d("master"),
            update_exe_path: d("dist/app.exe"),
            scheduled_notices: Vec::new(),
            goeasy_enabled: true,
            goeasy_appkey: env_credential("ROLLCALL_GOEASY_APPKEY"),
            goeasy_host: d("hangzhou.goeasy.io"),
            goeasy_channel: d("gzzx_class_notice_v1"),
            goeasy_port: 18765,
            goeasy_headless: true,
            goeasy_quick_display: true,
            board_autostart: true,
            class_data_enabled: true,
            class_data_owner: d("gzzxgch"),
            class_data_repo: d("classroom-data"),
            class_data_token: env_credential("ROLLCALL_CLASS_DATA_TOKEN"),
            class_data_dir: d("banji"),
            extra: Map::new(),
        }
    }
}

/// 从环境变量读凭据；未设置时返回空，绝不把任何密钥写进源码或 git 历史。
///
/// 首次部署请用 `config.json` 填写真实值，或设置对应环境变量：
/// - `ROLLCALL_GITEE_TOKEN`  Gitee 私人令牌（通知同步 / 自动更新）
/// - `ROLLCALL_GOEASY_APPKEY`  GoEasy Subscribe Key（实时唤醒）
/// - `ROLLCALL_CLASS_DATA_TOKEN`  云端名单仓库的只读令牌（可不下发，留空回退）
pub fn env_credential(key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    std::env::var(key).unwrap_or_default().trim().to_string()
}

impl Config {
    /// 云端名单仓库的有效令牌：优先用独立只读令牌，未配置时回退通知仓库令牌。
    /// 前者只发到各教室，后者只留在管理员的构建/上传机上。
    pub fn effective_class_data_token(&self) -> String {
        let own = self.class_data_token.trim();
        if own.is_empty() {
            self.gitee_token.trim().to_string()
        } else {
            own.to_string()
        }
    }

    /// 读取 + 规范化（等价 Python load_config 的补齐与回退逻辑）。
    pub fn load() -> Config {
        let mut cfg: Config = load_json(&paths::config_file(), Config::default());
        // 中文颜色名 → 色值（与 COLOR_TEXT_MAP 一致）
        if ["白色", "红色", "蓝色", "紫色"].contains(&cfg.notice_color.as_str()) {
            cfg.notice_color = theme::color_text_map(&cfg.notice_color).to_string();
        }
        // 全新安装允许班级列表为空（不再补内置 9 班）。当前班级失效时回退到
        // 第一个班；一个班都没有就保持空，等老师在设置窗口创建。
        let valid: Vec<&str> = cfg.class_list.iter().map(|c| c.file_name.as_str()).collect();
        if !valid.contains(&cfg.current_class.as_str()) {
            cfg.current_class = cfg
                .class_list
                .first()
                .map(|c| c.file_name.clone())
                .unwrap_or_default();
        }
        // GoEasy 订阅 key 空值回填：老机器 config.json 里保留着旧版内置 key，不受影响；
        // 全新安装 / 丢字段的机器这里补上默认值，让 presence 桥接能启动、在线圆点恢复。
        if cfg.goeasy_enabled && cfg.goeasy_appkey.trim().is_empty() {
            cfg.goeasy_appkey = DEFAULT_GOEASY_SUBSCRIBE_KEY.to_string();
        }
        cfg
    }

    pub fn save(&self) -> bool {
        save_json_atomic(&paths::config_file(), self)
    }

    /// 当前班级；找不到返回 None。
    pub fn current(&self) -> Option<&ClassEntry> {
        self.class_list.iter().find(|c| c.file_name == self.current_class)
    }

}

/// 班级显示名的短标签（对应 short_class_label）：
/// "赣州中学2025级9班" → "9班"；"赣州中学2025级09班" → "9班"；
/// 识别不到班号时原样返回。黑横幅退役后主流程不再使用，仅单测保留。
#[allow(dead_code)]
pub fn short_class_label(display_name: &str) -> String {
    let s = display_name.trim();
    if s.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    let Some(pos) = chars.iter().rposition(|&c| c == '班') else {
        return display_name.to_string();
    };
    if pos < 1 {
        return display_name.to_string();
    }
    let mut start = pos;
    while start > 0 && chars[start - 1].is_ascii_digit() {
        start -= 1;
    }
    let num: String = chars[start..pos].iter().collect::<String>().trim_start_matches('0').to_string();
    if num.is_empty() {
        return display_name.to_string();
    }
    format!("{}班", num)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_file_name() {
        assert_eq!(ClassEntry::build_file_name("赣州中学", "2025", "9"), "赣州中学_2025级09班.txt");
        assert_eq!(ClassEntry::build_file_name("赣州中学", "2025", "17"), "赣州中学_2025级17班.txt");
    }

    #[test]
    fn display_name() {
        assert_eq!(ClassEntry::build_display_name("赣州中学", "2025", "9"), "赣州中学2025级9班");
        assert_eq!(ClassEntry::build_display_name("赣州中学", "2025", "17"), "赣州中学2025级17班");
    }

    #[test]
    fn short_label() {
        assert_eq!(short_class_label("赣州中学2025级9班"), "9班");
        assert_eq!(short_class_label("赣州中学2025年9班"), "9班");
        assert_eq!(short_class_label("赣州中学2025级09班"), "9班");
        assert_eq!(short_class_label("赣州中学2025级10班"), "10班");
        assert_eq!(short_class_label("没有班号"), "没有班号");
    }

    #[test]
    fn credentials_are_never_embedded_in_source() {
        assert_eq!(env_credential(""), "");
        assert_eq!(env_credential("ROLLCALL_NO_SUCH_ENV_VAR_9F3A"), "");
        let c = Config::default();
        assert!(c.gitee_token.is_empty());
        assert!(c.goeasy_appkey.is_empty());
    }

    #[test]
    fn subscribe_key_is_public_readonly_and_default_is_valid() {
        // Subscribe Key（BS-）是设计上要下发到客户端的只读 key，可内置回填；
        // 真正的发布 key（BC-）与 Gitee token 不在此列，仍只走环境变量。
        assert!(super::DEFAULT_GOEASY_SUBSCRIBE_KEY.starts_with("BS-"));
        assert_eq!(super::DEFAULT_GOEASY_SUBSCRIBE_KEY.len(), 35); // BS- + 32 位十六进制
        assert!(!super::DEFAULT_GOEASY_SUBSCRIBE_KEY.contains("BC-"));
    }

    #[test]
    fn config_defaults_start_with_no_classes() {
        let c = Config::default();
        // 全新安装（无 config.json）不应预置任何班级 / 学生名单
        assert!(c.current_class.is_empty());
        assert!(c.class_list.is_empty());
        assert!(c.gitee_token.is_empty());
        assert!(c.auto_run);
        assert!(c.current().is_none());
        // v1.0.24 起「网络U盘同步」默认关闭，需在设置里手动勾选开启
        assert!(!c.gitee_enabled);
    }
}
