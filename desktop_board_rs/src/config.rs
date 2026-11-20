//! 读点名程序留下的配置：班级名、学生名单、作息模式、课表、通知正文。

use serde_json::Value;
use std::collections::HashMap;

use crate::paths;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub work_mode: String,
    pub current_class: String,
    pub raw_name: String,
    pub students: Vec<String>,
    /// 名单来源说明（值日表窗口里给老师看，避免误以为这就是本班名单）
    pub students_from: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            work_mode: "summer".into(),
            current_class: String::new(),
            raw_name: String::new(),
            students: Vec::new(),
            students_from: String::new(),
        }
    }
}

fn names_of(item: &Value) -> Vec<String> {
    item.get("students")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|s| {
                    s.as_str()
                        .map(|x| x.trim().to_string())
                        .unwrap_or_else(|| s.to_string().trim().to_string())
                })
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub fn load_app_config() -> AppConfig {
    let cfg: Value = std::fs::read_to_string(paths::config_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);

    let cur = cfg
        .get("current_class")
        .map(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| v.to_string())
        })
        .unwrap_or_default();
    let classes: Vec<&Value> = cfg
        .get("class_list")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter(|i| i.is_object()).collect())
        .unwrap_or_default();

    let mut raw_name = String::new();
    let mut students: Vec<String> = Vec::new();
    let mut source = String::new();

    for item in &classes {
        let file_name = item.get("file_name").and_then(|v| v.as_str()).unwrap_or("");
        if file_name == cur {
            raw_name = item
                .get("display_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            students = names_of(item);
            source = if raw_name.is_empty() {
                "当前班级".to_string()
            } else {
                raw_name.clone()
            };
            break;
        }
    }

    if students.is_empty() {
        // 当前班还没导入名单（例如新建的 17 班）：借用第一个已有名单的班，
        // 并在值日表窗口标明来源。
        for item in &classes {
            let cand = names_of(item);
            if !cand.is_empty() {
                students = cand;
                let disp = item
                    .get("display_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                source = format!(
                    "{}（当前班名单为空，暂用）",
                    if disp.is_empty() {
                        "内置名单"
                    } else {
                        disp
                    }
                );
                break;
            }
        }
    }

    let work_mode = if cfg.get("work_time").and_then(|v| v.as_str()) == Some("winter") {
        "winter"
    } else {
        "summer"
    };

    AppConfig {
        work_mode: work_mode.to_string(),
        current_class: cur,
        raw_name,
        students,
        students_from: source,
    }
}

/// 课表：key = (weekday 0=周一, period 1 起)。
pub fn load_courses(current_class: &str) -> HashMap<(i32, i32), String> {
    let dir = paths::schedule_dir();
    let mut files: Vec<std::path::PathBuf> = vec![
        dir.join(format!("class_{}.json", current_class)),
        dir.join("_default_9ban_schedule.json"),
    ];
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut rest: Vec<std::path::PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
            .collect();
        rest.sort();
        files.extend(rest);
    }

    for p in files {
        let Ok(txt) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(d) = serde_json::from_str::<Value>(&txt) else {
            continue;
        };
        let Some(rows) = d.get("schedule").and_then(|v| v.as_array()) else {
            continue;
        };
        let mut out: HashMap<(i32, i32), String> = HashMap::new();
        for r in rows {
            let (Some(wd), Some(period)) = (
                r.get("weekday").and_then(|v| v.as_i64()),
                r.get("period").and_then(|v| v.as_i64()),
            ) else {
                continue;
            };
            let course = r
                .get("course")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            out.insert((wd as i32, period as i32), course);
        }
        if !out.is_empty() {
            return out;
        }
    }
    HashMap::new()
}

/// 读点名程序落盘的最新通知正文，压平成一行供跑马灯使用。
pub fn load_board_notice() -> String {
    let Ok(txt) = std::fs::read_to_string(paths::notice_file()) else {
        return String::new();
    };
    let re = regex::Regex::new(r"\s+").unwrap();
    re.replace_all(txt.trim(), " ").trim().to_string()
}
