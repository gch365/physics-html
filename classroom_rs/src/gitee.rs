//! Gitee API：Contents / blobs 通道的读写（对应 GiteeAPI 类与模块级下载函数）。
//!
//! Gitee 已对 raw 端点的程序化访问返回 403，因此下载统一走
//! 「Contents API 拿 sha → blobs API 拿 base64」的通道；大文件走分块 manifest。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use std::time::Duration;

pub const API_BASE: &str = "https://gitee.com/api/v5";

pub struct GiteeApi {
    pub owner: String,
    pub repo: String,
    pub token: String,
    client: ureq::Agent,
}

/// 统一 HTTP 调用（ureq 同步模型，无 tokio runtime）。
/// 返回 (状态码, 响应体)；4xx/5xx 时返回响应体供上层报错，网络错误统一前缀「网络错误」。
fn http_call(
    agent: &ureq::Agent,
    method: &str,
    url: &str,
    token: &str,
    body: Option<String>,
    timeout: Duration,
) -> Result<(u16, String), String> {
    let base = match method {
        "POST" => agent.post(url),
        "PUT" => agent.put(url),
        "DELETE" => agent.delete(url),
        _ => agent.get(url),
    };
    let req = base.set("Authorization", &format!("Bearer {}", token)).timeout(timeout);
    let resp = match body {
        Some(b) => req.set("Content-Type", "application/json").send_string(&b),
        None => req.call(),
    };
    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => return Err(format!("网络错误: {}", e)),
    };
    let status = resp.status();
    let text = read_body_string(resp)?;
    Ok((status, text))
}

/// 读响应体为字符串（into_string 有 10MB 上限，改用 reader 读全量）。
fn read_body_string(resp: ureq::Response) -> Result<String, String> {
    use std::io::Read;
    let mut data = Vec::new();
    resp.into_reader()
        .read_to_end(&mut data)
        .map_err(|e| format!("网络错误: {}", e))?;
    String::from_utf8(data).map_err(|_| "响应体不是 UTF-8".to_string())
}

