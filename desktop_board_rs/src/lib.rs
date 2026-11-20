//! 班级桌面大屏 —— 库入口（供随机点名主程序同进程拉起）。
//!
//! bin 入口见 main.rs（--editor / --selftest / --render-only / --stop / --marquee-only）。
//! 库暴露 run_in_thread()：在独立线程运行壁纸服务 + 跑马灯消息循环，
//! 线程 panic 不影响调用方；stop 文件 / 互斥体防重机制与独立运行时完全一致。

pub mod board;
pub mod clock;
pub mod config;
pub mod paths;
pub mod reminder;
pub mod render;
pub mod schedule;
pub mod serve;
pub mod text;
pub mod theme;
pub mod win;

/// 在新线程运行大屏服务（对应独立运行时的 serve::run）。
/// 若旧实例刚退出，等待其互斥体释放（最多 15 秒）再启动。
pub fn run_in_thread() {
    std::thread::Builder::new()
        .name("desktop-board".into())
        .spawn(|| {
            // panic 留痕：线程 panic 写入 trace 文件（注意 set_hook 是进程级的）
            std::panic::set_hook(Box::new(|info| {
                paths::trace_hook(&format!("BOARD-PANIC: {}", info));
            }));
            win::dpi_aware();
            paths::trace("board-thread 启动");
            // 等旧实例退出释放互斥体（最多 15s），失败也照常尝试（serve 内部会再防重）
            for _ in 0..15 {
                if !win::board_mutex_held() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            serve::run();
            paths::trace("board-thread 退出");
        })
        .ok();
}
