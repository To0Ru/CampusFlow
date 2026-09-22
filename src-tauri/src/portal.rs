//! 认证门户协议实现。
//!
//! 全部来自对 <http://10.102.250.36> 前端 JS（Vue SPA）的逆向：
//!
//! ```text
//! GET  /api/v1/ip         -> {"code":200,"data":"10.115.17.191"}
//! POST /api/v1/pre_login  {"getuseronlinestate":"on_or_off","user_ipadress":ip}
//! POST /api/v1/login      认证/注销共用一个接口，靠 pagesign 区分
//!     firstauth   channel="_GET"  -> 校验账号密码；若账号有多个出口会返回 channels
//!     secondauth  channel="1"     -> 选定运营商后真正下发认证
//!     thirdauth   channel="0"     -> 关闭网络
//!     thirddauth  channel="0"     -> 「注销」按钮
//! ```
//!
//! 两个必须记住的坑：
//!
//! 1. **请求体必须是紧凑 JSON。** 门户后端做的是朴素子串匹配，
//!    冒号后多一个空格就解析失败，永远返回 `useronlinestate:"off"`。
//!    `serde_json::to_string` 默认就是紧凑的，所以这里天然安全——
//!    但千万别改成 `to_string_pretty`。
//! 2. **响应是 GBK 编码**，不能直接 `from_utf8`。

use std::io::Read;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::config::DEFAULT_CHANNEL;

const TIMEOUT: Duration = Duration::from_secs(8);
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

#[derive(Debug, Clone, Default, Serialize)]
pub struct PreLogin {
    pub state: String,
    pub username: String,
    pub balance: String,
    pub duration: String,
    pub outport: String,
    pub ip: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthSuccess {
    pub username: String,
    pub outport: String,
    pub balance: String,
    pub duration: String,
    pub reauth: bool,
}

#[derive(Debug, Clone)]
pub enum LoginOutcome {
    Done(AuthSuccess),
    /// 账号有多个运营商出口，且配置里选的那个不在列表里
    NeedChannel(Vec<Channel>),
    Failed(String),
}

pub struct Portal {
    base: String,
    agent: ureq::Agent,
}

impl Portal {
    pub fn new(base: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(TIMEOUT)
            // 门户在校园网内，必须直连，绝不能走系统代理
            .try_proxy_from_env(false)
            .build();
        Self {
            base: base.trim().trim_end_matches('/').to_string(),
            agent,
        }
    }

    // ---------------------------------------------------------------- //
    // 底层
    // ---------------------------------------------------------------- //

    fn post(&self, path: &str, payload: &Value) -> Result<Value, String> {
        let url = format!("{}{}", self.base, path);

        // 紧凑 JSON —— 见文件头注释，这里不能改
        let body = serde_json::to_string(payload).map_err(|e| e.to_string())?;

        let sent = self
            .agent
            .post(&url)
            .set("Content-Type", "application/json;charset=gbk")
            .set("Accept", "application/json, text/plain, */*")
            .set("User-Agent", USER_AGENT)
            .send_string(&body);

        let resp = match sent {
            Ok(r) => r,
            // 非 2xx 也把 body 读出来，门户的错误信息在里面
            Err(ureq::Error::Status(_, r)) => r,
            Err(e) => return Err(format!("请求 {path} 失败: {}", short(e))),
        };

        let text = read_text(resp)?;
        serde_json::from_str(&text).map_err(|e| {
            format!(
                "{path} 返回的不是 JSON: {e}；原文前 200 字: {}",
                truncate(&text, 200)
            )
        })
    }

    pub fn get_ip(&self) -> Result<String, String> {
        let url = format!("{}/api/v1/ip", self.base);
        let resp = self
            .agent
            .get(&url)
            .set("Accept", "application/json, text/plain, */*")
            .set("User-Agent", USER_AGENT)
            .call()
            .map_err(|e| format!("获取 IP 失败: {}", short(e)))?;

        let text = read_text(resp)?;
        let v: Value =
            serde_json::from_str(&text).map_err(|e| format!("IP 接口返回异常: {e}"))?;

        v.get("data")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("获取 IP 失败，返回: {}", truncate(&text, 120)))
    }

    // ---------------------------------------------------------------- //
    // 业务
    // ---------------------------------------------------------------- //

    pub fn pre_login(&self, ip: &str) -> Result<PreLogin, String> {
        let v = self.post(
            "/api/v1/pre_login",
            &json!({
                "getuseronlinestate": "on_or_off",
                "user_ipadress": ip,
            }),
        )?;

        if v.get("code").and_then(Value::as_i64) != Some(200) {
            return Err(format!("pre_login 失败: {}", compact(&v)));
        }

        let d = v.get("data").cloned().unwrap_or(Value::Null);
        Ok(PreLogin {
            state: str_of(&d, "useronlinestate"),
            username: str_of(&d, "username"),
            balance: str_of(&d, "balance"),
            duration: str_of(&d, "duration"),
            outport: str_of(&d, "outport"),
            ip: str_of(&d, "usripadd"),
        })
    }

    fn payload(ip: &str, user: &str, pass: &str, channel: &str, pagesign: &str,
               ifautologin: &str) -> Value {
        json!({
            "username": user,
            "password": pass,
            "ifautologin": ifautologin,
            "channel": channel,
            "pagesign": pagesign,
            "usripadd": ip,
        })
    }

