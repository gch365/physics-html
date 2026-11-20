//! 作息时间表与上课状态判断（对应 Python 顶部的时间常量与函数）。

use chrono::{Datelike, Local, NaiveTime, Timelike};

pub const SUMMER_TIME_RAW: [(&str, &str); 10] = [
    ("08:00", "08:40"),
    ("08:45", "09:25"),
    ("09:55", "10:35"),
    ("10:45", "11:25"),
    ("11:35", "12:10"),
    ("14:10", "14:50"),
    ("15:00", "15:40"),
    ("15:50", "16:30"),
    ("16:35", "17:15"),
    ("18:30", "21:40"), // 晚自习（学生自主学习）
];

pub const WINTER_TIME_RAW: [(&str, &str); 10] = [
    ("08:00", "08:40"),
    ("08:45", "09:25"),
    ("09:55", "10:35"),
    ("10:45", "11:25"),
    ("11:35", "12:10"),
    ("14:00", "14:40"),
    ("14:50", "15:30"),
    ("15:40", "16:20"),
    ("16:30", "17:05"),
    ("18:30", "21:40"), // 晚自习（学生自主学习）
];

/// "HH:MM" → NaiveTime；失败回落 00:00。
pub fn hm(s: &str) -> NaiveTime {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() == 2 {
        if let (Ok(h), Ok(m)) = (parts[0].trim().parse::<u32>(), parts[1].trim().parse::<u32>()) {
            if let Some(t) = NaiveTime::from_hms_opt(h.min(23), m.min(59), 0) {
                return t;
            }
        }
    }
    NaiveTime::from_hms_opt(0, 0, 0).unwrap()
}


// ====================== 运行时作息表（设置页可编辑，override 内置常量） ======================

pub type TimeRow = (String, String);

#[derive(Default, Clone, Debug)]
pub struct TimeTables {
    pub summer: Vec<TimeRow>,
    pub winter: Vec<TimeRow>,
}

static TIME_OVERRIDE: std::sync::RwLock<Option<TimeTables>> = std::sync::RwLock::new(None);

fn time_override_path(mode: &str) -> std::path::PathBuf {
    crate::paths::schedule_dir().join(format!("time_{}.json", mode))
}

/// 从 schedules/time_{summer|winter}.json 读用户自定义作息（设置页写入）。
pub fn load_time_overrides() {
    let mut t = TimeTables::default();
    for mode in ["summer", "winter"] {
        let rows = read_time_file(&time_override_path(mode));
        if mode == "summer" {
            t.summer = rows;
        } else {
            t.winter = rows;
        }
    }
    if let Ok(mut g) = TIME_OVERRIDE.write() {
        *g = Some(t);
    }
}

fn read_time_file(path: &std::path::Path) -> Vec<TimeRow> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
    let mut out = Vec::new();
    if let Some(items) = v.get("periods").and_then(|x| x.as_array()) {
        for it in items {
            let s = it.get("start").and_then(|x| x.as_str()).unwrap_or("");
            let e = it.get("end").and_then(|x| x.as_str()).unwrap_or("");
            if !s.is_empty() && !e.is_empty() {
                out.push((s.to_string(), e.to_string()));
            }
        }
    }
    out
}

/// 保存某模式的作息表到 override 文件；空表 = 删除文件恢复内置。
pub fn save_time_table(mode: &str, rows: &[TimeRow]) -> std::io::Result<()> {
    let path = time_override_path(mode);
    if rows.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else {
        let data = serde_json::json!({
            "mode": mode,
            "periods": rows.iter().map(|(s, e)| serde_json::json!({"start": s, "end": e})).collect::<Vec<_>>(),
        });
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&data).unwrap_or_default())?;
        std::fs::rename(&tmp, &path)?;
    }
    load_time_overrides();
    Ok(())
}

/// 读当前生效的作息表（override 优先，否则内置常量）。
pub fn time_rows(work_mode: &str) -> Vec<TimeRow> {
    let builtin: Vec<TimeRow> = current_builtin(work_mode)
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    if let Ok(g) = TIME_OVERRIDE.read() {
        if let Some(t) = g.as_ref() {
            let rows = if work_mode == "winter" { &t.winter } else { &t.summer };
            if !rows.is_empty() {
                return rows.clone();
            }
        }
    }
    builtin
}

