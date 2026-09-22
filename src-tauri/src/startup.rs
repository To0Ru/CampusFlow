//! 【任务 B】开机后的一次性检查。
//!
//! 和「自动重连」（任务 A，由状态页那个开关控制的后台常驻守护）是**两件独立的事**。
//! 这个任务只在启动时跑一次，跑完就退。
//!
//! 触发条件：设置里开了「开机自启」。
//!
//! ```text
//! 等 iFudan.stu 出现，最多 60 秒
//!   ├─ 等到了 → 看外网通不通
//!   │    ├─ 通   → 什么都不动（最好的情况）
//!   │    └─ 不通 → 认证一次
//!   └─ 没等到 → 放弃，交给后面的机制
//! ```
//!
//! 为什么要等：开机那一刻 WiFi 往往还没连上，不等的话第一次认证必然失败。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::config::{Config, STARTUP_SSID_WAIT_SECS};
use crate::fixer;
use crate::logbus::LogSink;
use crate::net;

/// 起一个后台线程跑这次检查，不阻塞界面启动。
pub fn spawn(cfg: Config, log: LogSink, busy: Arc<AtomicBool>) {
    thread::spawn(move || {
        // 这个线程是 daemon，进程退出就没了，不需要外部来叫停
        let stop = AtomicBool::new(false);
        let profile = cfg.profile.trim().to_string();

        log(&format!(
            "[..] 启动检查：等待 {profile}（最多 {STARTUP_SSID_WAIT_SECS} 秒）…"
        ));

        if !net::wait_for_ssid(
            &profile,
            Duration::from_secs(STARTUP_SSID_WAIT_SECS),
            &stop,
        ) {
            log(&format!(
                "[..] {STARTUP_SSID_WAIT_SECS} 秒内没等到 {profile}，本次启动检查放弃"
            ));
            return;
        }
        log(&format!("[ok] 已连上 {profile}"));

        // 先看一眼外网，通了就什么都不做
        let (ok, why) = net::check_internet();
        if ok {
            log(&format!("[ok] 外网正常（{why}），无需处理"));
            return;
        }

        // 不通才动手。这里和「自动重连」抢同一个 busy 锁，避免两边同时注销+认证
        if !fixer::try_acquire(&busy) {
            log("[..] 已有认证操作在执行，启动检查让位");
            return;
        }
        log(&format!("[..] 外网不通（{why}），开始认证…"));
        let good = fixer::fix(&cfg, &log);
        fixer::release(&busy);

        if good {
            log("[ok] 启动检查完成，网络已恢复");
        } else {
            log("[!!] 启动检查未能连上，交给后台守护或下次网络变化");
        }
    });
}
