//! 配置读写。
//!
//! 统一放在 `%APPDATA%\CampusFlow\config.json`。
//! 不放在 exe 同级目录：安装在 Program Files 时写不进去，
//! 而且卸载时会被一起删掉。

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const DEFAULT_PORTAL: &str = "http://10.102.250.36";
pub const DEFAULT_PROFILE: &str = "iFudan.stu";
pub const DEFAULT_CHANNEL: &str = "中国电信";

/// 检查方式
pub const MODE_EVENT: &str = "event";
pub const MODE_POLL: &str = "poll";

/// 启动检查：等这个校园网 SSID 出现，最多等多久（秒）
pub const STARTUP_SSID_WAIT_SECS: u64 = 60;
/// 启动检查的 SSID 轮询间隔（毫秒）
pub const STARTUP_SSID_POLL_MS: u64 = 2500;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub username: String,
    pub password: String,
    /// 出口运营商，可以是名称（中国电信）或 id（1）
    pub channel: String,
    pub portal: String,
    pub profile: String,
    pub watch_interval: u64,
    /// "event" = 网络变化时检查；"poll" = 定时轮询
    pub check_mode: String,
    /// 「自动重连」开关的状态。
    /// 不持久化的话重启就回到关闭，开机自启等于白搭。
    pub auto_watch: bool,
    /// relogin = 僵尸会话先注销再认证；login_only = 只认证不注销
    pub on_stale_session: String,
    pub autostart: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            channel: DEFAULT_CHANNEL.to_string(),
            portal: DEFAULT_PORTAL.to_string(),
            profile: DEFAULT_PROFILE.to_string(),
            watch_interval: 30,
            check_mode: MODE_EVENT.to_string(),
            auto_watch: false,
            on_stale_session: "relogin".to_string(),
            autostart: false,
        }
    }
}

impl Config {
    pub fn is_ready(&self) -> bool {
        !self.username.trim().is_empty() && !self.password.is_empty()
    }

    pub fn channel_or_default(&self) -> &str {
        let c = self.channel.trim();
        if c.is_empty() {
            DEFAULT_CHANNEL
        } else {
            c
        }
    }

    /// 归一化检查方式。字段里写了别的值一律当 event。
    pub fn mode(&self) -> &'static str {
        if self.check_mode.eq_ignore_ascii_case(MODE_POLL) {
            MODE_POLL
        } else {
            MODE_EVENT
        }
    }
}

pub fn config_path() -> PathBuf {
    let dir = appdata_dir().join("CampusFlow");
    let _ = fs::create_dir_all(&dir);
    dir.join("config.json")
}

fn appdata_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

pub fn load() -> Config {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

pub fn save(cfg: &Config) -> Result<(), String> {
    let path = config_path();
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

pub fn path_display() -> String {
    config_path().display().to_string()
}
