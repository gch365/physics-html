//! 教室随机点名系统 V7.2 —— Rust 移植版。
//!
//! 一个 exe 三种模式：
//! - 无参数        点名系统主程序（悬浮条 + 通知横幅 + 点名/倒计时窗）
//! - --settings    设置窗口（egui，与主进程通过 config.json 共享状态）
//! - --history     历史记录窗口
//! - --desktop-board  直接进入桌面大屏模式（调用同目录的大屏程序）
//! - --guard      无窗口下课守护：主程序退出后仍常驻，到课间自动拉起并补弹通知
//! - --selftest    自检信息（便于脚本化核对）

// release 版无控制台黑框；debug 版保留控制台便于调试
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod classcode;
mod config;
mod gitee;
mod goeasy;
mod history;
mod logging;
mod password;
mod paths;
mod schedule;
mod seed;
mod services;
mod text;
#[allow(dead_code)] // 常量库：保留与 Python 版对齐的设计规范值
mod theme;
mod update;
mod win;

/// GUI 子系统下从命令行调用（--selftest 等）时附加父控制台，保证 println 可见。
fn attach_parent_console() {
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn AttachConsole(dwProcessId: u32) -> i32;
        }
        const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn main() {
    attach_parent_console();
    let args: Vec<String> = std::env::args().skip(1).collect();
    win::dpi_aware();
    paths::ensure_dirs();
    logging::cleanup_old_logs(3);

    if args.iter().any(|a| a == "--selftest") {
        selftest(args.iter().any(|a| a == "--selftest-net"));
        return;
    }
    if args.iter().any(|a| a == "--gitee-test") {
        let cfg = config::Config::load();
        let api = gitee::GiteeApi::new(&cfg.gitee_owner, &cfg.gitee_repo, &cfg.gitee_token);
        let (ok, info) = api.test_connection(true, None);
        println!("GITEE_TEST: ok={} info={}", ok, info);
        return;
    }
    if args.iter().any(|a| a == "--guard") {
        // 无窗口后台守护：主程序整个退出后仍常驻，到课间把主程序拉起来补弹通知
        services::run_guard();
        return;
    }
    if args.iter().any(|a| a == "--desktop-board") {
        start_desktop_board();
        return;
    }
    if args.iter().any(|a| a == "--board-editor") {
        // 大屏编辑器：直接跑大屏 lib 的编辑器窗口（无需 desktop_board.exe）
        desktop_board::win::editor::run();
        return;
    }
    if args.iter().any(|a| a == "--confirm-exit") {
        // 退出确认小窗：输入 123 后写 exit.flag 由主程序消费；不加载主窗口
        settings::run_confirm_exit();
        return;
    }
    if args.iter().any(|a| a == "--settings") {
        settings::run();
        return;
    }
    if args.iter().any(|a| a == "--history") {
        settings::run_history();
        return;
    }

    // 主程序
    if !win::single_instance_ok(theme::APP_MUTEX_NAME) {
        win::sys::msg_box("程序已在运行！", "提示", 0x30); // MB_ICONWARNING
        return;
    }
    // panic 留痕：窗口过程无法 unwind，panic 会直接 abort，stderr 可能丢失，写入 trace 文件。
    std::panic::set_hook(Box::new(|info| {
        paths::trace(&format!("PANIC: {}", info));
    }));
    let cfg = std::sync::Arc::new(std::sync::Mutex::new(config::Config::load()));
    schedule::load_time_overrides();
    let history = std::sync::Arc::new(history::HistoryManager::new());
    services::spawn_history_saver(
        history.clone(),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    // 开机自启（与 Python 版一致：启动即按配置写注册表）。
    // 放后台线程：注册表 Run 键操作可能被安全管控/杀软挂起，不能卡启动流程。
    let ar = cfg.lock().unwrap().auto_run;
    let _ = std::thread::Builder::new().name("auto-run".into()).spawn(move || {
        win::set_auto_run(ar);
    });
    // 拉起下课守护：主程序关掉之后，下课时仍能把程序自动拉起来补弹通知
    let _ = std::thread::Builder::new()
        .name("guard-boot".into())
        .spawn(services::ensure_guard);
    // 单文件模式清理（v1.0.24）：app.exe 已内嵌全部大屏代码，同目录遗留的
    // desktop_board.exe 属冗余双版本（编辑器铅笔按钮会优先用它，导致更新错位），
    // 启动时后台删除一次；失败（被占用等）静默忽略，下次升级 bat 会再清。
    let _ = std::thread::Builder::new().name("board-cleanup".into()).spawn(|| {
        if let Ok(exe) = std::env::current_exe() {
            let board = exe.with_file_name("desktop_board.exe");
            if board.exists() {
                match std::fs::remove_file(&board) {
                    Ok(()) => logging::info("已删除遗留的 desktop_board.exe（单文件模式）"),
                    Err(e) => logging::warn(&format!("删除遗留 desktop_board.exe 失败（升级时再清）: {e}")),
                }
            }
        }
    });
    ui::run(cfg, history);
}
/// 自检：打印配置与作息状态，不出任何窗口。`net` = 额外联网核对 version.json（只读，不应用）。
fn selftest(net: bool) {
    let cfg = config::Config::load();
    println!("APP_VERSION: {}", theme::APP_VERSION);
    println!("DATA: {}", paths::data_dir().display());
    println!(
        "CLASS: 当前={} 班级数={}",
        cfg.current_class,
        cfg.class_list.len()
    );
    for c in &cfg.class_list {
        println!("  - {} ({} 人) [{}]", c.display_name, c.students.len(), c.file_name);
    }
    println!(
        "WORK_TIME: {} summer={}节 winter={}节（含晚自习）",
        cfg.work_time,
        schedule::time_rows("summer").len(),
        schedule::time_rows("winter").len()
    );
    let now = chrono::Local::now();
    let wd = chrono::Datelike::weekday(&now);
    println!(
        "NOW: {} {} | 上课中={} 当前节={:?} 周末={}",
        now.format("%Y-%m-%d %H:%M"),
        chrono::Datelike::weekday(&now),
        schedule::is_class_time(&cfg.work_time),
        schedule::current_class_period(&cfg.work_time),
        schedule::is_weekend()
    );
    let _ = wd;
    println!(
        "NOTICE: 字号={} 位置={} 颜色={} | 侧边条={} 倒计时={}s",
        cfg.notice_font_size,
        cfg.notice_position,
        cfg.notice_color,
        cfg.bar_size,
        cfg.default_countdown
    );
    println!(
        "SCHEDULED: {} 条",
        cfg.scheduled_notices.len()
    );
    println!(
        "GITEE: enabled={} owner={} repo={} token={}",
        cfg.gitee_enabled,
        cfg.gitee_owner,
        cfg.gitee_repo,
        if cfg.gitee_token.is_empty() { "空" } else { "已配置" }
    );
    println!(
        "GOEASY: enabled={} channel={} port={}",
        cfg.goeasy_enabled, cfg.goeasy_channel, cfg.goeasy_port
    );
    println!(
        "UPDATE: enabled={} branch={} exe_path={}",
        cfg.update_enabled, cfg.update_branch, cfg.update_exe_path
    );
    if net {
        // 只读核对 version.json（验证自动更新拉取链路；绝不应用更新）
        let api = gitee::GiteeApi::new(&cfg.gitee_owner, &cfg.gitee_repo, &cfg.gitee_token);
        match api.download_file_content("version.json") {
            Ok(data) => match serde_json::from_slice::<serde_json::Value>(&data) {
                Ok(v) => {
                    let remote = v
                        .get("version")
                        .and_then(|x| x.as_str())
                        .unwrap_or("0.0.0")
                        .to_string();
                    println!(
                        "UPDATE_NET: remote=v{} local=v{} has_update={} notes={}",
                        remote,
                        theme::APP_VERSION,
                        gitee::app_has_update(theme::APP_VERSION, &remote),
                        v.get("notes").and_then(|x| x.as_str()).unwrap_or("")
                    );
                }
                Err(e) => println!("UPDATE_NET: version.json 解析失败 {e}"),
            },
            Err(e) => println!("UPDATE_NET: 拉取失败 {e}"),
        }
    }
    println!("SCREEN: {}x{}", win::screen_size().0, win::screen_size().1);
    println!("BOARD_RUNNING: {}", win::board_is_running());
    println!("SELFTEST_OK");
}

/// 进入桌面大屏模式：优先调用同目录的 desktop_board.exe（Rust 版），
/// 找不到再尝试 python 启动 desktop_board.py。
fn start_desktop_board() {
    let exe = win::exe_path();
    let dir = exe.parent().unwrap_or(std::path::Path::new("."));
    let board = dir.join("desktop_board.exe");
    if board.exists() {
        match std::process::Command::new(&board).spawn() {
            Ok(_) => return,
            Err(e) => logging::error(&format!("启动大屏失败: {}", e)),
        }
    }
    // 回退：Python 版（脚本需与本 exe 同目录）
    let py = dir.join("desktop_board.py");
    if py.exists() {
        for python in ["pythonw.exe", "python.exe"] {
            if let Ok(_) = std::process::Command::new(python).arg(&py).spawn() {
                return;
            }
        }
    }
    // 都没有：当前进程内直接跑大屏服务（lib），desktop_board.exe 纯可选
    desktop_board::serve::run();
}

mod settings;
mod ui;