pub fn current_builtin(work_mode: &str) -> &'static [(&'static str, &'static str)] {
    if work_mode == "winter" {
        &WINTER_TIME_RAW
    } else {
        &SUMMER_TIME_RAW
    }
}

/// 设置进程用：读某模式当前生效的作息表（含 override）。
pub fn time_table_for_edit(work_mode: &str) -> Vec<TimeRow> {
    time_rows(work_mode)
}

/// 供测试注入时刻的状态计算。weekday(): 周一=0 … 周日=6。
/// 仅周日算周末；周六为自习日（学校按平时作息自主学习，通知按平时逻辑）。
pub fn is_weekend_at(now: &chrono::DateTime<Local>) -> bool {
    now.weekday().num_days_from_monday() == 6
}

/// 周六自习日。
pub fn is_self_study_day_at(now: &chrono::DateTime<Local>) -> bool {
    now.weekday().num_days_from_monday() == 5
}

/// 每天 17:15 之后通知默认弹出（不管晚自习是否上课）。
pub fn force_notice_show_at(now: &chrono::DateTime<Local>) -> bool {
    now.time() >= hm("17:15")
}

pub fn force_notice_show_now() -> bool {
    force_notice_show_at(&Local::now())
}

/// 周六自习的节次显示名（课表不管是否导入，周六 9 节一律显示自习）。
pub const SELF_STUDY_COURSE: &str = "自习";

pub fn is_class_time_at(work_mode: &str, now: &chrono::DateTime<Local>) -> bool {
    if is_weekend_at(now) {
        return false;
    }
    let now_t = now.time();
    time_rows(work_mode)
        .iter()
        .any(|(s, e)| hm(s) <= now_t && now_t < hm(e))
}

/// 当前课节 (start,end)；不在课节内返回 None。
pub fn current_class_period_at(
    work_mode: &str,
    now: &chrono::DateTime<Local>,
) -> Option<(String, String)> {
    if is_weekend_at(now) {
        return None;
    }
    let now_t = now.time();
    let rows = time_rows(work_mode);
    rows.iter()
        .find(|(s, e)| hm(s) <= now_t && now_t < hm(e))
        .cloned()
}

/// 某节课开始前 30 秒内（上课预备期：横幅自动收起、半屏不弹）。
pub fn is_within_pre_close_at(work_mode: &str, now: &chrono::DateTime<Local>) -> bool {
    if is_weekend_at(now) {
        return false;
    }
    for (start_str, _) in time_rows(work_mode) {
        let t = hm(&start_str);
        let start_dt = now
            .date_naive()
            .and_hms_opt(t.hour(), t.minute(), 0)
            .unwrap()
            .and_local_timezone(Local)
            .single();
        if let Some(start_dt) = start_dt {
            let diff = (start_dt - *now).num_seconds();
            if 0 < diff && diff <= 30 {
                return true;
            }
        }
    }
    false
}

pub fn is_weekend() -> bool {
    is_weekend_at(&Local::now())
}

pub fn is_class_time(work_mode: &str) -> bool {
    is_class_time_at(work_mode, &Local::now())
}

pub fn current_class_period(work_mode: &str) -> Option<(String, String)> {
    current_class_period_at(work_mode, &Local::now())
}

pub fn is_within_pre_close(work_mode: &str) -> bool {
    is_within_pre_close_at(work_mode, &Local::now())
}

/// 上课期间距本节课下课 ≤ 120 秒（下课前 2 分钟预拉取窗口）。
/// 用于「上课期间不轮询，只在下课前 2 分钟拉取一次为下课显示做准备」：
/// 每节课只在该窗口内触发一次通知预拉取，横幅内容下课铃一响即可上屏。
pub fn is_within_preload_window_at(work_mode: &str, now: &chrono::DateTime<Local>) -> bool {
    if is_weekend_at(now) {
        return false;
    }
    let now_t = now.time();
    if let Some((_, end_str)) = time_rows(work_mode)
        .iter()
        .find(|(s, e)| hm(s) <= now_t && now_t < hm(e))
    {
        let diff = (hm(end_str) - now_t).num_seconds().max(0);
        return diff <= 120;
    }
    false
}

