//! GoEasy PubSub 实时唤醒桥（对应 Python GoEasyBridge / _GoEasyWakeHandler）。
//!
//! GoEasy 官方没有 Rust 订阅 SDK：与 Python 版同构——生成本地订阅页
//! （优先内嵌 goeasy-lite.min.js，否则回退线上 SDK），用系统自带的 Edge/Chrome
//! 以无头模式运行官方 JS SDK 订阅频道；收到 PubSub 消息后页面 fetch 本机
//! 127.0.0.1:port 的 /wake，由桥转发 `UiMsg::GoEasyWake` 给主线程立即重新拉取通知。
//! Gitee 仍是唯一数据源：PubSub 只当门铃，消息丢失时原有 60/90 秒轮询照常兜底。
//!
//! 发布端统一在外部网页（gch365.github.io/physics-html/notify/，内置 Common Key +
//! JS SDK pubsub.publish）。本文件只保留订阅端；旧版 PC 端 REST publish
//! （设置窗口「立即发布通知」）已按需求移除。

use crate::logging;
use crate::paths;
use crate::ui::UiMsg;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 桥接页心跳判定窗：心跳（页端 60 秒一次，模板内硬编码 60000ms）在
/// 150 秒（2.5 拍）内没等到任何心跳/唤醒/连接上报 → 门铃判死，主程序切兜底轮询。
pub const BRIDGE_ALIVE_MAX_AGE_SECS: u64 = 150;

static BRIDGE_ALIVE: std::sync::OnceLock<Mutex<Option<Instant>>> = std::sync::OnceLock::new();

fn bridge_alive_cell() -> &'static Mutex<Option<Instant>> {
    BRIDGE_ALIVE.get_or_init(|| Mutex::new(None))
}

/// 桥接页心跳/门铃消息到达：门铃健康。
pub fn mark_bridge_alive() {
    *bridge_alive_cell().lock().unwrap() = Some(Instant::now());
}

/// 连接失败 / 心跳断档：门铃判死。
pub fn mark_bridge_dead() {
    *bridge_alive_cell().lock().unwrap() = None;
}

/// GoEasy 门铃是否健康：最近 150 秒内有过桥接页心跳（或门铃消息/连接上报）。
/// 供主程序决定是否需要兜底轮询——健康时上课期间零轮询。
pub fn doorbell_healthy() -> bool {
    bridge_alive_cell()
        .lock()
        .unwrap()
        .map(|t| t.elapsed() < Duration::from_secs(BRIDGE_ALIVE_MAX_AGE_SECS))
        .unwrap_or(false)
}

