//! 后台服务线程：Gitee 同步（网络U盘）、心跳上报、内置定时通知。
//!
//! 全部用 std::thread + AtomicBool 停止位；配置通过 Arc<Mutex<Config>> 每轮实时读取，
//! 与 Python 版「config_getter 实时取配置」的行为一致。

use crate::config::{Config, ScheduledNotice};
use crate::gitee::{GiteeApi, RemoteFile};
use crate::history::HistoryManager;
use crate::logging;
use crate::paths;
use crate::win;
use chrono::{Datelike, Local};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// GoEasy files 信号置位：让后台同步线程跳过剩余等待，立即跑下一轮。
/// 消息丢失不影响：原 5 分钟轮询照常兜底。
static SYNC_WAKE: AtomicBool = AtomicBool::new(false);
pub fn request_sync_now() {
    SYNC_WAKE.store(true, Ordering::Relaxed);
}

/// 后台线程共享的应用状态钩子：实时取最新配置。
pub type ConfigGetter = Arc<Mutex<Config>>;

pub const SCHEDULED_NOTICES_PATH: &str = "scheduled_notices.json";

fn fmt_size(mut size: f64) -> String {
    for unit in ["B", "KB", "MB", "GB"] {
        if size < 1024.0 {
            return format!("{:.1}{}", size, unit);
        }
        size /= 1024.0;
    }
    format!("{:.1}TB", size)
}

fn now_str() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

// ====================== 心跳上报 ======================

pub struct HeartbeatService {
    stop: Arc<AtomicBool>,
}