pub fn is_within_preload_window(work_mode: &str) -> bool {
    is_within_preload_window_at(work_mode, &Local::now())
}

// ====================== 课表 ======================

/// 从课表 JSON 还原 (weekday, period) → course。
/// 抽成纯函数后可在单测里核对「导入写盘 → 主程序读回」格式是否一致。
pub fn parse_schedule_json(text: &str) -> std::collections::HashMap<(usize, usize), String> {
    let mut map = std::collections::HashMap::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return map;
    };
    if let Some(items) = v.get("schedule").and_then(|x| x.as_array()) {
        for it in items {
            let wd = it.get("weekday").and_then(|x| x.as_i64()).unwrap_or(-1) as usize;
            let pe = it.get("period").and_then(|x| x.as_i64()).unwrap_or(-1) as usize;
            let course = it
                .get("course")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if !course.is_empty() {
                map.insert((wd, pe), course);
            }
        }
    }
    map
}

/// 课表 JSON 序列化（与 Python save_schedule 完全一致）。
pub fn schedule_json(
    class_id: &str,
    schedule: &std::collections::HashMap<(usize, usize), String>,
) -> String {
    let mut items: Vec<serde_json::Value> = Vec::new();
    for (w, p) in schedule.keys() {
        if let Some(c) = schedule.get(&(*w, *p)) {
            if !c.is_empty() {
                items.push(serde_json::json!({"weekday": w, "period": p, "course": c}));
            }
        }
    }
    items.sort_by_key(|it| {
        (
            it.get("weekday").and_then(|x| x.as_i64()).unwrap_or(0),
            it.get("period").and_then(|x| x.as_i64()).unwrap_or(0),
        )
    });
    let data = serde_json::json!({"class": class_id, "schedule": items});
    serde_json::to_string_pretty(&data).unwrap_or_default()
}

/// 读 schedules/class_{class_id}.json → (weekday, period) → course。
pub fn load_schedule(class_id: &str) -> std::collections::HashMap<(usize, usize), String> {
    if class_id.is_empty() {
        return std::collections::HashMap::new();
    }
    let path = crate::paths::schedule_dir().join(format!("class_{}.json", class_id));
    let Ok(text) = std::fs::read_to_string(&path) else {
        return std::collections::HashMap::new();
    };
    parse_schedule_json(&text)
}

