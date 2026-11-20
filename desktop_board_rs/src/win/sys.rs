//! Win32 窄封装：只声明本程序真正用到的那部分 user32 / kernel32 / gdi32。
//!
//! 刻意不引第三方绑定库：签名与结构体布局全部按 Win32 头文件手写，
//! 升级工具链时不会因为绑定库版本变动而编译不过。

#![allow(non_snake_case, dead_code)]

use std::ffi::c_void;

pub type HWND = *mut c_void;
pub type HINSTANCE = *mut c_void;
pub type HDC = *mut c_void;
pub type HBITMAP = *mut c_void;
pub type HBRUSH = *mut c_void;
pub type HICON = *mut c_void;
pub type HCURSOR = *mut c_void;
pub type HMONITOR = *mut c_void;

// ---- 窗口样式 ----
pub const WS_OVERLAPPED: u32 = 0x0000_0000;
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const WS_CLIPCHILDREN: u32 = 0x0200_0000;
pub const WS_CAPTION: u32 = 0x00C0_0000;
pub const WS_SYSMENU: u32 = 0x0008_0000;
pub const WS_MINIMIZEBOX: u32 = 0x0002_0000;
pub const WS_THICKFRAME: u32 = 0x0004_0000;
pub const WS_VSCROLL: u32 = 0x0020_0000;
pub const WS_TABSTOP: u32 = 0x0001_0000;
pub const WS_BORDER: u32 = 0x0080_0000;
pub const WS_OVERLAPPEDWINDOW: u32 = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_THICKFRAME;

pub const WS_EX_LAYERED: u32 = 0x0008_0000;
pub const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
pub const WS_EX_TOPMOST: u32 = 0x0000_0008;
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
pub const WS_EX_APPWINDOW: u32 = 0x0004_0000;
pub const WS_EX_CLIENTEDGE: u32 = 0x0000_0200;

pub const GWL_STYLE: i32 = -16;
pub const GWL_EXSTYLE: i32 = -20;
pub const GWLP_USERDATA: i32 = -21;

pub const SW_HIDE: i32 = 0;
pub const SW_SHOW: i32 = 5;
pub const SW_SHOWNORMAL: i32 = 1;

pub const SWP_NOSIZE: u32 = 0x0001;
pub const SWP_NOMOVE: u32 = 0x0002;
/// SetWindowPos 的 z 序参数：置顶（传 -1）
pub const HWND_TOPMOST: HWND = -1isize as HWND;
/// SetWindowPos 的 z 序参数：沉底（传 1）——仍在壁纸与桌面图标之上，但压在所有普通窗口之下
pub const HWND_BOTTOM: HWND = 1isize as HWND;
pub const SWP_NOZORDER: u32 = 0x0004;
pub const SWP_NOACTIVATE: u32 = 0x0010;
pub const SWP_SHOWWINDOW: u32 = 0x0040;

// ---- 消息 ----
pub const PM_REMOVE: u32 = 0x0001;

pub const WM_DESTROY: u32 = 0x0002;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_PAINT: u32 = 0x000F;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_SIZE: u32 = 0x0005;
pub const WM_SETFONT: u32 = 0x0030;
pub const WM_ERASEBKGND: u32 = 0x0014;
pub const WM_CTLCOLORSTATIC: u32 = 0x0138;
pub const WM_QUIT: u32 = 0x0012;
pub const WM_INITDIALOG: u32 = 0x0110;
pub const WM_USER: u32 = 0x0400;
pub const WM_MOUSEMOVE: u32 = 0x0200;
/// 光标设置消息：窗口过程里必须自行 SetCursor，否则类光标为 NULL 时会沿用上一个窗口的光标
/// （表现为鼠标移到横幅上还留着系统的蓝色忙碌圆环）
pub const WM_SETCURSOR: u32 = 0x0020;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_MOUSEWHEEL: u32 = 0x020A;
// ---- 触摸（双指捏合缩放）----
pub const WM_TOUCH: u32 = 0x0240;
pub const WM_GESTURE: u32 = 0x0119;
/// RegisterTouchWindow flags：0 = 标准触摸（不隐藏触摸反馈）
pub const TWF_NONE: u32 = 0x0000_0000;
/// TOUCHINPUT.dwFlags：触点按下 / 抬起
pub const TOUCHEVENTF_DOWN: u32 = 0x0001;
pub const TOUCHEVENTF_UP: u32 = 0x0004;
/// TOUCHINPUT 的 x/y 单位是 1/100 像素
pub const TOUCH_COORD_TO_PIXEL: f32 = 100.0;

