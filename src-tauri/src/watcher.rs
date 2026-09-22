//! 后台守护线程：定时探测，掉线自动修复。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Serialize;

use crate::config::Config;
use crate::fixer;
use crate::logbus::{now_ms, LogSink};
use crate::net;

#[derive(Debug, Clone, Serialize)]
pub struct WatcherState {
    pub running: bool,
    pub interval: u64,
    pub checks: u64,
    pub fixes: u64,
    pub last_check_ms: Option<u64>,
    pub last_result: Option<String>,
}

impl Default for WatcherState {
    fn default() -> Self {
        Self {
            running: false,
            interval: 30,
            checks: 0,
            fixes: 0,
            last_check_ms: None,
            last_result: None,
        }
    }
}

pub type SharedWatcherState = Arc<Mutex<WatcherState>>;

pub struct Watcher {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Watcher {
    pub fn spawn(cfg: Config, log: LogSink, interval: u64, state: SharedWatcherState) -> Self {
        let interval = interval.clamp(5, 3600);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_inner = Arc::clone(&stop);

        if let Ok(mut s) = state.lock() {
            s.running = true;
            s.interval = interval;
        }

        let handle = thread::spawn(move || {
            log(&format!("[..] 自动守护已启动，每 {interval}s 检查一次"));

            while !stop_inner.load(Ordering::Relaxed) {
                let (ok, why) = net::check_internet();

                if let Ok(mut s) = state.lock() {
                    s.checks += 1;
                    s.last_check_ms = Some(now_ms());
                    s.last_result = Some(if ok { "ok" } else { "down" }.to_string());
                }

                if !ok {
                    log(&format!("[..] 检测到掉线（{why}），开始修复…"));
                    let good = fixer::fix(&cfg, &log);
                    if let Ok(mut s) = state.lock() {
                        if good {
                            s.fixes += 1;
                        }
                    }
                    if good {
                        log("[ok] 自动修复成功");
                    } else {
                        log("[!!] 自动修复失败，稍后重试");
                    }
                }

                // 分片等待，保证能及时响应停止指令
                let mut waited: u64 = 0;
                let total = interval * 1000;
                while waited < total && !stop_inner.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(200));
                    waited += 200;
                }
            }

            if let Ok(mut s) = state.lock() {
                s.running = false;
            }
        });

        Self {
            stop,
            handle: Some(handle),
        }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
