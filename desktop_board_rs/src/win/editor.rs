//! 大屏设置面板（原生 Win32 控件）：**一个窗口，三块设置同窗切换**。
//!
//! 点击作业栏任意位置直接打开本窗口，默认就是「今日作业」编辑表；
//! 顶部三个按钮切换到「班干值周」「卫生值日」，只在同一窗口内换控件显隐，
//! 不再弹子窗口、不再有中间选择界面。
//!
//! 已移除整屏壁纸预览：`Renderer` / 1120×630 预览位图 / 状态栏里的作息来源
//! 全部删掉。预览本来只是「示意排版」，现在改完点保存由大屏自己的 400ms
//! 轮询读 board.json 生效，编辑器不需要重绘任何位图，也省下一份运行时大内存。
//!
//! 只做三件事：改作业、改班干值周、改卫生值日。

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;

use chrono::Datelike;

use crate::board::{self, Board, HomeworkItem};
use crate::config::{self, AppConfig};
use crate::theme::WEEK_CN;

use super::sys::*;
use super::editor_instance_ok;

// ---- 控件 ID ----
const ID_BTN_CANCEL: i32 = 201;
const ID_HW_ADD: i32 = 210;
const ID_WEEK_CLEAR: i32 = 220;
const ID_TAB_HW: i32 = 231;
const ID_TAB_DUTY: i32 = 232;
const ID_TAB_TASKS: i32 = 233;
const ID_DAY_BASE: i32 = 400; // 400..406 目标星期
const ID_WEEK_ROW: i32 = 420; // 420..426 七天输入框
const ID_STUDENTS: i32 = 500;
const ID_SEARCH: i32 = 501;
const ID_HW_ROW: i32 = 600; // 600+3i / 601+3i / 602+3i
const ID_STATUS: i32 = 810;
const ID_TIMER_AUTOSAVE: usize = 3; // 输入防抖自动保存定时器

/// 停止输入后多久自动落盘。
const AUTOSAVE_DEBOUNCE_MS: u32 = 800;

const MAX_HW_ROWS: usize = 10;
const WIN_W: i32 = 1040;
const WIN_H: i32 = 700;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Homework,
    Duty,
    Tasks,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Homework => "今日作业",
            Tab::Duty => "班干部值周",
            Tab::Tasks => "卫生值日",
        }
    }
}

struct Editor {
    board: Board,
    cfg: AppConfig,
    students: Vec<String>,
    main: HWND,
    status: HWND,
    close_btn: HWND,
    ctl: HashMap<i32, HWND>,
    /// 两组控件的句柄（含静态标签），切页时整组 ShowWindow / Hide。
    hw_ctls: Vec<HWND>,
    week_ctls: Vec<HWND>,
    tab: Tab,
    hw_rows: usize,
    /// 程序化 set_text（保存回填/切页填值/清空）期间置位：抑制 EN_CHANGE 触发自动保存。
    suppress_save: bool,
}

thread_local! {
    static ST: RefCell<Option<Box<Editor>>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Editor) -> R) -> Option<R> {
    // try_borrow_mut：创建带初始文本的 EDIT 会同步触发 EN_CHANGE，
    // 而此刻外层 with() 正持有借用（build_controls 在 with() 里跑），
    // 直接 borrow_mut 会 panic 且跨 FFI 无法 unwind → 进程 abort。
    ST.with(|s| {
        let mut b = s.try_borrow_mut().ok()?;
        let e = b.as_mut()?; // Option<&mut Box<Editor>>
        Some(f(e.as_mut()))
    })
}

// ====================== 控件工具 ======================

unsafe fn make_font() -> *mut c_void {
    let face = wide("Microsoft YaHei");
    CreateFontW(-16, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr())
}

static GUI_FONT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

unsafe fn mk(
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
        let f = *GUI_FONT.get_or_init(|| unsafe { make_font() as usize });
        SendMessageW(hwnd, WM_SETFONT, f, 1);
    }
    hwnd
}

unsafe fn mk_label(parent: HWND, text: &str, x: i32, y: i32, w: i32, h: i32) -> HWND {
    mk(parent, "STATIC", text, SS_LEFT, x, y, w, h, 0)
}