/// Win32 TOUCHINPUT（按 winuser.h 手写，尺寸布局必须与系统一致，否则 GetTouchInputInfo 失败）。
/// 注意 ULONG_PTR 在 64 位下是 8 字节——用 isize/usize 保证平台宽度。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TOUCHINPUT {
    pub x: i32,               // LONG，单位 1/100 px
    pub y: i32,
    pub hSource: *mut c_void, // HANDLE
    pub dwID: u32,            // 触点 ID（区分两根手指）
    pub dwFlags: u32,         // TOUCHEVENTF_*
    pub dwMask: u32,
    pub dwTime: u32,
    pub dwExtraInfo: usize,   // ULONG_PTR
    pub cxContact: u32,
    pub cyContact: u32,
}

pub const STM_SETIMAGE: u32 = 0x0172;
pub const IMAGE_BITMAP: usize = 0;

// ---- 控件样式 ----
pub const SS_LEFT: u32 = 0x0000;
pub const SS_CENTER: u32 = 0x0001;
pub const SS_LEFTNOWORDWRAP: u32 = 0x000C;
pub const SS_BITMAP: u32 = 0x000E;

pub const ES_LEFT: u32 = 0x0000;
pub const ES_MULTILINE: u32 = 0x0004;
pub const ES_AUTOHSCROLL: u32 = 0x0080;
pub const ES_NUMBER: u32 = 0x2000;

pub const BS_PUSHBUTTON: u32 = 0x0000;
pub const BS_DEFPUSHBUTTON: u32 = 0x0001;
pub const BS_AUTOCHECKBOX: u32 = 0x0003;
pub const BS_AUTORADIOBUTTON: u32 = 0x0009;
pub const BS_GROUPBOX: u32 = 0x0007;

pub const LBS_NOTIFY: u32 = 0x0001;
pub const LBS_NOINTEGRALHEIGHT: u32 = 0x0100;
pub const LB_ADDSTRING: u32 = 0x0180;
pub const LB_RESETCONTENT: u32 = 0x0184;
pub const LB_SETCURSEL: u32 = 0x0186;
pub const LB_GETCURSEL: u32 = 0x0188;
pub const LB_GETTEXT: u32 = 0x0189;
pub const LB_GETTEXTLEN: u32 = 0x018A;
pub const LB_GETCOUNT: u32 = 0x018B;

pub const EN_CHANGE: u16 = 0x0300;
pub const BN_CLICKED: u16 = 0;
pub const LBN_DBLCLK: u16 = 2;

// ---- 其它 ----
pub const IDC_ARROW: usize = 32512;
pub const COLOR_WINDOW: usize = 5;
pub const DIB_RGB_COLORS: u32 = 0;
pub const BI_RGB: u32 = 0;
pub const AC_SRC_ALPHA: u8 = 1;
pub const ULW_ALPHA: u32 = 0x0000_0002;
pub const SRCCOPY: u32 = 0x00CC_0020;

