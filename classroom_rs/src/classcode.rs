//! 6 位班级码（对应 Python register_class_code / _fetch_existing_class_code）。
//!
//! 映射登记在 Gitee `tongzhi/class_codes.json`：{ "123456": { fileName, school,
//! displayName, createdBy } }。网页端 https://gch365.github.io/physics-html/notify/
//! 输入 6 位班级码即可向对应班级发送通知（云函数 get_by_code 查 fileName）。

use crate::config::Config;
use crate::gitee::GiteeApi;
use serde_json::{Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

fn code_path(cfg: &Config) -> String {
    format!("{}/class_codes.json", cfg.gitee_tongzhi_dir.trim())
}

fn api(cfg: &Config) -> GiteeApi {
    GiteeApi::new(&cfg.gitee_owner, &cfg.gitee_repo, &cfg.gitee_token)
}

/// 读仓库里的班级码映射；404 / 解析失败视为空表。
fn read_map(cfg: &Config) -> Map<String, Value> {
    match api(cfg).download_file_content(&code_path(cfg)) {
        Ok(data) => serde_json::from_slice::<Value>(&data)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default(),
        Err(_) => Map::new(),
    }
}

/// 查当前班级已登记的 6 位码。
pub fn fetch_class_code(cfg: &Config, file_name: &str) -> Option<String> {
    let map = read_map(cfg);
    for (code, v) in &map {
        if v.get("fileName").and_then(|x| x.as_str()) == Some(file_name) {
            return Some(code.clone());
        }
    }
    None
}

fn random_code(used: &Map<String, Value>) -> String {
    for i in 0..80u128 {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            ^ (std::process::id() as u128)
            ^ (i * 2654435761);
        let c = format!("{:06}", (seed % 1_000_000) as u32);
        if !used.contains_key(&c) {
            return c;
        }
    }
    "000001".to_string()
}

/// 登记班级码：保留已有映射；没有则生成 6 位随机码并写回 Gitee。
/// 写仓库失败只记录警告，仍返回码（本地可显示，网页端可能查不到，可稍后重试）。
pub fn ensure_class_code(cfg: &Config, file_name: &str, display_name: &str) -> String {
    let mut map = read_map(cfg);
    // 先删掉该 fileName 的旧映射，避免一个班级多个码
    let old = map
        .iter()
        .find(|(_, v)| v.get("fileName").and_then(|x| x.as_str()) == Some(file_name))
        .map(|(k, _)| k.clone());
    let code = old.unwrap_or_else(|| random_code(&map));
    let school = display_name.split(|c: char| c.is_ascii_digit()).next().unwrap_or(display_name);
    map.insert(
        code.clone(),
        serde_json::json!({
            "fileName": file_name,
            "school": school,
            "displayName": display_name,
            "createdBy": "pc",
        }),
    );
    let content = serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_default();
    match api(cfg).create_text_file(&content, &code_path(cfg), "master") {
        Ok(_) => crate::logging::info(&format!("班级码 {code} 已登记到 Gitee")),
        Err(e) => crate::logging::warn(&format!("班级码 {code} 登记失败: {e}")),
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_format() {
        let c = random_code(&Map::new());
        assert_eq!(c.len(), 6);
        assert!(c.chars().all(|x| x.is_ascii_digit()));
    }
}
