//! 前端可调用的 Tauri 命令。
//!
//! 所有会阻塞的操作（HTTP、netsh、sleep）都扔进 `spawn_blocking`，
//! 否则会把 UI 线程卡死——同步命令在 Tauri 里是跑在主线程上的。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::config::{self, Config};
use crate::fixer::{self, StatusPayload};
use crate::logbus::LogEvent;
use crate::portal::{Channel, LoginOutcome, Portal};
use crate::watcher::Watcher;
use crate::{autostart, AppState, Inner};

// ---------------------------------------------------------------------- //
// 通用
// ---------------------------------------------------------------------- //

#[derive(Debug, Serialize)]
pub struct ActionResult {
    pub ok: bool,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<Channel>>,
}

impl ActionResult {
    fn new(ok: bool, msg: impl Into<String>) -> Self {
        Self {
            ok,
            msg: msg.into(),
            channels: None,
        }
    }
}

/// 同一时刻只允许一个认证类操作，避免并发打架。
fn acquire(inner: &Inner) -> bool {
    inner
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

fn release(inner: &Inner) {
    inner.busy.store(false, Ordering::SeqCst);
}

fn snapshot_cfg(inner: &Inner) -> Config {
    inner
        .cfg
        .lock()
        .map(|c| c.clone())
        .unwrap_or_else(|_| Config::default())
}

// ---------------------------------------------------------------------- //
// 守护线程控制
// ---------------------------------------------------------------------- //

fn watcher_running(inner: &Inner) -> bool {
    inner.watcher.lock().map(|w| w.is_some()).unwrap_or(false)
}

fn stop_watcher(inner: &Inner) {
    if let Ok(mut slot) = inner.watcher.lock() {
        if let Some(mut w) = slot.take() {
            w.stop();
        }
    }
}

fn start_watcher(inner: &Arc<Inner>) {
    let cfg = snapshot_cfg(inner);
    let interval = cfg.watch_interval;
    let sink = inner.bus.sink();
    let state = Arc::clone(&inner.watcher_state);
    let w = Watcher::spawn(cfg, sink, interval, state);
    if let Ok(mut slot) = inner.watcher.lock() {
        *slot = Some(w);
    }
}

// ---------------------------------------------------------------------- //
// 信息类
// ---------------------------------------------------------------------- //

#[derive(Debug, Serialize)]
pub struct AppInfo {
    pub version: String,
    pub config_path: String,
    pub portal: String,
    pub profile: String,
    pub autostart: bool,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    let cfg = snapshot_cfg(&state.0);
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        config_path: config::path_display(),
        portal: cfg.portal,
        profile: cfg.profile,
        autostart: autostart::is_enabled(),
    }
}