impl HeartbeatService {
    pub fn new() -> Self {
        HeartbeatService {
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(self: &Arc<Self>, cfg: ConfigGetter) {
        if std::env::var("ROLL_DISABLE_HB").is_ok() {
            return; // 诊断开关：跳过心跳线程
        }
        let stop = self.stop.clone();
        std::thread::Builder::new()
            .name("heartbeat".into())
            .spawn(move || {
                upload_once(&cfg);
                while !stop.load(Ordering::Relaxed) {
                    // GoEasy 桥接健康时降到 900s：notify 已走 presence 实时判定在线状态，
                    // Gitee 心跳文件仅作兜底，降频可释放绝大部分 Gitee 配额
                    //（50 台规模从 ~250 次/天/台 降到 ~30 次）。桥接断了恢复 180s 兜底节奏。
                    let interval =
                        heartbeat_base_secs(crate::goeasy::doorbell_healthy()) + rand_below(31);
                    for _ in 0..interval {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    upload_once(&cfg);
                }
            })
            .ok();
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// 心跳基础间隔（秒）：GoEasy 桥接健康时 900s（notify 走 presence 实时判定，
/// Gitee 心跳只是兜底）；桥接断开时 180s（旧节奏，保证无 presence 时圆点仍可用）。
fn heartbeat_base_secs(bridge_healthy: bool) -> u64 {
    if bridge_healthy {
        900
    } else {
        180
    }
}

/// 单次心跳：写 tongzhi/{班级去.txt}_heartbeat.txt = 当前时间戳。
fn upload_once(cfg: &ConfigGetter) {
    let cfg = cfg.lock().unwrap();
    let Some(cls) = cfg.current() else { return };
    let filename = format!(
        "{}/{}_heartbeat.txt",
        cfg.gitee_tongzhi_dir,
        cls.file_name.trim_end_matches(".txt")
    );
    let timestamp = now_str();
    let api = GiteeApi::new(&cfg.gitee_owner, &cfg.gitee_repo, &cfg.gitee_token);
    drop(cfg);
    match api.create_text_file(&timestamp, &filename, "master") {
        Ok(_) => logging::info(&format!("心跳上报成功: {}", timestamp)),
        Err(e) => logging::warn(&format!("心跳上报失败: {}", e)), // 静默
    }
}

/// 简易可复现随机（线程内用，非安全随机）。
fn rand_below(n: u64) -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    if n == 0 {
        return 0;
    }
    (t ^ (t >> 17) ^ (std::process::id() as u64)) % n
}

// ====================== 网络U盘 同步 ======================

pub struct GiteeSyncManager {
    stop: Arc<AtomicBool>,
    /// 新文件落盘回调（主线程弹提醒条用）。
    pub on_new_files: Arc<Mutex<Box<dyn Fn(Vec<String>) + Send>>>,
}

impl GiteeSyncManager {
    pub fn new() -> Self {
        GiteeSyncManager {
            stop: Arc::new(AtomicBool::new(false)),
            on_new_files: Arc::new(Mutex::new(Box::new(|_| {}))),
        }
    }

    pub fn start(self: &Arc<Self>, cfg: ConfigGetter) {
        let stop = self.stop.clone();
        let on_new = self.on_new_files.clone();
        std::thread::Builder::new()
            .name("gitee-sync".into())
            .spawn(move || sync_loop(cfg, stop, on_new))
            .ok();
        logging::info("Gitee 同步线程已启动");
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn sync_loop(cfg: ConfigGetter, stop: Arc<AtomicBool>, on_new: Arc<Mutex<Box<dyn Fn(Vec<String>) + Send>>>) {
    let (owner, repo, token, random_max, download_folder) = {
        let c = cfg.lock().unwrap();
        if !c.gitee_enabled {
            logging::info("Gitee 同步未启用（配置关闭）");
            return;
        }
        (
            c.gitee_owner.clone(),
            c.gitee_repo.clone(),
            c.gitee_token.clone(),
            c.gitee_random_max.max(0) as u64,
            c.gitee_download_folder.clone(),
        )
    };
    let mut download_folder = download_folder;
    if std::fs::create_dir_all(&download_folder).is_err() {
        logging::error(&format!("无法创建下载目录 {}；回退到用户目录", download_folder));
        download_folder = paths::home().join("wulikejian").to_string_lossy().to_string();
        if std::fs::create_dir_all(&download_folder).is_err() {
            logging::error("回退目录仍无法创建，本次同步跳过");
            return;
        }
    }
    let api = GiteeApi::new(&owner, &repo, &token);
    let (ok, info) = api.test_connection(false, None);
    if !ok {
        logging::error(&format!("Gitee 连接失败: {}", info));
        return;
    }
    logging::info(&format!("Gitee 同步已连接: {}", info));

    let mut downloaded: Map<String, Value> = crate::config::load_json(&paths::sync_record_file(), Map::new());
    let mut consecutive_errors = 0u32;

    // 启动立即同步一次
    match sync_once(&api, &download_folder, &mut downloaded, &on_new) {
        Ok(n) if n > 0 => logging::info(&format!("首次同步完成，新增 {} 个文件", n)),
        Ok(_) => logging::info("首次同步完成，无新文件"),
        Err(e) => logging::error(&format!("首次同步异常: {}", e)),
    }

    while !stop.load(Ordering::Relaxed) {
        match sync_once(&api, &download_folder, &mut downloaded, &on_new) {
            Ok(n) if n > 0 => logging::info(&format!("Gitee 同步完成，新增 {} 个文件", n)),
            Ok(_) => logging::info("✓ Gitee 同步检查完成，无新文件"),
            Err(e) => {
                consecutive_errors += 1;
                logging::error(&format!("Gitee 同步异常 ({}/10): {}", consecutive_errors, e));
                if consecutive_errors >= 10 {
                    logging::error("连续错误过多，暂停同步 5 分钟");
                    sleep_interruptible(&stop, 300);
                    consecutive_errors = 0;
                    continue;
                }
            }
        }
        let wait = 300 + rand_below(random_max + 1);
        sleep_interruptible(&stop, wait);
    }
}

fn sleep_interruptible(stop: &AtomicBool, secs: u64) {
    for _ in 0..secs {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        // GoEasy files 信号：立刻结束等待，下一轮会下载新文件
        if SYNC_WAKE.swap(false, Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// 一轮同步：处理 uploads 目录的分块（.parts/manifest.json）与普通文件。
fn sync_once(
    api: &GiteeApi,
    download_folder: &str,
    downloaded: &mut Map<String, Value>,
    on_new: &Arc<Mutex<Box<dyn Fn(Vec<String>) + Send>>>,
) -> Result<usize, String> {
    let remote_files = api.list_files("uploads");
    let mut manifests: Vec<(String, &RemoteFile)> = Vec::new();
    let mut regular: Vec<&RemoteFile> = Vec::new();
    for f in &remote_files {
        if f.name == "manifest.json" && f.path.contains(".parts/") {
            let dir = f.path.rsplit_once("/manifest.json").map(|x| x.0.to_string()).unwrap_or_default();
            manifests.push((dir, f));
        } else {
            regular.push(f);
        }
    }
    // 新格式分块组：<文件>.manifest.json + <文件>.chunks/NNNNN.bin（上传脚本 upload_udisk.py 生成）。
    // 顶层条目已被 list_files 合并为原文件名，走 regular→download_file_content 的 manifest 探测；
    // 这里处理的是子目录里未被合并、原样列出的 .manifest.json 条目。
    let mut new_count = 0usize;
    let mut new_files: Vec<String> = Vec::new();
    for f in remote_files.iter().filter(|f| f.name.ends_with(".manifest.json")) {
        let key = f.path.clone();
        if downloaded.get(&key).and_then(|v| v.get("sha")).and_then(|s| s.as_str()) == Some(f.sha.as_str()) {
            continue;
        }
        let m: Value = match api
            .download_file_content(&f.path)
            .and_then(|raw| serde_json::from_slice(&raw).map_err(|e| e.to_string()))
        {
            Ok(m) => m,
            Err(e) => {
                logging::error(&format!("分块 manifest 解析失败 {}: {}", f.path, e));
                continue;
            }
        };
        let Some(target) = f.path.strip_suffix(".manifest.json").map(|s| s.to_string()) else {
            continue;
        };
        match api.download_chunked(&target, &m) {
            Ok(merged) => {
                let rel = target.strip_prefix("uploads/").unwrap_or(&target);
                let local_path = std::path::PathBuf::from(download_folder).join(rel);
                if let Some(p) = local_path.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                if std::fs::write(&local_path, &merged).is_ok() {
                    logging::info(&format!(
                        "📦 分块文件: {} ({}) → {}",
                        target,
                        fmt_size(merged.len() as f64),
                        local_path.display()
                    ));
                    downloaded.insert(
                        key,
                        json!({
                            "sha": f.sha,
                            "size": merged.len(),
                            "downloaded_at": now_str(),
                            "local_path": local_path.to_string_lossy(),
                            "chunked": true,
                        }),
                    );
                    new_count += 1;
                    let fname = target.rsplit('/').next().unwrap_or(&target).to_string();
                    new_files.push(fname);
                }
            }
            Err(e) => logging::error(&format!("分块下载失败: {}", e)),
        }
    }

    // 分块文件
    for (parts_dir, mf) in &manifests {
        let key = mf.path.clone();
        if downloaded.get(&key).and_then(|v| v.get("sha")).and_then(|s| s.as_str()) == Some(mf.sha.as_str()) {
            continue;
        }
        match download_chunk_parts(api, &mf.path) {
            Ok((filename, total_size, merged)) => {
                // 保存目录：uploads/<日期目录>/
                let segs: Vec<&str> = mf.path.split('/').collect();
                let date_dir = if segs.len() >= 2 { segs[1] } else { "" };
                let save_dir = if date_dir.is_empty() {
                    std::path::PathBuf::from(download_folder)
                } else {
                    std::path::PathBuf::from(download_folder).join(date_dir)
                };
                let _ = std::fs::create_dir_all(&save_dir);
                let local_path = save_dir.join(&filename);
                if std::fs::write(&local_path, &merged).is_ok() {
                    logging::info(&format!(
                        "📦 分块文件: {} ({}, {} 分块) 📥 合并下载 → {}",
                        filename,
                        fmt_size(total_size as f64),
                        segs.len(),
                        local_path.display()
                    ));
                    downloaded.insert(
                        key,
                        json!({
                            "sha": mf.sha,
                            "size": total_size,
                            "downloaded_at": now_str(),
                            "local_path": local_path.to_string_lossy(),
                            "chunked": true,
                        }),
                    );
                    new_count += 1;
                    new_files.push(filename);
                }
            }
            Err(e) => logging::error(&format!("分块下载失败: {}", e)),
        }
        let _ = parts_dir;
    }

    // 普通文件（跳过分块碎片与 manifest，避免把碎片下载成本地垃圾）
    for f in regular {
        if f.path.contains(".parts/")
            || f.path.contains(".chunks/")
            || f.name.ends_with(".manifest.json")
        {
            continue;
        }
        if downloaded.get(&f.path).and_then(|v| v.get("sha")).and_then(|s| s.as_str()) == Some(f.sha.as_str()) {
            continue;
        }
        match api.download_file_content(&f.path) {
            Ok(content) => {
                let rel = f.path.strip_prefix("uploads/").unwrap_or(&f.path);
                let local_path = std::path::PathBuf::from(download_folder).join(rel);
                if let Some(p) = local_path.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                if std::fs::write(&local_path, &content).is_ok() {
                    logging::info(&format!(
                        "📥 下载: {} ({}) → {}",
                        f.name,
                        fmt_size(f.size as f64),
                        local_path.display()
                    ));
                    downloaded.insert(
                        f.path.clone(),
                        json!({
                            "sha": f.sha,
                            "size": f.size,
                            "downloaded_at": now_str(),
                            "local_path": local_path.to_string_lossy(),
                        }),
                    );
                    new_count += 1;
                    new_files.push(f.name.clone());
                }
            }
            Err(e) => logging::error(&format!("下载失败 {}: {}", f.name, e)),
        }
    }

    if new_count > 0 {
        crate::config::save_json_atomic(&paths::sync_record_file(), downloaded);
        if let Ok(cb) = on_new.lock() {
            cb(new_files);
        }
    }
    Ok(new_count)
}

/// 设置页「手动同步」：全量覆盖下载一轮（不做 sha 去重），返回覆盖文件数。
/// 对应 Python manual_sync（sync_manager._sync_once 覆盖语义）。
pub fn manual_sync_once(cfg: &Config) -> Result<usize, String> {
    if !cfg.gitee_enabled || cfg.gitee_token.is_empty() {
        return Err("请先在 Web与仓库 页启用同步并配置访问令牌".into());
    }
    let mut download_folder = cfg.gitee_download_folder.clone();
    if std::fs::create_dir_all(&download_folder).is_err() {
        download_folder = paths::home().join("wulikejian").to_string_lossy().to_string();
        std::fs::create_dir_all(&download_folder).map_err(|e| format!("下载目录无法创建: {}", e))?;
    }
    let api = GiteeApi::new(&cfg.gitee_owner, &cfg.gitee_repo, &cfg.gitee_token);
    let (ok, info) = api.test_connection(false, None);
    if !ok {
        return Err(format!("连接失败: {}", info));
    }
    let remote_files = api.list_files("uploads");
    let mut count = 0usize;
    for f in &remote_files {
        if f.path.contains(".parts/") || f.path.contains(".chunks/") {
            continue;
        }
        // 新格式分块组：.manifest.json 条目 → 按 manifest 合并下载原文件
        if let Some(target) = f.path.strip_suffix(".manifest.json") {
            let merged = api
                .download_file_content(&f.path)
                .and_then(|raw| serde_json::from_slice::<Value>(&raw).map_err(|e| e.to_string()))
                .and_then(|m| api.download_chunked(target, &m));
            match merged {
                Ok(content) => {
                    let rel = target.strip_prefix("uploads/").unwrap_or(target);
                    let local = std::path::PathBuf::from(&download_folder).join(rel);
                    if let Some(p) = local.parent() {
                        let _ = std::fs::create_dir_all(p);
                    }
                    if std::fs::write(&local, &content).is_ok() {
                        count += 1;
                    }
                }
                Err(e) => logging::error(&format!("分块下载失败 {}: {}", target, e)),
            }
            continue;
        }
        if let Ok(content) = api.download_file_content(&f.path) {
            let rel = f.path.strip_prefix("uploads/").unwrap_or(&f.path);
            let local = std::path::PathBuf::from(&download_folder).join(rel);
            if let Some(p) = local.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            if std::fs::write(&local, &content).is_ok() {
                count += 1;
            }
        }
    }
    Ok(count)
}

/// 下载 .parts/manifest.json 及其分块，返回 (文件名, 总大小, 合并内容)。
fn download_chunk_parts(
    api: &GiteeApi,
    manifest_path: &str,
) -> Result<(String, i64, Vec<u8>), String> {
    let raw = api.download_file_content(manifest_path)?;
    let m: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    let filename = m.get("filename").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let total_size = m.get("total_size").and_then(|x| x.as_i64()).unwrap_or(0);
    let mut merged = Vec::new();
    if let Some(parts) = m.get("parts").and_then(|x| x.as_array()) {
        for p in parts {
            let p_path = p.get("path").and_then(|x| x.as_str()).unwrap_or("");
            if p_path.is_empty() {
                continue;
            }
            let chunk = api.download_file_content(p_path)?;
            merged.extend(chunk);
        }
    }
    Ok((filename, total_size, merged))
}

// ====================== 内置定时通知 ======================

pub struct ScheduledNoticeService {
    stop: Arc<AtomicBool>,
}

impl ScheduledNoticeService {
    pub fn new() -> Self {
        ScheduledNoticeService {
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(self: &Arc<Self>, cfg: ConfigGetter) {
        let stop = self.stop.clone();
        std::thread::Builder::new()
            .name("ScheduledNotice".into())
            .spawn(move || {
                let mut last_reload: f64 = 0.0;
                let mut notices: Vec<ScheduledNotice> = Vec::new();
                // None = 尚未同步（启动后强制对齐一次远端）
                let mut active: Option<HashSet<String>> = None;
                while !stop.load(Ordering::Relaxed) {
                    tick(&cfg, &stop, &mut last_reload, &mut notices, &mut active);
                    sleep_interruptible(&stop, 20);
                }
            })
            .ok();
        logging::info("内置定时通知服务已启动");
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn hm_to_min(hm: &str) -> Option<i64> {
    let mut it = hm.trim().split(':');
    let h: i64 = it.next()?.trim().parse().ok()?;
    let m: i64 = it.next().unwrap_or("0").trim().parse().ok()?;
    Some(h * 60 + m)
}

fn load_scheduled_notices(cfg: &ConfigGetter) -> Vec<ScheduledNotice> {
    let c = cfg.lock().unwrap().clone();
    if c.gitee_enabled && !c.gitee_token.is_empty() {
        let api = GiteeApi::new(&c.gitee_owner, &c.gitee_repo, &c.gitee_token);
        if let Ok(raw) = api
            .download_file_content(&format!("{}/{}", c.gitee_tongzhi_dir, SCHEDULED_NOTICES_PATH))
        {
            if let Ok(list) = serde_json::from_slice::<Vec<ScheduledNotice>>(&raw) {
                return list;
            }
        }
    }
    c.scheduled_notices.clone()
}

fn tick(
    cfg: &ConfigGetter,
    _stop: &AtomicBool,
    last_reload: &mut f64,
    notices: &mut Vec<ScheduledNotice>,
    active: &mut Option<HashSet<String>>,
) {
    let now = Local::now();
    if *last_reload == 0.0 || now.timestamp() as f64 - *last_reload > 60.0 {
        *notices = load_scheduled_notices(cfg);
        *last_reload = now.timestamp() as f64;
        logging::info(&format!("已重新加载定时通知 {} 条", notices.len()));
    }
    let c = cfg.lock().unwrap();
    if notices.is_empty() {
        let need_clear = active.is_none() || active.as_ref().map(|a| !a.is_empty()).unwrap_or(false);
        drop(c); // publish_active 内部要再锁，先释放
        if need_clear && publish_active(cfg, &HashSet::new(), None) {
            *active = Some(HashSet::new());
        }
        return;
    }
    if !(c.gitee_enabled && !c.gitee_token.is_empty()) {
        return;
    }
    let Some(cls) = c.current() else { return };
    let remote_path = format!("{}/scheduled_{}", c.gitee_tongzhi_dir, cls.file_name);
    let wd = (now.weekday().num_days_from_monday() + 1) as i64; // 1=周一
    let now_min = chrono::Timelike::hour(&now) as i64 * 60 + chrono::Timelike::minute(&now) as i64;
    let mut should_active: HashSet<String> = HashSet::new();
    for n in notices {
        if !n.enabled {
            continue;
        }
        let content = n.content.trim().to_string();
        if content.is_empty() {
            continue;
        }
        let days = n.weekday_nums();
        if !days.contains(&wd) {
            continue;
        }
        let Some(s) = hm_to_min(if n.start.is_empty() { "00:00" } else { &n.start }) else { continue };
        let Some(e) = hm_to_min(if n.end.is_empty() { "23:59" } else { &n.end }) else { continue };
        if s <= now_min && now_min < e {
            should_active.insert(content);
        }
    }
    drop(c);
    if active.as_ref() != Some(&should_active) {
        if publish_active(cfg, &should_active, Some(&remote_path)) {
            *active = Some(should_active);
        }
    }
}

fn publish_active(cfg: &ConfigGetter, active: &HashSet<String>, remote_path: Option<&str>) -> bool {
    let c = cfg.lock().unwrap();
    if !(c.gitee_enabled && !c.gitee_token.is_empty()) {
        return false;
    }
    let Some(cls) = c.current() else { return false };
    let path = remote_path.map(String::from).unwrap_or_else(|| {
        format!("{}/scheduled_{}", c.gitee_tongzhi_dir, cls.file_name)
    });
    let api = GiteeApi::new(&c.gitee_owner, &c.gitee_repo, &c.gitee_token);
    drop(c);
    let mut items: Vec<String> = active.iter().cloned().collect();
    items.sort();
    let is_empty = items.is_empty();
    let content = if is_empty {
        " ".to_string()
    } else {
        items.join("\n")
    };
    match api.create_text_file(&content, &path, "master") {
        Ok(_) => {
            logging::info(&format!(
                "定时通知窗口更新：{}",
                if is_empty { "（清空）" } else { &content }
            ));
            true
        }
        Err(e) => {
            logging::warn(&format!("定时通知写入失败（20秒后自动重试）：{}", e));
            false
        }
    }
}

// ====================== 历史落盘线程 ======================

pub fn spawn_history_saver(history: Arc<HistoryManager>, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("history-saver".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                history.tick();
                std::thread::sleep(Duration::from_secs(5));
            }
            history.flush();
        })
        .ok();
}

/// 启动主程序 = 自动打开「作业栏」和「通知栏」需要恢复的两个开关。
///
/// 只 patch 这两个键，绝不整份重写 desktop_board.json（避免默认值回填覆盖
/// 用户填好的作业 / 值日内容）。上次点 ✕ 关掉的隐藏状态在下次启动时被清掉；
/// 但点过「彻底关闭」的作业栏例外：重启后仍保持隐藏，只能在设置窗口重新打开。
pub fn startup_display_keys(hard_hidden: bool) -> Vec<(&'static str, Value)> {
    vec![
        ("marquee_hidden", json!(false)),
        // homework_hidden = true 表示隐藏：彻底关闭时保持 true，否则清成 false
        ("homework_hidden", json!(hard_hidden)),
    ]
}

/// 大屏自动拉起（board_autostart）：主程序启动后调用。
/// 优先同进程线程拉起（合并运行）；线程异常时 exe 兜底。
pub fn maybe_start_board(cfg: &ConfigGetter) {
    if std::env::var("ROLL_DISABLE_BOARD").is_ok() {
        return; // 诊断开关：跳过大屏线程
    }
    // 无论大屏是不是已经在跑，都先恢复两个面板：
    // 大屏已在运行时，跑马灯 / 作业卡各自的轮询会读到新值并自己 ShowWindow。
    // 「彻底关闭」是唯一例外：作业栏保持隐藏，只有设置窗口能清掉这个标志。
    let hard = desktop_board::board::homework_hard_hidden();
    desktop_board::board::patch_keys(&startup_display_keys(hard));
    let _ = std::fs::remove_file(paths::board_stop_file());
    let autostart = cfg.lock().unwrap().board_autostart;
    if !autostart {
        return;
    }
    if win::board_is_running() {
        logging::info(if hard {
            "桌面大屏已在运行；通知栏已恢复，作业栏保持彻底关闭"
        } else {
            "桌面大屏已在运行，已自动恢复作业栏与通知栏显示"
        });
        return;
    }
    desktop_board::run_in_thread();
    logging::info(if hard {
        "已在主进程内拉起桌面大屏线程（通知栏自动显示；作业栏保持彻底关闭）"
    } else {
        "已在主进程内拉起桌面大屏线程（作业栏 / 通知栏自动显示）"
    });
    // 给线程 2 秒拿互斥体，失败则独立进程兜底：
    // 同目录有 desktop_board.exe 就用它，没有就用主程序自身的 --desktop-board 模式
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if !win::board_is_running() {
            let exe = win::exe_path();
            let dir = exe.parent().unwrap_or(std::path::Path::new("."));
            let board = dir.join("desktop_board.exe");
            let spawn_ok = if board.exists() {
                std::process::Command::new(&board).spawn().is_ok()
            } else {
                std::process::Command::new(&exe)
                    .arg("--desktop-board")
                    .spawn()
                    .is_ok()
            };
            if spawn_ok {
                logging::info("线程拉起失败，已用独立进程兜底启动大屏");
            }
        }
    });
}

/// 关闭桌面大屏（写 stop 文件，独立进程与同进程线程都响应）。
pub fn stop_board() {
    if let Some(p) = std::path::Path::new(&paths::board_stop_file())
        .parent()
        .map(|d| d.to_path_buf())
    {
        let _ = std::fs::create_dir_all(p);
    }
    let _ = std::fs::write(paths::board_stop_file(), b"stop");
    logging::info("已发送桌面大屏退出信号");
}

// ====================== 下课守护（--guard 模式） ======================
//
// 主程序被整个关掉之后，Gitee 上来了新通知没人接收。这个无窗口的极小进程
// 独立于主程序常驻，每 20 秒看一次作息：一到课间就把主程序拉起来；
// 若关机期间确实来了新通知，还会写补弹标记让横幅立刻弹出。
// 平时完全不联网，只在「刚下课那一拍」拉一次 Gitee，不额外增加流量。

/// 守护检查作息的间隔（秒）
pub const GUARD_TICK_SECS: u64 = 20;

/// 「还在教学时段的课间」：`is_break_now` 把工作日深夜 / 放学后也算不在上课，
/// 再用「今天还有下一节课」收窄，避免晚上或放假前把程序拉起来。
fn school_break_now() -> bool {
    // 直接复用大屏自己的配置读取，保证与横幅弹出的作息判定完全一致
    let work_mode = desktop_board::config::load_app_config().work_mode;
    desktop_board::schedule::is_break_now(&work_mode)
        && desktop_board::schedule::secs_until_next_class(&work_mode).is_some()
}

/// 纯函数：一拍的决策 → (是否拉起主程序, 是否需要写补弹标记)。
/// 只在「刚进入课间」且「主程序没在跑」时拉起，避免课间反复拉、也避免上课打扰。
fn guard_tick_decision(
    is_break: bool,
    was_break: bool,
    app_running: bool,
    has_new_notice: bool,
) -> (bool, bool) {
    if app_running || !(is_break && !was_break) {
        return (false, false);
    }
    // 没新通知也把程序恢复常驻（auto_run 本来就是这个意图），但不弹横幅打扰
    (true, has_new_notice)
}

/// 主程序启动时调用：守护没在跑就拉一个起来。
pub fn ensure_guard() {
    if win::guard_is_running() {
        return;
    }
    let _ = std::fs::remove_file(paths::guard_stop_file());
    if win::spawn_self(&["--guard"]) {
        logging::info("下课守护已拉起（主程序退出后仍常驻）");
    }
}

/// 守护主循环（`--guard` 模式入口）。
pub fn run_guard() {
    if !win::single_instance_ok(win::GUARD_MUTEX) {
        return; // 已经有一个守护在跑
    }
    logging::info("下课守护启动：每 20 秒检查作息，课间且主程序未运行时拉起");
    let notice_file = paths::data_dir().join("board_notice.txt");
    // 初始化成当前状态：课间中途启动守护不会立刻把用户刚关掉的程序拽回来
    let mut was_break = school_break_now();
    loop {
        std::thread::sleep(Duration::from_secs(GUARD_TICK_SECS));
        if paths::guard_stop_file().exists() {
            logging::info("下课守护收到停止标记，退出");
            return;
        }
        let cfg = Config::load();
        let is_break = school_break_now();
        if win::main_app_running() {
            // 主程序在跑：把当前展示的通知记为「已见」，关机后才有准确的比较基线
            was_break = is_break;
            if let Ok(t) = std::fs::read_to_string(&notice_file) {
                let _ = std::fs::write(paths::last_seen_notice_file(), t.trim());
            }
            continue;
        }
        let prev_break = was_break;
        was_break = is_break;
        if !(is_break && !prev_break) {
            continue;
        }
        // 只在下课这一拍联网核对一次，避免守护一直打 Gitee
        let has_new = remote_notice_changed(&cfg, &notice_file);
        let (launch, popup) = guard_tick_decision(is_break, prev_break, false, has_new);
        if !launch {
            continue;
        }
        if popup {
            let _ = std::fs::write(paths::break_popup_flag(), b"1");
        }
        if win::spawn_self(&[]) {
            logging::info(&format!("守护：下课且主程序未运行，已拉起（新通知={popup}）"));
        }
    }
}

/// 拉一次 Gitee，和「关机前已见」的通知比对；网络不通就退化比较本地文件。
/// 有新通知时顺手写进 board_notice.txt，让刚起来的大屏立刻有东西可显示。
fn remote_notice_changed(cfg: &Config, notice_file: &std::path::Path) -> bool {
    let last = std::fs::read_to_string(paths::last_seen_notice_file())
        .unwrap_or_default()
        .trim()
        .to_string();
    let fresh = match crate::gitee::fetch_notice_text(cfg) {
        Some(t) => t.trim().to_string(),
        None => std::fs::read_to_string(notice_file)
            .unwrap_or_default()
            .trim()
            .to_string(),
    };
    if fresh.is_empty() {
        return false;
    }
    let changed = fresh != last;
    if changed {
        let _ = std::fs::write(notice_file, &fresh);
    }
    changed
}

#[cfg(test)]
mod guard_tests {
    use super::{guard_tick_decision, heartbeat_base_secs, startup_display_keys};

    #[test]
    fn heartbeat_interval_follows_bridge_health() {
        // GoEasy 桥接健康 → 15 分钟一跳（notify 有 presence 实时判定，释放 Gitee 配额）；
        // 桥接断开 → 恢复 3 分钟旧节奏（presence 不可用时的圆点兜底）。
        assert_eq!(heartbeat_base_secs(true), 900);
        assert_eq!(heartbeat_base_secs(false), 180);
    }

    #[test]
    fn startup_opens_both_panels() {
        let keys = startup_display_keys(false);
        assert_eq!(keys.len(), 2);
        let names: Vec<&str> = keys.iter().map(|(k, _)| *k).collect();
        assert!(names.contains(&"marquee_hidden"));
        assert!(names.contains(&"homework_hidden"));
        // 两个开关都必须是「解除隐藏」，不能有一个漏掉或写反
        assert!(keys.iter().all(|(_, v)| v.as_bool() == Some(false)));
    }

    #[test]
    fn startup_respects_hard_closed_homework() {
        let keys = startup_display_keys(true);
        let get = |k: &str| {
            keys.iter()
                .find(|(n, _)| *n == k)
                .and_then(|(_, v)| v.as_bool())
        };
        assert_eq!(get("marquee_hidden"), Some(false), "通知栏照常自动打开");
        assert_eq!(get("homework_hidden"), Some(true), "彻底关闭后重启仍关闭");
    }

    #[test]
    fn only_launches_on_break_edge_when_app_is_closed() {
        // 上课中、课间内连续几拍都不拉
        assert_eq!(guard_tick_decision(false, false, false, true), (false, false));
        assert_eq!(guard_tick_decision(true, true, false, true), (false, false));
        // 主程序在跑绝不拉
        assert_eq!(guard_tick_decision(true, false, true, true), (false, false));
        // 刚下课 + 程序关着 + 有新通知 → 拉起并补弹
        assert_eq!(guard_tick_decision(true, false, false, true), (true, true));
        // 没有新通知也恢复常驻，但不弹横幅打扰
        assert_eq!(guard_tick_decision(true, false, false, false), (true, false));
    }
}
