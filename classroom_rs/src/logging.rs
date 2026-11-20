//! 简单日志：app.log 追加 + 超过 N 天整文件清理（对应 cleanup_old_logs / logging.basicConfig）。

use crate::paths;
use std::io::Write;
use std::sync::Mutex;

static LOG_LOCK: Mutex<()> = Mutex::new(());

fn write_line(level: &str, msg: &str) {
    let t = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let _g = LOG_LOCK.lock();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(paths::log_file()) {
        let _ = writeln!(f, "{} - {} - {}", t, level, msg);
    }
}

pub fn info(msg: &str) {
    write_line("INFO", msg);
}
pub fn error(msg: &str) {
    write_line("ERROR", msg);
}
pub fn warn(msg: &str) {
    write_line("WARNING", msg);
}

/// 超过 max_days 天的 app.log 直接删掉重建（与 Python 版一致）。
pub fn cleanup_old_logs(max_days: i64) {
    let p = paths::log_file();
    if !p.exists() {
        return;
    }
    let mtime = paths::file_mtime(&p);
    let file_date = chrono::DateTime::from_timestamp(mtime as i64, 0)
        .map(|d| d.with_timezone(&chrono::Local));
    if let Some(file_date) = file_date {
        let age = chrono::Local::now() - file_date;
        if age.num_days() > max_days {
            let _ = std::fs::remove_file(&p);
            info("程序启动");
        }
    }
}
