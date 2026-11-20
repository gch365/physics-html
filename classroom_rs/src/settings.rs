//! 设置窗口（egui/eframe）。4+1 结构：班级 / 显示 / 教学数据 / 通知 + 折叠「高级」。
//! 高级页含 Gitee 仓库、GoEasy、自动更新等敏感项，需密码解锁（password::verify）。
//!
//! 独立进程 `--settings` 运行，直接读写 ~/.RollCallData 的 config.json / schedules / auth.json；
//! 主进程在 100ms 的 T_POLL 里检测 config.json mtime 热加载（约 0.3 秒生效）。
//! 所有设置改动即时生效（config 帧末 300ms 防抖、课表/作息 500ms 防抖），无需点保存。
//!
//! 借用模型：每帧先 mem::take 配置与 UI 状态，渲染函数为纯函数（不持 App 引用），
//! 帧末写回并执行收集到的动作——规避 egui 闭包与 &mut self 的借用冲突。

use chrono::Datelike;
use eframe::egui;
use egui::{Color32, ScrollArea};

use crate::config::{ClassEntry, Config, ScheduledNotice};

/// 4 个常用页按使用频率前置，技术配置收进「高级」折叠区。
/// 页签编号：0 班级 / 1 显示 / 2 教学数据 / 3 通知 / 4 高级
const TAB_NAMES: [&str; 5] = [
    "班级",
    "显示",
    "教学数据",
    "定时通知",
    "高级 ▾",
];

const WEEKDAY_NAMES: [&str; 7] = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
const GRADES: [&str; 3] = ["高一", "高二", "高三"];

/// 每个教室默认只能创建 1 个班级；多班级需输入密码解锁后创建。
pub const MAX_CLASSES_FREE: usize = 1;
pub const CLASS_UNLOCK_PASSWORD: &str = "123321";
/// 「彻底关闭桌面作业栏」的确认码；与 `desktop_board_rs` 电源按钮确认框保持一致。
pub const HARD_CLOSE_CODE: &str = "123";

/// 「学生名单 / 课表」修改锁口令：已导入的数据需先输入该口令解锁才能改。
/// 与退出/关闭确认码同为 123，但作用域独立，便于以后分别调整。
pub const DATA_UNLOCK_PASSWORD: &str = "123";

/// 纯函数：只有输入正确口令才允许解锁名单/课表修改（容忍首尾空格）。
pub fn data_unlock_ok(code: &str) -> bool {
    let t = code.trim();
    !t.is_empty() && t == DATA_UNLOCK_PASSWORD
}

/// 纯函数：教学数据是否可写。
/// 解锁状态只在本次设置窗口会话内有效，重开窗口自动恢复锁定。
pub fn teaching_data_edit_allowed(unlocked: bool) -> bool {
    unlocked
}

/// 纯函数：只有输入确认码才允许写持久关闭标志。
pub fn hw_close_allowed(code: &str) -> bool {
    let t = code.trim();
    !t.is_empty() && t == HARD_CLOSE_CODE
}

/// 纯函数：一键清空所有班级，返回删掉的班级数（重复调用返回 0）。
/// 用于「本机批量导入了很多班，想直接回到 0 班」的场景。
pub fn clear_all_classes(cfg: &mut Config) -> usize {
    let n = cfg.class_list.len();
    cfg.class_list.clear();
    cfg.current_class.clear();
    n
}

/// 纯函数：班级数量 / 解锁状态 → 是否允许再创建一个班级。
pub fn can_create_class(count: usize, unlocked: bool) -> bool {
    count < MAX_CLASSES_FREE || unlocked
}

/// 年级标签 → 入学年份。9 月起算新学年：高一=当年、高二=去年、高三=前年。
/// 例：2026-11 → 高一 2026 / 高二 2025（与内置「2025级09班」一致）/ 高三 2024。
fn grade_to_year(grade: &str) -> String {
    let now = chrono::Local::now();
    let senior1 = if now.month() >= 9 { now.year() } else { now.year() - 1 };
    let y = match grade {
        "高一" => senior1,
        "高三" => senior1 - 2,
        _ => senior1 - 1,
    };
    y.to_string()
}

/// 设置窗口入口（--settings）。
/// 同一时刻只允许一个设置窗口：已有实例时把旧窗口还原并带到前台，本次直接退出。
/// （原版每点一次「设置」就 spawn 一个新窗口，可以叠出任意多个。）
pub fn run() {
    if !crate::win::single_instance_ok("GzzxSettingsMutex") {
        bring_existing_to_front();
        return;
    }
    crate::win::dpi_aware();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("系统设置")
            .with_inner_size([1000.0, 900.0])
            .with_min_inner_size([750.0, 700.0]),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "系统设置",
        options,
        Box::new(|cc| {
            install_fonts(cc);
            Ok(Box::new(SettingsApp::new()))
        }),
    );
}

/// 已有设置窗口在跑：按标题定位（eframe 窗口未注册类名），还原并置前。
fn bring_existing_to_front() {
    use crate::win::sys::{wide, FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE};
    unsafe {
        let title = wide("系统设置");
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }
}

/// 退出确认窗口入口（--confirm-exit）：输入 123 才允许退出主程序。
/// 独立子进程 + egui 小窗，确认后写 exit.flag，主程序 100ms 轮询消费后真正退出。
pub fn run_confirm_exit() {
    crate::win::dpi_aware();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("退出确认")
            .with_inner_size([400.0, 220.0])
            .with_always_on_top()
            .with_resizable(false),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "退出确认",
        options,
        Box::new(|cc| {
            install_fonts(cc);
            Ok(Box::new(ConfirmExitApp::default()))
        }),
    );
}

/// 退出确认对话框：输入 123 + 点「确认退出」→ 写 exit.flag；取消则什么都不发生。
#[derive(Default)]
struct ConfirmExitApp {
    input: String,
    error: Option<String>,
}

impl eframe::App for ConfirmExitApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            use egui::RichText as Rt;
            ui.add_space(8.0);
            ui.label(Rt::new("⚠ 即将退出点名程序").size(17.0).strong());
            ui.label(Rt::new("退出后通知横幅、半屏弹屏与语音播报都会停止。").size(13.0).weak());
            ui.add_space(10.0);
            ui.label(Rt::new("请输入 123 确认退出：").size(14.0));
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .password(true)
                    .desired_width(160.0)
                    .hint_text("•••"),
            );
            if resp.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
            {
                self.try_confirm();
            }
            if let Some(e) = &self.error {
                ui.label(Rt::new(e).size(13.0).color(egui::Color32::from_rgb(200, 40, 40)));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let ok = ui.button("确认退出").clicked();
                if ok {
                    self.try_confirm();
                }
                if ui.button("取消").clicked() {
                    std::process::exit(0);
                }
            });
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
    }
}

impl ConfirmExitApp {
    fn try_confirm(&mut self) {
        if self.input.trim() == "123" {
            crate::paths::write_exit_flag();
            std::process::exit(0);
        }
        self.error = Some("口令不对：请输入 123".to_string());
        self.input.clear();
    }
}

/// 历史窗口入口（--history）。
pub fn run_history() {
    crate::win::dpi_aware();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("点名记录")
            .with_inner_size([520.0, 470.0]),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "点名记录",
        options,
        Box::new(|cc| {
            install_fonts(cc);
            Ok(Box::new(HistoryApp::new()))
        }),
    );
}

/// 加载系统微软雅黑 + Emoji 字体，保证中文显示。
/// 注意：常规雅黑必须排最前；粗体只作兜底（push 到队尾），
/// 否则整个窗口所有文字都会渲染成粗体（曾经的老 Bug）。
fn install_fonts(cc: &eframe::CreationContext) {
    let mut fonts = egui::FontDefinitions::default();
    for (path, front) in [
        ("C:\\Windows\\Fonts\\msyh.ttc", true),
        ("C:\\Windows\\Fonts\\msyhbd.ttc", false),
        ("C:\\Windows\\Fonts\\seguiemj.ttf", false),
    ] {
        if let Ok(data) = std::fs::read(path) {
            let key = format!("sys-{}", path.rsplit('\\').next().unwrap_or("f"));
            fonts.font_data.insert(key.clone(), egui::FontData::from_owned(data).into());
            let fam = fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap();
            if front {
                fam.insert(0, key);
            } else {
                fam.push(key);
            }
        }
    }
    cc.egui_ctx.set_fonts(fonts);
}

// ==================== UI 状态 ====================

#[derive(Default)]
struct UiState {
    tab: usize,
    unlocked: bool,
    pwd_input: String,
    status: Option<String>,
    busy: Option<String>,
    rx: Option<std::sync::mpsc::Receiver<String>>,
    /// 当前班级的 6 位码（网页端发通知用）
    class_code: Option<String>,
    class_code_loading: bool,

    // 班级管理
    new_school: String,
    /// 年级标签：高一 / 高二 / 高三（内部换算成入学年份写入文件名）
    new_grade: String,
    new_class_num: String,

    // 学生名单
    stu_class_idx: usize,
    stu_selected: Option<usize>,
    stu_input: String,
    stu_paste: String,

    // 定时通知：表格中选中的行
    notice_selected: Option<usize>,

    // 课表
    sched_class_idx: usize,
    sched_grid: Vec<[String; 5]>, // [period-1][day]
    sched_periods: usize,
    sched_paste: String,
    sched_loaded_for: String,
    /// 课表网格被编辑的时刻（帧末防抖自动写盘，无需点保存）
    sched_dirty_since: Option<std::time::Instant>,
    /// 上述编辑所属班级（打标记时记录，切班后仍能写回正确的班，不串班不丢数据）
    sched_dirty_class: String,

    // 作息
    time_rows_summer: Vec<(String, String)>,
    time_rows_winter: Vec<(String, String)>,
    /// 作息表被编辑的时刻（帧末防抖自动写盘，无需点保存）
    time_dirty_since: Option<std::time::Instant>,

    // 定时通知
    notices: Vec<ScheduledNotice>,
    notices_loaded: bool,
    notices_requested: bool,
    /// 已用本地内置清单预填过列表（Gitee 拉取成功后覆盖；失败时列表也不至于空白）
    notices_prefilled: bool,
    #[allow(dead_code)] // 预留：延迟加载通知清单标记
    need_load_notices: bool,
    pending_notice: Option<NoticeEdit>,
    notice_edit: Option<NoticeEdit>,

    // Web 与仓库
    token_show: bool,
    new_pwd: String,

    // 云端名单拉取（独立 channel，不与通知/班级码的 st.rx 抢）
    cloud_rx: Option<std::sync::mpsc::Receiver<CloudFetchMsg>>,
    cloud_busy: bool,
    cloud_status: Option<String>,

    // 班级数量限制的解锁密码与状态（只在本次设置窗口会话内有效）
    class_pwd: String,
    class_unlocked: bool,

    // 学生名单 / 课表修改锁：解锁一次后本次设置窗口内可改（重开自动上锁）
    data_pwd: String,
    data_unlocked: bool,

    // 「彻底关闭桌面作业栏」二次确认（输入确认码才生效）
    hw_confirm_open: bool,
    hw_confirm_code: String,

    // 「清空所有班级」二次确认
    class_clear_confirm: bool,
}

#[derive(Clone)]
struct NoticeEdit {
    index: Option<usize>, // None = 新增
    enabled: bool,
    content: String,
    weekdays: [bool; 7],
    start: String,
    end: String,
}

/// 后台云端拉取结果。带上 file_name，避免拉取期间用户切了班级导致回填错班。
enum CloudFetchMsg {
    Ok {
        file_name: String,
        students: Vec<String>,
        schedule: Vec<crate::seed::SeedSlot>,
    },
    Err {
        file_name: String,
        msg: String,
    },
}

/// 帧内收集的动作，帧末执行。
enum Action {
    Save,
    CreateClass,
    DeleteClass(usize),
    SetCurrentClass(String),
    ResetTimeTable,
    AddPeriod,
    RemovePeriod(usize),
    ClearTimetable,
    ImportSchedule,
    ImportStudents,
    ClearStudents,
    SaveNotices,
    ReloadNotices,
    TestConn,
    ManualSync,
    Unlock,
    ChangePwd,
    CommitNotice,
    DeleteNotice(usize),
    CheckUpdates,
    LoadClassCode,
    /// 从私有仓库重新拉取当前班的名单与课表（断网/失败后的重试入口）
    FetchCloud,
    /// 校验密码，解锁「超过 1 个班级」的创建权限
    UnlockClasses,
    /// 校验口令，解锁「学生名单 / 课表」的修改权限
    UnlockData,
    /// 清空所有班级（需二次确认，执行前自动备份 config.json）
    ClearAllClasses,
    /// 导出当前班名单为 CSV（存桌面「点名系统导出」文件夹）
    ExportStudents,
    /// 导出「学生名单」页所选班级的名单为 CSV
    ExportStudentsAt(usize),
    /// 导出当前班课表为 CSV（存桌面「点名系统导出」文件夹）
    ExportTimetable,
    /// 导出「课表」页所选班级的课表为 CSV（优先用界面正在编辑的网格）
    ExportTimetableSched,
}

pub struct SettingsApp {
    cfg: Config,
    st: UiState,
    /// 上次落盘的配置快照：帧末比对，变化即自动异步保存（无需点「保存」）
    last_saved_json: String,
    dirty_since: Option<std::time::Instant>,
    /// 上次写注册表的自启状态，避免每次自动保存都碰注册表（可能挂起）
    last_auto_run: bool,
}

impl SettingsApp {
    fn new() -> Self {
        let cfg = Config::load();
        crate::schedule::load_time_overrides();
        let mut st = UiState {
            time_rows_summer: crate::schedule::time_table_for_edit("summer"),
            time_rows_winter: crate::schedule::time_table_for_edit("winter"),
            new_school: "赣州中学".into(),
            new_grade: "高二".into(),
            new_class_num: "9".into(),
            cloud_status: Some(
                "名单与课表存于私有仓库，创建班级时自动拉取本班数据".into(),
            ),
            ..Default::default()
        };
        // 预载课表网格
        let cls_id = cfg.class_list.first().map(|c| c.file_name.clone()).unwrap_or_default();
        load_grid(&mut st, &cfg, &cls_id);
        let last_saved_json = serde_json::to_string(&cfg).unwrap_or_default();
        let last_auto_run = cfg.auto_run;
        let mut app = SettingsApp {
            cfg,
            st,
            last_saved_json,
            dirty_since: None,
            last_auto_run,
        };
        app.load_class_code_async(); // 打开设置即查/生成 6 位班级码
        app
    }

