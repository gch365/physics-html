//! 云端班级名单与课表：创建班级时按班号从 Gitee **私有仓库**拉取本班数据。
//!
//! 安全边界（对应「数据存 Gitee、创建班级再马上拉取」）：
//! - 全校名单不再编进 exe、不再写进 `dist/seed/seed.json`、不再放本机 seed.json。
//!   源码仓库 / kejian 发布通道 / 发布 zip 里都不含任何学生姓名。
//! - 数据放在独立私有仓库 `classroom-data` 的 `banji/{学校}/{入学年份}/{NN}.json`，
//!   一个班一个文件；程序只请求当前班那一份，教室电脑磁盘上只留下本班名单。
//! - 运行时 `config.json` / `schedules/` 仍然会保存本班名单与课表（点名必需），
//!   这是「每台只留本班」而不是「完全不落盘」。
//! - 分发用**只读令牌**，上传用管理员的可读写令牌，两者分离；令牌绝不入库。
//! - 断网 / 令牌失效 / 云端没这一届时创建班级照常成功（建空班级），
//!   联网后点「☁ 从云端补全当前班」重试，或自行粘贴导入。
//!
//! 仓库内容由 `scripts/upload_class_data.py` 从学校 Excel 生成的 seed.json 拆分上传。
//! 程序端只依赖 `GiteeApi::blob_download()`，与通知读取是同一条稳定链路。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// 一节课：weekday 0=周一..4=周五，period 1..15（与 schedule.rs 的坐标一致）。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct SeedSlot {
    pub weekday: usize,
    pub period: usize,
    pub course: String,
}

/// 一个班的名单 + 课表（云端文件的主体，也是写入本机的形态）。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct SeedClass {
    pub students: Vec<String>,
    pub schedule: Vec<SeedSlot>,
}

/// 云端单个班级文件：`banji/{学校}/{入学年份}/{NN}.json`。
/// 元数据用于自校验，防止路径写错时把别班/别校的名单套进来。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct CloudClass {
    pub version: i64,
    pub school: String,
    pub year: String,
    pub class_num: String,
    #[serde(flatten)]
    pub data: SeedClass,
}

/// 班号归一化："09" / 9 / " 9 " → "9"；非数字原样保留。
pub fn normalize_class_num(raw: &str) -> String {
    let t = raw.trim();
    match t.parse::<i64>() {
        Ok(n) => n.to_string(),
        Err(_) => t.to_string(),
    }
}

/// 云端相对路径。班号补零到两位，与上传脚本 `class_file_name()` 保持一致。
/// 例：`cloud_path("banji", "赣州中学", "2025", "9")` →
/// `banji/赣州中学/2025/09.json`。
pub fn cloud_path(dir: &str, school: &str, year: &str, class_num: &str) -> String {
    let n = normalize_class_num(class_num);
    let nn = match n.parse::<i64>() {
        Ok(v) => format!("{v:02}"),
        Err(_) => n,
    };
    let d = dir.trim().trim_matches('/');
    if d.is_empty() {
        format!("{}/{}/{}.json", school.trim(), year.trim(), nn)
    } else {
        format!("{d}/{}/{}/{}.json", school.trim(), year.trim(), nn)
    }
}

/// 把课表坐标转成 `schedule::save_schedule` 需要的映射；越界/空科目直接丢掉，
/// 避免把坏数据写进用户的课表文件。
fn slots_to_map(slots: &[SeedSlot]) -> HashMap<(usize, usize), String> {
    let mut map = HashMap::new();
    for s in slots {
        let course = s.course.trim();
        if s.weekday < 5 && (1..=15).contains(&s.period) && !course.is_empty() {
            map.insert((s.weekday, s.period), course.to_string());
        }
    }
    map
}

impl SeedClass {
    pub fn schedule_map(&self) -> HashMap<(usize, usize), String> {
        slots_to_map(&self.schedule)
    }
}

impl CloudClass {
    /// 学校 / 届 / 班号自校验。字段为空时视为不限（兼容早期上传的文件）。
    pub fn matches(&self, school: &str, year: &str, class_num: &str) -> bool {
        let same_or_empty = |field: &str, want: &str| {
            field.trim().is_empty() || field.trim() == want.trim()
        };
        same_or_empty(&self.school, school)
            && same_or_empty(&self.year, year)
            && (self.class_num.trim().is_empty()
                || normalize_class_num(&self.class_num) == normalize_class_num(class_num))
    }
}

