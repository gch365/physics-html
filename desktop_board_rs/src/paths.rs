//! 路径与诊断：数据目录、配置文件、壁纸源图。
//!
//! 与 Python 版一致：所有数据都在 `~/.RollCallData`，不复制一份数据，
//! 与点名程序（main202609245.py）共用同一套配置 / 作息 / 课表。

use std::path::PathBuf;

/// exe 所在目录（打包后 __file__ 指向临时解包目录，这里等价于 exe 目录）。
pub fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 教室管理程序源码，作息表从它里面抽取，保证作息改动自动同步。
pub fn main_py() -> PathBuf {
    app_dir().join("main202609245.py")
}

pub fn data_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".RollCallData")
}

pub fn config_file() -> PathBuf {
    data_dir().join("config.json")
}

pub fn schedule_dir() -> PathBuf {
    data_dir().join("schedules")
}

pub fn board_file() -> PathBuf {
    data_dir().join("desktop_board.json")
}

pub fn stop_file() -> PathBuf {
    data_dir().join("desktop_board.stop")
}

/// 大屏启动前的系统壁纸路径（退出时据此还原桌面背景）。
pub fn wallpaper_backup_file() -> PathBuf {
    data_dir().join("desktop_board_prev_wallpaper.txt")
}

/// 点名程序写入的最新通知正文（大屏只读）。
pub fn notice_file() -> PathBuf {
    data_dir().join("board_notice.txt")
}

/// 守护进程在下课时写的「关机期间来了新通知，补弹一次」标记。
/// 大屏横幅消费后自行删除；与 classroom_rs::paths::break_popup_flag 指向同一文件。
pub fn break_popup_flag() -> PathBuf {
    data_dir().join("break_popup.flag")
}

/// 主程序「创建 / 切换班级」并把新班通知写进 board_notice.txt 之后写的标记。
/// 内容是写入时的 epoch 毫秒；横幅消费后自行删除。
/// 与 classroom_rs::paths::notice_popup_flag 指向同一文件。
pub fn notice_popup_flag() -> PathBuf {
    data_dir().join("notice_popup.flag")
}

/// 标记最多 90 秒内有效：大屏当时没开，隔天启动不要突然补弹。
pub const NOTICE_POPUP_TTL_MS: u64 = 90_000;

/// 纯函数：判断强制弹出标记是否新鲜。
pub fn notice_popup_fresh(raw: &str, now_ms: u64) -> bool {
    let Ok(ts) = raw.trim().parse::<u64>() else {
        return false;
    };
    now_ms.saturating_sub(ts) <= NOTICE_POPUP_TTL_MS
}

/// 渲染出的整屏壁纸。
pub fn wall_out() -> PathBuf {
    data_dir().join("board_wallpaper.png")
}

/// 退出诊断日志（启动时清空）。
pub fn quit_trace_file() -> PathBuf {
    data_dir().join("board_quit.log")
}

const WALL_SYS: &str = r"C:\Windows\Web\Wallpaper\Windows\img0.jpg";
const WALL_BUNDLED_NAME: &str = "wall_bloom.jpg";

/// 优先随包的 Bloom 副本（Win11 img0.jpg 原图），保证 Win10/11 观感一致。
pub fn wall_src() -> PathBuf {
    let bundled = app_dir().join(WALL_BUNDLED_NAME);
    if bundled.exists() {
        bundled
    } else {
        PathBuf::from(WALL_SYS)
    }
}

/// 退出诊断打点：追加写 board_quit.log。打包 exe 里「点关闭一直不退出」
/// 且源码跑不复现时，看最后一条即知卡在哪一步。
pub fn trace(msg: &str) {
    use std::io::Write;
    let line = format!("{} {}\n", chrono::Local::now().format("%H:%M:%S%.3f"), msg);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(quit_trace_file())
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 线程内 panic 留痕用的同步写函数（set_hook 捕获后写入 trace 文件）。
pub fn trace_hook(msg: &str) {
    trace(msg);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_flag_respects_ttl() {
        let now = 1_800_000_000_000u64;
        assert!(notice_popup_fresh(&now.to_string(), now));
        assert!(notice_popup_fresh(
            &(now - NOTICE_POPUP_TTL_MS).to_string(),
            now
        ));
        assert!(!notice_popup_fresh(
            &(now - NOTICE_POPUP_TTL_MS - 1).to_string(),
            now
        ));
        assert!(!notice_popup_fresh("", now));
        assert!(!notice_popup_fresh("not-a-number", now));
        // 时钟回拨：时间戳来自未来也不算过期
        assert!(notice_popup_fresh("9999999999999", now));
    }
}