    /// 落盘 config.json；注册表自启只在状态真变化时后台更新，避免阻塞 UI 线程。
    fn write_config(&mut self, status: &str) {
        if !self.cfg.save() {
            self.st.status = Some("保存失败".into());
            return;
        }
        self.last_saved_json = serde_json::to_string(&self.cfg).unwrap_or_default();
        self.dirty_since = None;
        if self.cfg.auto_run != self.last_auto_run {
            self.last_auto_run = self.cfg.auto_run;
            let ar = self.cfg.auto_run;
            let _ = std::thread::Builder::new().name("auto-run".into()).spawn(move || {
                crate::win::set_auto_run(ar);
            });
        }
        self.st.status = Some(status.into());
    }

    fn save_all(&mut self) {
        self.write_config("已即时生效");
    }

    /// 课表页当前选中的班级 ID。
    /// 清除/导入都用它，不再借用 `sched_loaded_for`（那是网格缓存标记，
    /// 被清空后会误判成「没有选择班级」）。
    fn sched_class_id(&self) -> String {
        self.cfg
            .class_list
            .get(self.st.sched_class_idx)
            .map(|c| c.file_name.clone())
            .unwrap_or_default()
    }

    fn timetable_map(&self) -> std::collections::HashMap<(usize, usize), String> {
        grid_to_map(&self.st.sched_grid)
    }

    /// 即时生效用的课表写盘：按「编辑发生时所记录的班级」(sched_dirty_class) 写，
    /// 而不是按可能已被切换的 sched_class_idx / sched_loaded_for，
    /// 避免编辑后立刻切班把 A 班课表写进 B 班、或 A 班编辑被 B 班数据覆盖丢失。
    /// 静默写盘（不刷状态栏），由帧末 500ms 防抖或帧首切班前 flush 调用。
    fn autosave_timetable(&mut self) {
        let cls_id = self.st.sched_dirty_class.clone();
        if cls_id.is_empty() || cls_id == "?" {
            return;
        }
        let map = self.timetable_map();
        let _ = crate::schedule::save_schedule(&cls_id, &map);
    }

    /// 即时生效用的作息写盘：校验格式后静默写盘。
    fn autosave_timetable_time(&mut self) {
        let mode = self.cfg.work_time.clone();
        let rows = if mode == "winter" {
            self.st.time_rows_winter.clone()
        } else {
            self.st.time_rows_summer.clone()
        };
        let valid = !rows.is_empty() && rows.iter().all(|(s, e)| is_hhmm(s) && is_hhmm(e));
        if valid {
            let _ = crate::schedule::save_time_table(&mode, &rows);
        }
    }

    /// 粘贴导入一步到位：解析 → 写进网格 → 立刻落盘。
    /// 关键是**保留** sched_loaded_for，否则 sync_grid 下一帧会用磁盘旧数据
    /// 把刚解析出来的网格覆盖掉（旧版就是这样导致「导入没反应」）。
    fn import_schedule_now(&mut self) {
        if !teaching_data_edit_allowed(self.st.data_unlocked) {
            self.st.status = Some("课表已锁定：请输入密码 123 解锁后再修改".into());
            return;
        }
        let cls_id = self.sched_class_id();
        if cls_id.is_empty() {
            self.st.status = Some("请先选择班级".into());
            return;
        }
        let (grid, n) = match parse_schedule_paste(&self.st.sched_paste, self.st.sched_periods) {
            Ok(v) => v,
            Err(msg) => {
                self.st.status = Some(msg);
                return;
            }
        };
        self.st.sched_grid = grid;
        self.st.sched_periods = self.st.sched_grid.len().clamp(1, 15);
        self.st.sched_loaded_for = cls_id.clone();
        let map = self.timetable_map();
        match crate::schedule::save_schedule(&cls_id, &map) {
            Ok(_) => {
                let name = self
                    .cfg
                    .class_list
                    .get(self.st.sched_class_idx)
                    .map(|c| c.display_name.clone())
                    .unwrap_or_default();
                self.st.sched_paste.clear();
                self.st.status = Some(format!("已导入并保存 {name} 课表（{n} 门课）"));
            }
            Err(e) => self.st.status = Some(format!("写入课表失败: {e}")),
        }
    }

    /// 创建 / 切换 / 删除班级后的统一收尾。
    ///
    /// 1. 立刻把新班设为当前班级并落盘（设置窗口与主程序是不同进程）；
    /// 2. 设置窗口自身的班级码、学生名单、课表网格也跟着换，避免还显示旧班；
    /// 3. 写显式切换标记，让主程序马上热加载 + 回拉并弹出新班通知，
    ///    不依赖 config.json 的秒级 mtime（同一秒连写会漏检）。
    fn switch_to_class(&mut self, file_name: String, tip: String) {
        self.cfg.current_class = file_name.clone();
        if let Some(i) = self
            .cfg
            .class_list
            .iter()
            .position(|c| c.file_name == file_name)
        {
            self.st.sched_class_idx = i;
            self.st.stu_class_idx = i;
            self.st.stu_selected = None;
        }
        // sync_grid() 下一帧按新班重载课表
        self.st.sched_loaded_for.clear();
        // 丢掉旧班在途的班级码请求，否则 load_class_code_async 会被 rx 挡住不执行
        self.st.rx = None;
        self.st.class_code_loading = false;
        self.st.class_code = None;

        self.save_all();
        self.load_class_code_async(); // 新班没有 6 位码时自动登记到 Gitee
        crate::paths::write_class_switch_flag(&file_name);
        self.st.status = Some(tip);
    }

    /// 后台从私有仓库拉取**一个班**的名单与课表。创建班级时自动调用，
    /// 失败也不阻塞创建：班级已经是空名单状态，可随时重试或自行导入。
    fn start_cloud_fetch(&mut self, file_name: &str, school: &str, year: &str, num: &str) {
        if !self.cfg.class_data_enabled {
            self.st.cloud_status = Some(
                "云端名单已关闭：可在「高级 → 云端班级名单」开启，或自行导入".into(),
            );
            return;
        }
        if self.st.cloud_busy {
            self.st.cloud_status = Some("已有一个云端拉取在进行中，请稍候".into());
            return;
        }
        let cfg = self.cfg.clone();
        let (file_name, school, year, num) = (
            file_name.to_string(),
            school.to_string(),
            year.to_string(),
            num.to_string(),
        );
        let (tx, rx) = std::sync::mpsc::channel::<CloudFetchMsg>();
        self.st.cloud_rx = Some(rx);
        self.st.cloud_busy = true;
        self.st.cloud_status = Some(format!("正在从私有仓库拉取 {school}{year}级{num}班…"));
        std::thread::Builder::new()
            .name("cloud-class".into())
            .spawn(move || {
                let msg = match crate::seed::fetch_class(&cfg, &school, &year, &num) {
                    Ok(c) => CloudFetchMsg::Ok {
                        file_name: file_name.clone(),
                        students: c.data.students,
                        schedule: c.data.schedule,
                    },
                    Err(e) => CloudFetchMsg::Err {
                        file_name: file_name.clone(),
                        msg: e,
                    },
                };
                let _ = tx.send(msg);
            })
            .ok();
    }

    /// 「☁ 从云端补全当前班」：按当前班级自身的学校/届/班号重新拉一次。
    fn fetch_current_class(&mut self) {
        let Some(entry) = self
            .cfg
            .class_list
            .iter()
            .find(|c| c.file_name == self.cfg.current_class)
            .cloned()
        else {
            self.st.status = Some("当前没有班级，请先创建班级".into());
            return;
        };
        self.start_cloud_fetch(
            &entry.file_name,
            &entry.school,
            &entry.year,
            &entry.class_num,
        );
    }

