//! 每周定期提醒（只在桌面大屏作业栏上方的通知条里展示）。
//!
//! 时段规则：
//! - 周四 14:00–21:50 →「请留宿同学及时填报周末留宿申请」
//! - 周五 17:00 – 周六 17:00 → 周末安全提醒
//!
//! 展示规则：与普通通知（board_notice.txt）互不覆盖——
//! 两者同时存在时按 ALT_SECS 相位交替显示；只有其一就只显示那个。

use chrono::{Datelike, Timelike, Weekday};

/// 周四留宿申请提醒正文
pub const STAY_REMINDER: &str = "请留宿同学及时填报周末留宿申请";
/// 周末安全提醒正文
pub const WEEKEND_REMINDER: &str = "周末将至，请大家做好自我管理与安全防护。注意交通、防溺水安全，合理作息，不熬夜、不沉迷网络，提高反诈意识，严守安全底线。";

/// 交替显示周期（秒）：提醒与普通通知每这么多秒切换一次
pub const ALT_SECS: i64 = 10;

/// 有效通知的最少字符数：去除首尾空白后 **超过 2 个字符** 才算有效
/// （≤2 个字符视为无效通知，作业栏通知条不显示，等价于没有通知）。
/// 与主程序 `classroom_rs::gitee::is_valid_notice` 口径保持一致。
pub fn is_valid_notice(t: &str) -> bool {
    t.trim().chars().count() > 2
}

/// 此刻应展示的定期提醒正文；None = 当前不在任何提醒时段。
pub fn current(now: chrono::DateTime<chrono::Local>) -> Option<&'static str> {
    let wd = now.weekday();
    let mins = now.hour() as i32 * 60 + now.minute() as i32;
    match wd {
        Weekday::Thu => {
            if (14 * 60..=21 * 60 + 50).contains(&mins) {
                Some(STAY_REMINDER)
            } else {
                None
            }
        }
        Weekday::Fri => {
            if mins >= 17 * 60 {
                Some(WEEKEND_REMINDER)
            } else {
                None
            }
        }
        Weekday::Sat => {
            if mins < 17 * 60 {
                Some(WEEKEND_REMINDER)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 作业栏顶部通知条此刻真正要显示的文本。
///
/// - 不在提醒时段 → 原样返回普通通知；
/// - 提醒时段且通知为空 → 只显示提醒；
/// - 两者都有 → 按 ALT_SECS 相位交替（偶数相位显示提醒，奇数相位显示通知）。
pub fn effective_notice(now: chrono::DateTime<chrono::Local>, notice: &str) -> String {
    // 无效通知（≤2 字符）先按空处理
    let notice = if is_valid_notice(notice) {
        notice
    } else {
        ""
    };
    match current(now) {
        None => notice.to_string(),
        Some(r) => {
            if notice.trim().is_empty() {
                return r.to_string();
            }
            let phase = (now.timestamp() / ALT_SECS) % 2;
            if phase == 0 {
                r.to_string()
            } else {
                notice.to_string()
            }
        }
    }
}

/// 交替相位值：提醒时段内随时间变化（用于触发卡片重渲染）；非提醒时段恒为 None。
pub fn alternation_phase(now: chrono::DateTime<chrono::Local>) -> Option<i64> {
    current(now).map(|_| now.timestamp() / ALT_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(y, m, d, h, mi, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn thursday_window() {
        // 周四 2026-11-19
        assert!(current(at(2026, 11, 19, 14, 0)).is_some());
        assert!(current(at(2026, 11, 19, 21, 50)).is_some());
        assert_eq!(current(at(2026, 11, 19, 15, 30)), Some(STAY_REMINDER));
        // 边界外
        assert_eq!(current(at(2026, 11, 19, 13, 59)), None);
        assert_eq!(current(at(2026, 11, 19, 21, 51)), None);
    }

    #[test]
    fn fri_to_sat_window() {
        // 周五 2026-11-20
        assert_eq!(current(at(2026, 11, 20, 16, 59)), None);
        assert_eq!(current(at(2026, 11, 20, 17, 0)), Some(WEEKEND_REMINDER));
        assert_eq!(current(at(2026, 11, 20, 23, 0)), Some(WEEKEND_REMINDER));
        // 周六 2026-11-21
        assert_eq!(current(at(2026, 11, 21, 8, 0)), Some(WEEKEND_REMINDER));
        assert_eq!(current(at(2026, 11, 21, 16, 59)), Some(WEEKEND_REMINDER));
        assert_eq!(current(at(2026, 11, 21, 17, 0)), None);
        // 周日 / 周一不提醒
        assert_eq!(current(at(2026, 11, 22, 10, 0)), None);
        assert_eq!(current(at(2026, 11, 16, 10, 0)), None);
    }

    #[test]
    fn effective_notice_alternates() {
        // 提醒时段 + 有通知 → 交替
        let t0 = at(2026, 11, 19, 15, 0);
        let t10 = t0 + chrono::Duration::seconds(ALT_SECS);
        let e = effective_notice(t0, "今日作业：预习第三章");
        assert_eq!(e, STAY_REMINDER);
        assert_eq!(effective_notice(t10, "今日作业：预习第三章"), "今日作业：预习第三章");
        // 提醒时段 + 通知为空 → 只显示提醒
        assert_eq!(effective_notice(t0, "  "), STAY_REMINDER);
        // 非提醒时段 → 原样
        let t_mon = at(2026, 11, 16, 15, 0);
        assert_eq!(effective_notice(t_mon, "普通通知"), "普通通知");
    }

    #[test]
    fn phase_none_outside_window() {
        assert_eq!(alternation_phase(at(2026, 11, 16, 15, 0)), None);
        assert!(alternation_phase(at(2026, 11, 19, 15, 0)).is_some());
    }

    #[test]
    fn invalid_notice_treated_as_empty() {
        let t0 = at(2026, 11, 19, 15, 0); // 提醒时段
        let t_mon = at(2026, 11, 16, 15, 0); // 非提醒时段
        // ≤2 字符（含纯空白）→ 无效 → 非提醒时段显示为空、提醒时段显示提醒
        for junk in ["", " ", "ab", "12", "　 ", "\n "] {
            assert!(!is_valid_notice(junk), "junk: {junk:?}");
            assert_eq!(effective_notice(t_mon, junk), "");
            assert_eq!(effective_notice(t0, junk), STAY_REMINDER);
        }
        // 3 个字符起算有效
        assert!(is_valid_notice("abc"));
        assert!(is_valid_notice("  重要通知  "));
        assert_eq!(effective_notice(t_mon, "  重要通知  "), "  重要通知  ");
    }
}