/// 写 schedules/class_{class_id}.json（原子写，格式与 Python save_schedule 一致）。
pub fn save_schedule(
    class_id: &str,
    schedule: &std::collections::HashMap<(usize, usize), String>,
) -> std::io::Result<()> {
    let path = crate::paths::schedule_dir().join(format!("class_{}.json", class_id));
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let data = schedule_json(class_id, schedule);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// 清除班级课表（对应删除 class_{id}.json）。
pub fn clear_schedule(class_id: &str) {
    let path = crate::paths::schedule_dir().join(format!("class_{}.json", class_id));
    let _ = std::fs::remove_file(path);
}

/// "下一节：科目 (HH:MM-HH:MM)"；全天课上完 / 无课表返回 None。
pub fn get_next_lesson(
    schedule: &std::collections::HashMap<(usize, usize), String>,
    work_mode: &str,
) -> Option<String> {
    get_next_lesson_at(schedule, work_mode, &Local::now())
}

pub fn get_next_lesson_at(
    schedule: &std::collections::HashMap<(usize, usize), String>,
    work_mode: &str,
    now: &chrono::DateTime<Local>,
) -> Option<String> {
    let weekday = now.weekday().num_days_from_monday() as usize;
    let now_t = now.time();
    let rows = time_rows(work_mode);
    let parsed: Vec<(NaiveTime, NaiveTime)> =
        rows.iter().map(|(a, b)| (hm(a), hm(b))).collect();

    let mut next_idx = None;
    for (idx, (s, e)) in parsed.iter().enumerate() {
        if now_t < *s {
            next_idx = Some(idx);
            break;
        } else if *s <= now_t && now_t < *e {
            next_idx = Some(idx + 1);
            break;
        }
    }
    let next_idx = next_idx?;
    if next_idx >= parsed.len() {
        return None;
    }
    // 课表节次 1-based；周六自习日：不管课表是否导入，节次一律显示「自习」
    let course = if is_self_study_day_at(now) {
        SELF_STUDY_COURSE.to_string()
    } else {
        schedule.get(&(weekday, next_idx + 1))?.clone()
    };
    let (start_str, end_str) = &rows[next_idx];
    Some(format!("下一节：{} ({}-{})", course, start_str, end_str))
}

/// 半屏弹屏 + 语音播报的触发关键词。
/// 「重要通知」本身含「通知」，这里单独列出只为让配置意图一眼可见；
/// 「重要消息」保留兼容旧通知文本。
pub const POPUP_KEYWORDS: [&str; 3] = ["通知", "重要通知", "重要消息"];

/// 纯函数：通知正文是否命中弹屏关键词。
pub fn has_popup_keyword(notice_text: &str) -> bool {
    let text = notice_text.trim();
    !text.is_empty() && POPUP_KEYWORDS.iter().any(|k| text.contains(k))
}

/// 是否该弹「半屏居中通知卡」并语音播报。
///
/// 用户约定：只在**课间 / 放学后**弹。上课、周休、课前 30 秒（横幅收回窗口）
/// 一律不弹；17:15 之后按既有「放学后强制显示」规则放开（晚自习也算）。
pub fn should_popup_notice(notice_text: &str, work_mode: &str) -> bool {
    if !has_popup_keyword(notice_text) {
        return false;
    }
    if force_notice_show_now() {
        return true;
    }
    if is_weekend() || is_class_time(work_mode) || is_within_pre_close(work_mode) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn imported_schedule_json_round_trips_back_to_the_same_grid() {
        // 设置窗口导入后写盘用 schedule_json，主程序读回用 parse_schedule_json，
        // 两者必须严格一致，否则「导入成功但界面不显示」。
        let mut m: std::collections::HashMap<(usize, usize), String> =
            std::collections::HashMap::new();
        m.insert((0, 1), "语文".into());
        m.insert((1, 1), "数学".into());
        m.insert((0, 2), "英语".into());
        m.insert((4, 9), "晚自习".into());
        let text = schedule_json("赣州中学_2025级17班.txt", &m);
        assert!(text.contains("\"class\""));
        assert_eq!(parse_schedule_json(&text), m);
        assert!(parse_schedule_json("not json").is_empty());
        assert!(parse_schedule_json("{\"schedule\":[]}").is_empty());
    }

    fn at(y: i32, m: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<Local> {
        Local
            .with_ymd_and_hms(y, m, d, h, mi, 0)
            .single()
            .unwrap()
    }

    // 2026-11-16 是周一；14 是周六、15 是周日
    #[test]
    fn weekend() {
        assert!(!is_weekend_at(&at(2026, 11, 14, 10, 0))); // 周六 = 自习日，非周末
        assert!(is_weekend_at(&at(2026, 11, 15, 10, 0))); // 周日
        assert!(!is_weekend_at(&at(2026, 11, 16, 10, 0))); // 周一
    }

    #[test]
    fn self_study_day() {
        assert!(is_self_study_day_at(&at(2026, 11, 14, 10, 0))); // 周六
        assert!(!is_self_study_day_at(&at(2026, 11, 15, 10, 0))); // 周日
        // 周六按平时作息上课（自习）
        assert!(is_class_time_at("summer", &at(2026, 11, 14, 8, 10)));
        assert!(!is_class_time_at("summer", &at(2026, 11, 15, 8, 10))); // 周日全休
    }

    #[test]
    fn evening_study() {
        // 晚自习 18:30-21:40 为第 10 节
        assert!(is_class_time_at("summer", &at(2026, 11, 16, 19, 0)));
        assert_eq!(
            current_class_period_at("summer", &at(2026, 11, 16, 19, 0)),
            Some(("18:30".to_string(), "21:40".to_string()))
        );
        assert!(!is_class_time_at("summer", &at(2026, 11, 16, 17, 30))); // 晚饭课间
        assert!(!is_class_time_at("summer", &at(2026, 11, 16, 21, 41))); // 晚自习后
    }

    #[test]
    fn force_notice_show() {
        assert!(!force_notice_show_at(&at(2026, 11, 16, 17, 14)));
        assert!(force_notice_show_at(&at(2026, 11, 16, 17, 15)));
        assert!(force_notice_show_at(&at(2026, 11, 16, 20, 0))); // 晚自习中也强制显示
        assert!(force_notice_show_at(&at(2026, 11, 15, 18, 0))); // 周日晚间
    }

    #[test]
    fn class_time() {
        assert!(is_class_time_at("summer", &at(2026, 11, 16, 8, 10)));
        assert!(!is_class_time_at("summer", &at(2026, 11, 16, 9, 30))); // 大课间
        assert!(!is_class_time_at("summer", &at(2026, 11, 16, 13, 0))); // 午休
        assert!(!is_class_time_at("summer", &at(2026, 11, 16, 7, 0))); // 早晨
    }

    #[test]
    fn period_lookup() {
        assert_eq!(
            current_class_period_at("summer", &at(2026, 11, 16, 8, 10)),
            Some(("08:00".to_string(), "08:40".to_string()))
        );
        assert_eq!(current_class_period_at("summer", &at(2026, 11, 16, 9, 30)), None);
    }

    #[test]
    fn saturday_next_lesson_self_study() {
        // 周六（2026-11-14）09:30 课间：不管课表是否导入，节次一律「自习」
        let empty = std::collections::HashMap::new();
        let s = get_next_lesson_at(&empty, "summer", &at(2026, 11, 14, 9, 30));
        assert!(s.is_some());
        assert!(s.unwrap().contains(SELF_STUDY_COURSE));
        // 周六 18:00（第 10 节晚自习前）也显示自习
        let s2 = get_next_lesson_at(&empty, "summer", &at(2026, 11, 14, 18, 0));
        assert!(s2.unwrap().contains(SELF_STUDY_COURSE));
        // 周日无课表：仍返回 None（周末）
        assert!(get_next_lesson_at(&empty, "summer", &at(2026, 11, 15, 9, 30)).is_none());
    }

    #[test]
    fn pre_close() {
        // 09:55:20 距 09:55 上课 -20 秒 → 已上课，不算预备期
        let t0 = Local
            .with_ymd_and_hms(2026, 11, 16, 9, 55, 20)
            .single()
            .unwrap();
        assert!(!is_within_pre_close_at("summer", &t0));
        // 09:54:40 距 09:55 上课 20 秒 → 预备期
        let t = Local
            .with_ymd_and_hms(2026, 11, 16, 9, 54, 40)
            .single()
            .unwrap();
        assert!(is_within_pre_close_at("summer", &t));
        // 09:54:10 距上课 50 秒 → 已超出 30 秒窗口
        let t2 = Local
            .with_ymd_and_hms(2026, 11, 16, 9, 54, 10)
            .single()
            .unwrap();
        assert!(!is_within_pre_close_at("summer", &t2));
    }

    #[test]
    fn hm_parse() {
        assert_eq!(hm("08:00"), hm("8:0"));
        assert_eq!(hm("bad"), NaiveTime::from_hms_opt(0, 0, 0).unwrap());
    }

    #[test]
    fn popup_keywords_cover_notice_and_important_notice() {
        assert!(has_popup_keyword("重要通知：测试测试2"));
        assert!(has_popup_keyword("通知：下午集合"));
        assert!(has_popup_keyword("重要消息：停课"));
        assert!(!has_popup_keyword("语文作业：第 3 题"));
        assert!(!has_popup_keyword("   "));
        assert!(!has_popup_keyword(""));
    }
}