    /// 帧首收取云端结果，回填本班名单并写课表；失败只改提示，不回滚班级。
    fn poll_cloud(&mut self) {
        let got = self
            .st
            .cloud_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        let Some(msg) = got else { return };
        self.st.cloud_rx = None;
        self.st.cloud_busy = false;
        match msg {
            CloudFetchMsg::Ok {
                file_name,
                students,
                schedule,
            } => {
                let n_stu = students.len();
                let class = crate::seed::SeedClass { students: students.clone(), schedule };
                let map = class.schedule_map();
                let n_course = map.len();
                let mut found = false;
                for entry in self.cfg.class_list.iter_mut() {
                    if entry.file_name == file_name {
                        entry.students = students;
                        found = true;
                        break;
                    }
                }
                if !found {
                    self.st.cloud_status = Some("班级在拉取期间已被删除，结果已丢弃".into());
                    return;
                }
                let schedule_saved = if map.is_empty() {
                    false
                } else {
                    crate::schedule::save_schedule(&file_name, &map).is_ok()
                };
                self.write_config("云端名单已写入本班配置");
                self.st.sched_loaded_for.clear(); // 下一帧从磁盘重读课表网格
                self.st.stu_selected = None;
                self.st.cloud_status = Some(format!(
                    "已从私有仓库拉取本班：{n_stu} 名学生、{n_course} 节课{}；本机只保存本班数据",
                    if schedule_saved { "，课表已写入" } else { "" }
                ));
                self.st.status = self.st.cloud_status.clone();
            }
            CloudFetchMsg::Err { file_name, msg } => {
                // 拉取期间班级可能已被删掉，这时不必再提示重试
                if !self.cfg.class_list.iter().any(|c| c.file_name == file_name) {
                    self.st.cloud_status = Some("班级已被删除，云端拉取结果已丢弃".into());
                } else {
                    self.st.cloud_status = Some(format!(
                        "云端拉取失败：{msg}。班级已创建（空名单），联网后点「☁ 从云端补全当前班」重试，也可自行粘贴导入"
                    ));
                }
                self.st.status = self.st.cloud_status.clone();
            }
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Save => self.save_all(),
            Action::FetchCloud => {
                if !teaching_data_edit_allowed(self.st.data_unlocked) {
                    self.st.status = Some("名单与课表已锁定：请先输入密码 123 解锁后再从云端补全".into());
                    return;
                }
                self.fetch_current_class();
            }
            Action::ExportStudents => self.export_students(),
            Action::ExportStudentsAt(i) => self.export_students_at(i),
            Action::ExportTimetable => self.export_timetable(),
            Action::ExportTimetableSched => self.export_timetable_sched(),
            Action::UnlockClasses => {
                if self.st.class_pwd.trim() == CLASS_UNLOCK_PASSWORD {
                    self.st.class_unlocked = true;
                    self.st.class_pwd.clear();
                    self.st.status = Some("已解锁：现在可以继续创建班级".into());
                } else {
                    self.st.class_pwd.clear();
                    self.st.status = Some("密码不正确，无法创建更多班级".into());
                }
            }
            Action::UnlockData => {
                if data_unlock_ok(&self.st.data_pwd) {
                    self.st.data_unlocked = true;
                    self.st.data_pwd.clear();
                    self.st.status = Some("已解锁：本次窗口内可修改学生名单与课表，重开窗口自动上锁".into());
                } else {
                    self.st.data_pwd.clear();
                    self.st.status = Some("口令不正确，名单与课表仍为锁定状态".into());
                }
            }
            Action::ClearAllClasses => {
                // 第一次点只把界面切成确认态，第二次点「确定清空」才真正写盘。
                if !self.st.class_clear_confirm {
                    self.st.class_clear_confirm = true;
                    self.st.status = Some(format!(
                        "再点一次「确定清空」将删除全部 {} 个班级；config.json 会先自动备份",
                        self.cfg.class_list.len()
                    ));
                    return;
                }
                let path = crate::paths::config_file();
                let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
                let bak = path.with_extension(format!("json.bak-clear-{stamp}"));
                let backed = std::fs::copy(&path, &bak).is_ok();
                let n = clear_all_classes(&mut self.cfg);
                // 清掉所有还指向旧班的 UI 状态，避免下一帧把旧网格/班级码画回来
                self.st.sched_class_idx = 0;
                self.st.stu_class_idx = 0;
                self.st.stu_selected = None;
                self.st.sched_loaded_for.clear();
                self.st.rx = None;
                self.st.cloud_rx = None;
                self.st.cloud_busy = false;
                self.st.cloud_status = None;
                self.st.class_code = None;
                self.st.class_code_loading = false;
                self.st.class_clear_confirm = false;
                self.save_all();
                self.st.status = Some(if backed {
                    format!(
                        "已清空 {n} 个班级，班级数 = 0；原配置备份：{}",
                        bak.display()
                    )
                } else {
                    format!("已清空 {n} 个班级，班级数 = 0（备份未成功，配置已保存）")
                });
            }
            Action::SetCurrentClass(f) => {
                let name = self
                    .cfg
                    .class_list
                    .iter()
                    .find(|c| c.file_name == f)
                    .map(|c| c.display_name.clone())
                    .unwrap_or_else(|| f.clone());
                self.switch_to_class(f, format!("已切换到 {name}，正在同步通知"));
            }
            Action::CreateClass => {
                if !can_create_class(self.cfg.class_list.len(), self.st.class_unlocked) {
                    self.st.status = Some(format!(
                        "每个教室最多 {MAX_CLASSES_FREE} 个班级；请在下方输入密码解锁后再创建"
                    ));
                    return;
                }
                let school = self.st.new_school.trim().to_string();
                let year = grade_to_year(self.st.new_grade.trim());
                let num = self.st.new_class_num.trim().to_string();
                if school.is_empty() || num.is_empty() {
                    self.st.status = Some("学校 / 班号 都不能为空".into());
                    return;
                }
                let file_name = ClassEntry::build_file_name(&school, &year, &num);
                if self.cfg.class_list.iter().any(|c| c.file_name == file_name) {
                    self.st.status = Some(format!("班级已存在：{}", file_name));
                    return;
                }
                let display_name = ClassEntry::build_display_name(&school, &year, &num);
                // 先建空班级并立刻落盘：名单/课表由私有仓库后台按班号拉取后回填。
                // 断网、只读令牌失效、云端没这一届都不会卡住创建流程，
                // 之后可点「☁ 从云端补全当前班」重试，或自行粘贴导入。
                // 云端有哪一届的数据就带出哪一届，不再硬编码「只有高二」。
                self.cfg.class_list.push(ClassEntry {
                    school: school.clone(),
                    year: year.clone(),
                    class_num: num.clone(),
                    display_name: display_name.clone(),
                    file_name: file_name.clone(),
                    students: Vec::new(),
                    extra: Default::default(),
                });
                self.switch_to_class(
                    file_name.clone(),
                    format!("已创建并切换到 {display_name}，正在从私有仓库拉取本班名单与课表"),
                );
                self.start_cloud_fetch(&file_name, &school, &year, &num);
            }
            Action::DeleteClass(idx) => {
                // 全新安装本来就没有班级，因此也允许删到空。
                if idx >= self.cfg.class_list.len() {
                    self.st.status = Some("当前没有班级可删除".into());
                    return;
                }
                let removed = self.cfg.class_list.remove(idx);
                let rf = removed.file_name.clone();
                if self.cfg.current_class == rf {
                    if let Some(first) = self.cfg.class_list.first() {
                        let f = first.file_name.clone();
                        let name = first.display_name.clone();
                        self.switch_to_class(
                            f,
                            format!("已删除 {rf}，自动切换到 {name}，正在同步通知"),
                        );
                    } else {
                        self.cfg.current_class.clear();
                        self.st.sched_class_idx = 0;
                        self.st.stu_class_idx = 0;
                        self.st.sched_loaded_for.clear();
                        self.save_all();
                        self.st.status =
                            Some(format!("已删除 {rf}；当前没有班级，请创建班级"));
                    }
                } else {
                    self.save_all();
                    self.st.status = Some(format!("已删除班级：{rf}"));
                }
            }
            Action::ResetTimeTable => {
                let mode = self.cfg.work_time.clone();
                let rows: Vec<(String, String)> = crate::schedule::current_builtin(&mode)
                    .iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect();
                if mode == "winter" {
                    self.st.time_rows_winter = rows;
                } else {
                    self.st.time_rows_summer = rows;
                }
            }
            Action::AddPeriod => {
                let rows = if self.cfg.work_time == "winter" { &mut self.st.time_rows_winter } else { &mut self.st.time_rows_summer };
                let last_end = rows.last().map(|r| r.1.clone()).unwrap_or_else(|| "08:00".into());
                let (ns, ne) = next_slot(&last_end);
                rows.push((ns, ne));
                self.st.time_dirty_since.get_or_insert_with(std::time::Instant::now);
            }
            Action::RemovePeriod(i) => {
                let rows = if self.cfg.work_time == "winter" { &mut self.st.time_rows_winter } else { &mut self.st.time_rows_summer };
                if rows.len() > 1 {
                    rows.remove(i);
                    self.st.time_dirty_since.get_or_insert_with(std::time::Instant::now);
                }
            }
            Action::ClearTimetable => {
                if !teaching_data_edit_allowed(self.st.data_unlocked) {
                    self.st.status = Some("课表已锁定：请输入密码 123 解锁后再修改".into());
                    return;
                }
                let cls_id = self.sched_class_id();
                if cls_id.is_empty() {
                    self.st.status = Some("请先选择班级".into());
                    return;
                }
                crate::schedule::clear_schedule(&cls_id);
                self.st.sched_grid = vec![blank_week(); self.st.sched_periods];
                self.st.sched_loaded_for = cls_id;
                self.st.status = Some("课表已清除".into());
            }
            Action::ImportSchedule => self.import_schedule_now(),
            Action::ImportStudents => {
                if !teaching_data_edit_allowed(self.st.data_unlocked) {
                    self.st.status = Some("名单已锁定：请输入密码 123 解锁后再修改".into());
                    return;
                }
                let names = parse_student_paste(&self.st.stu_paste);
                if names.is_empty() {
                    self.st.status = Some(
                        "没识别到姓名：支持每行一个，或 Tab / 逗号 / 顿号 / 空格分隔；表头与序号会自动跳过"
                            .into(),
                    );
                } else {
                    let idx = self.st.stu_class_idx;
                    if let Some(entry) = self.cfg.class_list.get_mut(idx) {
                        let n = names.len();
                        let preview: Vec<String> =
                            names.iter().take(3).cloned().collect();
                        entry.students = names;
                        self.st.stu_selected = None;
                        self.st.stu_paste.clear();
                        self.save_all();
                        self.st.status = Some(format!(
                            "已导入并保存 {} 名学生（{}…）",
                            n,
                            preview.join("、")
                        ));
                    } else {
                        self.st.status = Some("所选班级不存在，请重新选择班级".into());
                    }
                }
            }
            Action::ClearStudents => {
                if !teaching_data_edit_allowed(self.st.data_unlocked) {
                    self.st.status = Some("名单已锁定：请输入密码 123 解锁后再修改".into());
                    return;
                }
                let idx = self.st.stu_class_idx;
                if let Some(entry) = self.cfg.class_list.get_mut(idx) {
                    entry.students.clear();
                    self.st.stu_selected = None;
                    self.save_all();
                    self.st.status = Some("名单已清空并保存".into());
                }
            }
            Action::SaveNotices => self.save_notices_async(),
            Action::ReloadNotices => {
                // 同时重置 requested：否则首次拉取过后「重新拉取」永远不会再发请求
                self.st.notices_loaded = false;
                self.st.notices_requested = false;
            }
            Action::TestConn => self.test_connection(),
            Action::ManualSync => self.manual_sync(),
            Action::Unlock => {
                if crate::password::verify(&self.st.pwd_input) {
                    self.st.unlocked = true;
                    self.st.pwd_input.clear();
                    self.st.status = Some("已解锁，可编辑".into());
                } else {
                    self.st.status = Some("密码错误".into());
                }
            }
            Action::ChangePwd => {
                if !self.st.unlocked {
                    self.st.status = Some("请先解锁".into());
                    return;
                }
                let new = self.st.new_pwd.trim().to_string();
                if new.is_empty() {
                    self.st.status = Some("请先在「新密码」框输入新密码".into());
                } else if crate::password::set_password(&new) {
                    self.st.new_pwd.clear();
                    self.st.status = Some("密码已更新".into());
                } else {
                    self.st.status = Some("密码更新失败".into());
                }
            }
            Action::CommitNotice => {
                if let Some(ed) = self.st.pending_notice.take() {
                    let weekdays: Vec<serde_json::Value> = ed
                        .weekdays
                        .iter()
                        .enumerate()
                        .filter(|(_, on)| **on)
                        .map(|(i, _)| serde_json::json!(i + 1))
                        .collect();
                    let n = ScheduledNotice {
                        enabled: ed.enabled,
                        name: String::new(),
                        content: ed.content.trim().to_string(),
                        weekdays,
                        start: ed.start.trim().to_string(),
                        end: ed.end.trim().to_string(),
                        extra: Default::default(),
                    };
                    match ed.index {
                        Some(i) if i < self.st.notices.len() => self.st.notices[i] = n,
                        _ => self.st.notices.push(n),
                    }
                    self.st.notice_edit = None;
                    self.save_notices_async();
                }
            }
            Action::DeleteNotice(i) => {
                if i < self.st.notices.len() {
                    self.st.notices.remove(i);
                    self.save_notices_async();
                }
            }
            Action::CheckUpdates => self.check_updates_async(),
            Action::LoadClassCode => self.load_class_code_async(),
        }
    }

    // ---------- 后台任务 ----------

    fn load_notices_async(&mut self) {
        if self.st.busy.is_some() || self.st.rx.is_some() {
            return;
        }
        self.st.busy = Some("正在从 Gitee 拉取定时通知清单...".into());
        let cfg = self.cfg.clone();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.st.rx = Some(rx);
        std::thread::spawn(move || {
            let api = crate::gitee::GiteeApi::new(&cfg.gitee_owner, &cfg.gitee_repo, &cfg.gitee_token);
            let path = format!("{}/{}", cfg.gitee_tongzhi_dir, crate::services::SCHEDULED_NOTICES_PATH);
            let msg = match api.download_file_content(&path) {
                Ok(data) => {
                    // 清单标准格式是裸数组（与运行时服务、启动播种一致）；
                    // 兼容旧的 {"notices":[...]} 包装格式
                    let notices: Vec<ScheduledNotice> =
                        serde_json::from_slice(&data).or_else(|_| {
                            serde_json::from_slice::<serde_json::Value>(&data).map(|v| {
                                v.get("notices")
                                    .and_then(|x| x.as_array())
                                    .map(|list| {
                                        list.iter()
                                            .filter_map(|n| serde_json::from_value(n.clone()).ok())
                                            .collect()
                                    })
                                    .unwrap_or_default()
                            })
                        })
                        .unwrap_or_default();
                    format!("NOTICES:{}", serde_json::to_string(&notices).unwrap_or_default())
                }
                Err(e) => {
                    if e.contains("404") {
                        "NOTICES:[]".to_string() // 仓库还没有清单：视为空
                    } else {
                        format!("拉取失败: {}", e)
                    }
                }
            };
            let _ = tx.send(msg);
        });
    }

    fn save_notices_async(&mut self) {
        if self.st.busy.is_some() {
            return;
        }
        self.st.busy = Some("正在保存定时通知清单到 Gitee...".into());
        let cfg = self.cfg.clone();
        let notices = self.st.notices.clone();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.st.rx = Some(rx);
        std::thread::spawn(move || {
            let api = crate::gitee::GiteeApi::new(&cfg.gitee_owner, &cfg.gitee_repo, &cfg.gitee_token);
            // 写裸数组（与运行时服务 load_scheduled_notices、启动播种 seed_builtin_notices 一致；
            // 旧版写 {"notices":[...]} 会导致教室端读不回清单）
            let payload = serde_json::to_string_pretty(&notices).unwrap_or_default();
            let path = format!("{}/{}", cfg.gitee_tongzhi_dir, crate::services::SCHEDULED_NOTICES_PATH);
            let msg = match api.create_text_file(&payload, &path, "master") {
                Ok(_) => "定时通知清单已保存到 Gitee".to_string(),
                Err(e) => format!("保存失败: {}", e),
            };
            let _ = tx.send(msg);
        });
    }

    /// 导出当前班名单为 CSV（带 BOM，Excel/WPS 直接打开不乱码）。
    fn export_students(&mut self) {
        let idx = self.cfg.class_list.iter().position(|c| c.file_name == self.cfg.current_class);
        match idx {
            Some(i) => self.export_students_at(i),
            None => self.st.status = Some("请先创建并切换到班级".into()),
        }
    }

    /// 导出指定班级（按 class_list 下标）名单为 CSV。
    fn export_students_at(&mut self, idx: usize) {
        let Some(entry) = self.cfg.class_list.get(idx).cloned() else {
            self.st.status = Some("请先创建并切换到班级".into());
            return;
        };
        if entry.students.is_empty() {
            self.st.status = Some("当前班名单为空，无可导出".into());
            return;
        }
        let mut rows = vec![vec!["序号".into(), "姓名".into()]];
        for (i, name) in entry.students.iter().enumerate() {
            rows.push(vec![(i + 1).to_string(), name.clone()]);
        }
        let base = sanitize_file_name(&entry.display_name);
        match export_csv(&base, "名单", &rows) {
            Ok(path) => self.st.status = Some(format!("✅ 名单已导出：{path}")),
            Err(e) => self.st.status = Some(format!("❌ 导出失败：{e}")),
        }
    }

    /// 课表 CSV 行构建（节次 × 周一~周五），导出入口共用。
    fn timetable_csv_rows(map: &std::collections::HashMap<(usize, usize), String>) -> Vec<Vec<String>> {
        let max_period = map.keys().map(|(_, p)| *p).max().unwrap_or(0).max(9).min(15);
        let wd = ["周一", "周二", "周三", "周四", "周五"];
        let mut rows = vec![vec!["节次".to_string()]
            .into_iter()
            .chain(wd.iter().map(|s| s.to_string()))
            .collect::<Vec<_>>()];
        for p in 1..=max_period {
            let mut row = vec![format!("第{p}节")];
            for w in 0..5usize {
                row.push(map.get(&(w, p as usize)).cloned().unwrap_or_default());
            }
            rows.push(row);
        }
        rows
    }

    /// 导出当前班课表为 CSV（节次 × 周一~周五）。
    fn export_timetable(&mut self) {
        let Some(entry) = self.cfg.current().cloned() else {
            self.st.status = Some("请先创建并切换到班级".into());
            return;
        };
        let map = crate::schedule::load_schedule(&entry.file_name);
        if map.is_empty() {
            self.st.status = Some("当前班课表为空，无可导出".into());
            return;
        }
        let rows = Self::timetable_csv_rows(&map);
        let base = sanitize_file_name(&entry.display_name);
        match export_csv(&base, "课表", &rows) {
            Ok(path) => self.st.status = Some(format!("✅ 课表已导出：{path}")),
            Err(e) => self.st.status = Some(format!("❌ 导出失败：{e}")),
        }
    }

    /// 导出「课表」页所选班级的课表：若界面上正在编辑该班（sched_loaded_for 匹配），
    /// 直接用当前网格导出——未落盘的修改（500ms 防抖窗内）也能一并导出。
    fn export_timetable_sched(&mut self) {
        let Some(entry) = self.cfg.class_list.get(self.st.sched_class_idx).cloned() else {
            self.st.status = Some("请先选择班级".into());
            return;
        };
        let live = self.st.sched_loaded_for == entry.file_name && !self.st.sched_grid.is_empty();
        let map = if live {
            self.timetable_map()
        } else {
            crate::schedule::load_schedule(&entry.file_name)
        };
        if map.is_empty() {
            self.st.status = Some("所选班级课表为空，无可导出".into());
            return;
        }
        let rows = Self::timetable_csv_rows(&map);
        let base = sanitize_file_name(&entry.display_name);
        match export_csv(&base, "课表", &rows) {
            Ok(path) => self.st.status = Some(format!("✅ {}课表已导出：{path}", entry.display_name)),
            Err(e) => self.st.status = Some(format!("❌ 导出失败：{e}")),
        }
    }

    fn test_connection(&mut self) {
        if self.st.busy.is_some() {
            return;
        }
        self.st.busy = Some("正在测试连接...".into());
        let cfg = self.cfg.clone();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.st.rx = Some(rx);
        std::thread::spawn(move || {
            let api = crate::gitee::GiteeApi::new(&cfg.gitee_owner, &cfg.gitee_repo, &cfg.gitee_token);
            let dir = if cfg.gitee_download_folder.is_empty() {
                Some(crate::paths::data_dir().join("gitee_test"))
            } else {
                None
            };
            let (ok, info) = api.test_connection(true, dir.as_deref());
            let _ = tx.send(if ok { format!("连接成功：{}", info) } else { format!("连接失败：{}", info) });
        });
    }

