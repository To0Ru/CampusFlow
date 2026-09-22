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

pub struct Probe {
    pub url: &'static str,
    /// 期望状态码
    pub want: u16,
    /// 正文必须包含的关键词（大小写不敏感）
    pub needle: Option<&'static str>,
    /// 允许跳转到这个域名（含子域）。HTTP→HTTPS 升级用。
    pub ok_redirect: Option<&'static str>,
}

/// 探测点。
///
/// **必须用带 `www` 的 `http://www.baidu.com`：**
/// - 不带 www 的 `baidu.com` 会 301 跳 https；
/// - 带 www 的用浏览器 UA 请求会 302 跳 `https://www.baidu.com/`
///   （百度对浏览器 UA 做 HTTP→HTTPS 强制升级；curl 的 UA 才会给 200）。
///
/// 这两种跳转都要当成“网络正常”——它们跳的是百度自己。
/// 判断依据是 **Location 指向哪**，不是状态码：
/// 跳去 `*.baidu.com` 就算通，跳去别的（比如门户 `10.102.250.36`）就是劫持。
pub const PROBES: &[Probe] = &[Probe {
    url: "http://www.baidu.com",
    want: 200,
    needle: Some("baidu"),
    ok_redirect: Some("baidu.com"),
}];

/// 返回 (是否真的能上外网, 说明)。
pub fn check_internet() -> (bool, String) {
    let mut fails: Vec<String> = Vec::new();

    for probe in PROBES {
        let host = host_of(probe.url);
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
            .try_proxy_from_env(false)
            // 不跟随跳转：只有自己看 Location 才能分辨是升级还是劫持
            .redirects(0)
            .build();

        match agent.get(probe.url).set("User-Agent", USER_AGENT).call() {
            Ok(resp) => {
                let code = resp.status();
                if code != probe.want {
                    fails.push(format!("{host}: HTTP {code}（疑似门户劫持）"));
                    continue;
                }
                // 光看状态码不够：门户劫持后往往也是 200，得验正文
                if let Some(needle) = probe.needle {
                    let mut body = String::new();
                    let _ = resp.into_reader().take(8192).read_to_string(&mut body);
                    if !body.to_ascii_lowercase().contains(&needle.to_ascii_lowercase()) {
                        fails.push(format!("{host}: 内容异常（疑似门户劫持）"));
                        continue;
                    }
                }
                return (true, host.to_string());
            }
            Err(ureq::Error::Status(code, resp)) => {
                if is_benign_redirect(probe, &resp, host) {
                    return (true, host.to_string());
                }
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

/// 跳转目标是不是"自己人"（HTTP→HTTPS 升级），而不是跳到门户。
fn is_benign_redirect(probe: &Probe, resp: &ureq::Response, probe_host: &str) -> bool {
    let Some(suffix) = probe.ok_redirect else {
        return false;
    };
    let Some(loc) = resp.header("Location") else {
        return false;
    };
    // 相对跳转 = 还在同一个域，放行
    let target = if loc.contains("://") {
        host_of(loc)
    } else {
        probe_host
    };
    target == suffix || target.ends_with(&format!(".{suffix}"))
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
