//! 桌面壁纸读写：现在大屏不再替换壁纸，这里只保留「查当前壁纸 / 还原旧版遗留壁纸」。
//! 设置壁纸的几个函数留作迁移与排错，正常运行时不再调用。
#![allow(dead_code)]

use crate::paths;

use super::sys::*;

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as isize;
const KEY_SET_VALUE: u32 = 0x0002;
const REG_SZ: u32 = 1;

#[link(name = "advapi32")]
extern "system" {
    fn RegOpenKeyExW(
        hkey: isize,
        subkey: *const u16,
        opts: u32,
        sam: u32,
        result: *mut isize,
    ) -> i32;
    fn RegSetValueExW(
        hkey: isize,
        name: *const u16,
        reserved: u32,
        typ: u32,
        data: *const u8,
        len: u32,
    ) -> i32;
    fn RegCloseKey(hkey: isize) -> i32;
}

/// 壁纸显示方式设为「填充」且不平铺（幂等，每次启动写一遍也很快）。
pub fn wallpaper_style_once() {
    unsafe {
        let sub = wide(r"Control Panel\Desktop");
        let mut key: isize = 0;
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_SET_VALUE, &mut key) != 0 {
            return;
        }
        for (name, value) in [("WallpaperStyle", "10"), ("TileWallpaper", "0")] {
            let n = wide(name);
            let v = wide(value);
            RegSetValueExW(
                key,
                n.as_ptr(),
                0,
                REG_SZ,
                v.as_ptr() as *const u8,
                (v.len() * 2) as u32,
            );
        }
        RegCloseKey(key);
    }
}

fn set_global(path: &str) -> bool {
    let w = wide(path);
    unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            w.as_ptr() as *mut std::ffi::c_void,
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        ) != 0
    }
}

/// 读取当前系统壁纸路径（大屏启动前保存，退出时还原）。
pub fn get_global_wallpaper() -> Option<String> {
    let mut buf = [0u16; 1024];
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            buf.len() as u32,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            0,
        )
    };
    if ok == 0 {
        return None;
    }
    let s = String::from_utf16_lossy(&buf)
        .trim_end_matches('\0')
        .trim()
        .to_string();
    (!s.is_empty()).then_some(s)
}

/// 退出大屏时还原用户原来的桌面背景。
pub fn restore_wallpaper(path: &str) -> bool {
    if path.trim().is_empty() {
        return false;
    }
    set_global(path)
}

/// 只替换目标显示器（多屏时不殃及教师机屏幕）。COM 不可用返回 false。
fn set_for_monitor(path: &str, rect: (i32, i32, i32, i32)) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{DesktopWallpaper, IDesktopWallpaper};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dw: IDesktopWallpaper = match CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL) {
            Ok(d) => d,
            Err(e) => {
                println!("IDesktopWallpaper 不可用，退回全局设置: {e}");
                return false;
            }
        };
        let count = match dw.GetMonitorDevicePathCount() {
            Ok(c) => c,
            Err(_) => return false,
        };
        let (rx, ry, rw, rh) = rect;
        let mut target: Option<String> = None;
        for i in 0..count {
            let Ok(id) = dw.GetMonitorDevicePathAt(i) else {
                continue;
            };
            let Ok(name) = id.to_string() else { continue };
            let Ok(r) = dw.GetMonitorRECT(&HSTRING::from(&name)) else {
                continue;
            };
            if (r.left, r.top, r.right - r.left, r.bottom - r.top) == (rx, ry, rw, rh) {
                target = Some(name);
            }
        }
        let Some(name) = target else {
            return false;
        };
        dw.SetWallpaper(&HSTRING::from(name), &HSTRING::from(path))
            .is_ok()
    }
}

/// 设置桌面壁纸。rect 给定时优先只替换该显示器。
pub fn set_desktop_wallpaper(path: &std::path::Path, rect: Option<(i32, i32, i32, i32)>) -> bool {
    let p = path.to_string_lossy().to_string();
    if let Some(r) = rect {
        if set_for_monitor(&p, r) {
            return true;
        }
    }
    set_global(&p)
}

/// 把渲染好的图设为壁纸，并把显示方式固定为填充。
#[allow(dead_code)]
pub fn apply(path: &std::path::Path, rect: (i32, i32, i32, i32)) -> bool {
    if !path.exists() {
        println!("壁纸文件不存在: {}", path.display());
        return false;
    }
    wallpaper_style_once();
    set_desktop_wallpaper(path, Some(rect))
}

/// 壁纸输出路径（供外部打印）
#[allow(dead_code)]
pub fn out_path() -> std::path::PathBuf {
    paths::wall_out()
}