    /// 第一步：校验账号密码。多出口账号会拿到 channels 列表（此时**尚未认证**）。
    pub fn first_auth(&self, ip: &str, user: &str, pass: &str) -> Result<Value, String> {
        self.post(
            "/api/v1/login",
            &Self::payload(ip, user, pass, "_GET", "firstauth", "0"),
        )
    }

    /// 第二步：带上选定的运营商通道，真正下发认证。
    pub fn second_auth(&self, ip: &str, user: &str, pass: &str,
                       channel: &str) -> Result<Value, String> {
        let pagesign = if channel == "0" { "thirdauth" } else { "secondauth" };
        self.post(
            "/api/v1/login",
            &Self::payload(ip, user, pass, channel, pagesign, "0"),
        )
    }

    /// 注销当前会话（对应前端「注销」按钮）。
    pub fn logout(&self, ip: &str, user: &str) -> Result<Value, String> {
        self.post(
            "/api/v1/login",
            &Self::payload(ip, user, "123", "0", "thirddauth", "1"),
        )
    }

    /// 完整两步认证。
    pub fn login(&self, ip: &str, user: &str, pass: &str,
                 wanted: &str) -> Result<LoginOutcome, String> {
        let r1 = self.first_auth(ip, user, pass)?;
        if r1.get("code").and_then(Value::as_i64) != Some(200) {
            return Ok(LoginOutcome::Failed(describe(&r1)));
        }

        let data = r1.get("data").cloned().unwrap_or(Value::Null);
        let channels = parse_channels(&data);

        // 没有 channels 说明第一步就已经认证成功了
        if channels.is_empty() {
            return Ok(LoginOutcome::Done(parse_success(&data)));
        }

        let target = if wanted.trim().is_empty() {
            DEFAULT_CHANNEL
        } else {
            wanted.trim()
        };
        let chosen = channels
            .iter()
            .find(|c| c.id == target || c.name == target)
            .cloned();

        let Some(chosen) = chosen else {
            return Ok(LoginOutcome::NeedChannel(channels));
        };

        let r2 = self.second_auth(ip, user, pass, &chosen.id)?;
        if r2.get("code").and_then(Value::as_i64) != Some(200) {
            return Ok(LoginOutcome::Failed(describe(&r2)));
        }

        let d2 = r2.get("data").cloned().unwrap_or(Value::Null);
        let mut ok = parse_success(&d2);
        if ok.outport.is_empty() {
            ok.outport = chosen.name.clone();
        }
        Ok(LoginOutcome::Done(ok))
    }
}

// ---------------------------------------------------------------------- //
// 解析辅助
// ---------------------------------------------------------------------- //

pub fn str_of(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn parse_channels(data: &Value) -> Vec<Channel> {
    let Some(arr) = data.get("channels").and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|c| {
            let id = str_of(c, "id");
            if id.is_empty() {
                return None;
            }
            Some(Channel {
                id,
                name: str_of(c, "name"),
            })
        })
        .collect()
}

fn parse_success(data: &Value) -> AuthSuccess {
    AuthSuccess {
        username: str_of(data, "username"),
        outport: str_of(data, "outport"),
        balance: str_of(data, "balance"),
        duration: str_of(data, "duration"),
        reauth: data.get("reauth").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// 从失败响应里抽出给人看的错误信息。
pub fn describe(v: &Value) -> String {
    let d = v.get("data").unwrap_or(&Value::Null);
    let text = str_of(d, "text");
    if !text.is_empty() {
        return text;
    }
    let msg = str_of(v, "message");
    let code = v.get("code").map(|c| c.to_string()).unwrap_or_default();
    if msg.is_empty() {
        format!("code={code} {}", compact(v))
    } else {
        format!("code={code} {msg}")
    }
}

fn compact(v: &Value) -> String {
    truncate(&v.to_string(), 200)
}

fn truncate(s: &str, max: usize) -> String {
    let t: String = s.chars().take(max).collect();
    if t.chars().count() < s.chars().count() {
        format!("{t}…")
    } else {
        t
    }
}

pub fn short(e: ureq::Error) -> String {
    truncate(&e.to_string(), 60)
}

// ---------------------------------------------------------------------- //
// 编解码
// ---------------------------------------------------------------------- //

fn read_text(resp: ureq::Response) -> Result<String, String> {
    let ct = resp.header("Content-Type").map(str::to_string);
    let mut buf = Vec::new();
    resp.into_reader()
        .take(1 << 20) // 1 MiB 上限，防止意外拉一个大文件
        .read_to_end(&mut buf)
        .map_err(|e| format!("读取响应失败: {e}"))?;
    Ok(decode(&buf, ct.as_deref()))
}

/// 门户返回 GBK；万一以后改成 UTF-8 也能兼容。
fn decode(bytes: &[u8], content_type: Option<&str>) -> String {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    if ct.contains("gbk") || ct.contains("gb2312") || ct.contains("gb18030") {
        return encoding_rs::GBK.decode(bytes).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::GBK.decode(bytes).0.into_owned(),
    }
}

/// 解码控制台命令的输出（中文 Windows 是 cp936）。
pub fn decode_console(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::GBK.decode(bytes).0.into_owned(),
    }
}
