//! 连接编排 + 状态采集。

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

use serde::Serialize;

use crate::config::Config;
use crate::logbus::LogSink;
use crate::net;
use crate::portal::{LoginOutcome, Portal};
use crate::watcher::WatcherState;

/// 认证类操作（启动检查 / 后台守护 / 手动一键连接）同一时刻只允许一个，
/// 否则两个线程可能同时跑“注销 + 认证”，互相把对方的会话搞掉。
pub fn try_acquire(busy: &AtomicBool) -> bool {
    busy.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

pub fn release(busy: &AtomicBool) {
    busy.store(false, Ordering::SeqCst);
}

/// 把网络修好，返回最终是否能上网。
///
/// 核心逻辑（对应用户"必须退出账号重新登录"的痛点）：
///
/// ```text
/// 能看到外网 ? ── 是 ──> 什么都不做
///       │
///       否
///       ├─ 门户说 on  ──> 僵尸会话：先 thirddauth 注销，再走下面的认证
///       └─ 门户说 off ──> 直接走下面的认证
///
/// 认证：firstauth ──> 有 channels ? ──> secondauth(选定运营商) ──> 完成
/// ```
pub fn fix(cfg: &Config, log: &LogSink) -> bool {
    let (ok, why) = net::check_internet();
    if ok {
        log(&format!("[ok] 网络正常（{why}）"));
        return true;
    }
    log(&format!("[..] 公网不通: {why}"));

    let portal = Portal::new(&cfg.portal);

    let ip = match portal.get_ip() {
        Ok(ip) => ip,
        Err(e) => {
            log(&format!("[!!] 连门户都访问不到: {e}"));
            log("     请检查是否已连上 WiFi，或门户地址是否变更。");
            return false;
        }
    };

    // ---- 查门户侧状态 ---- //
    let mut user = cfg.username.clone();
    let mut state = String::new();
    match portal.pre_login(&ip) {
        Ok(p) => {
            state = p.state.clone();
            if !p.username.is_empty() {
                user = p.username.clone();
            }
            let shown = if p.state.is_empty() { "?" } else { p.state.as_str() };
            log(&format!("[..] 门户状态: {shown}  账号: {user}  IP: {ip}"));
        }
        Err(e) => log(&format!("[!!] pre_login 失败: {e}（继续尝试登录）")),
    }

    // ---- 僵尸会话：门户说在线但实际没网 ---- //
    if state == "on" && cfg.on_stale_session == "relogin" {
        log("[..] 门户认为在线但实际无网 -> 注销僵尸会话…");
        if let Err(e) = portal.logout(&ip, &user) {
            log(&format!("[??] 注销请求异常（忽略）: {e}"));
        }
        sleep(Duration::from_millis(1000));
    }

    // ---- 两步认证 ---- //
    log("[..] 提交认证（firstauth → secondauth）…");
    match portal.login(&ip, &cfg.username, &cfg.password, cfg.channel_or_default()) {
        Err(e) => {
            log(&format!("[!!] 认证请求失败: {e}"));
            return false;
        }
        Ok(LoginOutcome::NeedChannel(channels)) => {
            let opts: Vec<String> = channels
                .iter()
                .map(|c| format!("{}={}", c.id, c.name))
                .collect();
            log(&format!(
                "[!!] 账号有多个运营商出口，但没匹配到「{}」",
                cfg.channel_or_default()
            ));
            log(&format!("     可选: {}", opts.join("  ")));
            log("     请在「设置」里修改出口运营商。");
            return false;
        }
        Ok(LoginOutcome::Failed(msg)) => {
            log(&format!("[!!] 认证被拒绝: {msg}"));
            return false;
        }
        Ok(LoginOutcome::Done(s)) => {
            log(&format!(
                "[..] 认证响应: outport={} balance={} duration={} reauth={}",
                s.outport, s.balance, s.duration, s.reauth
            ));
            if !s.outport.is_empty() {
                log(&format!("[..] 运营商: {}", s.outport));
            }
        }
    }

    // ---- 复检 ---- //
    for i in 1..=6 {
        sleep(Duration::from_millis(1500));
        let (ok, why) = net::check_internet();
        if ok {
            log(&format!("[ok] 认证成功，网络已恢复（{why}）"));
            return true;
        }
        log(&format!("[..] 第 {i} 次复检仍未通（{why}）"));
    }

    log("[!!] 认证返回成功，但外网仍不通。可能账号欠费/被限制/需要二次验证。");
    false
}

// ---------------------------------------------------------------------- //
// 状态采集
// ---------------------------------------------------------------------- //

#[derive(Debug, Clone, Serialize)]
pub struct StatusPayload {
    pub ssid: Option<String>,
    pub internet: bool,
    pub probe: String,
    pub ip: Option<String>,
    pub portal_state: Option<String>,
    pub portal_user: Option<String>,
    pub outport: Option<String>,
    pub duration: Option<String>,
    pub balance: Option<String>,
    pub portal_error: Option<String>,
    pub channel: String,
    pub username: String,
    pub ready: bool,
    pub config_path: String,
    pub version: String,
    pub watcher: WatcherState,
}

pub fn collect_status(cfg: &Config, watcher: WatcherState) -> StatusPayload {
    let portal = Portal::new(&cfg.portal);

    let mut ip: Option<String> = None;
    let mut portal_state: Option<String> = None;
    let mut portal_user: Option<String> = None;
    let mut outport: Option<String> = None;
    let mut duration: Option<String> = None;
    let mut balance: Option<String> = None;
    let mut portal_error: Option<String> = None;

    match portal.get_ip() {
        Ok(v) => {
            ip = Some(v.clone());
            match portal.pre_login(&v) {
                Ok(p) => {
                    portal_state = Some(p.state);
                    portal_user = Some(p.username);
                    outport = Some(p.outport);
                    duration = Some(p.duration);
                    balance = Some(p.balance);
                }
                Err(e) => portal_error = Some(e),
            }
        }
        Err(e) => portal_error = Some(e),
    }

    let (internet, probe) = net::check_internet();

    StatusPayload {
        ssid: net::wifi_ssid(),
        internet,
        probe,
        ip,
        portal_state,
        portal_user,
        outport,
        duration,
        balance,
        portal_error,
        channel: cfg.channel_or_default().to_string(),
        username: cfg.username.clone(),
        ready: cfg.is_ready(),
        config_path: crate::config::path_display(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        watcher,
    }
}
