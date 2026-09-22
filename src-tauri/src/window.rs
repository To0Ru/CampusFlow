//! 主窗口的创建与显示。
//!
//! 窗口**不预建**（`tauri.conf.json` 里已经去掉了 `app.windows`），而是运行时按需创建：
//!
//! | 启动方式 | 窗口 |
//! |---|---|
//! | 用户双击（无参数） | 直接创建并显示 |
//! | 开机自启（带 `--tray`） | 不创建，只留托盘图标 |
//!
//! 关闭窗口时是**销毁**而不是隐藏——这样 WebView2 那三百多 MB 才会真的还回去。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "main";

/// 窗口已经存在就显示并聚焦，不存在就创建。
pub fn ensure(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }
    open(app)
}

/// 无条件新建窗口。
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    let w = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::default())
        .title("CampusFlow")
        .inner_size(430.0, 680.0)
        .min_inner_size(390.0, 540.0)
        .resizable(true)
        .center()
        .build()?;
    let _ = w.show();
    let _ = w.set_focus();
    Ok(())
}

/// 销毁窗口。返回 true 表示确实销毁了一个。
pub fn close(app: &AppHandle) -> bool {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.destroy();
        true
    } else {
        false
    }
}