    fn check_updates_async(&mut self) {
        if self.st.busy.is_some() {
            return;
        }
        self.st.busy = Some("正在检查更新...".into());
        let cfg = self.cfg.clone();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.st.rx = Some(rx);
        std::thread::spawn(move || {
            let msg = crate::update::check_manual(cfg);
            let _ = tx.send(msg);
        });
    }

    fn manual_sync(&mut self) {        if self.st.busy.is_some() {
            return;
        }
        self.st.busy = Some("正在手动同步...".into());
        let cfg = self.cfg.clone();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.st.rx = Some(rx);
        std::thread::spawn(move || {
            let msg = match crate::services::manual_sync_once(&cfg) {
                Ok(n) => format!("手动同步完成！覆盖下载 {} 个文件", n),
                Err(e) => format!("同步失败: {}", e),
            };
            let _ = tx.send(msg);
        });
    }

    /// 查询当前班级 6 位码；查不到则自动生成并登记到 Gitee（与 Python 建班逻辑一致）。
    fn load_class_code_async(&mut self) {
        if self.st.class_code_loading || self.st.rx.is_some() {
            return;
        }
        self.st.class_code_loading = true;
        let cfg = self.cfg.clone();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.st.rx = Some(rx);
        std::thread::spawn(move || {
            let file_name = cfg.current_class.clone();
            let display = cfg
                .class_list
                .iter()
                .find(|c| c.file_name == file_name)
                .map(|c| c.display_name.clone())
                .unwrap_or_default();
            let code = match crate::classcode::fetch_class_code(&cfg, &file_name) {
                Some(c) => c,
                None => crate::classcode::ensure_class_code(&cfg, &file_name, &display),
            };
            let _ = tx.send(format!("CLASSCODE:{code}"));
        });
    }

    fn poll_bg(&mut self) {
        self.poll_cloud(); // 云端名单用独立 channel，与通知/班级码互不抢占
        let msg = self.st.rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(msg) = msg {
            self.st.rx = None;
            self.st.busy = None;
            if let Some(code) = msg.strip_prefix("CLASSCODE:") {
                self.st.class_code_loading = false;
                self.st.class_code = Some(code.to_string());
            } else if let Some(payload) = msg.strip_prefix("NOTICES:") {
                if let Ok(list) = serde_json::from_str::<Vec<ScheduledNotice>>(payload) {
                    self.st.notices = list;
                    self.st.notices_loaded = true;
                    // 拉取成功不打扰用户（列表直接呈现即可），只清掉"正在拉取"提示
                    if self.st.status.as_deref() == Some("正在从 Gitee 拉取定时通知清单...") {
                        self.st.status = None;
                    }
                }
            } else {
                self.st.status = Some(msg);
            }
        }
    }

    /// 帧首：课表网格跟随所选班级。
    /// 切班前若上一班课表还在 500ms 防抖窗内未落盘，先按「旧班级」写盘再重载，
    /// 避免 load_grid 用新班数据覆盖网格后把旧班编辑永久丢失（不会串班，但会丢数据）。
    fn sync_grid(&mut self) {
        let cls_id = self
            .cfg
            .class_list
            .get(self.st.sched_class_idx)
            .map(|c| c.file_name.clone())
            .unwrap_or_default();
        if self.st.sched_loaded_for != cls_id {
            if self.st.sched_dirty_since.is_some() {
                self.autosave_timetable(); // 按 sched_loaded_for（旧班）写盘
                self.st.sched_dirty_since = None;
            }
            load_grid(&mut self.st, &self.cfg, &cls_id);
        }
    }
}

/// 载入某班课表到编辑网格。
fn load_grid(st: &mut UiState, _cfg: &Config, cls_id: &str) {
    st.sched_loaded_for = cls_id.to_string();
    if cls_id.is_empty() {
        st.sched_grid.clear();
        st.sched_periods = 9;
        return;
    }
    let map = crate::schedule::load_schedule(cls_id);
    let max_period = map.keys().map(|(_, p)| *p).max().unwrap_or(0).max(9);
    st.sched_periods = max_period.min(15);
    st.sched_grid = vec![[const { String::new() }; 5]; st.sched_periods];
    for ((wd, pe), course) in map {
        if wd < 5 && pe >= 1 && (pe as usize) <= st.sched_periods {
            st.sched_grid[pe as usize - 1][wd] = course;
        }
    }
}

// ==================== 粘贴解析（纯函数，便于单测） ====================

fn blank_week() -> [String; 5] {
    [const { String::new() }; 5]
}

fn trim_cell(s: &str) -> String {
    s.trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '“' || c == '”')
        .trim()
        .to_string()
}

/// 表头单元格：星期 / 节次 / 课程 / 时间 之类。
fn looks_like_header(s: &str) -> bool {
    let t = s.trim();
    t.contains("星期")
        || t.contains("周")
        || t.contains("节次")
        || t.contains("第") && t.contains("节")
        || t.eq_ignore_ascii_case("mon")
        || t.eq_ignore_ascii_case("time")
        || t == "课程"
        || t == "时间"
}

/// 首列是不是「第N节 / N / 节次」标签。
fn is_period_label(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || t == "节次" || t == "课程" {
        return true;
    }
    if t.parse::<usize>().is_ok() {
        return true;
    }
    (t.starts_with('第') && t.contains('节')) || t == "早读" || t == "晚自习"
}

/// 首列是不是星期标签；返回星期下标 0=周一。
fn weekday_label(s: &str) -> Option<usize> {
    let t = s.trim();
    let cn = ["一", "二", "三", "四", "五", "1", "2", "3", "4", "5"];
    for (i, suf) in cn.iter().enumerate() {
        let i = i % 5;
        if t == *suf
            || t == format!("周{suf}")
            || t == format!("星期{suf}")
            || t == format!("礼拜{suf}")
        {
            return Some(i);
        }
    }
    for (i, en) in ["mon", "tue", "wed", "thu", "fri"].iter().enumerate() {
        if t.to_ascii_lowercase().starts_with(en) {
            return Some(i);
        }
    }
    None
}

/// 单元格清成科目名；表头 / 占位符 / 标签列残留一律跳过。
fn course_cell(s: &str) -> String {
    let v = trim_cell(s);
    if v.is_empty() || matches!(v.as_str(), "-" | "—" | "–" | "/" | "无" | "空" | "无课") {
        return String::new();
    }
    if looks_like_header(&v) || is_period_label(&v) || weekday_label(&v).is_some() {
        return String::new();
    }
    if v.chars().count() > 20 {
        return String::new();
    }
    v
}

/// 解析粘贴的课表，返回 (网格, 有效科目数)。
///
/// 兼容：Excel 的 Tab / 逗号分隔；有表头行或无表头；有标签列（第1节 / 星期一）
/// 或纯数据；行=星期（横向节次）或 行=节次（纵向星期）自动识别。
fn parse_schedule_paste(
    text: &str,
    periods: usize,
) -> Result<(Vec<[String; 5]>, usize), String> {
    if text.trim().is_empty() {
        return Err("请先粘贴课表到下方文本框".into());
    }
    let mut rows: Vec<Vec<String>> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            l.split(['\t', ',', '，'])
                .map(trim_cell)
                .collect::<Vec<String>>()
        })
        .filter(|r| r.iter().any(|c| !c.is_empty()))
        .collect();
    if rows.is_empty() {
        return Err("无法解析课表：内容为空".into());
    }
    // 表头行会有多个「星期 / 节次」标签；只有首列一个标签的是数据行，不能丢。
    if rows[0].iter().filter(|c| looks_like_header(c)).count() >= 2 {
        rows.remove(0);
    }
    if rows.is_empty() {
        return Err("只有表头，没有课表数据".into());
    }
    // 只有首列确实是标签时才丢掉第一列；纯 5 列数据不能丢。
    let label_col = rows
        .iter()
        .filter(|r| !r.is_empty())
        .all(|r| is_period_label(&r[0]) || weekday_label(&r[0]).is_some());
    let data_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            if label_col && r.len() > 1 {
                r[1..].to_vec()
            } else {
                r.clone()
            }
        })
        .collect();
    let weekday_rows = rows
        .iter()
        .filter(|r| !r.is_empty() && weekday_label(&r[0]).is_some())
        .count();
    let by_weekday = weekday_rows >= 2;

    let mut grid: Vec<[String; 5]> = vec![blank_week(); periods.max(9)];
    let mut n = 0usize;
    for (ri, row) in data_rows.iter().enumerate() {
        for (ci, raw) in row.iter().enumerate() {
            let v = course_cell(raw);
            if v.is_empty() {
                continue;
            }
            if by_weekday {
                // 行=星期，列=节次
                if ri < 5 && ci < 15 {
                    if ci >= grid.len() {
                        grid.resize(ci + 1, blank_week());
                    }
                    grid[ci][ri] = v;
                    n += 1;
                }
            } else {
                // 行=节次，列=星期
                if ri < 15 && ci < 5 {
                    if ri >= grid.len() {
                        grid.resize(ri + 1, blank_week());
                    }
                    grid[ri][ci] = v;
                    n += 1;
                }
            }
        }
    }
    if n == 0 {
        return Err("无法解析课表：没识别到科目（检查是否含表头/序号列）".into());
    }
    Ok((grid, n))
}

/// 网格 → schedules/class_*.json 的坐标：(星期, 节次) → 科目。
/// 抽成纯函数，保证导入后的格子写入的位置可被单测核对。
fn grid_to_map(grid: &[[String; 5]]) -> std::collections::HashMap<(usize, usize), String> {
    let mut map = std::collections::HashMap::new();
    for (p, row) in grid.iter().enumerate() {
        for (d, course) in row.iter().enumerate() {
            let c = course.trim();
            if !c.is_empty() {
                map.insert((d, p + 1), c.to_string());
            }
        }
    }
    map
}

/// 表头 / 非姓名的列值。
fn is_student_noise(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || t.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    if t.chars().count() > 30 || matches!(t, "男" | "女") {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "姓名"
            | "名字"
            | "学生"
            | "学生姓名"
            | "学号"
            | "序号"
            | "座号"
            | "编号"
            | "班级"
            | "性别"
            | "备注"
            | "name"
            | "no"
            | "no."
            | "id"
    )
}

/// 解析粘贴的学生名单：支持每行一个姓名，或 Tab / 逗号 / 顿号 / 分号 /
/// 竖线 / 空白分隔的一整块；自动跳过表头、序号列、纯数字学号，并保持顺序去重。
fn parse_student_paste(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        for raw in line.split(|c: char| {
            matches!(c, '\t' | ',' | '，' | ';' | '；' | '、' | '|') || c.is_whitespace()
        }) {
            let s = trim_cell(raw);
            if is_student_noise(&s) {
                continue;
            }
            if !out.iter().any(|x| x == &s) {
                out.push(s);
            }
        }
    }
    out
}

// ==================== 渲染（纯函数） ====================

fn status_line(ui: &mut egui::Ui, msg: &Option<String>) {
    if let Some(m) = msg {
        ui.colored_label(Color32::from_rgb(0x1b, 0x7f, 0x2d), m);
    }
}

/// 一行「标签 + 50%~250% 缩放滑杆 + 重置」，用于点名窗 / 倒计时窗。
fn scale_row(ui: &mut egui::Ui, label: &str, id: &str, val: &mut f64) {
    let cur = if *val > 0.0 { *val } else { 1.0 };
    ui.horizontal(|ui| {
        ui.label(label);
        let mut pct = (cur * 100.0).round() as i64;
        let r = ui.push_id(id, |ui| {
            ui.add(egui::Slider::new(&mut pct, 50..=250).text("缩放"))
        });
        if r.inner.changed() {
            *val = pct as f64 / 100.0;
        }
        if ui.small_button("重置").clicked() {
            *val = 1.0;
        }
        ui.weak(format!("{:.0}%", cur * 100.0));
    });
}

// 「编辑桌面大屏」设置页入口已移除（open_board_editor 一并删除）：
// 打开编辑器走壁纸上的「今日作业」卡片。

