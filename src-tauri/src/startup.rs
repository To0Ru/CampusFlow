//! 【任务 B】开机后的一次性检查。
//!
//! 和「自动重连」（任务 A，由状态页那个开关控制的后台常驻守护）是**两件独立的事**。
//! 这个任务只在启动时跑一次，跑完就退。
//!
//! 触发条件：设置里开了「开机自启」。
//!
//! ```text
//! 等校园网就绪，最多 60 秒
//!   ├─ 就绪了 → 直接调 fixer::fix()
//!   │            （它自己会判断公网通不通：通就不动，不通才注销/认证）
//!   └─ 超时   → 放弃，交给后面的机制
//! ```
//!
//! 为什么是「等校园网就绪」而不是「等 iFudan.stu」：
//! SSID 是 WiFi 专有概念，**插网线时不存在**。旧实现会死等到超时，
//! 导致以太网用户的开机检查完全不工作。现在改成看门户可达 + 校园网段兑底，
//! 两种接入方式都覆盖。
//!
//! 另外任务 A 也会被挂在这个任务后面启动（见 `DoneHook`）——
//! 不然任务 A 在开机瞬间就会跑一次注定失败的检查，日志里刷一堆"连门户都访问不到"。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::config::{Config, STARTUP_WAIT_SECS};
use crate::fixer;
use crate::logbus::LogSink;
use crate::net;

/// 本任务结束后要执行的动作。
///
/// 目前用来"等 WiFi 就绪后再启动后台守护"——
/// 直接在这里启动任务 A 会跑早一步。
pub type DoneHook = Box<dyn FnOnce() + Send>;

/// 起一个后台线程跑这次检查，不阻塞界面启动。
///
/// `on_done` 无论等到 SSID 还是超时都会执行。
pub fn spawn(cfg: Config, log: LogSink, busy: Arc<AtomicBool>, on_done: Option<DoneHook>) {
    thread::spawn(move || {
        check(&cfg, &log, &busy);
        if let Some(hook) = on_done {
            hook();
        }
    });
}

fn check(cfg: &Config, log: &LogSink, busy: &AtomicBool) {
    // 这个线程是 daemon，进程退出就没了，不需要外部来叫停
    let stop = AtomicBool::new(false);

    log(&format!(
        "[..] 启动检查：等待校园网就绪（最多 {STARTUP_WAIT_SECS} 秒）…"
    ));

    let (ready, why) = net::wait_for_campus(
        &cfg.portal,
        Duration::from_secs(STARTUP_WAIT_SECS),
        &stop,
    );
    if !ready {
        log(&format!("[..] 本次启动检查放弃：{why}"));
        return;
    }
    log(&format!("[ok] 校园网已就绪（{why}），交给重连流程判断"));

    // 直接调重连工作流。它自己第一件事就是查公网——
    // 通就什么都不做，不通才走注销/认证。这里不必再查一遍。
    if !fixer::try_acquire(busy) {
        log("[..] 已有认证操作在执行，启动检查让位");
        return;
    }
    let good = fixer::fix(cfg, log);
    fixer::release(busy);

    if good {
        log("[ok] 启动检查完成");
    } else {
        log("[!!] 启动检查未能连上，交给后台守护或下次网络变化");
    }
}