/// 从私有仓库拉取**一个班**的名单与课表。
/// 任何失败都返回可读的 Err，由设置窗口决定「建空班级 + 可重试」，绝不 panic。
pub fn fetch_class(
    cfg: &crate::config::Config,
    school: &str,
    year: &str,
    class_num: &str,
) -> Result<CloudClass, String> {
    if !cfg.class_data_enabled {
        return Err("云端名单已关闭（可在「Web 与仓库」页开启）".into());
    }
    if school.trim().is_empty() || year.trim().is_empty() || class_num.trim().is_empty() {
        return Err("学校 / 入学年份 / 班号 不完整，无法定位云端文件".into());
    }
    let token = cfg.effective_class_data_token();
    if token.is_empty() {
        return Err("未配置 Gitee 令牌：请在「Web 与仓库 → 云端班级名单」填只读令牌".into());
    }
    let path = cloud_path(&cfg.class_data_dir, school, year, class_num);
    let api = crate::gitee::GiteeApi::new(&cfg.class_data_owner, &cfg.class_data_repo, &token);
    let bytes = api
        .blob_download(&path, 12)
        .map_err(|e| format!("拉取 {path} 失败：{e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "云端名单不是 UTF-8 文本".to_string())?;
    let class: CloudClass =
        serde_json::from_str(&text).map_err(|e| format!("云端名单格式错误：{e}"))?;
    if !class.matches(school, year, class_num) {
        return Err(format!(
            "云端文件与班级不匹配（文件写的是 {} {} 届 {} 班）",
            class.school, class.year, class.class_num
        ));
    }
    if class.data.students.is_empty() && class.data.schedule.is_empty() {
        return Err("云端文件里没有名单和课表".into());
    }
    Ok(class)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "version": 1,
      "school": "赣州中学",
      "year": "2025",
      "class_num": "9",
      "students": ["张三", "李四"],
      "schedule": [
        {"weekday": 0, "period": 1, "course": "数学"},
        {"weekday": 4, "period": 12, "course": "语晚"},
        {"weekday": 5, "period": 2, "course": "周日不应导入"},
        {"weekday": 1, "period": 0, "course": "早读不应导入"},
        {"weekday": 2, "period": 3, "course": "  "}
      ]
    }"#;

    #[test]
    fn cloud_file_parses_flattened_students_and_schedule() {
        let c: CloudClass = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(c.school, "赣州中学");
        assert_eq!(c.year, "2025");
        assert_eq!(c.class_num, "9");
        assert_eq!(c.data.students, ["张三", "李四"]);
        assert_eq!(c.data.schedule.len(), 5);
    }

    #[test]
    fn schedule_map_drops_out_of_range_slots() {
        let c: CloudClass = serde_json::from_str(SAMPLE).unwrap();
        let map = c.data.schedule_map();
        assert_eq!(map.len(), 2, "只保留周一~周五、节次 1..15、非空科目: {map:?}");
        assert_eq!(map.get(&(0, 1)).map(String::as_str), Some("数学"));
        assert_eq!(map.get(&(4, 12)).map(String::as_str), Some("语晚"));
    }

    #[test]
    fn cloud_path_pads_class_number_and_is_url_safe_shape() {
        assert_eq!(
            cloud_path("banji", "赣州中学", "2025", "9"),
            "banji/赣州中学/2025/09.json"
        );
        assert_eq!(
            cloud_path("banji/", "赣州中学", "2025", "09"),
            "banji/赣州中学/2025/09.json"
        );
        assert_eq!(
            cloud_path("", "赣州中学", "2025", "31"),
            "赣州中学/2025/31.json"
        );
        assert_eq!(
            cloud_path("banji", "赣州中学", "2025", "实验班"),
            "banji/赣州中学/2025/实验班.json"
        );
    }

    #[test]
    fn class_num_normalization() {
        assert_eq!(normalize_class_num("09"), "9");
        assert_eq!(normalize_class_num(" 31 "), "31");
        assert_eq!(normalize_class_num("实验班"), "实验班");
    }

    #[test]
    fn matches_only_its_own_school_year_and_class() {
        let c: CloudClass = serde_json::from_str(SAMPLE).unwrap();
        assert!(c.matches("赣州中学", "2025", "09"));
        assert!(!c.matches("赣州中学", "2026", "9"), "别的届不能套用");
        assert!(!c.matches("别的中学", "2025", "9"), "外校不能套用");
        assert!(!c.matches("赣州中学", "2025", "10"), "别班不能套用");
        // 早期文件没写元数据时不限制
        let loose: CloudClass = serde_json::from_str(r#"{"students":["甲"]}"#).unwrap();
        assert!(loose.matches("任意学校", "2030", "7"));
    }

    #[test]
    fn broken_or_missing_payload_is_not_fatal_to_parse() {
        let c: CloudClass = serde_json::from_str("not json").unwrap_or_default();
        assert!(c.data.students.is_empty());
        assert!(c.data.schedule.is_empty());
        let c: CloudClass = serde_json::from_str("{}").unwrap();
        assert!(c.data.students.is_empty());
    }

    /// 真实网络冒烟：需要本机 config.json 里有能访问私有仓库的令牌，
    /// 平时 `#[ignore]`。手动执行：
    /// `cargo test --release cloud_smoke -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn cloud_smoke_fetches_real_class_9() {
        let cfg = crate::config::Config::load();
        let c = fetch_class(&cfg, "赣州中学", "2025", "9").expect("云端拉取 9 班失败");
        assert!(c.data.students.len() >= 40, "9 班名单不应为空");
        assert!(!c.data.schedule_map().is_empty(), "9 班课表不应为空");
        println!(
            "云端 9 班：{} 人 / {} 节课",
            c.data.students.len(),
            c.data.schedule_map().len()
        );
    }
}