/// 原「基本设置」页拆成两段：section=0 班级（选择/班级码/班级管理），section=1 显示（缩放/位置/开机/作业栏）。
/// 两段共用同一批状态与 action，逻辑零改动，只按 section 各画各的。
fn ui_basic(ui: &mut egui::Ui, cfg: &mut Config, st: &mut UiState, action: &mut Option<Action>, section: u8) {
    let names: Vec<String> = cfg.class_list.iter().map(|c| c.display_name.clone()).collect();
    let cur = cfg.class_list.iter().position(|c| c.file_name == cfg.current_class).unwrap_or(0);

    // ---------- 班级段 ----------
    if section == 0 {
    ui.horizontal(|ui| {
        ui.label("班级：");
        egui::ComboBox::from_id_salt("current_class")
            .selected_text(names.get(cur).map(|s| s.as_str()).unwrap_or("（未创建班级）"))
            .width(220.0)
            .show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    if ui.selectable_label(i == cur, n).clicked() {
                        *action = Some(Action::SetCurrentClass(cfg.class_list[i].file_name.clone()));
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label("班级码：");
        if let Some(code) = &st.class_code {
            ui.monospace(code);
            if ui.small_button("复制").clicked() {
                ui.ctx().copy_text(code.clone());
                st.status = Some("6 位班级码已复制".into());
            }
        } else {
            ui.weak(if st.class_code_loading { "查询中…" } else { "未登记" });
        }
        if ui.small_button("刷新/生成").clicked() {
            *action = Some(Action::LoadClassCode);
        }
    });
    ui.label("网页端 https://gch365.github.io/physics-html/notify/ 输入 6 位班级码，即可发送通知到本班");
    ui.add_space(4.0);

    // ---------- 班级管理（仍属于班级段 section 0） ----------
    ui.separator();
    ui.heading("班级管理");
    ui.horizontal(|ui| {
        ui.label("学校：");
        ui.add(egui::TextEdit::singleline(&mut st.new_school).desired_width(110.0));
        ui.label("年级：");
        egui::ComboBox::from_id_salt("new_grade")
            .selected_text(st.new_grade.clone())
            .width(70.0)
            .show_ui(ui, |ui| {
                for g in GRADES {
                    if ui.selectable_label(st.new_grade == g, g).clicked() {
                        st.new_grade = g.to_string();
                    }
                }
            });
        ui.label("班号：");
        egui::ComboBox::from_id_salt("new_class_num")
            .selected_text(st.new_class_num.clone())
            .width(56.0)
            .show_ui(ui, |ui| {
                for n in 1..=35 {
                    let s = n.to_string();
                    if ui.selectable_label(st.new_class_num == s, &s).clicked() {
                        st.new_class_num = s;
                    }
                }
            });
        if ui.button("➕ 创建班级").clicked() {
            *action = Some(Action::CreateClass);
        }
    });
    ui.horizontal(|ui| {
        ui.label(format!(
            "本教室已有 {} 个班级（默认最多 {} 个）",
            cfg.class_list.len(),
            MAX_CLASSES_FREE
        ));
        if st.class_unlocked {
            ui.colored_label(Color32::from_rgb(0x18, 0x96, 0x58), "已解锁：可继续创建班级");
        } else if cfg.class_list.len() >= MAX_CLASSES_FREE {
            ui.label("解锁密码：");
            let edit = ui.add(
                egui::TextEdit::singleline(&mut st.class_pwd)
                    .password(true)
                    .desired_width(90.0),
            );
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("🔓 解锁").clicked() || enter {
                *action = Some(Action::UnlockClasses);
            }
            ui.label("创建更多班级需输入密码（本次设置窗口有效）");
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("☁ 云端班级名单");
        if ui
            .add_enabled(!st.cloud_busy, egui::Button::new("从云端补全当前班"))
            .clicked()
        {
            *action = Some(Action::FetchCloud);
        }
        if st.cloud_busy {
            ui.spinner();
        }
    });
    if let Some(s) = &st.cloud_status {
        ui.weak(s);
    }
    ui.horizontal(|ui| {
        if ui.button("📤 导出名单").clicked() {
            *action = Some(Action::ExportStudents);
        }
        if ui.button("📤 导出课表").clicked() {
            *action = Some(Action::ExportTimetable);
        }
        ui.label("CSV 存到桌面「点名系统导出」文件夹，Excel/WPS 可直接打开");
    });
    ui.horizontal(|ui| {
        if ui.button("🗑 删除当前班级").clicked() {
            *action = Some(Action::DeleteClass(cur));
        }
        if st.class_clear_confirm {
            ui.colored_label(Color32::from_rgb(0xC0, 0x39, 0x2B), "⚠ 会清空全部班级");
            if ui.button("确定清空").clicked() {
                *action = Some(Action::ClearAllClasses);
            }
            if ui.button("取消").clicked() {
                st.class_clear_confirm = false;
            }
        } else if !cfg.class_list.is_empty()
            && ui.button("🧹 清空所有班级（回到 0 班）").clicked()
        {
            *action = Some(Action::ClearAllClasses);
        }
    });
    } // end section 0 班级管理

    // ---------- 显示段 ----------
    if section == 1 {
    // 侧边栏大小：自由滑杆（滚轮/滑杆等效）；bar_scale=0 表示未设置，回落旧三档
    let bs = if cfg.bar_scale > 0.0 {
        cfg.bar_scale
    } else {
        match cfg.bar_size.as_str() {
            "小" => 1.1,
            "中" => 1.4,
            _ => 1.8,
        }
    };
    ui.horizontal(|ui| {
        ui.label("侧边栏大小：");
        let mut pct = (bs * 100.0).round() as i64; // 50%~250% 整数滑杆
        let r = ui.add(egui::Slider::new(&mut pct, 50..=250).text("缩放"));
        if r.changed() {
            cfg.bar_scale = pct as f64 / 100.0;
        }
        if ui.small_button("重置").clicked() {
            cfg.bar_scale = 0.0; // 回落旧三档（大）
            cfg.bar_size = "大".to_string();
        }
    });
    ui.weak(format!("当前：{:.0}%（提示：在侧边条上滚动鼠标滚轮也可自由调节）", bs * 100.0));
    // 点名窗 / 倒计时窗：与侧边栏同样的自由缩放滑杆（50%~250%）
    scale_row(ui, "点名窗口大小：", "roll_scale", &mut cfg.roll_scale);
    scale_row(ui, "倒计时窗口大小：", "cd_scale", &mut cfg.countdown_scale);
    ui.horizontal(|ui| {
        ui.label("默认倒计时(秒)：");
        ui.add(egui::Slider::new(&mut cfg.default_countdown, 30..=300));
    });
    // 通知横幅自由缩放（与横幅上的滚轮等效）；位置直接拖动横幅即可
    let ns = if cfg.notice_scale > 0.0 { cfg.notice_scale } else { 1.0 };
    ui.horizontal(|ui| {
        ui.label("通知横幅大小：");
        let mut pct = (ns * 100.0).round() as i64;
        let r = ui.add(egui::Slider::new(&mut pct, 50..=250).text("缩放"));
        if r.changed() {
            cfg.notice_scale = pct as f64 / 100.0;
        }
        if ui.small_button("重置").clicked() {
            cfg.notice_scale = 0.0;
        }
    });
    ui.weak("在通知横幅上滚动滚轮可自由缩放，按住拖动可改位置（自动保存）");

    // 「编辑桌面大屏」入口已按需求移除：直接点壁纸上的「今日作业」卡片即可打开编辑器

    // 点名总时长（滚动 + 定格，2~5 秒可调，默认 3 秒；自动保存）
    ui.horizontal(|ui| {
        ui.label("点名总时长：");
        let mut secs = cfg.roll_seconds;
        let r = ui.add(
            egui::Slider::new(&mut secs, 2.0..=5.0)
                .step_by(0.5)
                .text("秒"),
        );
        if r.changed() {
            cfg.roll_seconds = secs;
        }
        ui.weak(format!("当前 {:.1} 秒", cfg.roll_seconds));
    });
    ui.weak("从开始滚动到结果消失的总时长，2~5 秒可调（自动保存，下次点名生效）");

    // 垂直位置按「实际 y 的百分比」直接控制。旧版存像素偏移，一旦越界被
    // 夹在屏幕底部，上移/下移按钮点很多次都不动，看起来只有两个固定位置。
    let (sh, bh, cur_y, max_y) = {
        let (_, bh, y, _, _, max_y) = crate::ui::bar_dimensions(cfg);
        let (_, sh) = crate::win::screen_size();
        (sh, bh, y, max_y)
    };
    ui.horizontal(|ui| {
        ui.label("侧边栏垂直位置：");
        let mut pct = if max_y > 0 { cur_y as i64 * 100 / max_y as i64 } else { 0 };
        let r = ui.add(egui::Slider::new(&mut pct, 0..=100).text("位置"));
        if r.changed() {
            let new_y = (max_y as i64 * pct / 100) as i32;
            cfg.bar_top_offset = new_y - (sh - bh) / 2;
        }
        ui.weak(format!("{pct}%  ({cur_y}px)"));
    });
    ui.weak("两条侧边栏可直接按住拖动，左右、上下都能自由摆放（自动保存）");

    ui.checkbox(&mut cfg.auto_run, "开机自动启动");
    ui.checkbox(&mut cfg.board_autostart, "自动启动桌面大屏（通知/作业/值日）");

    ui.separator();
    ui.heading("桌面作业栏");
    ui.weak("作业栏的 ✕（隐藏）与电源按钮（彻底关闭）都会弹确认框，需输入确认码 123 并点「确定」才会生效。");
    let mut show_hw = !desktop_board::board::homework_hard_hidden();
    if ui
        .checkbox(
            &mut show_hw,
            "显示桌面作业栏（取消勾选 = 彻底关闭，需输入确认码）",
        )
        .changed()
    {
        if show_hw {
            // 重新打开不需要确认，立即生效
            desktop_board::board::patch_keys(&[
                ("homework_hard_hidden", serde_json::json!(false)),
                ("homework_hidden", serde_json::json!(false)),
            ]);
            // 大屏在跑时靠它自己的 400ms 轮询显示；没在跑就顺手拉起来
            let _ = std::fs::remove_file(crate::paths::board_stop_file());
            if !crate::win::board_is_running() {
                crate::win::spawn_self(&["--desktop-board"]);
            }
            st.status = Some("已重新打开桌面作业栏（切到桌面即可看到）".into());
        } else {
            // 关闭是危险操作：先弹确认框，确认通过后才写持久标志。
            // 此处不写盘，下一帧 checkbox 会自然回到勾选状态。
            st.hw_confirm_open = true;
            st.hw_confirm_code.clear();
        }
    }
    } // end section 1 显示段
    ui.add_space(6.0);
    status_line(ui, &st.status);
}

fn ui_time(ui: &mut egui::Ui, cfg: &mut Config, st: &mut UiState, action: &mut Option<Action>) {
    // 作息模式（中文 ↔ summer/winter）
    let mode_label = if cfg.work_time == "winter" { "冬季作息" } else { "夏季作息" };
    ui.horizontal(|ui| {
        ui.label("作息模式：");
        egui::ComboBox::from_id_salt("work_time")
            .selected_text(mode_label)
            .width(200.0)
            .show_ui(ui, |ui| {
                // 切模式前先把当前模式未落盘的编辑写盘，避免 500ms 防抖窗内切换丢失
                let flush = |st: &mut UiState, cfg: &Config| {
                    if st.time_dirty_since.is_some() {
                        let mode = cfg.work_time.clone();
                        let rows = if mode == "winter" {
                            &st.time_rows_winter
                        } else {
                            &st.time_rows_summer
                        };
                        let valid = !rows.is_empty()
                            && rows.iter().all(|(s, e)| is_hhmm(s) && is_hhmm(e));
                        if valid {
                            let _ = crate::schedule::save_time_table(&mode, rows);
                        }
                        st.time_dirty_since = None;
                    }
                };
                if ui.selectable_label(cfg.work_time != "winter", "夏季作息").clicked() {
                    flush(st, cfg);
                    cfg.work_time = "summer".into();
                }
                if ui.selectable_label(cfg.work_time == "winter", "冬季作息").clicked() {
                    flush(st, cfg);
                    cfg.work_time = "winter".into();
                }
            });
    });
    ui.separator();
    ui.heading("作息时间表（改动即时生效，无需保存）");
    let mut remove: Option<usize> = None;
    ScrollArea::vertical().id_salt("time_rows").max_height(420.0).show(ui, |ui| {
        let n_rows = if cfg.work_time == "winter" { st.time_rows_winter.len() } else { st.time_rows_summer.len() };
        let rows = if cfg.work_time == "winter" { &mut st.time_rows_winter } else { &mut st.time_rows_summer };
        // 三列排布：按 ceil(n/3) 均分成左/中/右三列（一屏看完，不用长滚动）
        let per = (n_rows + 2) / 3;
        let (left, rest) = rows.split_at_mut(per.min(n_rows));
        let (mid, right) = rest.split_at_mut(per.min(rest.len()));
        egui::Grid::new("time_rows_grid").spacing([8.0, 4.0]).show(ui, |ui| {
            for i in 0..per {
                let period_label = |p: usize| -> String {
                    if p == 9 { "晚自习".to_string() } else { format!("第{}节", p + 1) }
                };
                // 左列
                if i < left.len() {
                    ui.label(period_label(i));
                    let c0 = ui.add(egui::TextEdit::singleline(&mut left[i].0).desired_width(60.0)).changed();
                    ui.label("至");
                    let c1 = ui.add(egui::TextEdit::singleline(&mut left[i].1).desired_width(60.0)).changed();
                    if c0 || c1 {
                        st.time_dirty_since.get_or_insert_with(std::time::Instant::now);
                    }
                    if ui.small_button("✕").clicked() && n_rows > 1 {
                        remove = Some(i);
                    }
                }
                // 中列
                if i < mid.len() {
                    let j = per + i;
                    ui.label(period_label(j));
                    let c0 = ui.add(egui::TextEdit::singleline(&mut mid[i].0).desired_width(60.0)).changed();
                    ui.label("至");
                    let c1 = ui.add(egui::TextEdit::singleline(&mut mid[i].1).desired_width(60.0)).changed();
                    if c0 || c1 {
                        st.time_dirty_since.get_or_insert_with(std::time::Instant::now);
                    }
                    if ui.small_button("✕").clicked() && n_rows > 1 {
                        remove = Some(j);
                    }
                }
                // 右列
                if i < right.len() {
                    let j = per * 2 + i;
                    ui.label(period_label(j));
                    let c0 = ui.add(egui::TextEdit::singleline(&mut right[i].0).desired_width(60.0)).changed();
                    ui.label("至");
                    let c1 = ui.add(egui::TextEdit::singleline(&mut right[i].1).desired_width(60.0)).changed();
                    if c0 || c1 {
                        st.time_dirty_since.get_or_insert_with(std::time::Instant::now);
                    }
                    if ui.small_button("✕").clicked() && n_rows > 1 {
                        remove = Some(j);
                    }
                }
                ui.end_row();
            }
        });
    });
    if let Some(i) = remove {
        *action = Some(Action::RemovePeriod(i));
    }
    ui.horizontal(|ui| {
        if ui.button("➕ 添加一节").clicked() {
            *action = Some(Action::AddPeriod);
        }
        if ui.button("↩ 恢复内置默认").clicked() {
            *action = Some(Action::ResetTimeTable);
            st.time_dirty_since.get_or_insert_with(std::time::Instant::now);
        }
    });
    status_line(ui, &st.status);
}

/// 教学数据修改锁条：名单 / 课表默认只读，输入密码 123 解锁后本次窗口内可改。
fn ui_data_lock(ui: &mut egui::Ui, st: &mut UiState, action: &mut Option<Action>) {
    ui.horizontal(|ui| {
        if st.data_unlocked {
            ui.colored_label(Color32::from_rgb(0x18, 0x96, 0x58), "🔓 已解锁：本次窗口内可修改学生名单与课表");
            if ui.button("🔒 立即上锁").clicked() {
                st.data_unlocked = false;
                st.data_pwd.clear();
                st.status = Some("名单与课表已上锁".into());
            }
        } else {
            ui.colored_label(Color32::from_rgb(0xC0, 0x39, 0x2B), "🔒 名单与课表已锁定（修改需密码 123）");
            ui.label("口令：");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut st.data_pwd)
                    .password(true)
                    .desired_width(110.0)
                    .hint_text("•••"),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("🔓 解锁").clicked() || enter {
                *action = Some(Action::UnlockData);
            }
        }
    });
}

fn ui_students(ui: &mut egui::Ui, cfg: &mut Config, st: &mut UiState, action: &mut Option<Action>) {
    let editable = st.data_unlocked;
    let names: Vec<String> = cfg.class_list.iter().map(|c| c.display_name.clone()).collect();
    if st.stu_class_idx >= names.len() {
        st.stu_class_idx = 0;
    }
    ui.horizontal(|ui| {
        ui.label("选择班级：");
        egui::ComboBox::from_id_salt("stu_class")
            .selected_text(names.get(st.stu_class_idx).map(|s| s.as_str()).unwrap_or("—"))
            .width(220.0)
            .show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut st.stu_class_idx, i, n.clone());
                }
            });
    });
    let idx = st.stu_class_idx;
    let count = cfg.class_list.get(idx).map(|c| c.students.len()).unwrap_or(0);
    ui.label(format!("共 {} 名学生（修改自动保存）", count));
    ui.separator();

    let mut removed: Option<usize> = None;
    ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
        if let Some(entry) = cfg.class_list.get(idx) {
            // 一行 6 个名字（52 人名单一屏可见，不再一列长滚动）
            const STU_COLS: usize = 6;
            let names = entry.students.clone();
            let n = names.len();
            egui::Grid::new("stu_grid").spacing([6.0, 4.0]).show(ui, |ui| {
                let mut k = 0;
                while k < n {
                    let end = (k + STU_COLS).min(n);
                    for i in k..end {
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_label(st.stu_selected == Some(i), &names[i])
                                .clicked()
                            {
                                st.stu_selected = Some(i);
                            }
                            if ui.add_enabled(editable, egui::Button::new("✕").small()).clicked() {
                                removed = Some(i);
                            }
                        });
                    }
                    // 末行补空位，保证网格列数一致（条纹行不断裂）
                    for _ in end..k + STU_COLS {
                        ui.label("");
                    }
                    ui.end_row();
                    k += STU_COLS;
                }
            });
        }
    });
    if let Some(i) = removed {
        if editable {
            if let Some(entry) = cfg.class_list.get_mut(idx) {
                if i < entry.students.len() {
                    entry.students.remove(i);
                    st.stu_selected = None;
                    *action = Some(Action::Save);
                }
            }
        }
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("添加学生：");
        let resp = ui.add_enabled(
            editable,
            egui::TextEdit::singleline(&mut st.stu_input).desired_width(160.0),
        );
        let enter = editable
            && resp.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.add_enabled(editable, egui::Button::new("➕ 添加")).clicked() || enter {
            let name = st.stu_input.trim().to_string();
            if !name.is_empty() {
                if let Some(entry) = cfg.class_list.get_mut(idx) {
                    entry.students.push(name);
                    *action = Some(Action::Save);
                }
                st.stu_input.clear();
            }
        }
        if ui.add_enabled(editable, egui::Button::new("🗑 删除选中")).clicked() {
            if let (Some(i), Some(entry)) = (st.stu_selected, cfg.class_list.get_mut(idx)) {
                if i < entry.students.len() {
                    entry.students.remove(i);
                    st.stu_selected = None;
                    *action = Some(Action::Save);
                }
            }
        }
        if ui.add_enabled(editable, egui::Button::new("🧹 清空名单")).clicked() {
            *action = Some(Action::ClearStudents);
        }
        if ui.button("📤 导出名单").clicked() {
            *action = Some(Action::ExportStudentsAt(idx));
        }
        ui.weak("CSV 存到桌面「点名系统导出」文件夹，Excel/WPS 可直接打开");
    });
    ui.label("批量粘贴导入（替换现有名单）：每行一个姓名，或 Tab / 逗号 / 顿号 / 空格分隔；表头与序号列自动跳过");
    ui.add_enabled(
        editable,
        egui::TextEdit::multiline(&mut st.stu_paste).desired_rows(4).desired_width(420.0),
    );
    if ui.add_enabled(editable, egui::Button::new("📥 导入名单")).clicked() {
        *action = Some(Action::ImportStudents);
    }
    status_line(ui, &st.status);
}

