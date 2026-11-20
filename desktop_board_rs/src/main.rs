//! 班级桌面大屏（Rust 版）
//!
//! 用法：
//!   desktop_board                  # 贴壁纸：渲染整屏图设为壁纸 + 通知跑马灯（默认）
//!   desktop_board --editor         # 打开编辑器（普通窗口，不碰壁纸）
//!   desktop_board --selftest       # 打印自检信息并退出
//!   desktop_board --render-only p  # 只渲染一张 PNG 到 p（不设壁纸，调试用）
//!   desktop_board --stop           # 让已运行的实例退出
//!
//! 与 Python 版保持一致：作息 / 班级 / 课表直接读 main202609245.py 与 ~/.RollCallData，
//! 今日作业、值周、卫生值日、班级名均可编辑，存 desktop_board.json。

// release 版无控制台黑框；debug 版保留控制台便于调试
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use desktop_board::{board, clock, config, paths, render, schedule, serve, win};

/// GUI 子系统下从命令行调用（--selftest 等）时附加父控制台，保证 println 可见。
fn attach_parent_console() {
    #[cfg(windows)]
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
    let has = |k: &str| args.iter().any(|a| a == k);

    if has("--stop") {
        let _ = std::fs::create_dir_all(paths::data_dir());
        let _ = std::fs::write(paths::stop_file(), b"stop");
        println!("已发送退出信号");
        return;
    }

    win::dpi_aware();

    let selftest = has("--selftest");
    let editor = has("--editor") || has("--window") || has("--board-editor");

    if let Some(pos) = args.iter().position(|a| a == "--render-only") {
        let out = args
            .get(pos + 1)
            .cloned()
            .unwrap_or_else(|| "board_preview.png".to_string());
        render_to(std::path::Path::new(&out));
        return;
    }

    // 调试用：只画作业卡右下角那两个按钮（铅笔 / ✕），不动壁纸
    if let Some(pos) = args.iter().position(|a| a == "--hw-preview") {
        let out = args
            .get(pos + 1)
            .cloned()
            .unwrap_or_else(|| "hw_preview.png".to_string());
        // 可选第二个参数：通知条滚动偏移（调试滚动效果用）
        let offset = args
            .get(pos + 2)
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(0.0);
        let ok = win::homework::preview(std::path::Path::new(&out), offset);
        println!("{} {out:?}", if ok { "已渲染" } else { "渲染失败" });
        return;
    }

    if selftest {
        selftest_report();
        return;
    }

    // 调试用：只显示通知跑马灯条，不改壁纸
    if has("--marquee-only") {
        let bd = board::load(&config::load_app_config().raw_name);
        let (sx, sy, sw, sh) = win::screen_geometry(&bd);
        let gutter = if bd.icon_gutter {
            ((sw as f32 * 0.11) as i32).max(150)
        } else {
            0
        };
        win::marquee::run(sx, sy, sw, sh, gutter, bd);
        return;
    }

    if !editor {
        serve::run();
        return;
    }
    win::editor::run();
}

/// 只渲染一张 PNG（调试 / 出图核对版式，不动系统壁纸）。
fn render_to(out: &std::path::Path) {
    let cfg = config::load_app_config();
    let sched = schedule::load();
    let courses = config::load_courses(&cfg.current_class);
    let bd = board::load(&cfg.raw_name);
    let (_, _, w, h) = win::screen_geometry(&bd);
    let now = clock::now();
    let st = schedule::compute_status(&sched, &cfg.work_mode, &courses, now);
    let notice = config::load_board_notice();
    let pm = render::shared().render(&bd, &st, &notice, now, w as u32, h as u32);
    if render::save_png(&pm, out) {
        println!("已渲染 {out:?} ({w}x{h})");
    } else {
        println!("渲染失败");
    }
}

/// 自检：不出窗口、不动壁纸，只把关键状态打出来。
fn selftest_report() {
    let cfg = config::load_app_config();
    let sched = schedule::load();
    let courses = config::load_courses(&cfg.current_class);
    let bd = board::load(&cfg.raw_name);
    let now = clock::now();
    let st = schedule::compute_status(&sched, &cfg.work_mode, &courses, now);

    println!("SELFTEST_STATUS: {:?}", st);
    println!(
        "SCHEDULE: 来源={} 夏季={} 节 冬季={} 节",
        if sched.from_source {
            "main202609245.py"
        } else {
            "内置兜底表"
        },
        sched.summer.len(),
        sched.winter.len()
    );
    println!("SCREEN: {:?}", win::screen_geometry(&bd));
    println!("MONITORS: {:?}", win::list_monitors());
    println!(
        "WALL_SRC: {} (存在={})",
        paths::wall_src().display(),
        paths::wall_src().exists()
    );
    let notice = config::load_board_notice();
    println!(
        "NOTICE: {}",
        if notice.is_empty() {
            "(空)".to_string()
        } else {
            notice.chars().take(80).collect()
        }
    );
    println!(
        "BOARD: 班级={:?} 作业{}项 作息={} 课表{}条",
        bd.class_name,
        bd.homework.len(),
        cfg.work_mode,
        courses.len()
    );
    println!(
        "STUDENTS: {} 人 (来自 {:?})",
        cfg.students.len(),
        cfg.students_from
    );
    println!("DATA: {}", paths::data_dir().display());
}
