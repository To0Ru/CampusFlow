//! 【任务 A】后台常驻守护线程。
//!
//! 由状态页那个「自动重连」开关控制启停，开关状态持久化在 `config.json` 的 `auto_watch`。
//! 和「启动检查」（`startup.rs`，开机跑一次就退）是两件独立的事。
//!
//! 两种工作方式，由 `config.check_mode` 决定：
//!
//! | 方式 | 行为 |
//! |---|---|
//! | `poll` | 每 `watch_interval` 秒探测一次 |
//! | `event` | 阻塞等 Windows 的网络地址变化事件，变了才查 |
//!
//! ⚠️ `event` 的局限：`NotifyAddrChange` 只在 **IP 地址变化**时触发。
//! 校园网 AC 静默踢会话（IP 不变）不会触发它——界面上有如实标注。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Serialize;

use crate::config::{Config, MODE_POLL};
use crate::fixer;
use crate::logbus::{now_ms, LogSink};
use crate::net;
use crate::netchange;

#[derive(Debug, Clone, Serialize)]
pub struct WatcherState {
    pub running: bool,
    pub interval: u64,
    pub mode: String,
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
            mode: crate::config::MODE_EVENT.to_string(),
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
    /// `run_once_first`：是不是一起来就先查一次。
    /// 手动点开关启动时为 true；被别的操作顺带重启时为 false（刚查过没必要再来）。
    pub fn spawn(
        cfg: Config,
        log: LogSink,
        state: SharedWatcherState,
        busy: Arc<AtomicBool>,
        run_once_first: bool,
    ) -> Self {
        let mode = cfg.mode();
        let interval = cfg.watch_interval.clamp(5, 3600);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_inner = Arc::clone(&stop);

        if let Ok(mut s) = state.lock() {
            s.running = true;
            s.interval = interval;
            s.mode = mode.to_string();
        }

        let handle = thread::spawn(move || {
            if mode == MODE_POLL {
                log(&format!("[..] 自动重连已启动（定时轮询，每 {interval} 秒）"));
            } else {
                log("[..] 自动重连已启动（网络变化时检查）");
            }

            if run_once_first {
                run_once(&cfg, &log, &state, &busy);
            }

            if mode == MODE_POLL {
                let total = Duration::from_secs(interval);
                while !stop_inner.load(Ordering::Relaxed) {
                    netchange::sleep_interruptible(&stop_inner, total);
                    if stop_inner.load(Ordering::Relaxed) {
                        break;
                    }
                    run_once(&cfg, &log, &state, &busy);
                }
            } else {
                while !stop_inner.load(Ordering::Relaxed) {
                    // 返回 false 表示是被 stop 叫停的，直接退出
                    if !netchange::wait_for_change(&stop_inner) {
                        break;
                    }
                    log("[..] 检测到网络变化，开始检查…");
                    run_once(&cfg, &log, &state, &busy);
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

/// 探测一次，掉线就重连。
fn run_once(cfg: &Config, log: &LogSink, state: &SharedWatcherState, busy: &AtomicBool) {
    let (ok, why) = net::check_internet();

    if let Ok(mut s) = state.lock() {
        s.checks += 1;
        s.last_check_ms = Some(now_ms());
        s.last_result = Some(if ok { "ok" } else { "down" }.to_string());
    }
    if ok {
        return;
    }

    log(&format!("[..] 检测到掉线（{why}），开始重连…"));

    // 和「启动检查」/「手动一键连接」抢同一把锁
    if !fixer::try_acquire(busy) {
        log("[..] 已有认证操作在执行，本次跳过");
        return;
    }
    let good = fixer::fix(cfg, log);
    fixer::release(busy);

    if let Ok(mut s) = state.lock() {
        if good {
            s.fixes += 1;
        }
    }
    if good {
        log("[ok] 自动重连成功");
    } else {
        log("[!!] 自动重连失败，稍后重试");
    }
}