fn ui_timetable(ui: &mut egui::Ui, cfg: &mut Config, st: &mut UiState, action: &mut Option<Action>) {
    let editable = st.data_unlocked;
    let names: Vec<String> = cfg.class_list.iter().map(|c| c.display_name.clone()).collect();
    if st.sched_class_idx >= names.len() {
        st.sched_class_idx = 0;
    }
    // 本帧编辑的网格归属班级：打 dirty 标记时一并记录，切班后仍能写回正确的班
    let edit_class = st.sched_loaded_for.clone();
    ui.horizontal(|ui| {
        ui.label("选择班级：");
        let before = st.sched_class_idx;
        egui::ComboBox::from_id_salt("sched_class")
            .selected_text(names.get(st.sched_class_idx).map(|s| s.as_str()).unwrap_or("—"))
            .width(220.0)
            .show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut st.sched_class_idx, i, n.clone());
                }
            });
        if st.sched_class_idx != before {
            st.sched_loaded_for = String::from("?");
        }
    });

    ui.horizontal(|ui| {
        if ui.add_enabled(editable, egui::Button::new("📥 粘贴导入")).clicked() {
            *action = Some(Action::ImportSchedule);
        }
        if ui.add_enabled(editable, egui::Button::new("🗑️ 清除课表")).clicked() {
            *action = Some(Action::ClearTimetable);
        }
        if ui.button("📤 导出课表").clicked() {
            *action = Some(Action::ExportTimetableSched);
        }
        ui.separator();
        ui.label("节数：");
        if ui.add_enabled(editable && st.sched_periods < 15, egui::Button::new("➕")).clicked() {
            st.sched_periods += 1;
            st.sched_grid.push([const { String::new() }; 5]);
            st.sched_dirty_class = edit_class.clone();
            st.sched_dirty_since.get_or_insert_with(std::time::Instant::now);
        }
        ui.monospace(st.sched_periods.to_string());
        if ui.add_enabled(editable && st.sched_periods > 1, egui::Button::new("➖")).clicked() {
            st.sched_periods -= 1;
            st.sched_grid.truncate(st.sched_periods);
            st.sched_dirty_class = edit_class.clone();
            st.sched_dirty_since.get_or_insert_with(std::time::Instant::now);
        }
    });
    status_line(ui, &st.status);

    ui.add_space(4.0);
    ScrollArea::both().id_salt("sched_grid").show(ui, |ui| {
        egui::Grid::new("sched_table").striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
            ui.strong("节次");
            for d in WEEKDAY_NAMES.iter().take(5) {
                ui.strong(*d);
            }
            ui.end_row();
            for (p, row) in st.sched_grid.iter_mut().enumerate() {
                let label = if p == 9 {
                    "晚自习".to_string()
                } else {
                    format!("第{}节", p + 1)
                };
                ui.label(label);
                for cell in row.iter_mut() {
                    if ui.add_enabled(
                        editable,
                        egui::TextEdit::singleline(cell).desired_width(100.0),
                    ).changed() {
                        st.sched_dirty_class = edit_class.clone();
                        st.sched_dirty_since.get_or_insert_with(std::time::Instant::now);
                    }
                }
                ui.end_row();
            }
        });
    });

    ui.add_space(6.0);
    ui.label("粘贴导入（Excel 直接复制，Tab / 逗号分隔；有表头或「第1节 / 星期一」标签列都行，行=星期 或 行=节次 自动识别；点「粘贴导入」即保存）：");
    ui.add_enabled(
        editable,
        egui::TextEdit::multiline(&mut st.sched_paste).desired_rows(3).desired_width(560.0),
    );
    status_line(ui, &st.status);
}

fn ui_notices(ui: &mut egui::Ui, cfg: &Config, st: &mut UiState, action: &mut Option<Action>) {
    ui.separator();

    if let Some(ed) = &mut st.notice_edit {
        let mut committed: Option<NoticeEdit> = None;
        let mut cancel = false;
        let mut del_idx: Option<usize> = None;
        egui::Grid::new("notice_edit").show(ui, |ui| {
            ui.label("内容：");
            ui.add(egui::TextEdit::singleline(&mut ed.content).desired_width(420.0));
            ui.end_row();
            ui.label("星期：");
            ui.horizontal(|ui| {
                for (i, name) in WEEKDAY_NAMES.iter().enumerate() {
                    ui.toggle_value(&mut ed.weekdays[i], *name);
                }
            });
            ui.end_row();
            ui.label("起止：");
            ui.add(egui::TextEdit::singleline(&mut ed.start).desired_width(70.0));
            ui.label("至");
            ui.add(egui::TextEdit::singleline(&mut ed.end).desired_width(70.0));
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if ui.button("✅ 确定").clicked() {
                committed = Some(ed.clone());
            }
            if ui.button("取消").clicked() {
                cancel = true;
            }
            if ui.button("🗑 删除此条").clicked() {
                del_idx = ed.index;
            }
        });
        if let Some(e) = committed {
            st.pending_notice = Some(e);
            *action = Some(Action::CommitNotice);
        }
        if cancel {
            st.notice_edit = None;
        }
        if let Some(i) = del_idx {
            st.notice_edit = None;
            *action = Some(Action::DeleteNotice(i));
        }
    }

    ui.horizontal(|ui| {
        if ui.button("＋ 添加").clicked() {
            st.notice_edit = Some(NoticeEdit {
                index: None,
                enabled: true,
                content: String::new(),
                weekdays: [false; 7],
                start: "14:00".into(),
                end: "21:40".into(),
            });
        }
        let sel = st.notice_selected.filter(|&i| i < st.notices.len());
        if ui
            .add_enabled(sel.is_some(), egui::Button::new("✎ 编辑"))
            .clicked()
        {
            if let Some(n) = st.notices.get(sel.unwrap()) {
                let mut weekdays = [false; 7];
                for w in n.weekday_nums() {
                    if (1..=7).contains(&w) {
                        weekdays[(w - 1) as usize] = true;
                    }
                }
                st.notice_edit = Some(NoticeEdit {
                    index: Some(sel.unwrap()),
                    enabled: n.enabled,
                    content: n.content.clone(),
                    weekdays,
                    start: n.start.clone(),
                    end: n.end.clone(),
                });
            }
        }
        if ui
            .add_enabled(sel.is_some(), egui::Button::new("🗑 删除"))
            .clicked()
        {
            *action = Some(Action::DeleteNotice(sel.unwrap()));
            st.notice_selected = None;
        }
        if ui
            .add_enabled(sel.is_some(), egui::Button::new("✓/✗ 启用切换"))
            .clicked()
        {
            if let Some(n) = st.notices.get_mut(sel.unwrap()) {
                n.enabled = !n.enabled;
            }
            *action = Some(Action::SaveNotices);
        }
        if ui.button("🔄 重新拉取").clicked() {
            *action = Some(Action::ReloadNotices);
        }
    });

    if !st.notices_loaded && !st.notices_requested {
        // update 帧末会触发拉取，这里只显示提示
        ui.weak("正在加载定时通知清单...");
    }

    // 表格：启用 | 星期 | 起止时间 | 内容（点任意格选中整行，工具行按钮对选中行生效）
    let mut sel_new: Option<usize> = None;
    ScrollArea::vertical().id_salt("notice_list").max_height(380.0).show(ui, |ui| {
        egui::Grid::new("notice_table").striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
            ui.label("启用");
            ui.label("星期");
            ui.label("起止时间");
            ui.label("内容");
            ui.end_row();
            for (i, n) in st.notices.iter().enumerate() {
                let wds = n
                    .weekday_nums()
                    .iter()
                    .filter_map(|w| WEEKDAY_NAMES.get((*w - 1) as usize).copied())
                    .collect::<Vec<&str>>()
                    .concat();
                let is_sel = st.notice_selected == Some(i);
                let content_disp: String = n.content.chars().take(80).collect();
                let mut clicked: Option<usize> = None;
                let mut cell = |txt: &str, is_sel: bool, ui: &mut egui::Ui| {
                    let r = ui.selectable_label(is_sel, txt);
                    if r.clicked() {
                        clicked = Some(i);
                    }
                };
                cell(if n.enabled { "✓" } else { "✘" }, is_sel, ui);
                cell(if wds.is_empty() { "—" } else { wds.as_str() }, is_sel, ui);
                cell(&format!("{} - {}", n.start, n.end), is_sel, ui);
                cell(&content_disp, is_sel, ui);
                if let Some(sel) = clicked {
                    sel_new = Some(sel);
                }
                ui.end_row();
            }
        });
    });
    if let Some(sel) = sel_new {
        st.notice_selected = Some(sel);
    }
    let _ = cfg;
    status_line(ui, &st.status);
    status_line(ui, &st.busy);
}