#[tauri::command]
pub async fn status(state: State<'_, AppState>) -> Result<StatusPayload, String> {
    let inner = Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = snapshot_cfg(&inner);
        let ws = inner
            .watcher_state
            .lock()
            .map(|w| w.clone())
            .unwrap_or_default();
        fixer::collect_status(&cfg, ws)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn logs(state: State<'_, AppState>) -> Vec<LogEvent> {
    state.0.bus.history()
}

#[tauri::command]
pub fn clear_logs(state: State<'_, AppState>) {
    state.0.bus.clear();
}

// ---------------------------------------------------------------------- //
// 动作类
// ---------------------------------------------------------------------- //

#[tauri::command]
pub async fn do_fix(state: State<'_, AppState>) -> Result<ActionResult, String> {
    let inner = Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || {
        if !acquire(&inner) {
            return ActionResult::new(false, "已有操作在执行中");
        }
        let cfg = snapshot_cfg(&inner);
        if !cfg.is_ready() {
            release(&inner);
            return ActionResult::new(false, "还没配置账号密码，请先在「设置」里填好");
        }

        inner.bus.push("[..] ── 开始手动连接 ──");
        let sink = inner.bus.sink();
        let ok = fixer::fix(&cfg, &sink);
        release(&inner);

        ActionResult::new(ok, if ok { "连接成功" } else { "连接失败，看日志" })
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn do_login(state: State<'_, AppState>) -> Result<ActionResult, String> {
    let inner = Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || {
        if !acquire(&inner) {
            return ActionResult::new(false, "已有操作在执行中");
        }
        let cfg = snapshot_cfg(&inner);
        if !cfg.is_ready() {
            release(&inner);
            return ActionResult::new(false, "还没配置账号密码");
        }

        let portal = Portal::new(&cfg.portal);
        let outcome = (|| -> Result<LoginOutcome, String> {
            let ip = portal.get_ip()?;
            portal.login(&ip, &cfg.username, &cfg.password, cfg.channel_or_default())
        })();

        let result = match outcome {
            Err(e) => {
                inner.bus.push(&format!("[!!] 认证异常: {e}"));
                ActionResult::new(false, e)
            }
            Ok(LoginOutcome::Done(s)) => {
                inner.bus.push(&format!(
                    "[ok] 认证成功: outport={} balance={}",
                    s.outport, s.balance
                ));
                ActionResult::new(true, format!("认证成功 · {}", s.outport))
            }
            Ok(LoginOutcome::NeedChannel(ch)) => {
                let opts: Vec<String> = ch.iter().map(|c| format!("{}={}", c.id, c.name)).collect();
                inner.bus.push(&format!(
                    "[!!] 未匹配到运营商「{}」，可选: {}",
                    cfg.channel_or_default(),
                    opts.join("  ")
                ));
                ActionResult {
                    ok: false,
                    msg: format!("运营商「{}」不在列表里", cfg.channel_or_default()),
                    channels: Some(ch),
                }
            }
            Ok(LoginOutcome::Failed(msg)) => {
                inner.bus.push(&format!("[!!] 认证被拒绝: {msg}"));
                ActionResult::new(false, msg)
            }
        };

        release(&inner);
        result
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn do_logout(state: State<'_, AppState>) -> Result<ActionResult, String> {
    let inner = Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || {
        if !acquire(&inner) {
            return ActionResult::new(false, "已有操作在执行中");
        }
        let cfg = snapshot_cfg(&inner);
        let portal = Portal::new(&cfg.portal);

        let outcome = (|| -> Result<(), String> {
            let ip = portal.get_ip()?;
            // 门户记录里的账号才是真正在线的那个
            let user = portal
                .pre_login(&ip)
                .map(|p| p.username)
                .ok()
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| cfg.username.clone());
            portal.logout(&ip, &user)?;
            Ok(())
        })();

        release(&inner);
        match outcome {
            Ok(()) => {
                inner.bus.push("[ok] 已注销当前会话");
                ActionResult::new(true, "已注销")
            }
            Err(e) => {
                inner.bus.push(&format!("[!!] 注销失败: {e}"));
                ActionResult::new(false, e)
            }
        }
    })
    .await
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------- //
// 配置
// ---------------------------------------------------------------------- //

#[derive(Debug, Clone, Serialize)]
pub struct ConfigView {
    pub username: String,
    pub channel: String,
    pub interval: u64,
    pub portal: String,
    pub profile: String,
    pub has_password: bool,
    pub autostart: bool,
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> ConfigView {
    let cfg = snapshot_cfg(&state.0);
    // 先在移走字段前把需要借整个 cfg 的值算出来
    let channel = cfg.channel_or_default().to_string();
    let has_password = !cfg.password.is_empty();
    ConfigView {
        username: cfg.username,
        channel,
        interval: cfg.watch_interval,
        portal: cfg.portal,
        profile: cfg.profile,
        has_password,
        autostart: autostart::is_enabled(),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct ConfigPatch {
    pub username: Option<String>,
    pub password: Option<String>,
    pub channel: Option<String>,
    pub interval: Option<u64>,
    pub autostart: Option<bool>,
}

#[tauri::command]
pub async fn save_config(
    state: State<'_, AppState>,
    patch: ConfigPatch,
) -> Result<ActionResult, String> {
    let inner = Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || {
        let mut cfg = snapshot_cfg(&inner);
        let mut changed: Vec<&str> = Vec::new();

        if let Some(v) = patch.username {
            let v = v.trim().to_string();
            if v != cfg.username {
                cfg.username = v;
                changed.push("用户名");
            }
        }
        // 密码留空表示不修改
        if let Some(v) = patch.password {
            if !v.is_empty() {
                cfg.password = v;
                changed.push("密码");
            }
        }
        if let Some(v) = patch.channel {
            let v = v.trim().to_string();
            if v != cfg.channel {
                cfg.channel = v;
                changed.push("运营商");
            }
        }
        if let Some(v) = patch.interval {
            let v = v.clamp(5, 3600);
            if v != cfg.watch_interval {
                cfg.watch_interval = v;
                changed.push("检查间隔");
            }
        }
        if let Some(v) = patch.autostart {
            if v != cfg.autostart {
                cfg.autostart = v;
                changed.push("开机自启");
            }
        }

        if !cfg.is_ready() {
            return ActionResult::new(false, "用户名和密码都不能为空");
        }

        if let Err(e) = config::save(&cfg) {
            inner.bus.push(&format!("[!!] 保存配置失败: {e}"));
            return ActionResult::new(false, e);
        }

        if patch.autostart.is_some() {
            if let Err(e) = autostart::set_enabled(cfg.autostart) {
                inner.bus.push(&format!("[??] 设置开机自启失败: {e}"));
            }
        }

        if let Ok(mut slot) = inner.cfg.lock() {
            *slot = cfg.clone();
        }

        // 守护线程持有的是启动时的配置副本，改了配置要重启才生效
        let restart = watcher_running(&inner);
        if restart {
            stop_watcher(&inner);
        }
        if restart {
            start_watcher(&inner);
            inner.bus.push("[..] 守护已按新配置重启");
        }

        let msg = if changed.is_empty() {
            "没有变化".to_string()
        } else {
            format!("已保存: {}", changed.join("、"))
        };
        inner.bus.push(&format!("[ok] {msg}"));
        ActionResult::new(true, msg)
    })
    .await
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------- //
// 守护开关
// ---------------------------------------------------------------------- //

#[tauri::command]
pub async fn watch_control(
    state: State<'_, AppState>,
    action: String,
    interval: Option<u64>,
) -> Result<ActionResult, String> {
    let inner = Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || {
        let new_iv = interval.map(|v| v.clamp(5, 3600));

        // 不管哪个动作，先把间隔落盘
        if let Some(iv) = new_iv {
            let snapshot = if let Ok(mut c) = inner.cfg.lock() {
                c.watch_interval = iv;
                Some((*c).clone())
            } else {
                None
            };
            if let Some(cfg) = snapshot {
                let _ = config::save(&cfg);
            }
        }

        match action.as_str() {
            "start" => {
                if watcher_running(&inner) {
                    let cur = inner.watcher_state.lock().map(|s| s.interval).unwrap_or(0);
                    if new_iv.is_none() || new_iv == Some(cur) {
                        return ActionResult::new(true, "自动连接已在运行");
                    }
                    stop_watcher(&inner);
                    inner.bus.push("[..] 检查间隔变了，自动连接重启中…");
                }
                start_watcher(&inner);
                ActionResult::new(true, "自动连接已启动")
            }
            "stop" => {
                if !watcher_running(&inner) {
                    return ActionResult::new(true, "自动连接本来就没开");
                }
                stop_watcher(&inner);
                inner.bus.push("[..] 自动连接已停止");
                ActionResult::new(true, "自动连接已停止")
            }
            // 只改间隔：没在跑就单纯存下配置，在跑就重启让它生效
            "set" => {
                if watcher_running(&inner) {
                    stop_watcher(&inner);
                    start_watcher(&inner);
                    ActionResult::new(true, format!("检查间隔已改为 {} 秒", new_iv.unwrap_or(30)))
                } else {
                    ActionResult::new(true, "已保存")
                }
            }
            _ => ActionResult::new(false, "action 必须是 start / stop / set"),
        }
    })
    .await
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------- //
// 杂项
// ---------------------------------------------------------------------- //

#[tauri::command]
pub fn open_portal(state: State<'_, AppState>) -> Result<(), String> {
    let cfg = snapshot_cfg(&state.0);
    let base = cfg.portal.trim().trim_end_matches('/');
    let url = format!("{base}/#/login?wlanuserfirsturl=http://www.msftconnecttest.com/redirect");
    open_url(&url)
}

/// 关闭窗口 = 收进托盘；真正退出走这里。
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[cfg(windows)]
fn open_url(url: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // start 的第一个参数是窗口标题，必须留空占位
    std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn open_url(url: &str) -> Result<(), String> {
    Err(format!("暂不支持当前平台，请手动打开 {url}"))
}
