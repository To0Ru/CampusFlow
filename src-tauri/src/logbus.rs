//! 日志总线：环形历史 + 向所有窗口广播 Tauri 事件。
//!
//! 时间戳存 **epoch 毫秒**，由前端用 `new Date(ms)` 转本地时间显示。
//! 这样后端不用引 chrono 之类的时区库。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub const EVENT_NAME: &str = "cf-log";
const MAX_HISTORY: usize = 500;

#[derive(Debug, Clone, Serialize)]
pub struct LogEvent {
    /// epoch 毫秒
    pub ts: u64,
    pub level: String,
    pub msg: String,
}

/// 可以跨线程共享的日志回调。
pub type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

pub struct LogBus {
    history: Mutex<VecDeque<LogEvent>>,
    app: Mutex<Option<AppHandle>>,
}

impl LogBus {
    pub fn new() -> Self {
        Self {
            history: Mutex::new(VecDeque::with_capacity(MAX_HISTORY)),
            app: Mutex::new(None),
        }
    }

    /// 窗口就绪后再挂上 AppHandle。
    pub fn attach(&self, app: AppHandle) {
        if let Ok(mut slot) = self.app.lock() {
            *slot = Some(app);
        }
    }

    /// 拿一个可以丢给后台线程用的日志函数。
    pub fn sink(self: &Arc<Self>) -> LogSink {
        let me = Arc::clone(self);
        Arc::new(move |msg: &str| me.push(msg))
    }

    pub fn push(&self, msg: &str) {
        let ev = LogEvent {
            ts: now_ms(),
            level: level_of(msg).to_string(),
            msg: msg.to_string(),
        };

        if let Ok(mut h) = self.history.lock() {
            while h.len() >= MAX_HISTORY {
                h.pop_front();
            }
            h.push_back(ev.clone());
        }

        if let Ok(slot) = self.app.lock() {
            if let Some(app) = slot.as_ref() {
                let _ = app.emit(EVENT_NAME, ev);
            }
        }
    }

    pub fn history(&self) -> Vec<LogEvent> {
        self.history
            .lock()
            .map(|h| h.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn clear(&self) {
        if let Ok(mut h) = self.history.lock() {
            h.clear();
        }
    }
}

impl Default for LogBus {
    fn default() -> Self {
        Self::new()
    }
}

/// 从 `[xx]` 前缀推断日志等级，前端据此上色。
fn level_of(msg: &str) -> &'static str {
    if msg.starts_with("[ok]") {
        "ok"
    } else if msg.starts_with("[!!]") {
        "error"
    } else if msg.starts_with("[??]") {
        "warn"
    } else if msg.starts_with("[..]") {
        "info"
    } else {
        "info"
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
