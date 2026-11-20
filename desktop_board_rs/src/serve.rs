//! 大屏服务主循环：通知跑马灯 + 作业卡悬浮窗（主线程消息泵）。
//!
//! 旧版每分钟把整屏合成图设成系统壁纸；现在作业卡是自己的 layered 窗口，
//! **不再渲染、也不再替换系统壁纸**，用户换什么壁纸都能直接用。
//! `desktop_board.stop` 出现即退出（主程序 / --stop 控制）。

use std::sync::atomic::Ordering;

use crate::board;
use crate::config;
use crate::paths;
use crate::win::{self, wallpaper};

/// 从旧版升级过来时，系统壁纸可能还停在我们合成的 `board_wallpaper.png` 上，
/// 上面烤着一张旧作业卡。这里只在这种情况下还原一次，绝不动用户自己选的壁纸。
fn restore_legacy_wallpaper_once() {
    let cur = wallpaper::get_global_wallpaper().unwrap_or_default();
    let ours = paths::wall_out().to_string_lossy().to_ascii_lowercase();
    let legacy = !ours.is_empty()
        && (cur.to_ascii_lowercase() == ours
            || cur.to_ascii_lowercase().contains("board_wallpaper"));
    if !legacy {
        return;
    }
    let prev = std::fs::read_to_string(paths::wallpaper_backup_file())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| paths::wall_src().to_string_lossy().to_string());
    if wallpaper::restore_wallpaper(&prev) {
        paths::trace(&format!("已从旧版合成壁纸还原：{prev}"));
    }
    let _ = std::fs::remove_file(paths::wallpaper_backup_file());
}

pub fn run() {
    let _ = std::fs::create_dir_all(paths::data_dir());
    if !win::single_instance_ok() {
        println!("大屏服务已在运行，跳过本次启动");
        return;
    }
    let _ = std::fs::remove_file(paths::stop_file());
    let _ = std::fs::remove_file(paths::quit_trace_file());
    paths::trace("serve 启动（透明悬浮作业卡 + 跑马灯条，不改系统壁纸）");

    restore_legacy_wallpaper_once();

    win::marquee::STOP_SEEN.store(false, Ordering::Relaxed);
    let board0 = board::load(&config::load_app_config().raw_name);

    // 不再模拟 Win+D 显示桌面：守护在下课拉起主程序 → 大屏重启 → 旧版会把
    // 老师正在用的 PPT / 课件全部最小化。通知横幅与跑马灯本身就是
    // WS_EX_TOPMOST 置顶层窗口，下课直接浮在所有窗口之上，无需回到桌面；
    // 作业卡则相反——永远沉底（HWND_BOTTOM），所有程序关闭后才露出壁纸上的卡片。

    let (sx, sy, sw, sh) = win::screen_geometry(&board0);
    let gutter = if board0.icon_gutter {
        ((sw as f32 * 0.11) as i32).max(150)
    } else {
        0
    };
    // 跑马灯窗口 + 作业卡窗口同线程，共用下面这个消息泵
    if !win::marquee::run(sx, sy, sw, sh, gutter, board0) {
        paths::trace("跑马灯创建失败，降级为纯消息循环");
        while !paths::stop_file().exists() {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        println!("大屏服务已退出");
    } else {
        paths::trace("跑马灯 mainloop 返回");
    }
    let _ = std::fs::remove_file(paths::stop_file());
    board::flush();
    win::release_single_instance();
}
