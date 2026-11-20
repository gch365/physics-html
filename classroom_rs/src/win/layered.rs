//! 分层悬浮窗口基座：WS_POPUP + 逐像素 alpha（UpdateLayeredWindow）。
//!
//! 与 Python 版 Tk 的 overrideredirect+topmost+noactivate 等价，
//! 圆角与抗锯齿由 tiny-skia 画进 alpha 通道实现。

use tiny_skia::Pixmap;

use super::dib::Dib;
use super::sys::*;

/// 注册一个无边框、置顶、不抢焦点的窗口类并创建实例。
pub fn create_floating(class: &str, title: &str, w: i32, h: i32, proc_: WNDPROC) -> Option<HWND> {
    let inst = unsafe { GetModuleHandleW(std::ptr::null()) };
    // 类名 buffer 必须活过 RegisterClassW（WNDCLASSW 只存指针）
    let cls_buf = wide(class);
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(proc_),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: inst,
        hIcon: std::ptr::null_mut(),
        hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16) },
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: cls_buf.as_ptr(),
    };
    // 类已注册时忽略失败
    unsafe { RegisterClassW(&wc) };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            cls_buf.as_ptr(),
            wide(title).as_ptr(),
            WS_POPUP,
            0,
            0,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            inst,
            std::ptr::null_mut(),
        )
    };
    if hwnd.is_null() {
        None
    } else {
        Some(hwnd)
    }
}

/// 显示 / 隐藏（不抢焦点）。
pub fn show(hwnd: HWND, visible: bool) {
    unsafe {
        ShowWindow(hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
    }
}

/// 移动窗口（不重绘内容，需要调用方 present）。
pub fn move_to(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

/// 把渲染好的 Pixmap 贴到分层窗口（RGBA→预乘 BGRA）。
pub fn present(hwnd: HWND, pm: &Pixmap) -> bool {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let Some(mut dib) = Dib::new(w, h) else { return false };
    let src = pm.data();
    let dst = dib.as_mut_slice();
    for i in 0..(src.len() / 4) {
        let (r, g, b, a) = (src[i * 4], src[i * 4 + 1], src[i * 4 + 2], src[i * 4 + 3]);
        dst[i * 4] = b;
        dst[i * 4 + 1] = g;
        dst[i * 4 + 2] = r;
        dst[i * 4 + 3] = a;
    }
    let size = SIZE { cx: w, cy: h };
    let src_pt = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: 0,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA,
    };
    unsafe {
        UpdateLayeredWindow(
            hwnd,
            std::ptr::null_mut(),
            std::ptr::null(),
            &size,
            dib.hdc,
            &src_pt,
            0,
            &blend,
            ULW_ALPHA,
        ) != 0
    }
}

/// 通用绘制底图：透明底 + 圆角矩形填充。
pub fn rounded_bg(pm: &mut Pixmap, radius: f32, fill: [u8; 4]) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let mut pb = tiny_skia::PathBuilder::new();
    push_round_rect(&mut pb, 0.0, 0.0, w, h, radius);
    if let Some(path) = pb.finish() {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color(tiny_skia::Color::from_rgba8(fill[0], fill[1], fill[2], fill[3]));
        paint.anti_alias = true;
        pm.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    }
}

/// PathBuilder 版圆角矩形（tiny-skia 0.12 无 push_round_rect，用三次贝塞尔近似）。
pub fn push_round_rect(pb: &mut tiny_skia::PathBuilder, x: f32, y: f32, w: f32, h: f32, r: f32) {
    let r = r.max(0.0).min(w / 2.0).min(h / 2.0);
    if r <= 0.01 {
        if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) {
            pb.push_rect(rect);
        }
        return;
    }
    let k = 0.5523_f32 * r;
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
}
