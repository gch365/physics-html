//! Windows 平台层：DPI、显示器枚举、壁纸设置、单实例、跑马灯、编辑器。
//! （旧版启动时模拟 Win+D 最小化其它窗口的逻辑已移除：置顶横幅无需回到桌面。）

pub mod confirm;
pub mod dib;
pub mod editor;
pub mod homework;
pub mod marquee;
pub mod sys;
pub mod touch;
pub mod wallpaper;

use crate::board::Board;
use sys::*;

/// 高 DPI 下拿到真实像素（85 寸白板常见 4K + 150%/200% 缩放）。
pub fn dpi_aware() {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
            SetProcessDPIAware();
        }
    }
}

/// 单实例互斥体句柄（同进程线程模式下线程退出时需显式释放，否则大屏无法重启）。
static BOARD_MUTEX: std::sync::atomic::AtomicPtr<std::ffi::c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// 命名互斥体防重复：已被主程序同步拉起时，再次启动直接退出。
pub fn single_instance_ok() -> bool {
    mark_once("GzzxDesktopBoardRustMutex", true)
}

/// 编辑器用独立互斥体：大屏正在运行时也能打开编辑器（原来的坑：
/// 两者共用同一个互斥体，导致「编辑桌面大屏」永远只打印一句“已在运行”就退出）。
pub fn editor_instance_ok() -> bool {
    mark_once("GzzxBoardEditorRustMutex", false)
}

fn mark_once(name: &str, own_board: bool) -> bool {
    use std::sync::atomic::Ordering;
    unsafe {
        let name = wide(name);
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if h.is_null() {
            return true;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(h);
            return false;
        }
        // 只有大屏本体才登记句柄供 release_single_instance 释放
        if own_board {
            BOARD_MUTEX.store(h, Ordering::Release);
        }
        true
    }
}

/// 释放单实例互斥体（大屏服务退出时调用）。
pub fn release_single_instance() {
    use std::sync::atomic::Ordering;
    let h = BOARD_MUTEX.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if !h.is_null() {
        unsafe { CloseHandle(h) };
    }
}

/// 大屏互斥体当前是否被持有（旧实例是否仍在运行）。
pub fn board_mutex_held() -> bool {
    unsafe {
        let name = wide("GzzxDesktopBoardRustMutex");
        let h = OpenMutexW(SYNCHRONIZE, 0, name.as_ptr());
        if h.is_null() {
            return false;
        }
        CloseHandle(h);
        true
    }
}

/// 枚举所有显示器，返回按从左到右排序的 (x, y, w, h) 物理像素矩形。
pub fn list_monitors() -> Vec<(i32, i32, i32, i32)> {
    unsafe extern "system" fn cb(hmon: HMONITOR, _hdc: HDC, _rc: *mut RECT, data: isize) -> i32 {
        let out = &mut *(data as *mut Vec<(i32, i32, i32, i32)>);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            let r = mi.rcMonitor;
            out.push((r.left, r.top, r.right - r.left, r.bottom - r.top));
        }
        1
    }

    let mut out: Vec<(i32, i32, i32, i32)> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(cb),
            &mut out as *mut _ as isize,
        );
    }
    out.sort_by_key(|r| (r.0, r.1));
    out
}

/// 确定大屏窗口 / 壁纸的真实矩形 (x, y, w, h)。
///
/// Tk / Win32 的逻辑像素与物理像素在缩放屏上并不一致，这里统一以 Win32 物理像素为准。
pub fn screen_geometry(board: &Board) -> (i32, i32, i32, i32) {
    let (pw, ph) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let mons = list_monitors();
    let mut rect = board.manual_rect();
    if rect.is_none() {
        let idx = board.screen_index;
        if idx >= 0 && (idx as usize) < mons.len() {
            rect = Some(mons[idx as usize]);
        }
    }
    let (x, y, mut w, mut h) = rect.unwrap_or((0, 0, pw, ph));
    if w <= 0 || h <= 0 {
        w = if pw > 0 { pw } else { 1920 };
        h = if ph > 0 { ph } else { 1080 };
    }
    println!(
        "屏幕: Win32物理={}x{} | 显示器={:?} | 使用=({},{}) {}x{}",
        pw, ph, mons, x, y, w, h
    );
    (x, y, w, h)
}

/// 供调用方判断某个句柄是否有效
#[allow(dead_code)]
pub fn is_window(h: HWND) -> bool {
    unsafe { !h.is_null() && IsWindow(h) != 0 }
}