// 按钮消息（BM_*）
pub const BM_GETCHECK: u32 = 0x00F0;
pub const BM_SETCHECK: u32 = 0x00F1;
pub const BM_CLICK: u32 = 0x00F5;
pub const SM_CXSCREEN: i32 = 0;
pub const SM_CYSCREEN: i32 = 1;
pub const SPI_SETDESKWALLPAPER: u32 = 20;
pub const SPI_GETDESKWALLPAPER: u32 = 0x0073;
pub const SPIF_UPDATEINIFILE: u32 = 0x01;
pub const SPIF_SENDCHANGE: u32 = 0x02;
pub const MB_OK: u32 = 0x0000;
pub const MB_ICONINFORMATION: u32 = 0x0040;
pub const MB_ICONERROR: u32 = 0x0010;
pub const MB_YESNO: u32 = 0x0004;
pub const IDYES: i32 = 6;
/// 每屏独立 DPI 感知（Win10 1703+）
pub const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SIZE {
    pub cx: i32,
    pub cy: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MONITORINFO {
    pub cbSize: u32,
    pub rcMonitor: RECT,
    pub rcWork: RECT,
    pub dwFlags: u32,
}

#[repr(C)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: usize,
    pub lParam: isize,
    pub time: u32,
    pub pt: POINT,
    pub lPrivate: u32,
}