fn ui_web(ui: &mut egui::Ui, cfg: &mut Config, st: &mut UiState, action: &mut Option<Action>) {
    let edit_ok = st.unlocked;

    // 顶部：访问密码锁。令牌类配置默认收起，解锁是唯一查看/修改入口。
    ui.horizontal(|ui| {
        if st.unlocked {
            ui.colored_label(Color32::from_rgb(0x85, 0x64, 0x04), "✅ 已解锁（可编辑敏感配置）");
            if ui.button("🔒 上锁").clicked() {
                st.unlocked = false;
                st.token_show = false; // 上锁立即隐藏令牌，避免明文残留
            }
        } else {
            ui.label("访问密码：");
            let resp = ui.add(egui::TextEdit::singleline(&mut st.pwd_input).password(true).desired_width(160.0));
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("🔓 解锁").clicked() || enter {
                *action = Some(Action::Unlock);
            }
        }
        ui.label("新密码：");
        ui.add(egui::TextEdit::singleline(&mut st.new_pwd).password(true).desired_width(120.0));
        if ui.button("🔑 修改密码").clicked() {
            *action = Some(Action::ChangePwd);
        }
    });
    ui.separator();
    ui.weak("技术配置默认收起；令牌需解锁后才能查看或修改。");
    ui.add_space(2.0);

    // ---- 折叠：网络U盘 同步 ----
    egui::CollapsingHeader::new("📁 网络U盘 同步（Gitee）").show(ui, |ui| {
        ui.checkbox(&mut cfg.gitee_enabled, "启用 网络U盘 同步");
        ui.horizontal(|ui| {
            ui.label("仓库所有者：");
            ui.add_enabled(edit_ok, egui::TextEdit::singleline(&mut cfg.gitee_owner).desired_width(220.0));
        });
        ui.horizontal(|ui| {
            ui.label("仓库名称：");
            ui.add_enabled(edit_ok, egui::TextEdit::singleline(&mut cfg.gitee_repo).desired_width(220.0));
        });
        ui.horizontal(|ui| {
            ui.label("访问令牌：");
            ui.add_enabled(
                edit_ok,
                egui::TextEdit::singleline(&mut cfg.gitee_token)
                    .password(!st.token_show)
                    .desired_width(190.0),
            );
            if ui
                .add_enabled(
                    edit_ok,
                    egui::Button::new(if st.token_show { "隐藏" } else { "显示" }),
                )
                .clicked()
            {
                st.token_show = !st.token_show;
            }
        });
        ui.horizontal(|ui| {
            ui.label("下载目录：");
            ui.add_enabled(edit_ok, egui::TextEdit::singleline(&mut cfg.gitee_download_folder).desired_width(220.0));
        });
        ui.horizontal(|ui| {
            if ui.button("🧪 测试连接").clicked() {
                *action = Some(Action::TestConn);
            }
            if ui.button("🔄 手动同步").clicked() {
                *action = Some(Action::ManualSync);
            }
        });
    });

    // ---- 折叠：云端班级名单 ----
    egui::CollapsingHeader::new("☁ 云端班级名单（独立私有仓库）").show(ui, |ui| {
        ui.checkbox(
            &mut cfg.class_data_enabled,
            "创建班级时自动从私有仓库拉取本班名单与课表",
        );
        ui.horizontal(|ui| {
            ui.label("所有者：");
            ui.add(egui::TextEdit::singleline(&mut cfg.class_data_owner).desired_width(120.0));
            ui.label("仓库：");
            ui.add(egui::TextEdit::singleline(&mut cfg.class_data_repo).desired_width(160.0));
        });
        ui.horizontal(|ui| {
            ui.label("数据目录：");
            ui.add(egui::TextEdit::singleline(&mut cfg.class_data_dir).desired_width(90.0));
        });
        ui.horizontal(|ui| {
            ui.label("只读令牌：");
            ui.add_enabled(
                edit_ok,
                egui::TextEdit::singleline(&mut cfg.class_data_token)
                    .password(!st.token_show)
                    .desired_width(190.0),
            );
            if ui
                .add_enabled(
                    edit_ok,
                    egui::Button::new(if st.token_show { "隐藏" } else { "显示" }),
                )
                .clicked()
            {
                st.token_show = !st.token_show;
            }
        });
        if ui.button("☁ 测试并补全当前班").clicked() {
            *action = Some(Action::FetchCloud);
        }
    });

    // ---- 常驻：程序自动更新 ----
    ui.separator();
    ui.heading("程序自动更新");
    ui.checkbox(&mut cfg.update_enabled, "启用程序自动更新（启动后自动检查）");
    ui.horizontal(|ui| {
        ui.label("更新分支：");
        ui.add(egui::TextEdit::singleline(&mut cfg.update_branch).desired_width(140.0));
        ui.label("更新程序路径：");
        ui.add(egui::TextEdit::singleline(&mut cfg.update_exe_path).desired_width(180.0));
    });
    ui.horizontal(|ui| {
        if ui.button("⬆ 检查更新").clicked() {
            *action = Some(Action::CheckUpdates);
        }
        ui.weak(format!("当前版本 v{}", crate::theme::APP_VERSION));
    });

    ui.add_space(6.0);
    status_line(ui, &st.status);
    status_line(ui, &st.busy);
}

// ==================== eframe App ====================

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_bg();
        self.sync_grid();
        if self.st.rx.is_some() || self.st.cloud_rx.is_some() {
            ctx.request_repaint();
        }

        // Tab 栏：前 4 个常用页正常样式，末尾「高级」弱化，提示技术配置收在里面
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                for (i, name) in TAB_NAMES.iter().enumerate() {
                    let is_advanced = i == 4;
                    let label = egui::RichText::new(*name);
                    let selected = self.st.tab == i;
                    let btn = if is_advanced {
                        ui.selectable_label(selected, label.weak())
                    } else {
                        ui.selectable_label(selected, label)
                    };
                    if btn.clicked() {
                        self.st.tab = i;
                    }
                    if !is_advanced {
                        ui.separator();
                    } else {
                        ui.add_space(10.0);
                    }
                }
            });
        });

        // 中央面板：mem::take 状态，纯函数渲染，帧末写回
        let mut cfg = std::mem::take(&mut self.cfg);
        let mut st = std::mem::take(&mut self.st);
        let mut action: Option<Action> = None;

        egui::CentralPanel::default().show(ctx, |ui| {
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                match st.tab {
                    // 班级：班级选择 / 班级码 / 班级管理
                    0 => ui_basic(ui, &mut cfg, &mut st, &mut action, 0),
                    // 显示：缩放 / 位置 / 开机 / 作业栏
                    1 => ui_basic(ui, &mut cfg, &mut st, &mut action, 1),
                    // 教学数据：作息 + 学生名单 + 课表，三段连排
                    2 => {
                        ui.heading("教学数据修改锁");
                        ui_data_lock(ui, &mut st, &mut action);
                        ui.separator();
                        ui.heading("作息时间");
                        ui_time(ui, &mut cfg, &mut st, &mut action);
                        ui.separator();
                        ui.heading("学生名单");
                        ui_students(ui, &mut cfg, &mut st, &mut action);
                        ui.separator();
                        ui.heading("课表");
                        ui_timetable(ui, &mut cfg, &mut st, &mut action);
                    }
                    // 通知：定时通知清单
                    3 => ui_notices(ui, &cfg, &mut st, &mut action),
                    // 高级：Gitee / GoEasy / 自动更新（技术配置）
                    _ => ui_web(ui, &mut cfg, &mut st, &mut action),
                }
            });
        });

        // 「彻底关闭桌面作业栏」二次确认：必须在 CentralPanel 之后画，
        // Modal 的半透明遮罩才能盖住整个设置窗口并拦住其它输入。
        if st.hw_confirm_open {
            let mut decision: Option<bool> = None;
            let modal = egui::Modal::new(egui::Id::new("hw_hard_close_confirm")).show(ctx, |ui| {
                ui.set_width(340.0);
                ui.heading("确认彻底关闭桌面作业栏？");
                ui.separator();
                ui.label("关闭后重启程序、重启大屏都不会自动打开，只能回到设置窗口重新开启。");
                ui.add_space(6.0);
                ui.label("请输入确认码 123 后点「确定」：");
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut st.hw_confirm_code)
                        .password(true)
                        .desired_width(120.0),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let enter = edit.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("确定").clicked() || enter {
                        decision = Some(true);
                    }
                    if ui.button("取消").clicked() {
                        decision = Some(false);
                    }
                });
            });
            if modal.should_close() {
                decision = Some(false);
            }
            match decision {
                Some(true) => {
                    if hw_close_allowed(&st.hw_confirm_code) {
                        desktop_board::board::patch_keys(&[
                            ("homework_hard_hidden", serde_json::json!(true)),
                            ("homework_hidden", serde_json::json!(true)),
                        ]);
                        st.hw_confirm_open = false;
                        st.hw_confirm_code.clear();
                        st.status = Some(
                            "已彻底关闭桌面作业栏：重启也不会自动打开，只能在此处重新开启".into(),
                        );
                    } else {
                        st.hw_confirm_code.clear();
                        st.status = Some("确认码不正确，作业栏没有被关闭".into());
                    }
                }
                Some(false) => {
                    st.hw_confirm_open = false;
                    st.hw_confirm_code.clear();
                }
                None => {}
            }
        }

        // 定时通知页懒加载（首次切入时向 Gitee 拉取）；通知页编号为 3
        let need_notices = st.tab == 3 && !st.notices_loaded && !st.notices_requested;
        if need_notices {
            // 先用本地内置清单立即显示（Gitee 拉取成功后覆盖；失败时列表不空白）
            if st.notices.is_empty() && !st.notices_prefilled {
                st.notices = cfg.scheduled_notices.clone();
            }
            st.notices_prefilled = true;
            st.notices_requested = true;
        }

        self.cfg = cfg;
        self.st = st;

        if need_notices {
            self.load_notices_async();
        }
        if let Some(a) = action {
            self.apply(a);
        }

        // 控件改动自动生效（无需点「保存」）：300ms 防抖避免拖滑杆时每帧写盘，
        // 主进程 250ms 内热加载 → 从改动到界面生效约 0.3 秒。
        let snapshot = serde_json::to_string(&self.cfg).unwrap_or_default();
        if snapshot != self.last_saved_json {
            if self.dirty_since.is_none() {
                self.dirty_since = Some(std::time::Instant::now());
            }
            let ready = self
                .dirty_since
                .map(|t| t.elapsed().as_secs_f32() >= 0.3)
                .unwrap_or(false);
            if ready {
                self.write_config("设置已自动生效");
            } else {
                ctx.request_repaint();
            }
        } else {
            self.dirty_since = None;
        }

        // 课表网格：编辑后 500ms 防抖自动写盘（按 sched_loaded_for，切班也不串数据）
        if let Some(t) = self.st.sched_dirty_since {
            if t.elapsed().as_secs_f32() >= 0.5 {
                self.autosave_timetable();
                self.st.sched_dirty_since = None;
            } else {
                ctx.request_repaint();
            }
        }
        // 作息表：编辑后 500ms 防抖自动写盘
        if let Some(t) = self.st.time_dirty_since {
            if t.elapsed().as_secs_f32() >= 0.5 {
                self.autosave_timetable_time();
                self.st.time_dirty_since = None;
            } else {
                ctx.request_repaint();
            }
        }
    }
}

// ==================== 历史窗口 ====================

struct HistoryApp {
    history: crate::history::HistoryManager,
    confirm_clear: bool,
}

impl HistoryApp {
    fn new() -> Self {
        HistoryApp { history: crate::history::HistoryManager::new(), confirm_clear: false }
    }
}

impl eframe::App for HistoryApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("点名历史记录");
            ui.separator();
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let items = self.history.get_all();
                if items.is_empty() {
                    ui.weak("（暂无记录）");
                }
                for item in items.iter().rev() {
                    let name = item.get("name").and_then(|x| x.as_str()).unwrap_or("");
                    let class = item.get("class").and_then(|x| x.as_str()).unwrap_or("");
                    let time = item.get("time").and_then(|x| x.as_str()).unwrap_or("");
                    ui.label(format!(" {}   班级: {}   时间: {}", name, class, time));
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if self.confirm_clear {
                    ui.colored_label(Color32::from_rgb(0xC0, 0x39, 0x2B), "确定要清空所有点名记录吗？");
                    if ui.button("确定清空").clicked() {
                        self.history.clear();
                        self.history.flush();
                        self.confirm_clear = false;
                    }
                    if ui.button("取消").clicked() {
                        self.confirm_clear = false;
                    }
                } else if ui.button("🗑 清空记录").clicked() {
                    self.confirm_clear = true;
                }
            });
        });
    }
}

// ==================== 工具 ====================

fn is_hhmm(s: &str) -> bool {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() != 2 {
        return false;
    }
    match (parts[0].trim().parse::<u32>(), parts[1].trim().parse::<u32>()) {
        (Ok(h), Ok(m)) => h < 24 && m < 60,
        _ => false,
    }
}

/// 从上一节结束时间推下一节：+10 分钟开始，+40 分钟结束。
fn next_slot(last_end: &str) -> (String, String) {
    let parts: Vec<&str> = last_end.split(':').collect();
    let (h, m) = match (
        parts.first().and_then(|x| x.parse::<i64>().ok()),
        parts.get(1).and_then(|x| x.parse::<i64>().ok()),
    ) {
        (Some(h), Some(m)) => (h, m),
        _ => (8, 0),
    };
    let start_min = h * 60 + m + 10;
    let end_min = start_min + 40;
    (fmt_hm(start_min), fmt_hm(end_min))
}

fn fmt_hm(total_min: i64) -> String {
    let h = (total_min / 60) % 24;
    let m = total_min % 60;
    format!("{:02}:{:02}", h, m)
}

/// 文件名清洗：去掉 Windows 非法字符。
fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .filter(|c| !matches!(c, '\\'|'/'|':'|'*'|'?'|'"'|'<'|'>'|'|'))
        .collect();
    if cleaned.is_empty() {
        "班级".into()
    } else {
        cleaned
    }
}

/// 导出目录：优先桌面「点名系统导出」；桌面不可用则回退数据目录下的「导出」。
fn export_dir() -> Result<std::path::PathBuf, String> {
    if let Ok(home) = std::env::var("USERPROFILE") {
        let desktop = std::path::Path::new(&home).join("Desktop");
        if desktop.is_dir() {
            let d = desktop.join("点名系统导出");
            std::fs::create_dir_all(&d).map_err(|e| format!("创建导出文件夹失败：{e}"))?;
            return Ok(d);
        }
    }
    let d = crate::paths::data_dir().join("导出");
    std::fs::create_dir_all(&d).map_err(|e| format!("创建导出文件夹失败：{e}"))?;
    Ok(d)
}

/// 写 UTF-8 BOM 的 CSV（Excel/WPS 打开中文不乱码）；同名文件按时间戳避开占用。
fn export_csv(base: &str, kind: &str, rows: &[Vec<String>]) -> Result<String, String> {
    let dir = export_dir()?;
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M");
    let path = dir.join(format!("{base}_{kind}_{stamp}.csv"));
    let mut out = String::from("\u{FEFF}");
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .map(|c| {
                if c.contains(',') || c.contains('"') || c.contains('\n') {
                    format!("\"{}\"", c.replace('"', "\"\""))
                } else {
                    c.clone()
                }
            })
            .collect();
        out.push_str(&cells.join(","));
        out.push_str("\r\n");
    }
    std::fs::write(&path, out.as_bytes()).map_err(|e| format!("写文件失败：{e}"))?;
    Ok(path.display().to_string())
}

#[cfg(test)]
mod paste_tests {
    use super::{grid_to_map, parse_schedule_paste, parse_student_paste, ClassEntry};

