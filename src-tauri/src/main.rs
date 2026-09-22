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

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

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
            });
            app.manage(AppState(Arc::clone(&inner)));

            if let Err(e) = tray::build(app, Arc::clone(&inner)) {
                // 托盘建不出来不影响主功能，但要留个记录
                bus.push(&format!("[??] 托盘图标创建失败: {e}"));
            }

            bus.push(&format!("[ok] CampusFlow v{} 已启动", env!("CARGO_PKG_VERSION")));
            if !cfg.is_ready() {
                bus.push("[??] 还没配置账号密码，请在「设置」里填一下");
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // 关窗 = 收进托盘，守护继续跑
                api.prevent_close();
                let _ = window.hide();
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
