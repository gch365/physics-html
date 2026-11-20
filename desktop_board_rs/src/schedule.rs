//! 作息表抽取与状态计算。
//!
//! 作息不复制一份数据，直接从教室管理程序源码 `main202609245.py` 里抽取
//! `SUMMER_TIME_RAW` / `WINTER_TIME_RAW`（Python 版用 ast，这里用正则做等价抽取），
//! 抽取失败则用内置兜底表，保证大屏在任何机器上都能起来。

use chrono::{DateTime, Datelike, Local, Timelike};
use std::collections::HashMap;

use crate::paths;

pub type RawPeriods = Vec<(String, String)>;

/// 主程序不可用时（拷到别的电脑）的兜底作息。
pub const FALLBACK_SUMMER: [(&str, &str); 9] = [
    ("08:00", "08:40"),
    ("08:45", "09:25"),
    ("09:55", "10:35"),
    ("10:45", "11:25"),
    ("11:35", "12:10"),
    ("14:10", "14:50"),
    ("15:00", "15:40"),
    ("15:50", "16:30"),
    ("16:35", "17:15"),
];

pub const FALLBACK_WINTER: [(&str, &str); 9] = [
    ("08:00", "08:40"),
    ("08:45", "09:25"),
    ("09:55", "10:35"),
    ("10:45", "11:25"),
    ("11:35", "12:10"),
    ("14:00", "14:40"),
    ("14:50", "15:30"),
    ("15:40", "16:20"),
    ("16:30", "17:05"),
];

#[derive(Clone, Debug, Default)]
pub struct Schedule {
    pub summer: RawPeriods,
    pub winter: RawPeriods,
    /// 作息是从主程序抽到的（true）还是兜底表（false），仅用于自检打印
    pub from_source: bool,
}

impl Schedule {
    pub fn raw(&self, work_mode: &str) -> &RawPeriods {
        if work_mode == "winter" {
            &self.winter
        } else {
            &self.summer
        }
    }
}

/// 当前是否正处在上课时段内（课间、课前、放学后、周末都返回 false）。
///
/// 直接按区间判断，不依赖 `Status::state` 的中文文案，改文案也不会影响这里的逻辑。
pub fn in_class_now(work_mode: &str) -> bool {
    let sched = load();
    in_class_at(&sched, work_mode, Local::now())
}

/// `in_class_now` 的纯函数版，方便单测。
pub fn in_class_at(sched: &Schedule, work_mode: &str, now: DateTime<Local>) -> bool {
    let cur = now.hour() as i32 * 3600 + now.minute() as i32 * 60 + now.second() as i32;
    sched
        .raw(work_mode)
        .iter()
        .any(|(a, b)| cur >= secs(a) && cur < secs(b))
}

/// 是否「该弹通知横幅」的时机：工作日且不在上课时段（课间 / 午休 / 放学后）。
///
/// 周末不弹——教室里没人，弹了也只是白闪一下，通知照样进作业卡。
pub fn is_break_now(work_mode: &str) -> bool {
    let sched = load();
    is_break_at(&sched, work_mode, Local::now())
}

/// `is_break_now` 的纯函数版，方便单测。
pub fn is_break_at(sched: &Schedule, work_mode: &str, now: DateTime<Local>) -> bool {
    now.weekday().num_days_from_monday() < 5 && !in_class_at(sched, work_mode, now)
}

/// 今天是不是上学日（周一~周五）。周末收到的通知不攒着等补弹。
pub fn is_school_day_now() -> bool {
    is_school_day_at(Local::now())
}

/// `is_school_day_now` 的纯函数版。
pub fn is_school_day_at(now: DateTime<Local>) -> bool {
    now.weekday().num_days_from_monday() < 5
}

/// 距离下一节课开始还有多少秒（当天）；已经放学 / 没有下一节返回 None。
pub fn secs_until_next_class(work_mode: &str) -> Option<i32> {
    let sched = load();
    secs_until_next_class_at(&sched, work_mode, Local::now())
}

/// `secs_until_next_class` 的纯函数版，方便单测。
pub fn secs_until_next_class_at(
    sched: &Schedule,
    work_mode: &str,
    now: DateTime<Local>,
) -> Option<i32> {
    let cur = now.hour() as i32 * 3600 + now.minute() as i32 * 60 + now.second() as i32;
    sched
        .raw(work_mode)
        .iter()
        .map(|(a, _)| secs(a))
        .filter(|s| *s > cur)
        .min()
        .map(|s| s - cur)
}

fn fallback_schedule() -> Schedule {
    Schedule {
        summer: FALLBACK_SUMMER
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
        winter: FALLBACK_WINTER
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
        from_source: false,
    }
}

