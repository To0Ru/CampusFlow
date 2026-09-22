//! 联网探测 + WiFi SSID 读取。
//!
//! 探测的关键不是"能不能连上某个站点"，而是**区分"真的通"和"被门户劫持"**。
//! 未认证时网关会把 HTTP 请求 302 到认证页，此时状态码往往还是 200，
//! 所以必须校验返回内容，并且不要跟随跳转。

use std::io::Read;
use std::time::Duration;

use crate::portal::decode_console;

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

/// (探测地址, 期望状态码, 正文必须包含的关键词（大小写不敏感）)
///
/// 只需要一个点：百度。
///
/// 为什么必须是 `www.baidu.com`：
/// - 不带 www 的 `baidu.com` 会 301 跳 `https://www.baidu.com/`，
///   而我们在 Cargo.toml 里关掉了 ureq 的 TLS 特性，跳过去就废物了；
///   再加上这里 `redirects(0)` 不跟跳转，就会把 301 当成“被门户劫持”误报。
/// - 带 www 是 200 直出，不跳转。
///
/// 想加备用点防单站抽风，在这里追一行即可，格式一致。
pub const PROBES: &[(&str, u16, Option<&str>)] = &[
    ("http://www.baidu.com", 200, Some("baidu")),
];

/// 返回 (是否真的能上外网, 说明)。
///
/// 多个探测点全部失败才算断网，避免单个站点抽风造成误判。
pub fn check_internet() -> (bool, String) {
    let mut fails: Vec<String> = Vec::new();

    for (url, want, needle) in PROBES {
        let host = host_of(url);
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
            .try_proxy_from_env(false)
            // 不跟随跳转：被门户接管时才能识别出来
            .redirects(0)
            .build();

        match agent.get(*url).set("User-Agent", USER_AGENT).call() {
            Ok(resp) => {
                let code = resp.status();
                if code != *want {
                    fails.push(format!("{host}: HTTP {code}（疑似门户劫持）"));
                    continue;
                }
                // 光看状态码不够：门户劫持后往往也是 200，得验正文
                if let Some(needle) = needle {
                    let mut body = String::new();
                    let _ = resp.into_reader().take(8192).read_to_string(&mut body);
                    let body_lc = body.to_ascii_lowercase();
                    if !body_lc.contains(&needle.to_ascii_lowercase()) {
                        fails.push(format!("{host}: 内容异常（疑似门户劫持）"));
                        continue;
                    }
                }
                return (true, host.to_string());
            }
            Err(ureq::Error::Status(code, _)) => {
                fails.push(format!("{host}: HTTP {code}（疑似门户劫持）"));
            }
            Err(e) => fails.push(format!("{host}: {}", crate::portal::short(e))),
        }
    }

    (
        false,
        format!(
            "{}/{} 探测点失败（{}）",
            fails.len(),
            PROBES.len(),
            fails.join("；")
        ),
    )
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
}

/// 当前连接的 WLAN SSID。
#[cfg(windows)]
pub fn wifi_ssid() -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let out = std::process::Command::new("netsh")
        .args(["wlan", "show", "interfaces"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;

    let text = decode_console(&out.stdout);
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("SSID") && !l.starts_with("BSSID") {
            if let Some((_, v)) = l.split_once(':') {
                let v = v.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn wifi_ssid() -> Option<String> {
    None
}
