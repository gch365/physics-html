//! 危险操作确认框：必须输入确认码并点「确定」才放行。
//!
//! `MessageBoxW` 只能点是/否，不能输入文字，所以这里用原生 Win32 控件自建一个
//! 小型模态窗口（STATIC 提示 + EDIT 输入框 + 确定/取消）。调用方在窗口过程里
//! 阻塞等待：只有「输入正确 + 点确定」返回 true，取消 / 关闭 / ESC / 输错都返回 false。
//!
//! 目前用于作业卡片右下角的电源按钮（彻底关闭作业栏）和红色 ✕（本次隐藏作业栏）。

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::OnceLock;

use super::sys::*;

const ID_EDIT: i32 = 1001;
const ID_OK: i32 = 1002;
const ID_CANCEL: i32 = 1003;

const WM_KEYDOWN: u32 = 0x0100;
const VK_RETURN: usize = 0x0D;
const VK_ESCAPE: usize = 0x1B;
const WS_EX_DLGMODALFRAME: u32 = 0x0000_0001;

const DLG_W: i32 = 470;
const DLG_H: i32 = 278;

struct State {
    edit: HWND,
    code: String,
    ok: bool,
}

/// 纯函数，便于单测：输入码与期望码一致（允许首尾空格），空输入一律拒绝。
pub fn code_matches(typed: &str, expected: &str) -> bool {
    let t = typed.trim();
    let e = expected.trim();
    !t.is_empty() && !e.is_empty() && t == e
}

thread_local! {
    static ST: RefCell<Option<State>> = const { RefCell::new(None) };
}

static GUI_FONT: OnceLock<usize> = OnceLock::new();

fn gui_font() -> usize {
    unsafe {
        *GUI_FONT.get_or_init(|| {
            let face = wide("Microsoft YaHei");
            CreateFontW(-16, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr()) as usize
        })
    }
}

unsafe fn control(
    parent: HWND,
    class: &str,
    text: &str,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
) -> HWND {
    let cls = wide(class);
    let t = wide(text);
    let hwnd = CreateWindowExW(
        0,
        cls.as_ptr(),
        t.as_ptr(),
        WS_CHILD | WS_VISIBLE | style,
        x,
        y,
        w,
        h,
        parent,
        id as usize as *mut c_void,
        GetModuleHandleW(std::ptr::null()),
        std::ptr::null_mut(),
    );
    if !hwnd.is_null() {
        SendMessageW(hwnd, WM_SETFONT, gui_font(), 1);
    }
    hwnd
}

unsafe fn create(owner: HWND, title: &str) -> HWND {
    let inst = GetModuleHandleW(std::ptr::null());
    let cls = wide("BoardConfirmRust");
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: inst,
        hIcon: std::ptr::null_mut(),
        hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16),
        hbrBackground: GetStockObject(WHITE_BRUSH),
        lpszMenuName: std::ptr::null(),
        lpszClassName: cls.as_ptr(),
    };
    RegisterClassW(&wc);
    let (sx, sy) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
    CreateWindowExW(
        // 作业卡片本身是 TOPMOST，确认框也置顶，否则会被卡片盖住。
        WS_EX_DLGMODALFRAME | WS_EX_TOPMOST,
        cls.as_ptr(),
        wide(title).as_ptr(),
        WS_OVERLAPPEDWINDOW & !WS_THICKFRAME & !WS_MINIMIZEBOX,
        (sx - DLG_W) / 2,
        (sy - DLG_H) / 2,
        DLG_W,
        DLG_H,
        owner,
        std::ptr::null_mut(),
        inst,
        std::ptr::null_mut(),
    )
}