/// 从一段 Python 源码里抽出某个列表常量中的所有 "HH:MM" 并按对分组。
fn extract_pairs(src: &str, var: &str) -> Option<RawPeriods> {
    let pattern = format!(r#"(?s){}\s*=\s*\[(.*?)\]"#, regex::escape(var));
    let re = regex::Regex::new(&pattern).ok()?;
    let block = re.captures(src)?.get(1)?.as_str();
    let time_re = regex::Regex::new(r#""(\d{1,2}:\d{2})""#).ok()?;
    let times: Vec<String> = time_re
        .captures_iter(block)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect();
    if times.len() < 2 {
        return None;
    }
    let mut out = Vec::new();
    let mut it = times.chunks_exact(2);
    for pair in &mut it {
        out.push((pair[0].clone(), pair[1].clone()));
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 读取作息：优先主程序源码，失败退兜底表。
pub fn load() -> Schedule {
    let src = std::fs::read_to_string(paths::main_py()).ok();
    let Some(src) = src else {
        return fallback_schedule();
    };
    match (
        extract_pairs(&src, "SUMMER_TIME_RAW"),
        extract_pairs(&src, "WINTER_TIME_RAW"),
    ) {
        (Some(summer), Some(winter)) => Schedule {
            summer,
            winter,
            from_source: true,
        },
        _ => fallback_schedule(),
    }
}

/// 当前状态 / 时间段 / 倒计时 / 下节课。
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub state: String,
    pub range_text: String,
    pub cd_label: String,
    pub cd_text: String,
    pub next_text: String,
}

fn secs(hhmm: &str) -> i32 {
    let mut it = hhmm.split(':');
    let h: i32 = it.next().unwrap_or("0").trim().parse().unwrap_or(0);
    let m: i32 = it.next().unwrap_or("0").trim().parse().unwrap_or(0);
    h * 3600 + m * 60
}

/// 倒计时文案：小时级只说「X 小时 Y 分钟」，分钟级带秒。
fn fmt_duration(seconds: i32) -> String {
    let s = seconds.max(0);
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{} 小时 {} 分钟", h, m)
    } else if m > 0 {
        if sec > 0 {
            format!("{} 分钟 {} 秒", m, sec)
        } else {
            format!("{} 分钟", m)
        }
    } else {
        format!("{} 秒", sec)
    }
}

/// 按作息表算当前状态（纯函数，便于单测）。
pub fn compute_status(
    schedule: &Schedule,
    work_mode: &str,
    courses: &HashMap<(i32, i32), String>,
    now: DateTime<Local>,
) -> Status {
    let raw = schedule.raw(work_mode);
    let weekday = now.weekday().num_days_from_monday() as i32;
    let cur = now.hour() as i32 * 3600 + now.minute() as i32 * 60 + now.second() as i32;

    let starts: Vec<i32> = raw.iter().map(|(a, _)| secs(a)).collect();
    let ends: Vec<i32> = raw.iter().map(|(_, b)| secs(b)).collect();

    let next_course = |period: i32, start: &str| -> String {
        let course = courses.get(&(weekday, period)).cloned().unwrap_or_default();
        if course.is_empty() {
            format!("下节课：{} 开始", start)
        } else {
            format!("下节课：{} {} 开始", course, start)
        }
    };

    if starts.is_empty() {
        return Status {
            state: "作息未配置".into(),
            cd_text: "--".into(),
            ..Default::default()
        };
    }

    if weekday >= 5 {
        // 周末中间区域不再显示「周末时光 / 周末愉快」等文案（按需求全删），
        // 只保留右侧今日作业与值日。
        return Status {
            state: String::new(),
            range_text: String::new(),
            cd_label: String::new(),
            cd_text: String::new(),
            next_text: String::new(),
        };
    }
    if cur < starts[0] {
        return Status {
            state: "课前准备".into(),
            range_text: format!("现在 - {}", raw[0].0),
            cd_label: "距离第一节课还有".into(),
            cd_text: fmt_duration(starts[0] - cur),
            next_text: next_course(1, &raw[0].0),
        };
    }
    if cur >= *ends.last().unwrap() {
        let last = raw.last().unwrap();
        return Status {
            state: "放学啦".into(),
            range_text: format!("{} 放学", last.1),
            cd_label: String::new(),
            cd_text: "今天辛苦了".into(),
            next_text: "明天继续加油".into(),
        };
    }

    // 上课中
    if let Some(idx) = (0..raw.len()).find(|&i| starts[i] <= cur && cur < ends[i]) {
        let nxt = idx + 1;
        let ntext = if nxt < raw.len() {
            next_course(nxt as i32 + 1, &raw[nxt].0)
        } else {
            "这是今天最后一节课".to_string()
        };
        return Status {
            state: "上课中".into(),
            range_text: format!("{} - {} · 第 {} 节", raw[idx].0, raw[idx].1, idx + 1),
            cd_label: "距离下课还有".into(),
            cd_text: fmt_duration(ends[idx] - cur),
            next_text: ntext,
        };
    }

    // 课间 / 午休
    let nxt = starts
        .iter()
        .position(|&s| s > cur)
        .unwrap_or(raw.len() - 1);
    let prev_end = if nxt > 0 { ends[nxt - 1] } else { starts[0] };
    let gap = starts[nxt] - prev_end;
    let state = if gap >= 40 * 60 {
        "午休"
    } else {
        "课间休息"
    };
    Status {
        state: state.into(),
        range_text: if nxt > 0 {
            format!("{} - {}", raw[nxt - 1].1, raw[nxt].0)
        } else {
            String::new()
        },
        cd_label: "距离下节课还有".into(),
        cd_text: fmt_duration(starts[nxt] - cur),
        next_text: next_course(nxt as i32 + 1, &raw[nxt].0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32, s: u32) -> DateTime<Local> {
        // 2026-11-16 是星期一
        Local
            .with_ymd_and_hms(2026, 11, 16, h, m, s)
            .single()
            .unwrap()
    }

    fn sched() -> Schedule {
        Schedule {
            summer: FALLBACK_SUMMER
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            winter: FALLBACK_WINTER
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            from_source: false,
        }
    }

    #[test]
    fn before_first_class() {
        let st = compute_status(&sched(), "summer", &HashMap::new(), at(7, 30, 0));
        assert_eq!(st.state, "课前准备");
        assert_eq!(st.cd_text, "30 分钟");
    }

    #[test]
    fn during_class() {
        let st = compute_status(&sched(), "summer", &HashMap::new(), at(8, 10, 30));
        assert_eq!(st.state, "上课中");
        assert!(st.range_text.contains("第 1 节"));
        assert_eq!(st.cd_text, "29 分钟 30 秒");
        assert_eq!(st.next_text, "下节课：08:45 开始");
    }

    #[test]
    fn break_popup_only_on_weekday_breaks() {
        let s = sched();
        // 工作日课间：弹
        assert!(is_break_at(&s, "summer", at(9, 40, 0)));
        // 工作日上课中：不弹
        assert!(!is_break_at(&s, "summer", at(8, 10, 0)));
        // 周末：不弹（教室里没人）
        let sun = Local.with_ymd_and_hms(2026, 11, 15, 9, 40, 0).single().unwrap();
        assert!(!is_break_at(&s, "summer", sun));
        let sat = Local.with_ymd_and_hms(2026, 11, 14, 15, 0, 0).single().unwrap();
        assert!(!is_break_at(&s, "summer", sat));
    }

    #[test]
    fn next_class_gap() {
        let s = sched();
        // 09:40 课间，下一节 09:55 → 900 秒
        assert_eq!(secs_until_next_class_at(&s, "summer", at(9, 40, 0)), Some(900));
        // 08:10 上课中，下一节 08:45 → 35 分钟
        assert_eq!(
            secs_until_next_class_at(&s, "summer", at(8, 10, 0)),
            Some(35 * 60)
        );
        // 放学后没有下一节
        assert_eq!(secs_until_next_class_at(&s, "summer", at(17, 30, 0)), None);
    }

    #[test]
    fn in_class_flags_for_marquee_popup() {
        let s = sched();
        // 上课中：不应该弹横幅打断
        assert!(in_class_at(&s, "summer", at(8, 10, 0)));
        assert!(in_class_at(&s, "summer", at(11, 45, 0)));
        // 课间 / 课前 / 放学后：横幅可以弹出
        assert!(!in_class_at(&s, "summer", at(9, 40, 0)));
        assert!(!in_class_at(&s, "summer", at(7, 30, 0)));
        assert!(!in_class_at(&s, "summer", at(17, 30, 0)));
        // 冬夏令时差异：14:05 夏季是课间（14:10 才上第 6 节），冬季已在课（14:00-14:40）
        assert!(!in_class_at(&s, "summer", at(14, 5, 0)));
        assert!(in_class_at(&s, "winter", at(14, 5, 0)));
        // 16:25 夏季在课（15:50-16:30），冬季是课间（16:20 已下课，16:30 才上第 9 节）
        assert!(in_class_at(&s, "summer", at(16, 25, 0)));
        assert!(!in_class_at(&s, "winter", at(16, 25, 0)));
    }

    #[test]
    fn lunch_break() {
        let st = compute_status(&sched(), "summer", &HashMap::new(), at(12, 30, 0));
        assert_eq!(st.state, "午休");
        assert_eq!(st.cd_text, "1 小时 40 分钟");
    }

    #[test]
    fn after_school() {
        let st = compute_status(&sched(), "summer", &HashMap::new(), at(18, 0, 0));
        assert_eq!(st.state, "放学啦");
        assert_eq!(st.next_text, "明天继续加油");
    }

    #[test]
    fn weekend() {
        let sat = Local
            .with_ymd_and_hms(2026, 11, 21, 10, 0, 0)
            .single()
            .unwrap();
        let st = compute_status(&sched(), "summer", &HashMap::new(), sat);
        // 周末中间文案按需求全部清空
        assert!(st.state.is_empty());
        assert!(st.cd_text.is_empty());
    }

    #[test]
    fn parse_python_source() {
        let src = r#"
SUMMER_TIME_RAW = [
    ("08:00", "08:40"), ("08:45", "09:25"),
]
WINTER_TIME_RAW = [("08:00", "08:40"), ("09:00", "09:40")]
"#;
        let s = extract_pairs(src, "SUMMER_TIME_RAW").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].0, "08:45");
        let w = extract_pairs(src, "WINTER_TIME_RAW").unwrap();
        assert_eq!(w.len(), 2);
    }
}
