//! 数据目录与文件路径（与 Python 版逐项对应）。

use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    home().join(".RollCallData")
}

pub fn home() -> PathBuf {
    // Python: Path.home()
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_file() -> PathBuf {
    data_dir().join("config.json")
}
pub fn history_file() -> PathBuf {
    data_dir().join("history.json")
}
pub fn log_file() -> PathBuf {
    data_dir().join("app.log")
}
pub fn schedule_dir() -> PathBuf {
    data_dir().join("schedules")
}
// 名单与课表改由 Gitee 私有仓库按班拉取（见 seed.rs），本机不再有
// seed/seed.json 或 exe 同目录的随包名单；这里刻意不提供任何本地种子路径，
// 避免以后有人又把全校名单放回磁盘。
/// 桌面大屏退出信号文件（与 desktop_board --stop 同一机制）。
pub fn board_stop_file() -> PathBuf {
    data_dir().join("desktop_board.stop")
}
/// 下课守护写的「关机期间有未展示的新通知，补弹一次」标记（大屏消费后删除）。
pub fn break_popup_flag() -> PathBuf {
    data_dir().join("break_popup.flag")
}
/// 守护记录的「关机前最后一次已展示的通知」，用来判断关机期间是否来了新通知。
pub fn last_seen_notice_file() -> PathBuf {
    data_dir().join("notice_last_seen.txt")
}
/// 手动停止下课守护的标记（主程序下次启动时会清除并重新拉起守护）。
pub fn guard_stop_file() -> PathBuf {
    data_dir().join("guard.stop")
}
/// 设置窗口「创建班级 / 切换班级」写的显式信号。
/// 内容两行：新班 file_name + 写入时的 epoch 毫秒。
/// 主程序消费后立刻热加载并回拉通知，不依赖 config.json 的 mtime 精度。
pub fn class_switch_flag() -> PathBuf {
    data_dir().join("class_switch.flag")
}
/// 主程序把新班通知写进 board_notice.txt 之后才写的「立刻弹横幅」标记。
/// 内容是写入时的 epoch 毫秒，大屏横幅消费后自行删除。
pub fn notice_popup_flag() -> PathBuf {
    data_dir().join("notice_popup.flag")
}
pub fn sync_record_file() -> PathBuf {
    data_dir().join("gitee_downloaded.json")
}
pub fn password_file() -> PathBuf {
    data_dir().join("auth.json")
}
pub fn ensure_dirs() {
    let _ = std::fs::create_dir_all(data_dir());
    let _ = std::fs::create_dir_all(schedule_dir());
}

/// 文件 mtime（秒）；不存在返回 0。日志清理按「天」比较，仍用秒。
pub fn file_mtime(p: &PathBuf) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 文件 mtime（毫秒）：配置热加载专用。
/// 秒级 mtime 会在「设置页同一秒内连续保存」时漏检，班级切换就卡住不生效。
pub fn file_mtime_ms(p: &PathBuf) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 切换标记最多 2 分钟内有效：主程序当时没开，隔天启动不要突然补弹。
pub const CLASS_SWITCH_TTL_MS: u64 = 120_000;

/// 解析切换标记内容；纯函数，方便单测。
/// 兼容旧格式（只有一行班级名，没有时间戳）。
pub fn parse_class_switch(raw: &str, now_ms: u64) -> Option<String> {
    let mut lines = raw.lines();
    let class_file = lines.next().unwrap_or("").trim();
    if class_file.is_empty() {
        return None;
    }
    if let Some(ts) = lines.next() {
        if let Ok(ts) = ts.trim().parse::<u64>() {
            if now_ms.saturating_sub(ts) > CLASS_SWITCH_TTL_MS {
                return None;
            }
        }
    }
    Some(class_file.to_string())
}

pub fn write_class_switch_flag(class_file: &str) {
    let _ = std::fs::write(
        class_switch_flag(),
        format!("{}\n{}", class_file.trim(), epoch_ms()),
    );
}

/// 读走并删除切换标记（返回新班 file_name）。
pub fn take_class_switch_flag() -> Option<String> {
    let p = class_switch_flag();
    let raw = std::fs::read_to_string(&p).ok();
    let _ = std::fs::remove_file(&p);
    raw.as_deref().and_then(|s| parse_class_switch(s, epoch_ms()))
}

/// 退出确认窗口（--confirm-exit 子进程）写标记。
pub fn exit_flag() -> PathBuf {
    data_dir().join("exit.flag")
}

pub fn write_exit_flag() {
    let _ = std::fs::write(exit_flag(), epoch_ms().to_string());
}

/// 读走并删除退出确认标记。带 5 分钟 TTL：残留旧标记不会让下次启动直接退出。
pub fn take_exit_flag() -> bool {
    let p = exit_flag();
    let raw = std::fs::read_to_string(&p).ok();
    let _ = std::fs::remove_file(&p);
    match raw.as_deref().and_then(|s| s.trim().parse::<u64>().ok()) {
        Some(ms) => epoch_ms().saturating_sub(ms) < 5 * 60 * 1000,
        None => false,
    }
}

/// 通知已经落盘，通知大屏横幅立即弹出一次。
pub fn write_notice_popup_flag() {
    let _ = std::fs::write(notice_popup_flag(), epoch_ms().to_string());
}

pub fn trace(line: &str) {
    let t = chrono::Local::now().format("%H:%M:%S%.3f");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir().join("roll_debug.log"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{t} {line}\n").as_bytes()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_class_switch_signal_is_accepted() {
        let now = 1_800_000_000_000u64;
        assert_eq!(
            parse_class_switch("赣州中学_2025级09班.txt\n1799999999000", now),
            Some("赣州中学_2025级09班.txt".to_string())
        );
    }

    #[test]
    fn stale_class_switch_signal_is_dropped() {
        let now = 1_800_000_000_000u64;
        let old = now - CLASS_SWITCH_TTL_MS - 1;
        assert_eq!(parse_class_switch(&format!("A班.txt\n{old}"), now), None);
        // 恰好在 TTL 边界仍然有效
        let edge = now - CLASS_SWITCH_TTL_MS;
        assert_eq!(
            parse_class_switch(&format!("A班.txt\n{edge}"), now),
            Some("A班.txt".to_string())
        );
    }

    #[test]
    fn legacy_or_broken_signal_is_safe() {
        assert_eq!(
            parse_class_switch("A班.txt", 1_800_000_000_000),
            Some("A班.txt".to_string())
        );
        assert_eq!(parse_class_switch("", 1_800_000_000_000), None);
        assert_eq!(parse_class_switch("\n", 1_800_000_000_000), None);
        // 时间戳来自未来（系统时钟回拨）也不该被当成过期
        assert_eq!(
            parse_class_switch("A班.txt\n9999999999999", 1_800_000_000_000),
            Some("A班.txt".to_string())
        );
    }
}