unsafe fn mk_top(title: &str, x: i32, y: i32, w: i32, h: i32) -> Option<HWND> {
    let inst = GetModuleHandleW(std::ptr::null());
    let cls = wide("BoardEditorRust");
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(panel_proc),
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
    let hwnd = CreateWindowExW(
        0,
        cls.as_ptr(),
        wide(title).as_ptr(),
        // 可拉伸：保留 THICKFRAME / MAXIMIZEBOX，4K 屏可自由调整窗口大小
        WS_OVERLAPPEDWINDOW,
        x,
        y,
        w,
        h,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        inst,
        std::ptr::null_mut(),
    );
    if hwnd.is_null() {
        None
    } else {
        Some(hwnd)
    }
}

fn center_rect(w: i32, h: i32) -> (i32, i32) {
    let (sx, sy) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    ((sx - w) / 2, (sy - h) / 2)
}

// ====================== 入口 ======================

pub fn run() {
    if !editor_instance_ok() {
        println!("大屏编辑器已在运行，跳过本次启动");
        return;
    }
    let cfg = config::load_app_config();
    let board = board::load(&cfg.raw_name);
    let (x, y) = center_rect(WIN_W, WIN_H);
    let Some(main) = (unsafe {
        mk_top("班级桌面大屏设置（作业 / 班干 / 卫生）", x, y, WIN_W, WIN_H)
    }) else {
        println!("创建设置面板失败");
        return;
    };

    let close_btn = unsafe {
        // 顶部三个切换按钮固定在顶部；「关闭」和状态栏在 WM_SIZE 里锚定到底部
        mk(main, "BUTTON", "今日作业", BS_PUSHBUTTON, 20, 12, 150, 34, ID_TAB_HW);
        mk(main, "BUTTON", "班干部值周", BS_PUSHBUTTON, 182, 12, 150, 34, ID_TAB_DUTY);
        mk(main, "BUTTON", "卫生值日", BS_PUSHBUTTON, 344, 12, 150, 34, ID_TAB_TASKS);
        mk(main, "BUTTON", "关闭", BS_PUSHBUTTON, WIN_W - 110, WIN_H - 62, 90, 36, ID_BTN_CANCEL)
    };
    let status = unsafe {
        mk(main, "STATIC", "", SS_LEFTNOWORDWRAP, 20, WIN_H - 96, WIN_W - 140, 24, ID_STATUS)
    };

    let st = Box::new(Editor {
        board,
        cfg,
        students: Vec::new(),
        main,
        status,
        close_btn,
        ctl: HashMap::new(),
        hw_ctls: Vec::new(),
        week_ctls: Vec::new(),
        tab: Tab::Homework,
        hw_rows: 0,
        suppress_save: false,
    });
    ST.with(|s| *s.borrow_mut() = Some(st));

    with(|e| {
        e.students = e.cfg.students.clone();
        e.build_controls();
        e.switch_tab(Tab::Homework);
    });

    unsafe {
        ShowWindow(main, SW_SHOW);
        UpdateWindow(main);
    }
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if r <= 0 {
            break;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    board::flush();
}

// ====================== 面板逻辑 ======================

impl Editor {
    fn set_status(&self, text: &str) {
        set_text(self.status, text);
    }

    /// 一次性把三组控件都建好，之后切页只改显隐，不重建窗口。
    fn build_controls(&mut self) {
        let win = self.main;
        unsafe {
            // ---- 今日作业：10 行 × (科目 / 内容 / 备注) ----
            self.hw_ctls.push(mk_label(
                win,
                "科目 / 作业内容 / 备注（留空科目即删除该行）",
                20,
                58,
                760,
                22,
            ));
            let rows = self.board.homework.clone();
            for i in 0..MAX_HW_ROWS {
                let y = 88 + i as i32 * 46;
                let (s, c, n) = rows
                    .get(i)
                    .map(|r| (r.subject.clone(), r.content.clone(), r.note.clone()))
                    .unwrap_or_default();
                let a = mk(win, "EDIT", &s, ES_AUTOHSCROLL | WS_BORDER, 20, y, 100, 30, ID_HW_ROW + i as i32 * 3);
                let b = mk(win, "EDIT", &c, ES_AUTOHSCROLL | WS_BORDER, 130, y, 560, 30, ID_HW_ROW + 1 + i as i32 * 3);
                let ctl = mk(win, "EDIT", &n, ES_AUTOHSCROLL | WS_BORDER, 700, y, 310, 30, ID_HW_ROW + 2 + i as i32 * 3);
                self.ctl.insert(ID_HW_ROW + i as i32 * 3, a);
                self.ctl.insert(ID_HW_ROW + 1 + i as i32 * 3, b);
                self.ctl.insert(ID_HW_ROW + 2 + i as i32 * 3, ctl);
                self.hw_ctls.push(a);
                self.hw_ctls.push(b);
                self.hw_ctls.push(ctl);
                if i >= rows.len() {
                    ShowWindow(a, SW_HIDE);
                    ShowWindow(b, SW_HIDE);
                    ShowWindow(ctl, SW_HIDE);
                }
            }
            let add = mk(win, "BUTTON", "+ 添加科目", BS_PUSHBUTTON, 20, 552, 120, 34, ID_HW_ADD);
            self.hw_ctls.push(add);
            self.hw_rows = rows.len();

            // ---- 班干值周 / 卫生值日：共用一套「一周 7 行 + 学生名单」控件 ----
            self.week_ctls.push(mk_label(
                win,
                "先点「填入」选中星期，再双击右侧姓名即填入该天；也可直接输入，多项用「；」隔开。",
                20,
                58,
                960,
                22,
            ));
            self.week_ctls.push(mk_label(win, "填入：", 20, 92, 60, 22));
            let today = crate::clock::now().date_naive().weekday().num_days_from_monday() as i32;
            for (i, day) in WEEK_CN.iter().enumerate() {
                let short = day.replace("星期", "周");
                let r = mk(
                    win,
                    "BUTTON",
                    &short,
                    BS_AUTORADIOBUTTON | (if i == 0 { WS_TABSTOP } else { 0 }),
                    80 + i as i32 * 66,
                    88,
                    62,
                    24,
                    ID_DAY_BASE + i as i32,
                );
                SendMessageW(r, BM_SETCHECK, if i as i32 == today { 1usize } else { 0 }, 0);
                self.ctl.insert(ID_DAY_BASE + i as i32, r);
                self.week_ctls.push(r);
            }
            for (i, day) in WEEK_CN.iter().enumerate() {
                let y = 130 + i as i32 * 44;
                self.week_ctls.push(mk_label(win, *day, 20, y + 4, 70, 22));
                let h = mk(win, "EDIT", "", ES_AUTOHSCROLL | WS_BORDER, 100, y, 560, 30, ID_WEEK_ROW + i as i32);
                self.ctl.insert(ID_WEEK_ROW + i as i32, h);
                self.week_ctls.push(h);
            }
            let students_label = mk_label(
                win,
                &format!("本班学生（{} 人）", self.students.len()),
                690,
                100,
                300,
                22,
            );
            self.week_ctls.push(students_label);
            if !self.cfg.students_from.is_empty() {
                self.week_ctls.push(mk_label(
                    win,
                    &format!("来源：{}", self.cfg.students_from),
                    690,
                    122,
                    300,
                    20,
                ));
            }
            let search = mk(win, "EDIT", "", ES_AUTOHSCROLL | WS_BORDER, 690, 146, 300, 28, ID_SEARCH);
            self.ctl.insert(ID_SEARCH, search);
            self.week_ctls.push(search);
            let lb = mk(
                win,
                "LISTBOX",
                "",
                LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | WS_BORDER | WS_VSCROLL,
                690,
                180,
                300,
                340,
                ID_STUDENTS,
            );
            self.ctl.insert(ID_STUDENTS, lb);
            self.week_ctls.push(lb);
            for s in &self.students {
                let ws = wide(s);
                SendMessageW(lb, LB_ADDSTRING, 0, ws.as_ptr() as isize);
            }
            let clear = mk(win, "BUTTON", "清空全部", BS_PUSHBUTTON, 20, 552, 110, 34, ID_WEEK_CLEAR);
            self.week_ctls.push(clear);
        }
    }

    /// 切页：整组显隐 + 把对应数据填进共用的 7 行输入框。
    fn switch_tab(&mut self, tab: Tab) {
        // 1) 先取消上一页挂着的自动保存定时器：定时器回调按当前页保存，
        //    跨页触发会把 A 页数据写进 B 页。
        unsafe { KillTimer(self.main, ID_TIMER_AUTOSAVE) };
        self.tab = tab;
        let show_hw = tab == Tab::Homework;
        self.suppress_save = true;
        unsafe {
            for h in &self.hw_ctls {
                ShowWindow(*h, if show_hw { SW_SHOW } else { SW_HIDE });
            }
            for h in &self.week_ctls {
                ShowWindow(*h, if show_hw { SW_HIDE } else { SW_SHOW });
            }
        }
        // 作业行本来就按需显隐，切回来时别把未使用的行一起露出来
        if show_hw {
            self.sync_hw_rows();
        } else {
            let data = if tab == Tab::Duty {
                self.board.duty.clone()
            } else {
                self.board.tasks_week.clone()
            };
            for (i, day) in WEEK_CN.iter().enumerate() {
                if let Some(h) = self.ctl.get(&(ID_WEEK_ROW + i as i32)).copied() {
                    let v = data.get(*day).cloned().unwrap_or_default();
                    set_text(h, &v);
                }
            }
        }
        self.suppress_save = false;
        self.set_status(&format!("当前：{} — 边输入边自动保存，无需点任何按钮", tab.label()));
    }

    /// 输入防抖：最后一条 EN_CHANGE 后 800ms 才真正落盘。
    fn schedule_autosave(&self) {
        unsafe { SetTimer(self.main, ID_TIMER_AUTOSAVE, AUTOSAVE_DEBOUNCE_MS, std::ptr::null_mut()) };
    }

    /// 隐藏/显示作业行时保持与 hw_rows 一致。
    fn sync_hw_rows(&self) {
        for i in 0..MAX_HW_ROWS {
            for k in 0..3 {
                if let Some(h) = self.ctl.get(&(ID_HW_ROW + i as i32 * 3 + k)).copied() {
                    unsafe { ShowWindow(h, if i < self.hw_rows { SW_SHOW } else { SW_HIDE }) };
                }
            }
        }
    }

    fn add_hw_row(&mut self) {
        if self.hw_rows >= MAX_HW_ROWS {
            return;
        }
        let n = self.hw_rows;
        if let Some(a) = self.ctl.get(&(ID_HW_ROW + n as i32 * 3)).copied() {
            set_text(a, "新科目");
        }
        if let Some(b) = self.ctl.get(&(ID_HW_ROW + 1 + n as i32 * 3)).copied() {
            set_text(b, "");
        }
        self.hw_rows += 1;
        self.sync_hw_rows();
    }

    fn clear_week(&self) {
        for i in 0..7 {
            if let Some(h) = self.ctl.get(&(ID_WEEK_ROW + i)).copied() {
                set_text(h, "");
            }
        }
    }

    fn filter_students(&self) {
        let (Some(lb), Some(se)) = (
            self.ctl.get(&ID_STUDENTS).copied(),
            self.ctl.get(&ID_SEARCH).copied(),
        ) else {
            return;
        };
        let q = text_of(se).trim().to_string();
        unsafe { SendMessageW(lb, LB_RESETCONTENT, 0, 0) };
        for s in &self.students {
            if q.is_empty() || s.contains(&q) {
                let ws = wide(s);
                unsafe { SendMessageW(lb, LB_ADDSTRING, 0, ws.as_ptr() as isize) };
            }
        }
    }

    /// 双击名单 → 追加到当前选中星期那一行（不重复）。
    fn pick_student(&self) {
        let Some(lb) = self.ctl.get(&ID_STUDENTS).copied() else {
            return;
        };
        let sel = unsafe { SendMessageW(lb, LB_GETCURSEL, 0, 0) };
        if sel < 0 {
            return;
        }
        let len = unsafe { SendMessageW(lb, LB_GETTEXTLEN, sel as usize, 0) };
        if len <= 0 {
            return;
        }
        let mut buf = vec![0u16; len as usize + 2];
        unsafe { SendMessageW(lb, LB_GETTEXT, sel as usize, buf.as_mut_ptr() as isize) };
        let name = String::from_utf16_lossy(&buf[..len as usize]);
        let mut day = 0usize;
        for i in 0..7 {
            if let Some(r) = self.ctl.get(&(ID_DAY_BASE + i as i32)).copied() {
                if unsafe { SendMessageW(r, BM_GETCHECK, 0, 0) } != 0 {
                    day = i;
                }
            }
        }
        if let Some(h) = self.ctl.get(&(ID_WEEK_ROW + day as i32)).copied() {
            append_name(h, &name);
            unsafe { SetFocus(h) };
        }
    }

    fn apply_hw_text(&mut self, rows: Vec<(String, String, String)>) {
        let mut new: Vec<HomeworkItem> = Vec::new();
        for (s, c, n) in rows {
            let s = s.trim().to_string();
            if s.is_empty() {
                continue;
            }
            new.push(HomeworkItem {
                subject: s,
                content: c.trim().to_string(),
                note: n.trim().to_string(),
            });
        }
        if new.is_empty() {
            new = Board::default().homework;
        }
        self.board.homework = new;
    }

    /// 保存当前页 → board.json（自动保存定时器 / 手动关闭前都会走到这里）；
    /// 大屏自己的 400ms 轮询会读到新值。回填文本期间抑制 EN_CHANGE。
    fn save_current_tab(&mut self) {
        self.suppress_save = true;
        match self.tab {
            Tab::Homework => {
                let n = self.hw_rows;
                let mut rows = Vec::new();
                for i in 0..n {
                    let get = |k: i32| {
                        text_of(self.ctl.get(&k).copied().unwrap_or(std::ptr::null_mut()))
                    };
                    rows.push((
                        get(ID_HW_ROW + i as i32 * 3),
                        get(ID_HW_ROW + 1 + i as i32 * 3),
                        get(ID_HW_ROW + 2 + i as i32 * 3),
                    ));
                }
                self.apply_hw_text(rows);
                // 空科目行可能被删掉，按结果重建行显示；
                // 回填只在文本真的变化时才执行（set_text_if_changed），
                // 否则每轮自动保存都会把正在输入那格的光标顶回行首。
                self.hw_rows = self.board.homework.len();
                for i in 0..MAX_HW_ROWS {
                    let (s, c, n) = self
                        .board
                        .homework
                        .get(i)
                        .map(|r| (r.subject.clone(), r.content.clone(), r.note.clone()))
                        .unwrap_or_default();
                    for (k, v) in [(0, s), (1, c), (2, n)] {
                        if let Some(h) = self.ctl.get(&(ID_HW_ROW + i as i32 * 3 + k)).copied() {
                            set_text_if_changed(h, &v);
                        }
                    }
                }
                self.sync_hw_rows();
            }
            Tab::Duty | Tab::Tasks => {
                let mut map = BTreeMap::new();
                for (i, day) in WEEK_CN.iter().enumerate() {
                    let v = self
                        .ctl
                        .get(&(ID_WEEK_ROW + i as i32))
                        .map(|h| text_of(*h).trim().to_string())
                        .unwrap_or_default();
                    map.insert(day.to_string(), v);
                }
                if self.tab == Tab::Duty {
                    self.board.duty = map;
                } else {
                    self.board.tasks_week = map;
                }
            }
        }
        board::save(&self.board);
        self.suppress_save = false;
        self.set_status(&format!("{} 已自动保存，桌面大屏稍后自动刷新", self.tab.label()));
    }
}

/// 仅当内容真的变化时才重设文本。
/// `SetWindowTextW` 会把 EDIT 控件的光标重置到行首：自动保存（800ms 防抖）
/// 每轮都无条件回填，会把正在输入的那一格光标顶回最前面——表现为
/// 「填了几个字、停顿一下，光标就跳到最前面」。内容没变就跳过，光标不动。
fn set_text_if_changed(hwnd: HWND, s: &str) {
    if text_of(hwnd) != s {
        set_text(hwnd, s);
    }
}

/// 追加姓名到某天的输入框（已存在则不重复添加）。
fn append_name(hwnd: HWND, name: &str) {
    let cur = text_of(hwnd).trim().to_string();
    let exist: Vec<String> = cur
        .split(|c| c == '；' || c == ';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if exist.iter().any(|s| s == name) {
        return;
    }
    let new = if cur.is_empty() {
        name.to_string()
    } else {
        format!("{}；{}", cur, name)
    };
    set_text(hwnd, &new);
}

// ====================== 窗口过程 ======================

unsafe extern "system" fn panel_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    if msg == WM_COMMAND {
        let id = (wp & 0xFFFF) as i32;
        let code = ((wp >> 16) & 0xFFFF) as u16;
        return match id {
            ID_TAB_HW => {
                with(|e| e.switch_tab(Tab::Homework));
                0
            }
            ID_TAB_DUTY => {
                with(|e| e.switch_tab(Tab::Duty));
                0
            }
            ID_TAB_TASKS => {
                with(|e| e.switch_tab(Tab::Tasks));
                0
            }
            ID_HW_ADD => {
                with(|e| e.add_hw_row());
                0
            }
            ID_WEEK_CLEAR => {
                with(|e| e.clear_week());
                0
            }
            // 作业行 / 值周值日行：任何输入改动都重置防抖定时器，800ms 后自动保存
            id if (ID_HW_ROW..ID_HW_ROW + (MAX_HW_ROWS as i32) * 3).contains(&id)
                || (ID_WEEK_ROW..ID_WEEK_ROW + 7).contains(&id) =>
            {
                if code == EN_CHANGE {
                    with(|e| {
                        if !e.suppress_save {
                            e.schedule_autosave();
                        }
                    });
                }
                0
            }
            ID_SEARCH if code == EN_CHANGE => {
                with(|e| e.filter_students());
                0
            }
            ID_STUDENTS if code == LBN_DBLCLK => {
                with(|e| e.pick_student());
                0
            }
            // DestroyWindow 必须放在 with() 外面：窗口过程重入会撞上 RefCell 借用。
            ID_BTN_CANCEL => {
                board::flush();
                DestroyWindow(hwnd);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        };
    }
    match msg {
        WM_TIMER if wp == ID_TIMER_AUTOSAVE => {
            // 借用被占（别的消息处理中）就先不杀定时器，下个周期再存
            let saved = with(|e| e.save_current_tab());
            if saved.is_some() {
                unsafe { KillTimer(hwnd, ID_TIMER_AUTOSAVE) };
            }
            0
        }
        // 窗口拉伸时把状态栏和「关闭」锚到底部，任何尺寸下都完整可见（4K 屏）
        WM_SIZE => {
            let cw = (lp & 0xFFFF) as i32; // 客户区宽
            let ch = ((lp >> 16) & 0xFFFF) as i32; // 客户区高
            with(|e| {
                unsafe {
                    MoveWindow(e.status, 20, ch - 96, cw - 140, 24, 1);
                    MoveWindow(e.close_btn, cw - 110, ch - 62, 90, 36, 1);
                }
            });
            0
        }
        WM_CLOSE => {
            // 关闭前把防抖里还没落盘的改动存掉
            unsafe { KillTimer(hwnd, ID_TIMER_AUTOSAVE) };
            with(|e| e.save_current_tab());
            board::flush();
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            board::flush();
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

#[cfg(test)]
mod tests {
    use super::Tab;

    #[test]
    fn three_tabs_have_distinct_labels() {
        let labels = [Tab::Homework.label(), Tab::Duty.label(), Tab::Tasks.label()];
        assert_eq!(labels, ["今日作业", "班干部值周", "卫生值日"]);
    }
}

/// 真实 GUI 冒烟：会短暂弹出设置面板，平时 `#[ignore]`。
/// 手动执行 `cargo test --release panel_smoke -- --ignored --nocapture`。
#[cfg(test)]
mod panel_smoke {
    use super::*;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "会弹出真实窗口，手动验证时加 --ignored"]
    fn panel_opens_switches_tabs_and_closes() {
        let cls = wide("BoardEditorRust");
        // 已经有编辑器在跑就别碰它，避免把用户正在用的窗口切页/关掉。
        if !unsafe { FindWindowW(cls.as_ptr(), std::ptr::null()) }.is_null() {
            eprintln!("已有设置面板在运行，跳过 GUI 冒烟");
            return;
        }
        let h = thread::spawn(|| super::run());
        let mut win = std::ptr::null_mut();
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline {
            win = unsafe { FindWindowW(cls.as_ptr(), std::ptr::null()) };
            if !win.is_null() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!win.is_null(), "设置面板没有创建出来");
        thread::sleep(Duration::from_millis(120));
        for id in [ID_TAB_DUTY, ID_TAB_TASKS, ID_TAB_HW] {
            unsafe { SendMessageW(win, WM_COMMAND, id as usize, 0) };
            assert!(unsafe { IsWindow(win) } != 0, "切页时窗口被意外销毁");
        }
        unsafe { SendMessageW(win, WM_CLOSE, 0, 0) };
        h.join().expect("设置面板的消息循环没有退出");
    }
}