/// 订阅页模板（与 Python BRIDGE_TEMPLATE 一致）。
const BRIDGE_TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="zh-CN"><head><meta charset="UTF-8"><title>GoEasy 订阅桥</title></head><body>
__SDK_TAG__
<script>
(function () {
  var C = __CFG__;
  function wake(a, text) {
    var q = "/wake?f=" + encodeURIComponent(C.f) +
            "&a=" + encodeURIComponent(a || "update") +
            "&t=" + Date.now();
    // 广播里带的正文只作「秒显预览」，超长会撑爆桥的读取缓冲，超长就退回纯门铃
    if (text) {
      var enc = encodeURIComponent(String(text).slice(0, 600));
      if (enc.length <= 1500) { q += "&text=" + enc; }
    }
    try { fetch("http://127.0.0.1:" + C.port + q); } catch (e) {}
  }
  if (typeof GoEasy === "undefined") {
    // 静默 return 会让「SDK 根本没加载」看起来和「连接正常但没消息」一模一样，
    // 主动上报一次，日志里能直接看到原因。
    try {
      fetch("http://127.0.0.1:" + C.port + "/status?err=" +
            encodeURIComponent("sdk-missing: goeasy-lite.min.js 未加载"));
    } catch (e) {}
    return;
  }
  try {
    var g = GoEasy.getInstance({host: C.host, appkey: C.key, modules: ["pubsub"]});
    g.connect({
      id: C.uid,
      data: C.ud,
      onSuccess: function () {
        g.pubsub.subscribe({channel: C.ch, presence: {enable: true},
                            onMessage: function (m) {
          try {
            var d = JSON.parse(m.content);
            if (d.f && C.f && d.f !== C.f) { return; }
            wake(d.a || "update", d.text || "");
          } catch (e) { wake("update", ""); }
        }});
        wake("connected");
        // 周期心跳：每 60 秒向主程序上报一次 GoEasy 真实连接状态。
        // 主程序据此判断门铃健康：健康时零轮询，心跳断档（页挂了/连接掉了）
        // 自动切 120 秒兜底轮询，新通知不丢。
        // 反过来，心跳连续 3 次失败 = 主程序已退出 → 页面自关，
        // 避免无头页变成占着 presence 名额、响应旧班级消息的僵尸。
        var hbFail = 0;
        setInterval(function () {
          var cs = "";
          try { cs = (g.getConnectionStatus && g.getConnectionStatus()) || ""; } catch (e) {}
          cs = String(cs).toLowerCase();
          var alive = (cs === "") || cs === "connected";
          var q = alive ? "ok=1&cs=" : "err=cs%3A";
          fetch("http://127.0.0.1:" + C.port + "/status?" + q + encodeURIComponent(cs || "unknown"))
            .then(function () { hbFail = 0; })
            .catch(function () {
              if (++hbFail >= 3) { try { window.close(); } catch (e) {} }
            });
        }, 60000);
      },
      onFailed: function (e) {
        try {
          fetch("http://127.0.0.1:" + C.port + "/status?err=" +
                encodeURIComponent((e && (e.code + " " + e.content)) || "unknown"));
        } catch (err) {}
      }
    });
  } catch (e) {}
})();
</script></body></html>"#;

fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into())
}

/// 与 Python _find_browser 一致：按候选顺序找 Edge/Chrome。
fn find_browser() -> Option<PathBuf> {
    let cands = [
        ("ProgramFiles(x86)", r"Microsoft\Edge\Application\msedge.exe"),
        ("ProgramFiles", r"Microsoft\Edge\Application\msedge.exe"),
        ("LocalAppData", r"Microsoft\Edge\Application\msedge.exe"),
        ("ProgramFiles", r"Google\Chrome\Application\chrome.exe"),
        ("ProgramFiles(x86)", r"Google\Chrome\Application\chrome.exe"),
        ("LocalAppData", r"Google\Chrome\Application\chrome.exe"),
    ];
    for (var, rest) in cands {
        if let Ok(base) = std::env::var(var) {
            let p = PathBuf::from(base).join(rest);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// 优先把本地 SDK 内嵌进订阅页（完全自包含）；没有则返回空（模板回退线上文件）。
fn load_sdk() -> String {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            cands.push(dir.join("goeasy-lite.min.js"));
        }
    }
    cands.push(paths::data_dir().join("goeasy-lite.min.js"));
    for p in cands {
        if let Ok(text) = std::fs::read_to_string(&p) {
            return text;
        }
    }
    String::new()
}

fn file_uri(p: &PathBuf) -> String {
    let s = p.display().to_string().replace('\\', "/");
    let mut out = String::from("file:///");
    for c in s.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '%' => out.push_str("%25"),
            '"' => out.push_str("%22"),
            _ => out.push(c),
        }
    }
    out
}

