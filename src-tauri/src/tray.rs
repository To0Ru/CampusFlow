//! 托盘图标 + 右键菜单。
//!
//! 关闭窗口只是收进托盘，后台守护继续跑；真正退出在托盘菜单里。

use std::sync::Arc;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::{fixer, Inner};

/// 返回 `TrayIcon` 句柄，调用方**必须存住它**。
/// 在 Tauri v2 里丢下这个句柄，托盘图标会跟着消失。
pub fn build(app: &tauri::App, inner: Arc<Inner>) -> tauri::Result<TrayIcon> {
    let show = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
    let fix = MenuItem::with_id(app, "fix", "一键修复", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &fix, &sep, &quit])?;

    let inner_for_menu = Arc::clone(&inner);

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("CampusFlow · 校园网自动认证")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "show" => show_window(app),
            "fix" => {
                let inner = Arc::clone(&inner_for_menu);
                std::thread::spawn(move || {
                    let cfg = inner
                        .cfg
                        .lock()
                        .map(|c| c.clone())
                        .unwrap_or_default();
                    if !cfg.is_ready() {
                        inner.bus.push("[!!] 还没配置账号密码");
                        return;
                    }
                    inner.bus.push("[..] ── 托盘触发修复 ──");
                    let sink = inner.bus.sink();
                    fixer::fix(&cfg, &sink);
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.handle().default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    let icon = builder.build(app)?;
    Ok(icon)
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