/// percent-encode 单段路径（urllib.parse.quote(p, safe="") 等价）。
pub fn encode_segment(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

pub fn encode_path(path: &str) -> String {
    path.split('/').map(encode_segment).collect::<Vec<_>>().join("/")
}

pub fn urlencode(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", encode_segment(k), encode_segment(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// 远程目录里的一个文件项。
#[derive(Debug, Clone, Default)]
pub struct RemoteFile {
    pub path: String,
    pub sha: String,
    pub size: i64,
    pub name: String,
    #[allow(dead_code)] // manifest 语义字段（对照 Python 版结构保留）
    pub chunked: bool,
    #[allow(dead_code)]
    pub chunks: i64,
}

impl GiteeApi {
    pub fn new(owner: &str, repo: &str, token: &str) -> Self {
        GiteeApi {
            owner: owner.to_string(),
            repo: repo.to_string(),
            token: token.to_string(),
            client: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(15))
                .timeout(Duration::from_secs(60))
                .build(),
        }
    }

    /// GET/POST/PUT JSON API。HTTPError → Err("API <code>: <body前200字>")。
    pub fn request(
        &self,
        method: &str,
        path: &str,
        params: &[(String, String)],
    ) -> Result<Value, String> {
        let mut url = format!("{}{}", API_BASE, encode_path(path));
        if !params.is_empty() {
            url.push('?');
            url.push_str(&urlencode(params));
        }
        let body = if method != "GET" { Some(String::new()) } else { None };
        let (status, body) = http_call(&self.client, method, &url, &self.token, body, Duration::from_secs(60))?;
        if !(200..300).contains(&status) {
            let snippet: String = body.chars().take(200).collect();
            return Err(format!("API {}: {}", status, snippet));
        }
        if body.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_str(&body).map_err(|e| format!("JSON 解析失败: {}", e))
    }

    /// 测试连接（可选测试下载能力）。返回 (成功?, 信息文本)。
    pub fn test_connection(&self, test_download: bool, download_dir: Option<&std::path::Path>) -> (bool, String) {
        match self.request("GET", &format!("/repos/{}/{}", self.owner, self.repo), &[]) {
            Ok(v) => {
                let mut info = format!(
                    "仓库 {} (private={})",
                    v.get("full_name").and_then(|x| x.as_str()).unwrap_or("?"),
                    v.get("private").and_then(|x| x.as_bool()).unwrap_or(false)
                );
                if !test_download {
                    return (true, info);
                }
                let files = self.list_files("uploads");
                info.push_str(&format!("，uploads {} 个文件", files.len()));
                let small = files.iter().find(|f| f.size < 1024 * 1024);
                match small {
                    None => info.push_str("；下载测试跳过（无小文件）"),
                    Some(f) => match self.download_file_content(&f.path) {
                        Ok(data) => {
                            if let Some(dir) = download_dir {
                                let _ = std::fs::create_dir_all(dir);
                                let _ = std::fs::write(dir.join(&f.name), &data);
                            }
                            info.push_str(&format!(
                                "；下载测试成功 {}（{} 字节）",
                                f.name,
                                data.len()
                            ));
                        }
                        Err(e) => info.push_str(&format!("；下载测试失败: {}", e)),
                    },
                }
                (true, info)
            }
            Err(e) => (false, e),
        }
    }

    /// 列目录：manifest 分块组合并为单文件项，跳过 .chunks 目录。
    pub fn list_files(&self, remote_dir: &str) -> Vec<RemoteFile> {
        let mut out = Vec::new();
        let items = match self.request(
            "GET",
            &format!("/repos/{}/{}/contents/{}", self.owner, self.repo, remote_dir),
            &[],
        ) {
            Ok(v) => v,
            Err(_) => return out,
        };
        let Some(arr) = items.as_array() else { return out };
        for item in arr {
            let name = item.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let path = item.get("path").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let typ = item.get("type").and_then(|x| x.as_str()).unwrap_or("");
            if typ == "dir" {
                if name.ends_with(".chunks") {
                    continue;
                }
                self.list_recursive(&path, &mut out);
                continue;
            }
            if name.ends_with(".manifest.json") {
                // Gitee Contents「列目录」接口不返回 content 字段（只有单文件详情接口返回）。
                // 若直接用列表项 parse_manifest 会因无 content 失败，导致所有分块文件被跳过、
                // 教室端看不到网络U盘文件。这里对无 content 的条目补发一次单文件详情请求。
                let detail = if item.get("content").is_some() {
                    Some(item.clone())
                } else {
                    self.request(
                        "GET",
                        &format!("/repos/{}/{}/contents/{}", self.owner, self.repo, path),
                        &[],
                    )
                    .ok()
                };
                if let Some(detail) = detail {
                    if let Some(m) = parse_manifest(&detail) {
                        if let Some(orig) = m.get("originalName").and_then(|x| x.as_str()) {
                            out.push(RemoteFile {
                                path: format!("{}/{}", remote_dir.trim_end_matches('/'), orig),
                                sha: item.get("sha").and_then(|x| x.as_str()).unwrap_or("").into(),
                                size: m.get("size").and_then(|x| x.as_i64()).unwrap_or(0),
                                name: orig.to_string(),
                                chunked: true,
                                chunks: m.get("chunks").and_then(|x| x.as_i64()).unwrap_or(0),
                            });
                            continue;
                        }
                    }
                }
                // 兜底：manifest 无法解析时保留原始条目，交给 sync_once 的 .manifest.json 分支处理
                out.push(RemoteFile {
                    path,
                    sha: item.get("sha").and_then(|x| x.as_str()).unwrap_or("").into(),
                    size: item.get("size").and_then(|x| x.as_i64()).unwrap_or(0),
                    name,
                    chunked: false,
                    chunks: 0,
                });
                continue;
            }
            out.push(RemoteFile {
                path,
                sha: item.get("sha").and_then(|x| x.as_str()).unwrap_or("").into(),
                size: item.get("size").and_then(|x| x.as_i64()).unwrap_or(0),
                name,
                chunked: false,
                chunks: 0,
            });
        }
        out
    }

    fn list_recursive(&self, remote_dir: &str, out: &mut Vec<RemoteFile>) {
        let items = match self.request(
            "GET",
            &format!("/repos/{}/{}/contents/{}", self.owner, self.repo, remote_dir),
            &[],
        ) {
            Ok(v) => v,
            Err(_) => return,
        };
        let Some(arr) = items.as_array() else { return };
        for item in arr {
            let typ = item.get("type").and_then(|x| x.as_str()).unwrap_or("");
            match typ {
                "dir" => {
                    let p = item.get("path").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    self.list_recursive(&p, out);
                }
                "file" => {
                    out.push(RemoteFile {
                        path: item.get("path").and_then(|x| x.as_str()).unwrap_or("").into(),
                        sha: item.get("sha").and_then(|x| x.as_str()).unwrap_or("").into(),
                        size: item.get("size").and_then(|x| x.as_i64()).unwrap_or(0),
                        name: item.get("name").and_then(|x| x.as_str()).unwrap_or("").into(),
                        chunked: false,
                        chunks: 0,
                    });
                }
                _ => {}
            }
        }
    }

    /// 下载文件内容：manifest 分块 → contents base64 → blobs → raw 兜底。
    pub fn download_file_content(&self, remote_path: &str) -> Result<Vec<u8>, String> {
        // 探测 manifest
        if let Ok(info) = self.request(
            "GET",
            &format!(
                "/repos/{}/{}/contents/{}.manifest.json",
                self.owner, self.repo, remote_path
            ),
            &[],
        ) {
            if let Some(m) = parse_manifest(&info) {
                if m.get("chunks").and_then(|x| x.as_i64()).unwrap_or(0) > 0 {
                    return self.download_chunked(remote_path, &m);
                }
            }
        }
        let info = self.request(
            "GET",
            &format!("/repos/{}/{}/contents/{}", self.owner, self.repo, remote_path),
            &[],
        )?;
        let content_b64 = info
            .get("content")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .replace('\n', "");
        let file_size = info.get("size").and_then(|x| x.as_i64()).unwrap_or(0);

        if !content_b64.is_empty() && file_size > 0 {
            if let Ok(data) = b64_decode_lenient(&content_b64) {
                if data.len() as i64 == file_size {
                    return Ok(data);
                }
            }
        }
        // blobs 通道
        let sha = info.get("sha").and_then(|x| x.as_str()).unwrap_or("");
        if !sha.is_empty() {
            if let Ok(blob) = self.request(
                "GET",
                &format!("/repos/{}/{}/git/blobs/{}", self.owner, self.repo, sha),
                &[],
            ) {
                let b = blob
                    .get("content")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .replace('\n', "");
                if let Ok(data) = b64_decode_lenient(&b) {
                    if file_size == 0 || data.len() as i64 == file_size {
                        return Ok(data);
                    }
                }
            }
        }
        // raw 兜底
        let raw_url = format!(
            "https://gitee.com/{}/{}/raw/master/{}",
            self.owner,
            self.repo,
            encode_path(remote_path)
        );
        self.download_large_file(&raw_url)
    }

    pub fn download_large_file(&self, url: &str) -> Result<Vec<u8>, String> {
        use std::io::Read;
        let resp = self
            .client
            .get(url)
            .set("Authorization", format!("Bearer {}", self.token).as_str())
            .timeout(Duration::from_secs(300))
            .call();
        let resp = match resp {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) => return Err(format!("下载失败 HTTP {}", code)),
            Err(e) => return Err(format!("网络错误: {}", e)),
        };
        let mut data = Vec::new();
        resp.into_reader()
            .read_to_end(&mut data)
            .map_err(|e| format!("网络错误: {}", e))?;
        Ok(data)
    }

    /// 按 manifest 分块下载合并。
    pub fn download_chunked(&self, remote_path: &str, manifest: &Value) -> Result<Vec<u8>, String> {
        let n = manifest.get("chunks").and_then(|x| x.as_i64()).unwrap_or(0);
        if n <= 0 {
            return Err("manifest 缺少 chunks 信息，无法分块下载".into());
        }
        let mut data: Vec<u8> = Vec::new();
        for i in 0..n {
            let chunk_path = format!("{}.chunks/{:05}.bin", remote_path, i);
            let raw_url = format!(
                "https://gitee.com/{}/{}/raw/master/{}",
                self.owner,
                self.repo,
                encode_path(&chunk_path)
            );
            let chunk = self.download_large_file(&raw_url)?;
            data.extend(chunk);
        }
        if let Some(expect) = manifest.get("size").and_then(|x| x.as_i64()) {
            if expect > 0 && data.len() as i64 != expect {
                return Err(format!(
                    "分块合并大小不符：期望 {} 字节，实际 {} 字节",
                    expect,
                    data.len()
                ));
            }
        }
        Ok(data)
    }

    /// 创建/更新文本文件（自动判断 sha → PUT/POST）。
    pub fn create_text_file(
        &self,
        content: &str,
        remote_path: &str,
        branch: &str,
    ) -> Result<Value, String> {
        let encoded = B64.encode(content.as_bytes());
        let mut sha = String::new();
        if let Ok(info) = self.request(
            "GET",
            &format!("/repos/{}/{}/contents/{}", self.owner, self.repo, remote_path),
            &[],
        ) {
            sha = info.get("sha").and_then(|x| x.as_str()).unwrap_or("").to_string();
        }
        let (method, body) = if !sha.is_empty() {
            (
                "PUT",
                json!({
                    "access_token": self.token,
                    "content": encoded,
                    "message": format!("更新文件: {}", remote_path),
                    "branch": branch,
                    "sha": sha,
                }),
            )
        } else {
            (
                "POST",
                json!({
                    "access_token": self.token,
                    "content": encoded,
                    "message": format!("创建文件: {}", remote_path),
                    "branch": branch,
                }),
            )
        };
        let url = format!(
            "{}/repos/{}/{}/contents/{}",
            API_BASE,
            self.owner,
            self.repo,
            encode_path(remote_path)
        );
        let (status, text) =
            http_call(&self.client, method, &url, &self.token, Some(body.to_string()), Duration::from_secs(60))?;
        if !(200..300).contains(&status) {
            let snippet: String = text.chars().take(200).collect();
            return Err(format!("文件操作失败 {}: {}", status, snippet));
        }
        if text.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_str(&text).map_err(|e| format!("JSON 解析失败: {}", e))
    }

    /// 通过 blobs API 下载（fetch_notice_text / raw_download 的降级通道）。
    pub fn blob_download(&self, path: &str, timeout_secs: u64) -> Result<Vec<u8>, String> {
        let token_q = encode_segment(&self.token);
        let info_url = format!(
            "{}/repos/{}/{}/contents/{}?access_token={}",
            API_BASE,
            self.owner,
            self.repo,
            encode_path(path),
            token_q
        );
        let (status, body) = http_call(
            &self.client,
            "GET",
            &info_url,
            &self.token,
            None,
            Duration::from_secs(60),
        )?;
        if !(200..300).contains(&status) {
            return Err(format!("网络错误: HTTP {}", status));
        }
        let info: Value = serde_json::from_str(&body).map_err(|e| format!("JSON 解析失败: {}", e))?;
        let sha = info.get("sha").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if sha.is_empty() {
            return Err(format!("未获取到文件 sha：{}", path));
        }
        let blob_url = format!(
            "{}/repos/{}/{}/git/blobs/{}?access_token={}",
            API_BASE, self.owner, self.repo, sha, token_q
        );
        let (status, body) = http_call(
            &self.client,
            "GET",
            &blob_url,
            &self.token,
            None,
            Duration::from_secs(timeout_secs.max(1)),
        )?;
        if !(200..300).contains(&status) {
            return Err(format!("网络错误: HTTP {}", status));
        }
        let blob: Value = serde_json::from_str(&body).map_err(|e| format!("JSON 解析失败: {}", e))?;
        let content = blob
            .get("content")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .replace('\n', "");
        B64.decode(content.as_bytes()).map_err(|e| format!("base64 解码失败: {}", e))
    }
}

