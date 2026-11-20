//! 统一取当前时间。
//!
//! 设置环境变量 `BOARD_NOW="2026-11-16 10:30"` 可锁定「当前时刻」，
//! 用来预览上课中 / 课间 / 倒计时等时段的大屏样子（不改系统时间）。

use chrono::{DateTime, Local};
use std::sync::OnceLock;

fn fake() -> Option<DateTime<Local>> {
    let raw = std::env::var("BOARD_NOW").ok()?;
    let naive = chrono::NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M").ok()?;
    naive.and_local_timezone(Local).single()
}

pub fn now() -> DateTime<Local> {
    static FAKE: OnceLock<Option<DateTime<Local>>> = OnceLock::new();
    match FAKE.get_or_init(fake) {
        Some(t) => *t,
        None => Local::now(),
    }
}
