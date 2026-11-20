//! 历史记录：内存持有 + 30 秒自动落盘（对应 HistoryManager）。

use crate::config::{load_json, save_json_atomic};
use crate::paths;
use serde_json::{Map, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MAX_HISTORY: usize = 200;

#[derive(Default)]
pub struct HistoryManager {
    inner: Mutex<HistoryInner>,
}

#[derive(Default)]
struct HistoryInner {
    history: Vec<Map<String, Value>>,
    dirty: bool,
    last_save: Option<Instant>,
}

impl HistoryManager {
    pub fn new() -> Self {
        let data: Vec<Map<String, Value>> = load_json(&paths::history_file(), Vec::new());
        HistoryManager {
            inner: Mutex::new(HistoryInner {
                history: data,
                dirty: false,
                last_save: Some(Instant::now()),
            }),
        }
    }

    /// 添加一条点名记录：{name, class, time}。
    pub fn add(&self, name: &str, class: &str) {
        let mut e = Map::new();
        e.insert("name".into(), Value::String(name.to_string()));
        e.insert("class".into(), Value::String(class.to_string()));
        e.insert(
            "time".into(),
            Value::String(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()),
        );
        let mut g = self.inner.lock().unwrap();
        g.history.push(e);
        if g.history.len() > MAX_HISTORY {
            let cut = g.history.len() - MAX_HISTORY;
            g.history.drain(0..cut);
        }
        g.dirty = true;
    }

    pub fn get_all(&self) -> Vec<Map<String, Value>> {
        self.inner.lock().unwrap().history.clone()
    }

    /// 取该班级最近 N 次被点到的姓名（对应 _pick_student 的 recent）。
    pub fn recent_names(&self, class: &str, n: usize) -> Vec<String> {
        let g = self.inner.lock().unwrap();
        g.history
            .iter()
            .filter(|e| e.get("class").and_then(|v| v.as_str()) == Some(class))
            .rev()
            .take(n)
            .filter_map(|e| e.get("name").and_then(|v| v.as_str()).map(String::from))
            .collect()
    }

    pub fn clear(&self) {
        let mut g = self.inner.lock().unwrap();
        g.history.clear();
        g.dirty = true;
    }

    /// 周期调用：脏了且距上次落盘 ≥30 秒则写文件。
    pub fn tick(&self) {
        let need = {
            let mut g = self.inner.lock().unwrap();
            match g.last_save {
                Some(t) if t.elapsed() < Duration::from_secs(30) => false,
                _ => {
                    g.last_save = Some(Instant::now());
                    g.dirty
                }
            }
        };
        if need {
            self.flush();
        }
    }

    pub fn flush(&self) {
        let mut g = self.inner.lock().unwrap();
        if g.dirty {
            if save_json_atomic(&paths::history_file(), &g.history) {
                g.dirty = false;
            }
        }
    }
}