fn build_bridge_html(cfg: &crate::config::Config, port: u16, sdk: &str) -> String {
    let f = cfg.current_class.trim().to_string();
    // presence 身份：pc-班级码（去 .txt；未建班退回计算机名，保证 id 恒非空）
    let uid = format!(
        "pc-{}",
        if f.is_empty() {
            hostname()
        } else {
            f.trim_end_matches(".txt").to_string()
        }
    );
    let display = cfg
        .current()
        .map(|e| e.display_name.clone())
        .unwrap_or_default();
    let ud = serde_json::json!({
        "name": if display.is_empty() {
            if f.is_empty() { "未命名班级".to_string() } else { f.clone() }
        } else {
            display
        },
        "type": "pc"
    });
    let conf = serde_json::json!({
        "key": cfg.goeasy_appkey.trim(),
        "host": if cfg.goeasy_host.trim().is_empty() { "hangzhou.goeasy.io".to_string() } else { cfg.goeasy_host.trim().to_string() },
        "ch": if cfg.goeasy_channel.trim().is_empty() { "gzzx_class_notice_v1".to_string() } else { cfg.goeasy_channel.trim().to_string() },
        "port": port,
        "f": f,
        "uid": uid,
        "ud": ud,
    });
    let sdk_tag = if sdk.is_empty() {
        // 回退到线上同名文件（与 Python 版一致）
        r#"<script src="https://gch365.github.io/physics-html/notify/goeasy-lite.min.js"></script>"#.to_string()
    } else {
        format!("<script>{}</script>", sdk.replace("</script>", "<\\/script>"))
    };
    BRIDGE_TEMPLATE
        .replace("__SDK_TAG__", &sdk_tag)
        .replace("__CFG__", &conf.to_string())
}

/// 本次启动专用的无头浏览器 profile 目录。
///
/// 不能共用固定目录：上一次实例没退干净时，僵尸无头 Edge 会锁住 profile，
/// 新启动的 Edge 检测到 user-data-dir 已被占用，把 URL 转交给老实例后**立即退出**，
/// 结果是「无头订阅页已启动」日志照打、但新班级的桥接页永远不连 GoEasy
/// （僵尸页还占着旧班级的 presence 名额）。每次启动带毫秒时间戳即可彻底避开；
/// 旧目录顺手清理（被占用的清不掉，忽略）。
fn unique_profile_dir(data_dir: &std::path::Path, boot_ms: u128) -> std::path::PathBuf {
    let current = data_dir.join(format!("goeasy_edge_profile_{boot_ms}"));
    // 旧目录清理**必须异步**：僵尸无头 Edge 占用的目录里有数千个文件，
    // 同步 remove_dir_all 会逐个尝试删除、耗时极长，把启动线程卡死——
    // 实测表现为桥接页文件写完后再无下文（Edge 永远起不来、无任何日志）。
    let data = data_dir.to_path_buf();
    let cur = current.clone();
    std::thread::Builder::new()
        .name("goeasy-profile-gc".into())
        .spawn(move || {
            if let Ok(rd) = std::fs::read_dir(&data) {
                for e in rd.flatten() {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with("goeasy_edge_profile")
                        && e.path() != cur
                        && e.path().is_dir()
                    {
                        let _ = std::fs::remove_dir_all(e.path());
                    }
                }
            }
        })
        .ok();
    current
}

fn start_headless(cfg: &crate::config::Config, port: u16) {
    let exe = match find_browser() {
        Some(p) => p,
        None => {
            logging::warn("未找到 Edge/Chrome：GoEasy 实时推送需手动保持 client.html 打开");
            return;
        }
    };
    let sdk = load_sdk();
    let html = build_bridge_html(cfg, port, &sdk);
    let html_path = paths::data_dir().join("goeasy_bridge.html");
    if let Err(e) = std::fs::write(&html_path, html) {
        logging::warn(&format!("GoEasy 订阅页生成失败：{e}"));
        return;
    }
    let boot_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let profile = unique_profile_dir(&paths::data_dir(), boot_ms);
    if let Err(e) = std::fs::create_dir_all(&profile) {
        logging::warn(&format!("GoEasy 无头浏览器 profile 创建失败：{e}"));
        return;
    }
    let args = [
        "--headless=new".to_string(),
        "--disable-gpu".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-extensions".to_string(),
        "--disable-background-networking".to_string(),
        "--mute-audio".to_string(),
        "--remote-allow-origins=*".to_string(),
        format!("--user-data-dir={}", profile.display()),
        file_uri(&html_path),
    ];
    use std::os::windows::process::CommandExt;
    match std::process::Command::new(&exe)
        .args(&args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => logging::info(&format!(
            "GoEasy 无头订阅页已启动：{}",
            exe.file_name().unwrap_or_default().to_string_lossy()
        )),
        Err(e) => logging::warn(&format!("GoEasy 无头浏览器启动失败：{e}")),
    }
}