/// base64：补齐 padding 后解码。
fn b64_decode_lenient(s: &str) -> Result<Vec<u8>, String> {
    let mut s = s.to_string();
    let pad = 4 - s.len() % 4;
    if pad != 4 {
        s.push_str(&"=".repeat(pad));
    }
    B64.decode(s.as_bytes()).map_err(|e| e.to_string())
}

/// 从 Contents API 返回的 manifest 文件条目解析 JSON。
pub fn parse_manifest(info: &Value) -> Option<Value> {
    let content_b64 = info.get("content").and_then(|x| x.as_str())?.replace('\n', "");
    if content_b64.is_empty() {
        return None;
    }
    let data = b64_decode_lenient(&content_b64).ok()?;
    serde_json::from_slice(&data).ok()
}

/// "1.2.3" / "v1.2" / "1.2.3-beta" → (主, 次, 修订)。
pub fn parse_version(v: &str) -> (i64, i64, i64) {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let mut parts: Vec<i64> = Vec::new();
    for seg in v.split(['.', '-', '+']) {
        let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
        parts.push(digits.parse().unwrap_or(0));
    }
    while parts.len() < 3 {
        parts.push(0);
    }
    (parts[0], parts[1], parts[2])
}

/// 远端版本是否比本地新。
pub fn app_has_update(local: &str, remote: &str) -> bool {
    parse_version(remote) > parse_version(local)
}