    #[test]
    fn seed_import_file_names_match_class_entry() {
        // scripts/build_seed.py 用 f"{school}_{year}级{NN:02d}班.txt" 命名，
        // 必须与 Rust 侧一致，否则脚本导入的课表 / 班级程序读不到。
        assert_eq!(
            ClassEntry::build_file_name("赣州中学", "2025", "9"),
            "赣州中学_2025级09班.txt"
        );
        assert_eq!(
            ClassEntry::build_file_name("赣州中学", "2025", "31"),
            "赣州中学_2025级31班.txt"
        );
        assert_eq!(
            ClassEntry::build_display_name("赣州中学", "2025", "9"),
            "赣州中学2025级9班"
        );
    }

    #[test]
    fn students_one_per_line() {
        assert_eq!(
            parse_student_paste("张三\n李四\n王五\n"),
            vec!["张三", "李四", "王五"]
        );
    }

    #[test]
    fn students_excel_with_header_and_index_column() {
        let s = "序号\t姓名\t性别\n1\t张三\t男\n2\t李四\t女\n3\t王五\t男\n";
        assert_eq!(
            parse_student_paste(s),
            vec!["张三", "李四", "王五"]
        );
        // 学号列是纯数字，同样要跳过
        assert_eq!(
            parse_student_paste("20250101\t张三\n20250102\t李四"),
            vec!["张三", "李四"]
        );
    }

    #[test]
    fn students_mixed_separators_and_dedup() {
        assert_eq!(
            parse_student_paste("张三，李四、王五 赵六; 孙七"),
            vec!["张三", "李四", "王五", "赵六", "孙七"]
        );
        assert_eq!(parse_student_paste("张三\n张三\n李四"), vec!["张三", "李四"]);
        assert_eq!(parse_student_paste("\"张三\"\n'李四'"), vec!["张三", "李四"]);
        assert!(parse_student_paste("").is_empty());
        assert!(parse_student_paste("序号\t姓名\n1\t2").is_empty());
    }

    #[test]
    fn schedule_rows_are_periods_with_header_and_label_column() {
        let s = "节次\t星期一\t星期二\t星期三\t星期四\t星期五\n\
                 第1节\t语文\t数学\t英语\t物理\t化学\n\
                 第2节\t数学\t语文\t生物\t化学\t物理";
        let (g, n) = parse_schedule_paste(s, 9).unwrap();
        assert_eq!(n, 10);
        assert_eq!(g[0][0], "语文");
        assert_eq!(g[0][1], "数学");
        assert_eq!(g[0][4], "化学");
        assert_eq!(g[1][0], "数学");
        assert_eq!(g[1][3], "化学");
    }

    #[test]
    fn schedule_rows_are_weekdays() {
        let s = "星期\t第1节\t第2节\n\
                 星期一\t语文\t数学\n\
                 星期二\t英语\t物理\n\
                 星期三\t化学\t生物";
        let (g, n) = parse_schedule_paste(s, 9).unwrap();
        assert_eq!(n, 6);
        assert_eq!(g[0][0], "语文"); // 周一第1节
        assert_eq!(g[1][0], "数学"); // 周一第2节
        assert_eq!(g[0][1], "英语"); // 周二第1节
        assert_eq!(g[1][2], "生物"); // 周三第2节
    }

    #[test]
    fn schedule_plain_five_columns_keeps_first_column() {
        // 旧版无条件丢掉每行第一列，纯数据粘贴会整体左移一格
        let s = "语文\t数学\t英语\t物理\t化学\n数学\t语文\t生物\t化学\t物理";
        let (g, n) = parse_schedule_paste(s, 9).unwrap();
        assert_eq!(n, 10);
        assert_eq!(g[0][0], "语文");
        assert_eq!(g[0][4], "化学");
        assert_eq!(g[1][0], "数学");
    }

    #[test]
    fn schedule_comma_and_placeholders() {
        let (g, n) = parse_schedule_paste("第1节,语文,-,英语,,物理", 9).unwrap();
        assert_eq!(n, 3);
        assert_eq!(g[0][0], "语文");
        assert_eq!(g[0][1], "");
        assert_eq!(g[0][2], "英语");
        assert_eq!(g[0][4], "物理");
    }

    #[test]
    fn schedule_rejects_empty_or_header_only() {
        assert!(parse_schedule_paste("", 9).is_err());
        assert!(parse_schedule_paste("   \n\n", 9).is_err());
        assert!(parse_schedule_paste("节次\t星期一\t星期二", 9).is_err());
        assert!(parse_schedule_paste("-\t-\n无\t/", 9).is_err());
    }

    #[test]
    fn schedule_weekday_rows_without_header_are_not_dropped() {
        let s = "星期一\t语文\t数学\n星期二\t英语\t物理";
        let (g, n) = parse_schedule_paste(s, 9).unwrap();
        assert_eq!(n, 4);
        assert_eq!(g[0][0], "语文");
        assert_eq!(g[0][1], "英语");
    }

    #[test]
    fn imported_grid_maps_to_correct_weekday_period_coordinates() {
        // 模拟「粘贴导入 → 写盘」的中间结果，坐标必须与 load_schedule 一致
        let s = "节次\t星期一\t星期二\n第1节\t语文\t数学\n第2节\t英语\t物理";
        let (g, _) = parse_schedule_paste(s, 9).unwrap();
        let m = grid_to_map(&g);
        assert_eq!(m.get(&(0, 1)).map(String::as_str), Some("语文")); // 周一第1节
        assert_eq!(m.get(&(1, 1)).map(String::as_str), Some("数学")); // 周二第1节
        assert_eq!(m.get(&(0, 2)).map(String::as_str), Some("英语")); // 周一第2节
        assert_eq!(m.get(&(1, 2)).map(String::as_str), Some("物理")); // 周二第2节
        assert_eq!(m.len(), 4);
    }
}

#[cfg(test)]
mod class_limit_tests {
    use super::{can_create_class, CLASS_UNLOCK_PASSWORD, MAX_CLASSES_FREE};

    // 两个测试都会改进程级 USERPROFILE 环境变量做数据隔离，必须串行，
    // 否则并行时后设置的目录会覆盖前一个测试的配置读写路径。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn one_class_free_more_requires_unlock() {
        assert_eq!(MAX_CLASSES_FREE, 1);
        assert!(can_create_class(0, false));
        assert!(!can_create_class(1, false));
        assert!(!can_create_class(9, false));
        assert!(can_create_class(1, true));
        assert!(can_create_class(9, true));
    }

    #[test]
    fn unlock_password_is_fixed() {
        assert_eq!(CLASS_UNLOCK_PASSWORD, "123321");
    }

    /// 全流程（隔离 USERPROFILE 到临时目录，不碰真实数据）：
    /// 满 1 班未解锁 → 超额创建被拒 → 错误密码不解锁 → 正确密码（带空格 trim）解锁
    /// → 创建第 2 班自动切换并落盘 → 删除测试班自动切回首班并落盘。
    #[test]
    fn create_class_full_flow_limit_unlock_create_delete() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use super::{grade_to_year, Action, SettingsApp};
        use crate::config::ClassEntry;
        // 隔离数据目录：paths::data_dir() = USERPROFILE/.RollCallData，实时读环境变量
        let tmp = std::env::temp_dir().join(format!("rollcall_test_flow_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("USERPROFILE", &tmp);

        let mut app = SettingsApp::new();
        app.cfg.class_data_enabled = false; // 关掉云端拉取，避免测试期间发起 Gitee 请求
        let year = grade_to_year("高二"); // 跟随当前学年换算，不写死年份

        // 预置一个班（已用满免费额度）
        let num = "9";
        app.cfg.class_list.push(ClassEntry {
            school: "赣州中学".into(),
            year: year.clone(),
            class_num: num.into(),
            display_name: ClassEntry::build_display_name("赣州中学", &year, num),
            file_name: ClassEntry::build_file_name("赣州中学", &year, num),
            students: Vec::new(),
            extra: Default::default(),
        });
        app.cfg.current_class = app.cfg.class_list[0].file_name.clone();

        // ① 满 1 班未解锁：创建被拒
        app.apply(Action::CreateClass);
        assert_eq!(app.cfg.class_list.len(), 1);
        assert!(
            app.st.status.as_deref().unwrap().contains("最多 1 个班级"),
            "实际提示：{:?}",
            app.st.status
        );

        // ② 错误密码不解锁
        app.st.class_pwd = "wrong".into();
        app.apply(Action::UnlockClasses);
        assert!(!app.st.class_unlocked);
        assert!(app.st.status.as_deref().unwrap().contains("密码不正确"));

        // ③ 正确密码解锁（带尾随空格，顺带验证 trim；本次设置会话内有效）
        app.st.class_pwd = format!("{CLASS_UNLOCK_PASSWORD} ");
        app.apply(Action::UnlockClasses);
        assert!(app.st.class_unlocked, "正确密码应解锁");

        // ④ 创建第 2 个班：自动切换 + 配置落盘
        app.st.new_school = "赣州中学".into();
        app.st.new_grade = "高二".into();
        app.st.new_class_num = "17".into();
        app.apply(Action::CreateClass);
        assert_eq!(app.cfg.class_list.len(), 2);
        let expect_file = ClassEntry::build_file_name("赣州中学", &year, "17");
        assert_eq!(app.cfg.current_class, expect_file);
        assert!(
            app.st.status.as_deref().unwrap().contains("已创建并切换"),
            "实际提示：{:?}",
            app.st.status
        );
        let saved: crate::config::Config = serde_json::from_str(
            &std::fs::read_to_string(crate::paths::config_file()).unwrap(),
        )
        .unwrap();
        assert_eq!(saved.class_list.len(), 2, "配置应落盘 2 个班");
        assert_eq!(saved.current_class, expect_file);

        // ⑤ 删除测试班：自动切回首班并落盘
        app.apply(Action::DeleteClass(1));
        assert_eq!(app.cfg.class_list.len(), 1);
        assert_eq!(app.cfg.current_class, app.cfg.class_list[0].file_name);
        let saved: crate::config::Config = serde_json::from_str(
            &std::fs::read_to_string(crate::paths::config_file()).unwrap(),
        )
        .unwrap();
        assert_eq!(saved.class_list.len(), 1, "删除后配置应落盘 1 个班");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn hard_close_requires_exact_code() {
        assert_eq!(super::HARD_CLOSE_CODE, "123");
        assert!(super::hw_close_allowed("123"));
        assert!(super::hw_close_allowed(" 123 "));
        assert!(!super::hw_close_allowed(""));
        assert!(!super::hw_close_allowed("321"));
        assert!(!super::hw_close_allowed("1234"));
    }

    #[test]
    fn teaching_data_lock_requires_123_and_blocks_all_writes() {
        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use super::{data_unlock_ok, teaching_data_edit_allowed, Action, SettingsApp, DATA_UNLOCK_PASSWORD};
        use crate::config::ClassEntry;
        assert_eq!(DATA_UNLOCK_PASSWORD, "123");
        assert!(data_unlock_ok("123"));
        assert!(data_unlock_ok(" 123 "));
        assert!(!data_unlock_ok(""));
        assert!(!data_unlock_ok("1234"));
        assert!(!data_unlock_ok("321"));
        assert!(!teaching_data_edit_allowed(false));
        assert!(teaching_data_edit_allowed(true));

        // 隔离数据目录，避免测试改动真实名单 / 课表
        let tmp = std::env::temp_dir().join(format!("rollcall_test_datalock_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("USERPROFILE", &tmp);

        let mut app = SettingsApp::new();
        app.cfg.class_data_enabled = false;
        app.cfg.class_list.push(ClassEntry {
            file_name: "lock-test.json".into(),
            display_name: "锁定测试班".into(),
            students: vec!["张三".into()],
            ..Default::default()
        });
        app.cfg.current_class = "lock-test.json".into();
        app.st.sched_class_idx = 0;
        app.st.stu_class_idx = 0;

        // 未解锁：导入名单 / 清空名单 / 导入课表 / 清除课表 全部被拒
        app.st.stu_paste = "李四\n王五".into();
        app.apply(Action::ImportStudents);
        assert_eq!(app.cfg.class_list[0].students, vec!["张三"], "锁定态不应导入名单");
        assert!(app.st.status.as_deref().unwrap().contains("锁定"));

        app.apply(Action::ClearStudents);
        assert_eq!(app.cfg.class_list[0].students.len(), 1, "锁定态不应清空名单");

        app.st.sched_paste = "节次\t星期一\n第1节\t语文".into();
        app.apply(Action::ImportSchedule);
        assert!(app.st.status.as_deref().unwrap().contains("锁定"), "锁定态导入课表应被拒");

        app.apply(Action::ClearTimetable);
        assert!(app.st.status.as_deref().unwrap().contains("锁定"), "锁定态清除课表应被拒");

        // 错误口令不解锁，写入仍被拒
        app.st.data_pwd = "000".into();
        app.apply(Action::UnlockData);
        assert!(!app.st.data_unlocked);
        assert!(app.st.status.as_deref().unwrap().contains("口令不正确"));

        // 正确口令 123 解锁（带空格），写入生效
        app.st.data_pwd = format!("{DATA_UNLOCK_PASSWORD} ");
        app.apply(Action::UnlockData);
        assert!(app.st.data_unlocked, "正确口令应解锁");

        app.apply(Action::ClearStudents);
        assert!(app.cfg.class_list[0].students.is_empty(), "解锁后应能清空名单");

        app.st.stu_paste = "李四\n王五".into();
        app.apply(Action::ImportStudents);
        assert_eq!(app.cfg.class_list[0].students, vec!["李四", "王五"]);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn clear_all_classes_returns_to_zero_and_is_idempotent() {
        let mut c = super::Config::default();
        for f in ["a.txt", "b.txt", "c.txt"] {
            c.class_list.push(super::ClassEntry {
                file_name: f.into(),
                ..Default::default()
            });
        }
        c.current_class = "a.txt".into();
        assert_eq!(super::clear_all_classes(&mut c), 3);
        assert!(c.class_list.is_empty());
        assert!(c.current_class.is_empty());
        assert!(c.current().is_none());
        assert_eq!(super::clear_all_classes(&mut c), 0);
    }

    #[test]
    fn grade_year_offsets_are_stable() {
        // 不写死年份（跨年也要过），只校验三届相差 1 年、高二在高一之前。
        let a = super::grade_to_year("高一").parse::<i32>().unwrap();
        let b = super::grade_to_year("高二").parse::<i32>().unwrap();
        let c = super::grade_to_year("高三").parse::<i32>().unwrap();
        assert_eq!(a - b, 1);
        assert_eq!(b - c, 1);
    }
}