/// 启动桥：绑定本地唤醒端口 + 可选无头订阅页。失败自动降级为轮询（不致命）。
pub fn start(tx: Sender<UiMsg>, cfg: Arc<Mutex<crate::config::Config>>) {
    let (enabled, key, port, headless) = {
        let c = cfg.lock().unwrap();
        (
            c.goeasy_enabled,
            c.goeasy_appkey.trim().to_string(),
            if c.goeasy_port == 0 { 18765 } else { c.goeasy_port },
            c.goeasy_headless,
        )
    };
    if !enabled || key.is_empty() {
        logging::info("GoEasy 实时推送未启用（未配置 Subscribe Key）");
        return;
    }
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            logging::warn(&format!("GoEasy 唤醒端口 {port} 不可用，实时推送降级为轮询：{e}"));
            return;
        }
    };
    logging::info(&format!("GoEasy 实时推送已启动（本地端口 {port}）"));
    let tx_http = tx;
    let cfg_http = cfg.clone();
    std::thread::Builder::new()
        .name("goeasy-http".into())
        .spawn(move || serve_wake(listener, tx_http, cfg_http))
        .ok();
    if headless {
        let snapshot = { let c = cfg.lock().unwrap(); c.clone() };
        start_headless(&snapshot, port);
    }
}

fn serve_wake(
    listener: TcpListener,
    tx: Sender<UiMsg>,
    cfg: Arc<Mutex<crate::config::Config>>,
) {
    let last_wake = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(10)));
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let tx = tx.clone();
        let last = last_wake.clone();
        let cfg = cfg.clone();
        std::thread::spawn(move || handle_conn(&mut stream, tx, last, cfg));
    }
}

/// 班级过滤，语义与 Python `GoEasyBridge.on_wake` 完全一致：
///
/// ```python
/// if file_name and cur and file_name != cur:
///     return          # 只有「消息指定班级」且「本机已选班级」且「两者不同」才忽略
/// ```
///
/// 因此广播消息（f 为空）和未建班（cur 为空）都必须照常唤醒。
/// 早期版本误写成 `!(f.is_empty() || cur.is_empty() || f != cur)`，
/// 恰好把这三类正常场景全部吞掉，只剩「双方非空且相等」能通过。
fn wake_passes(f: &str, cur: &str) -> bool {
    f.is_empty() || cur.is_empty() || f == cur
}

/// 广播正文只作预览：限长、去控制字符、过滤空白/单字。
/// 真正的权威文本仍由主程序回拉 Gitee 拿到，这里不做任何信任提升。
fn sanitize_quick_text(raw: &str) -> Option<String> {
    let t: String = raw
        .trim()
        .chars()
        .take(600)
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let t = t.trim().to_string();
    if crate::gitee::is_valid_notice(&t) {
        Some(t)
    } else {
        None
    }
}