/// 拼 raw URL（自动更新用，14MB exe 也能下）。
pub fn build_raw_url(owner: &str, repo: &str, branch: &str, path: &str) -> String {
    format!(
        "https://gitee.com/{}/{}/raw/{}/{}",
        owner,
        repo,
        branch,
        encode_path(path)
    )
}

/// 拉当前班级通知正文（合并 scheduled_{班级}.txt）；无班级 / 网络异常返回 None。
/// 有效通知的最少字符数：去除首尾空白后 **超过 2 个字符** 才算有效
/// （≤2 个字符视为无效通知，横幅与作业栏都不显示，等价于没有通知）。
pub fn is_valid_notice(t: &str) -> bool {
    t.trim().chars().count() > 2
}

pub fn fetch_notice_text(cfg: &crate::config::Config) -> Option<String> {
    let cls = cfg.current()?;
    let api = GiteeApi::new(&cfg.gitee_owner, &cfg.gitee_repo, &cfg.gitee_token);
    let dir = &cfg.gitee_tongzhi_dir;
    // 新版（v1.0.10 起）：通知正文与定时通知合并为单个 JSON 文件，
    // 一轮拉取从 4 次 API 调用（contents+blobs × 2 文件）降到 2 次，
    // 20 秒轮询下 Gitee API 配额压力直接减半。
    // 文件内容：{"notice": "...", "scheduled": "..."}
    let base = cls.file_name.trim_end_matches(".txt");
    if let Ok(b) = api.blob_download(&format!("{}/{}.json", dir, base), 10) {
        if let Ok(text) = std::str::from_utf8(&b) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                let get_part = |key: &str| -> String {
                    let raw = v.get(key).and_then(|x| x.as_str()).unwrap_or("").trim();
                    if is_valid_notice(raw) {
                        raw.to_string()
                    } else {
                        String::new()
                    }
                };
                let (n, s) = (get_part("notice"), get_part("scheduled"));
                let mut parts = Vec::new();
                if !n.is_empty() {
                    parts.push(n);
                }
                if !s.is_empty() {
                    parts.push(s);
                }
                return Some(if parts.is_empty() {
                    String::new()
                } else {
                    parts.join("\n")
                });
            }
            // JSON 存在但不是合法 JSON：按空通知处理（不再回退旧 txt，避免半新半旧）
            return Some(String::new());
        }
        // 编码异常：按空通知处理
        return Some(String::new());
    }
    // JSON 文件不存在（旧版 notify 页还没写过）：回退旧版双 txt 通道
    let text = match api.blob_download(&format!("{}/{}", dir, cls.file_name), 10) {
        Ok(b) => String::from_utf8_lossy(&b).trim().to_string(),
        Err(_) => return None,
    };
    // 无效通知（≤2 字符）一律按空处理：横幅不弹、作业栏不显示
    let text = if is_valid_notice(&text) {
        text
    } else {
        String::new()
    };
    let sched = match api.blob_download(&format!("{}/scheduled_{}", dir, cls.file_name), 10) {
        Ok(b) => String::from_utf8_lossy(&b).trim().to_string(),
        Err(_) => String::new(),
    };
    let mut parts = Vec::new();
    if !text.is_empty() {
        parts.push(text);
    }
    if !sched.is_empty() {
        parts.push(sched);
    }
    Some(if parts.is_empty() {
        String::new()
    } else {
        parts.join("\n")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_notice_threshold() {
        // ≤2 字符（含纯空白 / 控制字符外空白）无效；3 个字符起有效
        for junk in ["", " ", "  ", "\n", "ab", "12", "　 "] {
            assert!(!is_valid_notice(junk), "junk: {junk:?}");
        }
        assert!(is_valid_notice("abc"));
        assert!(is_valid_notice("  重要通知  "));
    }

    #[test]
    fn encode() {
        assert_eq!(encode_segment("a b/c"), "a%20b%2Fc");
        assert_eq!(encode_segment("赣州中学_2025级09班.txt"), {
            let mut s = String::new();
            for b in "赣州中学_2025级09班.txt".bytes() {
                match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                        s.push(b as char)
                    }
                    _ => s.push_str(&format!("%{:02X}", b)),
                }
            }
            s
        });
        assert!(encode_segment("赣州中学_2025级09班.txt").contains(".txt"));
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("1.2.3"), (1, 2, 3));
        assert_eq!(parse_version("v1.2"), (1, 2, 0));
        assert_eq!(parse_version("1.2.3-beta"), (1, 2, 3));
        assert!(app_has_update("1.0.2", "1.0.10"));
        assert!(!app_has_update("1.0.2", "1.0.2"));
        assert!(!app_has_update("1.0.2", "bad"));
    }

    #[test]
    fn manifest_parse() {
        let m = json!({"content": B64.encode(br#"{"chunks":3,"size":100,"originalName":"a.png"}"#)});
        let p = parse_manifest(&m).unwrap();
        assert_eq!(p.get("chunks").and_then(|x| x.as_i64()), Some(3));
    }
}