/// 点「确定」/ 回车：校验输入码。错误时保留窗口让用户重输，绝不执行操作。
fn submit(win: HWND) {
    let (edit, ok) = ST.with(|s| {
        let mut b = s.borrow_mut();
        let Some(st) = b.as_mut() else {
            return (std::ptr::null_mut(), false);
        };
        let typed = text_of(st.edit);
        let ok = code_matches(&typed, &st.code);
        st.ok = ok;
        (st.edit, ok)
    });
    if ok {
        unsafe { DestroyWindow(win) };
    } else {
        let t = wide("确认码不正确，作业栏没有被关闭。请重新输入后点「确定」。");
        let c = wide("无法关闭");
        // 以确认框为 owner：MessageBox 弹出期间禁用它，避免重复点「确定」造成嵌套。
        unsafe { MessageBoxW(win, t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONERROR) };
        if !edit.is_null() {
            set_text(edit, "");
            unsafe { SetFocus(edit) };
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    if msg == WM_COMMAND {
        match (wp & 0xFFFF) as i32 {
            ID_OK => {
                submit(hwnd);
                return 0;
            }
            ID_CANCEL => {
                DestroyWindow(hwnd);
                return 0;
            }
            _ => {}
        }
    }
    if msg == WM_CLOSE {
        DestroyWindow(hwnd);
        return 0;
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// 弹出确认框并阻塞等待；输入 `code` 且点「确定」才返回 true。
/// `owner` 为空时不做禁用（仍是本线程内的模态循环）。
pub fn confirm_code(owner: HWND, code: &str) -> bool {
    confirm_code_ex(
        owner,
        code,
        "确认彻底关闭作业栏",
        "此操作将永久关闭桌面作业栏：重启程序、重启大屏都不会自动打开。\r\n\
         需要重新显示时，只能到主程序「设置 → 基本 → 桌面作业栏」打开。\r\n\r\n\
         请输入确认码 123 后点「确定」继续：",
    )
}

/// 同 `confirm_code`，但标题与提示正文由调用方指定（✕ 隐藏 / 电源彻底关闭共用一套控件）。
pub fn confirm_code_ex(owner: HWND, code: &str, title: &str, body: &str) -> bool {
    ST.with(|s| *s.borrow_mut() = None);
    let (win, edit) = unsafe {
        let win = create(owner, title);
        if win.is_null() {
            return false;
        }
        control(
            win,
            "STATIC",
            body,
            SS_LEFT,
            24,
            18,
            420,
            112,
            0,
        );
        control(win, "STATIC", "确认码：", SS_LEFT, 24, 148, 78, 26, 0);
        let edit = control(
            win,
            "EDIT",
            "",
            ES_AUTOHSCROLL | ES_NUMBER | WS_BORDER | WS_TABSTOP,
            104,
            144,
            130,
            30,
            ID_EDIT,
        );
        control(
            win,
            "BUTTON",
            "确定",
            BS_DEFPUSHBUTTON | WS_TABSTOP,
            224,
            196,
            100,
            36,
            ID_OK,
        );
        control(
            win,
            "BUTTON",
            "取消",
            BS_PUSHBUTTON | WS_TABSTOP,
            338,
            196,
            100,
            36,
            ID_CANCEL,
        );
        (win, edit)
    };
    ST.with(|s| {
        *s.borrow_mut() = Some(State {
            edit,
            code: code.trim().to_string(),
            ok: false,
        })
    });

    unsafe {
        if !owner.is_null() {
            EnableWindow(owner, 0);
        }
        ShowWindow(win, SW_SHOW);
        SetForegroundWindow(win);
        SetFocus(edit);
        UpdateWindow(win);
    }

    let mut msg = MSG::default();
    loop {
        // 必须用非阻塞的 PeekMessageW：窗口被 DestroyWindow 后不会产生队列消息，
        // 若用阻塞的 GetMessageW，循环会一直卡在取消息上，IsWindow 检查永远轮不到，
        // 表现为「点了确定/取消但确认框关不掉、整个大屏卡住」。
        if unsafe { IsWindow(win) } == 0 {
            break;
        }
        let got = unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) };
        if got == 0 {
            unsafe { Sleep(10) };
            continue;
        }
        if msg.message == WM_QUIT {
            // 还回队列，否则主窗口的正常退出会被这个局部循环吞掉。
            unsafe { PostQuitMessage(msg.wParam as i32) };
            break;
        }
        // 普通窗口不会自动把回车映射到默认按钮，手动处理回车 / ESC。
        if msg.message == WM_KEYDOWN && msg.wParam == VK_RETURN {
            submit(win);
            continue;
        }
        if msg.message == WM_KEYDOWN && msg.wParam == VK_ESCAPE {
            unsafe { DestroyWindow(win) };
            continue;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        if !owner.is_null() {
            EnableWindow(owner, 1);
        }
        if IsWindow(win) != 0 {
            DestroyWindow(win);
        }
    }
    let ok = ST.with(|s| s.borrow().as_ref().map(|s| s.ok).unwrap_or(false));
    ST.with(|s| *s.borrow_mut() = None);
    ok
}

#[cfg(test)]
mod tests {
    use super::code_matches;

    #[test]
    fn only_exact_code_is_accepted() {
        assert!(code_matches("123", "123"));
        assert!(code_matches(" 123 ", "123"));
        assert!(!code_matches("", "123"));
        assert!(!code_matches("12", "123"));
        assert!(!code_matches("1234", "123"));
        assert!(!code_matches("321", "123"));
        assert!(!code_matches("abc", "123"));
        assert!(!code_matches("123", ""), "空期望码不能放行");
    }
}

/// 真实 GUI 冒烟：会短暂弹出确认框，因此平时 `#[ignore]`，手动执行
/// `cargo test --release -- --ignored --nocapture gui_smoke` 验证窗口链路。
#[cfg(test)]
mod gui_smoke {
    use super::*;
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    #[link(name = "user32")]
    extern "system" {
        fn FindWindowExW(
            parent: HWND,
            after: HWND,
            class: *const u16,
            title: *const u16,
        ) -> HWND;
    }

    /// 等确认窗口出现，返回窗口与输入框句柄。
    fn wait_window() -> (HWND, HWND) {
        let cls = wide("BoardConfirmRust");
        let mut win = std::ptr::null_mut();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            win = unsafe { FindWindowW(cls.as_ptr(), std::ptr::null()) };
            if !win.is_null() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!win.is_null(), "确认框没有创建出来");
        let edit_cls = wide("EDIT");
        let edit = unsafe {
            FindWindowExW(
                win,
                std::ptr::null_mut(),
                edit_cls.as_ptr(),
                std::ptr::null(),
            )
        };
        assert!(!edit.is_null(), "确认框里找不到输入控件");
        // 让窗口线程把 ShowWindow / SetFocus 处理完
        thread::sleep(Duration::from_millis(120));
        (win, edit)
    }

    fn run_confirm() -> (Arc<AtomicI32>, thread::JoinHandle<()>) {
        let done = Arc::new(AtomicI32::new(-1));
        let r = done.clone();
        let h = thread::spawn(move || {
            let ok = super::confirm_code(std::ptr::null_mut(), "123");
            r.store(if ok { 1 } else { 0 }, Ordering::Relaxed);
        });
        (done, h)
    }

    #[test]
    #[ignore = "会弹出真实窗口，手动验证时加 --ignored"]
    fn cancel_and_ok_paths() {
        // 1) 直接关闭窗口 → 必须返回 false，且不写任何持久标志
        let (done, h) = run_confirm();
        let (win, _) = wait_window();
        unsafe { SendMessageW(win, WM_CLOSE, 0, 0) };
        h.join().expect("取消路径的模态循环没有退出");
        assert_eq!(done.load(Ordering::Relaxed), 0, "取消时必须返回 false");

        // 2) 输入 123 + 点确定 → 返回 true
        let (done, h) = run_confirm();
        let (win, edit) = wait_window();
        set_text(edit, "123");
        unsafe { SendMessageW(win, WM_COMMAND, ID_OK as usize, 0) };
        h.join().expect("确定路径的模态循环没有退出");
        assert_eq!(done.load(Ordering::Relaxed), 1, "输入正确时必须返回 true");
    }
}