fn handle_conn(
    stream: &mut TcpStream,
    tx: Sender<UiMsg>,
    last: Arc<Mutex<Instant>>,
    cfg: Arc<Mutex<crate::config::Config>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    // 带广播正文的 /wake 请求行会比纯门铃长，缓冲留一倍余量
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf).unwrap_or(0);
    if n == 0 {
        return;
    }
    let req = String::from_utf8_lossy(&buf[..n]);
    let first_line = req.lines().next().unwrap_or("");
    let method = first_line.split_whitespace().next().unwrap_or("");
    // file:// 页面 fetch 127.0.0.1 属于 Private Network Access，
    // Chrome/Edge 会先发 OPTIONS 预检；不给 2xx 的话 /wake 和 /status
    // 会被浏览器全部拦掉，日志里连一条错误都看不到。
    if method == "OPTIONS" {
        reply_preflight(stream);
        return;
    }
    let target = first_line.split_whitespace().nth(1).unwrap_or("");
    let (route, query) = match target.split_once('?') {
        Some((r, q)) => (r, q),
        None => (target, ""),
    };
    match route {
        "/wake" => {
            let f = query_param(query, "f");
            let a = query_param(query, "a");
            let cur = { cfg.lock().unwrap().current_class.clone() };
            let pass = wake_passes(&f, &cur);
            // 收到门铃消息本身即证明链路活着（连接成功、订阅有效）
            mark_bridge_alive();
            // 别班广播连预览都不给，避免跨班串屏
            let quick_text = if pass {
                sanitize_quick_text(&query_param(query, "text"))
            } else {
                None
            };
            // 去重 1.5s：无头页与手动大屏页可能同时在线
            let dup = {
                let mut l = last.lock().unwrap();
                if l.elapsed() < Duration::from_millis(1500) {
                    true
                } else {
                    *l = Instant::now();
                    false
                }
            };
            let has_text = quick_text.is_some();
            // files = 网页上传完文件：与通知无关，只触发一次 Gitee 同步
            let sent = if a == "files" {
                if pass && !dup { tx.send(UiMsg::GoEasyFiles).is_ok() } else { false }
            } else if pass && !dup {
                tx.send(UiMsg::GoEasyWake { quick_text }).is_ok()
            } else {
                false
            };
            if sent {
                logging::info(if a == "files" {
                    "GoEasy 文件信号：立即唤醒 Gitee 同步下载"
                } else if has_text {
                    "GoEasy 实时唤醒：广播正文秒显，同时回拉 Gitee 校正"
                } else {
                    "GoEasy 实时唤醒：立即重新拉取通知"
                });
            }
            reply(stream, 200, br#"{"ok":1}"#);
        }
        "/status" => {
            if query_param(query, "ok") == "1" {
                // 周期心跳：cs 带 SDK 连接状态时以它为准（非 connected 即判死，
                // 触发主程序兜底轮询）；老版桥接页/取不到状态时页活着即算健康。
                let cs = query_param(query, "cs");
                if cs.is_empty() || cs == "connected" {
                    mark_bridge_alive();
                } else {
                    logging::warn(&format!(
                        "GoEasy 桥接心跳异常：连接状态 {cs}（门铃已降级，切兜底轮询）"
                    ));
                    mark_bridge_dead();
                }
                reply(stream, 200, br#"{"ok":1}"#);
                return;
            }
            let err = query_param(query, "err");
            mark_bridge_dead();
            if err.contains("sdk-missing") {
                logging::warn(&format!(
                    "GoEasy 订阅 SDK 未加载：{err}；请把 goeasy-lite.min.js 放到 exe 同目录或数据目录"
                ));
            } else {
                logging::warn(&format!(
                    "GoEasy 连接失败：{err}（请核对 AppKey 与应用区域；实时推送已降级为轮询）"
                ));
            }
            reply(stream, 200, br#"{"ok":1}"#);
        }
        _ => reply(stream, 404, br#"{"ok":0}"#),
    }
}

/// CORS / Private Network Access 预检响应（对应 Python `do_OPTIONS`）。
fn reply_preflight(stream: &mut TcpStream) {
    let head = "HTTP/1.1 204 No Content\r\n\
Access-Control-Allow-Origin: *\r\n\
Access-Control-Allow-Methods: GET, OPTIONS\r\n\
Access-Control-Allow-Headers: *\r\n\
Access-Control-Allow-Private-Network: true\r\n\
Access-Control-Max-Age: 86400\r\n\
Cache-Control: no-store\r\n\
Content-Length: 0\r\n\
Connection: close\r\n\r\n";
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.flush();
}

fn reply(stream: &mut TcpStream, code: u16, body: &[u8]) {
    let status = match code {
        200 => "200 OK",
        404 => "404 Not Found",
        _ => "500 Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json; charset=utf-8\r\n\
         Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Private-Network: true\r\n\
         Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// 从 query string 取参数并做百分号解码（+ 视为空格）。
fn query_param(query: &str, key: &str) -> String {
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return percent_decode(v);
        }
    }
    String::new()
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = |b: u8| -> Option<u8> {
                    match b {
                        b'0'..=b'9' => Some(b - b'0'),
                        b'a'..=b'f' => Some(b - b'a' + 10),
                        b'A'..=b'F' => Some(b - b'A' + 10),
                        _ => None,
                    }
                };
                if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    out.push(h * 16 + l);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, Receiver};

    #[test]
    fn unique_profile_dir_cleans_old_and_avoids_current() {
        let base = std::env::temp_dir().join(format!("rc_prof_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&base);
        // 旧目录（含无时间戳的固定目录）都应被清掉
        for name in ["goeasy_edge_profile", "goeasy_edge_profile_111", "goeasy_edge_profile_222"] {
            let _ = std::fs::create_dir_all(base.join(name));
            std::fs::write(base.join(name).join("x.txt"), b"1").unwrap();
        }
        let cur = unique_profile_dir(&base, 333);
        assert!(cur.ends_with("goeasy_edge_profile_333"));
        // 清理是后台异步的：轮询等待 GC 线程完成（僵尸目录场景下同步清理
        // 会卡死启动线程，这正是它改成异步的原因）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let all_gone = ["goeasy_edge_profile", "goeasy_edge_profile_111", "goeasy_edge_profile_222"]
                .iter()
                .all(|n| !base.join(n).exists());
            if all_gone || std::time::Instant::now() > deadline {
                assert!(all_gone, "5 秒内旧 profile 目录应被后台 GC 清理");
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(cur.is_dir() || std::fs::create_dir_all(&cur).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 起一个真实 HTTP 桥（随机端口），每个用例独立实例以避开 1.5s 去重。
    fn spawn_bridge(cur: &str) -> (u16, Receiver<UiMsg>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = channel();
        let mut c = crate::config::Config::default();
        c.current_class = cur.to_string();
        let cfg = Arc::new(Mutex::new(c));
        std::thread::spawn(move || serve_wake(listener, tx, cfg));
        (port, rx)
    }

    fn http(port: u16, method: &str, target: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            s,
            "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    #[test]
    fn decode() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(
            percent_decode("%E8%B5%A3%E5%B7%9E%E4%B8%AD%E5%AD%A6"),
            "赣州中学"
        );
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn query() {
        assert_eq!(query_param("f=abc.txt&a=update&t=1", "f"), "abc.txt");
        assert_eq!(query_param("f=", "f"), "");
        assert_eq!(query_param("err=500%20bad", "err"), "500 bad");
    }

    #[test]
    fn class_filter_matches_python_on_wake() {
        // Python: if file_name and cur and file_name != cur: return
        assert!(wake_passes("", "赣州中学_2025级09班.txt")); // 广播消息必须唤醒
        assert!(wake_passes("赣州中学_2025级09班.txt", "")); // 未建班必须唤醒
        assert!(wake_passes("a.txt", "a.txt")); // 本班消息
        assert!(!wake_passes("b.txt", "a.txt")); // 别班消息 → 忽略
    }

    #[test]
    fn preflight_is_allowed_for_private_network() {
        let (port, _rx) = spawn_bridge("a.txt");
        let r = http(port, "OPTIONS", "/wake?f=a.txt");
        assert!(r.starts_with("HTTP/1.1 204"), "{r}");
        assert!(r.contains("Access-Control-Allow-Private-Network: true"), "{r}");
        assert!(r.contains("Access-Control-Allow-Methods: GET, OPTIONS"), "{r}");
    }

    #[test]
    fn broadcast_wake_reaches_ui() {
        let (port, rx) = spawn_bridge("a.txt");
        let r = http(port, "GET", "/wake?f=&a=connected&t=1");
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(1000)),
            Ok(UiMsg::GoEasyWake { .. })
        ));
    }

    #[test]
    fn files_signal_reaches_ui_as_goeasy_files() {
        let (port, rx) = spawn_bridge("a.txt");
        let r = http(port, "GET", "/wake?f=&a=files&t=1");
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(1000)),
            Ok(UiMsg::GoEasyFiles)
        ));
    }

    #[test]
    fn other_class_wake_is_ignored() {
        let (port, rx) = spawn_bridge("a.txt");
        let _ = http(port, "GET", "/wake?f=b.txt&a=update&text=%E4%BC%AA%E9%80%A0&t=1");
        assert!(rx.recv_timeout(Duration::from_millis(250)).is_err());
    }

    #[test]
    fn bridge_html_forwards_broadcast_text_to_wake() {
        let mut c = crate::config::Config::default();
        c.current_class = "a.txt".into();
        c.goeasy_appkey = "BS-test".into();
        let html = build_bridge_html(&c, 18765, "");
        assert!(html.contains("d.text || \"\""), "订阅页必须把广播正文转给 /wake");
        assert!(html.contains("&text="), "/wake 请求必须带 text 参数");
        assert!(html.contains("enc.length <= 1500"), "必须有超长保护");
    }

    #[test]
    fn bridge_html_reports_periodic_heartbeat() {
        let mut c = crate::config::Config::default();
        c.current_class = "a.txt".into();
        c.goeasy_appkey = "BS-test".into();
        let html = build_bridge_html(&c, 18765, "");
        assert!(html.contains("setInterval"), "订阅页必须有周期心跳");
        assert!(html.contains("/status?"), "心跳必须走 /status（不触发拉取）");
        assert!(html.contains("ok=1&cs="), "心跳必须上报 ok 与连接状态");
        assert!(html.contains("getConnectionStatus"), "心跳必须带 SDK 真实连接状态");
        assert!(
            html.contains("hbFail >= 3"),
            "心跳连续失败必须自关页面（防僵尸无头 Edge）"
        );
    }

    #[test]
    fn doorbell_health_marks() {
        // 初始无心跳 → 不健康
        mark_bridge_dead();
        assert!(!doorbell_healthy());
        // 心跳到达 → 健康
        mark_bridge_alive();
        assert!(doorbell_healthy());
        // 连接失败上报 → 判死
        mark_bridge_dead();
        assert!(!doorbell_healthy());
    }

    #[test]
    fn quick_text_is_sanitized() {
        assert_eq!(sanitize_quick_text("  重要通知：开会  "), Some("重要通知：开会".into()));
        assert_eq!(sanitize_quick_text(""), None);
        assert_eq!(sanitize_quick_text("A"), None);
        // ≤2 字符按无效通知处理（与 gitee::is_valid_notice 口径一致）
        assert_eq!(sanitize_quick_text("ab"), None);
        assert_eq!(sanitize_quick_text("  你好  "), None);
        // 控制字符被剔除后剩 2 字符 → 仍按无效处理；3 字符以上正常通过
        assert_eq!(sanitize_quick_text("a\u{0007}b"), None);
        assert_eq!(sanitize_quick_text("ab\u{0007}c"), Some("abc".into()));
        let long = "通".repeat(900);
        assert_eq!(sanitize_quick_text(&long).unwrap().chars().count(), 600);
    }

    /// 网页端 pubsub.publish 带的 text 应随门铃一起到 UI（仅作预览）。
    #[test]
    fn wake_with_text_carries_preview_to_ui() {
        let (port, rx) = spawn_bridge("a.txt");
        let enc = crate::gitee::encode_segment("重要通知：开会");
        let target = format!("/wake?f=a.txt&a=update&text={enc}&t=1");
        let r = http(port, "GET", &target);
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        match rx.recv_timeout(Duration::from_millis(1000)).ok() {
            Some(UiMsg::GoEasyWake { quick_text }) => {
                assert_eq!(quick_text.as_deref(), Some("重要通知：开会"));
            }
            _ => panic!("期望带预览正文的唤醒消息"),
        }
    }

}
