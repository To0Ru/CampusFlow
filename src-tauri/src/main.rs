// 发布版不要带控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod commands;
mod config;
mod fixer;
mod logbus;
mod net;
mod portal;
mod tray;
mod watcher;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::tray::TrayIcon;
use tauri::Manager;

use crate::logbus::LogBus;
use crate::watcher::{SharedWatcherState, Watcher, WatcherState};

/// 全局共享状态。所有字段要么本身可共享，要么包在 Mutex 里。
pub struct Inner {
    pub cfg: Mutex<config::Config>,
    pub bus: Arc<LogBus>,
    pub watcher: Mutex<Option<Watcher>>,
    pub watcher_state: SharedWatcherState,
    pub busy: AtomicBool,
    /// TrayIcon 句柄必须一直存着，否则托盘图标会被析构掉
    pub tray: Mutex<Option<TrayIcon>>,
    /// 托盘没建成功时，关窗要真的退出，否则窗口一隐藏就再也叫不回来了
    pub tray_ok: AtomicBool,
}

pub struct AppState(pub Arc<Inner>);

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let cfg = config::load();
            let bus = Arc::new(LogBus::new());
            bus.attach(app.handle().clone());

            let inner = Arc::new(Inner {
                cfg: Mutex::new(cfg.clone()),
                bus: Arc::clone(&bus),
                watcher: Mutex::new(None),
                watcher_state: Arc::new(Mutex::new(WatcherState {
                    interval: cfg.watch_interval,
                    ..Default::default()
                })),
                busy: AtomicBool::new(false),
                tray: Mutex::new(None),
                tray_ok: AtomicBool::new(false),
            });
            app.manage(AppState(Arc::clone(&inner)));

            match tray::build(app, Arc::clone(&inner)) {
                Ok(icon) => {
                    // 句柄存进 Inner，随应用存活
                    if let Ok(mut slot) = inner.tray.lock() {
                        *slot = Some(icon);
                    }
                    inner.tray_ok.store(true, Ordering::SeqCst);
                }
                Err(e) => bus.push(&format!("[??] 托盘图标创建失败: {e}（关窗将直接退出）")),
            }

            bus.push(&format!("[ok] CampusFlow v{} 已启动", env!("CARGO_PKG_VERSION")));
            if !cfg.is_ready() {
                bus.push("[??] 还没配置账号密码，请在「设置」里填一下");
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // 托盘可用才收进托盘；否则放行，让用户真能把程序关掉
                let tray_ok = window
                    .app_handle()
                    .state::<AppState>()
                    .0
                    .tray_ok
                    .load(Ordering::SeqCst);
                if tray_ok {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::status,
            commands::logs,
            commands::clear_logs,
            commands::do_fix,
            commands::do_login,
            commands::do_logout,
            commands::get_config,
            commands::save_config,
            commands::watch_control,
            commands::open_portal,
            commands::quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("CampusFlow 启动失败");
}
