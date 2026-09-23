//! 联网探测 + WiFi SSID 读取。
//!
//! 探测的关键不是"能不能连上某个站点"，而是**区分"真的通"和"被门户劫持"**。
//! 未认证时网关会把 HTTP 请求 302 到认证页，此时状态码往往还是 200，
//! 所以必须校验返回内容，并且不要跟随跳转。

use std::io::Read;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
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

        let sent = agent.get(probe.url).set("User-Agent", USER_AGENT).call();

        // ureq 对 3xx 的处理会随配置变化：redirects(0) 时 302 有可能走 Ok，
        // 也有可能包在 Err(Status) 里。两个分支合并处理，免得再踩一次。
        let resp = match sent {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(e) => {
                fails.push(format!("{host}: {}", crate::portal::short(e)));
                continue;
            }
        };

        let code = resp.status();

        // 3xx：不是看状态码，是看它跳去哪
        if (300..400).contains(&code) {
            if is_benign_redirect(probe, &resp, host) {
                return (true, host.to_string());
            }
            let loc = resp.header("Location").unwrap_or("(无 Location)");
            fails.push(format!("{host}: HTTP {code} → {loc}（疑似门户劫持）"));
            continue;
        }

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

/// 把门户地址解成 `SocketAddr`。
fn portal_socket_addr(portal: &str) -> Option<SocketAddr> {
    let rest = portal.split("://").nth(1).unwrap_or(portal);
    let rest = rest.split('/').next().unwrap_or(rest);
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None => (rest, 80),
    };
    (host, port).to_socket_addrs().ok()?.next()
}

/// 认证门户能不能连上（TCP 通就算）。
///
/// 门户是校园网内网地址（`10.x`），校外访问不到——所以能连上就说明在校园网里。
/// **这个判断跟走 WiFi 还是网线无关**。
pub fn portal_reachable(portal: &str, timeout: Duration) -> bool {
    match portal_socket_addr(portal) {
        Some(addr) => TcpStream::connect_timeout(&addr, timeout).is_ok(),
        None => false,
    }
}

/// 去目标地址时，本机实际会用哪个源 IP。
///
/// 用 UDP `connect` 做一次路由查询——**它不发任何包**，只是让内核选好出口。
/// 所以能瞬间拿到「去校园网会走哪张网卡」，不关心结果是 WiFi 还是网线。
fn local_ip_for(addr: SocketAddr) -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect(addr).ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}

/// 等校园网就绪（WiFi / 网线都行），最多等 `timeout`。
///
/// 返回 `(是否就绪, 说明)`。两条判据满足任一即可：
///
/// 1. **认证门户可达** —— 最强证据，能连上认证服务器
/// 2. **去门户的源 IP 落在校园网段** —— 已拿到校园网的 DHCP 地址
///
/// 为什么不用 SSID：那是 WiFi 专有概念。插网线时读不到任何 SSID，
/// 旧实现会死等到超时，导致**以太网用户的开机检查完全不工作**。
pub fn wait_for_campus(portal: &str, timeout: Duration, stop: &AtomicBool) -> (bool, String) {
    let step = Duration::from_millis(crate::config::STARTUP_POLL_MS);
    let mut waited = Duration::ZERO;

    loop {
        if stop.load(Ordering::Relaxed) {
            return (false, "已取消".to_string());
        }

        // 判据 1：能连上认证门户
        if portal_reachable(portal, Duration::from_millis(1200)) {
            return (true, "认证门户可达".to_string());
        }

        // 判据 2：兑底。门户一时抽风时，只要拿到的地址在校园网段也算就绪
        if let Some(addr) = portal_socket_addr(portal) {
            if let Some(ip) = local_ip_for(addr) {
                if ip.to_string().starts_with(crate::config::CAMPUS_IP_PREFIX) {
                    return (true, format!("已拿到校园网地址 {ip}"));
                }
            }
        }

        if waited >= timeout {
            return (false, format!("{} 秒内未就绪", timeout.as_secs()));
        }
        std::thread::sleep(step);
        waited += step;
    }
}
