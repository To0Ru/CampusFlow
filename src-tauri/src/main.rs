// 发布版不要带控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod commands;
mod config;
mod fixer;
mod logbus;
mod net;
mod netchange;
mod portal;
mod startup;
mod tray;
mod watcher;
mod window;

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
    /// 认证类操作的互斥锁（启动检查 / 后台守护 / 手动一键连接 共用）
    pub busy: Arc<AtomicBool>,
    /// TrayIcon 句柄必须一直存着，否则托盘图标会被析构掉
    pub tray: Mutex<Option<TrayIcon>>,
    /// 托盘没建成功时，关窗要真的退出，否则窗口一销毁就再也叫不回来了
    pub tray_ok: AtomicBool,
}

pub struct AppState(pub Arc<Inner>);

/// 启动参数里带 `--tray` 吗？
///
/// 只有「开机自启」写进注册表的那条命令会带这个参数，用户双击不会。
/// 用它来区分「开机静默进托盘」和「用户主动打开」。
fn launched_as_tray() -> bool {
    std::env::args().any(|a| a == "--tray")
}

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
                    mode: cfg.mode().to_string(),
                    ..Default::default()
                })),
                busy: Arc::new(AtomicBool::new(false)),
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

            bus.push(&format!(
                "[ok] CampusFlow v{} 已启动",
                env!("CARGO_PKG_VERSION")
            ));

            // ---------------- 窗口 ----------------
            // 窗口不预建，这里决定要不要开。
            // 没配置过账号密码时无论如何都要开——不然用户打开一片空白，不知道要干嘛。
            if !cfg.is_ready() {
                bus.push("[??] 还没配置账号密码，请先在「设置」里填好");
                let _ = window::ensure(app.handle());
            } else if launched_as_tray() {
                bus.push("[..] 以托盘模式启动（开机自启），窗口未打开");
            } else {
                let _ = window::ensure(app.handle());
            }

            // ---------------- 任务 A：后台常驻守护 ----------------
            if cfg.is_ready() && cfg.auto_watch {
                commands::start_watcher(&inner);
            }

            // ---------------- 任务 B：开机后的一次性检查 ----------------
            // 跟着「开机自启」走：开了自启就执行
            if cfg.is_ready() && cfg.autostart {
                startup::spawn(cfg.clone(), bus.sink(), Arc::clone(&inner.busy));
            }

            Ok(())
        })
        .on_window_event(|win, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = win.app_handle();
                let tray_ok = app.state::<AppState>().0.tray_ok.load(Ordering::SeqCst);
                if tray_ok {
                    // 不隐藏，直接销毁，把 WebView2 那三百多 MB 还回去。
                    // 托盘图标还在，点一下就重建。
                    api.prevent_close();
                    let _ = window::close(app);
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
