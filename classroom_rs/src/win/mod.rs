//! Windows 平台层：DPI、单实例互斥、大屏互斥探测、开机自启。

pub mod dib;
pub mod layered;
pub mod sys;

use sys::*;
use std::ffi::c_void;
use std::path::PathBuf;

/// Per-Monitor V2 DPI 感知，与 Python 版三级回退链一致：
/// SetProcessDpiAwarenessContext(-4) → shcore.SetProcessDpiAwareness(2) → SetProcessDPIAware()。
/// 必须在创建任何窗口 / 读取 GetSystemMetrics 之前调用。
pub fn dpi_aware() {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) != 0 {
            return;
        }
        // Win10 1703 以下走 shcore（Win 8.1+）；动态加载避免旧系统缺 shcore.dll 启动失败。
        let lib_name = wide("shcore.dll");
        let lib = LoadLibraryW(lib_name.as_ptr());
        if !lib.is_null() {
            let proc_name = wide("SetProcessDpiAwareness");
            let proc = GetProcAddress(lib, proc_name.as_ptr() as *const u8);
            if !proc.is_null() {
                // PROCESS_PER_MONITOR_DPI_AWARE = 2
                let func: extern "system" fn(u32) -> i32 = std::mem::transmute(proc);
                if func(2) == 0 {
                    return;
                }
            }
        }
        // 最后回退：system DPI aware（Win Vista+）
        SetProcessDPIAware();
    }
}

/// 命名互斥体防重复。已运行返回 false。
pub fn single_instance_ok(name: &str) -> bool {
    unsafe {
        let n = wide(name);
        let h = CreateMutexW(std::ptr::null(), 0, n.as_ptr());
        if h.is_null() {
            return true;
        }
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// 桌面大屏是否在运行：探测它的互斥体（Python 版与 Rust 版两个名字都试）。
pub fn board_is_running() -> bool {
    for name in ["GzzxDesktopBoardMutex", "GzzxDesktopBoardRustMutex"] {
        let n = wide(name);
        let h = unsafe { OpenMutexW(SYNCHRONIZE, 0, n.as_ptr()) };
        if !h.is_null() {
            unsafe { CloseHandle(h) };
            return true;
        }
    }
    false
}

/// 下课守护进程持有的互斥体名字。
pub const GUARD_MUTEX: &str = "GzzxRollCallGuardRustMutex";

/// 命名互斥体是否已存在（只探测，不创建，避免自己的探测把它造出来）。
fn mutex_exists(name: &str) -> bool {
    let n = wide(name);
    let h = unsafe { OpenMutexW(SYNCHRONIZE, 0, n.as_ptr()) };
    if h.is_null() {
        return false;
    }
    unsafe { CloseHandle(h) };
    true
}

/// 下课守护是否已经在跑。
pub fn guard_is_running() -> bool {
    mutex_exists(GUARD_MUTEX)
}

/// 点名主程序是否已经在跑（守护用来判断要不要拉起）。
pub fn main_app_running() -> bool {
    mutex_exists(crate::theme::APP_MUTEX_NAME)
}

/// 以脱离父进程、无控制台的方式启动自身（拉起守护 / 下课拉起主程序都用它）。
/// DETACHED_PROCESS 让子进程不随父进程退出，这正是「主程序关了守护还在」的关键。
pub fn spawn_self(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    match std::process::Command::new(exe_path())
        .args(args)
        .creation_flags(DETACHED_PROCESS)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => true,
        Err(e) => {
            crate::logging::warn(&format!("启动自身失败：{e}"));
            false
        }
    }
}

/// 开机自启（HKCU\...\Run，值名 RollCallSystem）：直接写 exe 路径。
/// v1.0.28 起移除 wscript + run.vbs 延迟脚本——教室机的杀软/管控软件常拦
/// wscript 执行 vbs，导致"有时能自启有时不能"。exe 直写没有脚本环节，无从拦截。
pub fn set_auto_run(enable: bool) {
    let sub = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let value_name = wide("RollCallSystem");
    unsafe {
        let mut key: *mut c_void = std::ptr::null_mut();
        let r = RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_WRITE, &mut key);
        if r != 0 || key.is_null() {
            crate::logging::warn("自启设置失败：打不开注册表键");
            return;
        }
        if enable {
            let cmd = auto_run_command(&exe_path());
            let data = wide(&cmd);
            let bytes: Vec<u8> = data.iter().flat_map(|c| c.to_le_bytes()).collect();
            RegSetValueExW(key, value_name.as_ptr(), 0, REG_SZ, bytes.as_ptr(), bytes.len() as u32);
        } else {
            RegDeleteValueW(key, value_name.as_ptr());
        }
        RegCloseKey(key);
    }
    // 注册表已覆盖为 exe 后，清掉历史版本遗留的 run.vbs（留着也会被误判为仍在用）。
    // 上面提前 return 时不删——那时旧 Run 值可能还引用着它。
    let _ = std::fs::remove_file(crate::paths::data_dir().join("run.vbs"));
}

/// Run 键的命令值：仅 exe 路径加引号，不含任何脚本/解释器。
fn auto_run_command(exe: &std::path::Path) -> String {
    format!("\"{}\"", exe.display())
}

/// 当前 exe 路径。
pub fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("app.exe"))
}

#[cfg(test)]
mod tests {
    use super::auto_run_command;
    use std::path::Path;

    #[test]
    fn auto_run_value_is_plain_exe_no_script() {
        let cmd = auto_run_command(Path::new(r"C:\Program Files\RollCall\app.exe"));
        assert_eq!(cmd, r#""C:\Program Files\RollCall\app.exe""#);
        assert!(!cmd.contains("wscript"));
        assert!(!cmd.contains(".vbs"));
    }
}


/// DPI 缩放比例（96 DPI = 1.0）。
pub fn dpi_scale() -> f64 {
    unsafe {
        let dpi = GetDpiForSystem();
        if dpi > 0 {
            dpi as f64 / 96.0
        } else {
            1.0
        }
    }
}

/// 希沃白板工具栏高度：找到窗口就取实际高度，否则按分辨率估算（对应 detect_toolbar_height）。
pub fn seewo_toolbar_height() -> i32 {
    for name in ["SeewoBar", "SeewoFloatingBar"] {
        let w = wide(name);
        let hwnd = unsafe { FindWindowW(w.as_ptr(), std::ptr::null()) };
        if !hwnd.is_null() {
            let mut rect = RECT::default();
            if unsafe { GetWindowRect(hwnd, &mut rect) } != 0 {
                return rect.bottom - rect.top;
            }
        }
    }
    let (_, sh) = screen_size();
    if sh >= 2160 {
        100
    } else if sh >= 1440 {
        80
    } else {
        60
    }
}

/// 高 DPI 下拿主屏物理分辨率。
pub fn screen_size() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}