impl Default for MSG {
    fn default() -> Self {
        Self {
            hwnd: std::ptr::null_mut(),
            message: 0,
            wParam: 0,
            lParam: 0,
            time: 0,
            pt: POINT::default(),
            lPrivate: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RGBQUAD {
    pub rgbBlue: u8,
    pub rgbGreen: u8,
    pub rgbRed: u8,
    pub rgbReserved: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct BITMAPINFOHEADER {
    pub biSize: u32,
    pub biWidth: i32,
    pub biHeight: i32,
    pub biPlanes: u16,
    pub biBitCount: u16,
    pub biCompression: u32,
    pub biSizeImage: u32,
    pub biXPelsPerMeter: i32,
    pub biYPelsPerMeter: i32,
    pub biClrUsed: u32,
    pub biClrImportant: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct BITMAPINFO {
    pub bmiHeader: BITMAPINFOHEADER,
    pub bmiColors: [RGBQUAD; 1],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct BLENDFUNCTION {
    pub BlendOp: u8,
    pub BlendFlags: u8,
    pub SourceConstantAlpha: u8,
    pub AlphaFormat: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PAINTSTRUCT {
    pub hdc: HDC,
    pub fErase: i32,
    pub rcPaint: RECT,
    pub fRestore: i32,
    pub fIncUpdate: i32,
    pub rgbReserved: [u8; 32],
}

pub type WNDPROC = unsafe extern "system" fn(HWND, u32, usize, isize) -> isize;
pub type MONITORENUMPROC = unsafe extern "system" fn(HMONITOR, HDC, *mut RECT, isize) -> i32;

#[repr(C)]
pub struct WNDCLASSW {
    pub style: u32,
    pub lpfnWndProc: Option<WNDPROC>,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: HINSTANCE,
    pub hIcon: HICON,
    pub hCursor: HCURSOR,
    pub hbrBackground: HBRUSH,
    pub lpszMenuName: *const u16,
    pub lpszClassName: *const u16,
}

/// 转成以 0 结尾的宽字符，供所有 W 版 API 使用。
pub fn wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

#[link(name = "user32")]
extern "system" {
    pub fn RegisterClassW(lpWndClass: *const WNDCLASSW) -> u16;
    // ---- 触摸注册与原始触点读取（双指捏合缩放）----
    pub fn RegisterTouchWindow(hwnd: HWND, flags: u32) -> i32;
    pub fn GetTouchInputInfo(
        hTouchInput: isize,
        cInputs: u32,
        pInputs: *mut TOUCHINPUT,
        cbSize: i32,
    ) -> i32;
    pub fn CloseTouchInputHandle(hTouchInput: isize) -> i32;
    pub fn CreateWindowExW(
        dwExStyle: u32,
        lpClassName: *const u16,
        lpWindowName: *const u16,
        dwStyle: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: HWND,
        menu: *mut c_void,
        inst: HINSTANCE,
        param: *mut c_void,
    ) -> HWND;
    pub fn DefWindowProcW(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize;
    pub fn DestroyWindow(hwnd: HWND) -> i32;
    pub fn IsWindow(hwnd: HWND) -> i32;
    pub fn ShowWindow(hwnd: HWND, cmd: i32) -> i32;
    pub fn SetWindowPos(
        hwnd: HWND,
        after: HWND,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    pub fn GetMessageW(msg: *mut MSG, hwnd: HWND, min: u32, max: u32) -> i32;
    pub fn PeekMessageW(
        msg: *mut MSG,
        hwnd: HWND,
        min: u32,
        max: u32,
        remove: u32,
    ) -> i32;
    pub fn TranslateMessage(msg: *const MSG) -> i32;
    pub fn DispatchMessageW(msg: *const MSG) -> isize;
    pub fn PostQuitMessage(code: i32);
    pub fn SetTimer(hwnd: HWND, id: usize, ms: u32, proc: *mut c_void) -> usize;
    pub fn KillTimer(hwnd: HWND, id: usize) -> i32;
    pub fn InvalidateRect(hwnd: HWND, rect: *const RECT, erase: i32) -> i32;
    pub fn BeginPaint(hwnd: HWND, ps: *mut PAINTSTRUCT) -> HDC;
    pub fn EndPaint(hwnd: HWND, ps: *const PAINTSTRUCT) -> i32;
    pub fn GetDC(hwnd: HWND) -> HDC;
    pub fn ReleaseDC(hwnd: HWND, hdc: HDC) -> i32;
    pub fn GetClientRect(hwnd: HWND, rect: *mut RECT) -> i32;
    pub fn SetWindowLongPtrW(hwnd: HWND, index: i32, value: isize) -> isize;
    pub fn GetWindowLongPtrW(hwnd: HWND, index: i32) -> isize;
    pub fn SetWindowTextW(hwnd: HWND, text: *const u16) -> i32;
    pub fn GetWindowTextW(hwnd: HWND, buf: *mut u16, max: i32) -> i32;
    pub fn GetWindowTextLengthW(hwnd: HWND) -> i32;
    pub fn SendMessageW(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize;
    pub fn MessageBoxW(hwnd: HWND, text: *const u16, caption: *const u16, flags: u32) -> i32;
    pub fn EnableWindow(hwnd: HWND, enable: i32) -> i32;
    pub fn SetFocus(hwnd: HWND) -> HWND;
    pub fn BringWindowToTop(hwnd: HWND) -> i32;
    pub fn SetForegroundWindow(hwnd: HWND) -> i32;
    pub fn UpdateWindow(hwnd: HWND) -> i32;
    pub fn LoadCursorW(inst: HINSTANCE, name: *const u16) -> HCURSOR;
    pub fn EnumDisplayMonitors(
        hdc: HDC,
        clip: *const RECT,
        cb: Option<MONITORENUMPROC>,
        data: isize,
    ) -> i32;
    pub fn GetMonitorInfoW(hmon: HMONITOR, info: *mut MONITORINFO) -> i32;
    pub fn GetSystemMetrics(index: i32) -> i32;
    pub fn SystemParametersInfoW(action: u32, param: u32, pv: *mut c_void, winini: u32) -> i32;
    pub fn GetForegroundWindow() -> HWND;
    pub fn GetClassNameW(hwnd: HWND, buf: *mut u16, max: i32) -> i32;
    pub fn FindWindowW(class: *const u16, title: *const u16) -> HWND;
    pub fn keybd_event(bvk: u8, scan: u8, flags: u32, extra: usize);
    pub fn SetProcessDPIAware() -> i32;
    pub fn SetProcessDpiAwarenessContext(ctx: isize) -> i32;
    pub fn MoveWindow(hwnd: HWND, x: i32, y: i32, w: i32, h: i32, repaint: i32) -> i32;
    pub fn GetWindowRect(hwnd: HWND, rect: *mut RECT) -> i32;
    pub fn GetCursorPos(pt: *mut POINT) -> i32;
    pub fn SetCapture(hwnd: HWND) -> HWND;
    pub fn ReleaseCapture() -> i32;
    /// 设置当前光标；返回上一个光标句柄
    pub fn SetCursor(cursor: HCURSOR) -> HCURSOR;
    pub fn UpdateLayeredWindow(
        hwnd: HWND,
        dst: HDC,
        dst_pt: *const POINT,
        size: *const SIZE,
        src: HDC,
        src_pt: *const POINT,
        key: u32,
        blend: *const BLENDFUNCTION,
        flags: u32,
    ) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    pub fn CreateCompatibleDC(hdc: HDC) -> HDC;
    pub fn DeleteDC(hdc: HDC) -> i32;
    pub fn CreateDIBSection(
        hdc: HDC,
        bmi: *const BITMAPINFO,
        usage: u32,
        bits: *mut *mut c_void,
        section: *mut c_void,
        offset: u32,
    ) -> HBITMAP;
    pub fn SelectObject(hdc: HDC, obj: *mut c_void) -> *mut c_void;
    pub fn DeleteObject(obj: *mut c_void) -> i32;
    pub fn GetStockObject(index: i32) -> *mut c_void;
    pub fn StretchDIBits(
        hdc: HDC,
        xdest: i32,
        ydest: i32,
        wdest: i32,
        hdest: i32,
        xsrc: i32,
        ysrc: i32,
        wsrc: i32,
        hsrc: i32,
        bits: *const c_void,
        bmi: *const BITMAPINFO,
        usage: u32,
        rop: u32,
    ) -> i32;
    pub fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strikeout: u32,
        charset: u32,
        outprecision: u32,
        clipprecision: u32,
        quality: u32,
        pitchandfamily: u32,
        facename: *const u16,
    ) -> *mut c_void;
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
    pub fn GetLastError() -> u32;
    pub fn CreateMutexW(attrs: *const c_void, owner: i32, name: *const u16) -> *mut c_void;
    pub fn OpenMutexW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
    pub fn CloseHandle(h: *mut c_void) -> i32;
    pub fn Sleep(ms: u32);
    // ---- 只读内存映射（字体文件用它，避免把 36MB 字体读成私有堆内存）----
    pub fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        attrs: *mut c_void,
        disposition: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    pub fn GetFileSize(file: *mut c_void, high: *mut u32) -> u32;
    pub fn CreateFileMappingW(
        file: *mut c_void,
        attrs: *mut c_void,
        protect: u32,
        max_high: u32,
        max_low: u32,
        name: *const u16,
    ) -> *mut c_void;
    pub fn MapViewOfFile(
        map: *mut c_void,
        access: u32,
        offset_high: u32,
        offset_low: u32,
        bytes: usize,
    ) -> *mut c_void;
}

pub const GENERIC_READ: u32 = 0x8000_0000;
pub const FILE_SHARE_READ: u32 = 0x0000_0001;
pub const OPEN_EXISTING: u32 = 3;
pub const PAGE_READONLY: u32 = 0x02;
pub const FILE_MAP_READ: u32 = 0x0004;
/// CreateFileW 失败时返回的伪句柄
pub const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;

pub const ERROR_ALREADY_EXISTS: u32 = 183;
pub const SYNCHRONIZE: u32 = 0x00100000;
pub const WHITE_BRUSH: i32 = 0;
pub const BTN_FACE_BRUSH: i32 = 0;

/// 读控件文本。
pub fn text_of(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 2];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

pub fn set_text(hwnd: HWND, s: &str) {
    let w = wide(s);
    unsafe {
        SetWindowTextW(hwnd, w.as_ptr());
    }
}

pub fn msg_box(text: &str, caption: &str, flags: u32) {
    let t = wide(text);
    let c = wide(caption);
    unsafe {
        MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), flags);
    }
}
