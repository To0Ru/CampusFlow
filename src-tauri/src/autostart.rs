//! 开机自启：直接写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`。
//!
//! 没有引 tauri-plugin-autostart，就为了少一层依赖和权限配置——
//! 调 reg.exe 已经够了，而且写的是当前用户键，不需要管理员。

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const VALUE_NAME: &str = "CampusFlow";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
pub fn is_enabled() -> bool {
    use std::os::windows::process::CommandExt;

    std::process::Command::new("reg")
        .args(["query", RUN_KEY, "/v", VALUE_NAME])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
pub fn set_enabled(enable: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    if enable {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // 带引号（路径含空格时 Windows 才能正确解析），
        // 并带上 --tray：开机启动时只进托盘不弹窗。
        // 用户双击不带这个参数，所以双击永远会出窗口。
        let value = format!("\"{}\" --tray", exe.display());

        let out = std::process::Command::new("reg")
            .args([
                "add", RUN_KEY, "/v", VALUE_NAME, "/t", "REG_SZ", "/d", &value, "/f",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| e.to_string())?;

        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
    } else {
        // 不存在时 reg delete 会失败，忽略即可
        let _ = std::process::Command::new("reg")
            .args(["delete", RUN_KEY, "/v", VALUE_NAME, "/f"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn is_enabled() -> bool {
    false
}

#[cfg(not(windows))]
pub fn set_enabled(_enable: bool) -> Result<(), String> {
    Err("暂只支持 Windows".to_string())
}
