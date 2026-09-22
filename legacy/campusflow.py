#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
CampusFlow — 复旦大学 iFudan.stu 校园网自动认证工具

协议来源：对门户 http://10.102.250.36 前端 JS（Vue SPA）的逆向分析。
    GET  /api/v1/ip
         -> {"code":200,"data":"10.x.x.x"}          本机在校园网内的 IP

    POST /api/v1/pre_login
         {"getuseronlinestate":"on_or_off","user_ipadress":<ip>}
         -> {"code":200,"data":{"useronlinestate":"on|off",
                                "username":"...","balance":"...",
                                "duration":"...","outport":"中国电信",
                                "usripadd":"<ip>"}}

    POST /api/v1/login   （认证 / 注销 都是这个接口，靠 pagesign 区分）
      认证: {"username","password","ifautologin":"0","channel":"_GET",
             "pagesign":"firstauth","usripadd":<ip>}
      注销: {"username","password":"123","ifautologin":"1","channel":"0",
             "pagesign":"thirddauth","usripadd":<ip>}

    注：密码为明文 JSON 传输，无客户端加密；响应体是 GBK 编码。

典型故障说明
------------
本校园网的认证网关（AC）与门户（10.102.250.36）是两套状态：
    * AC 侧会话超时/被踢  -> 实际没网
    * 门户侧仍记录为 on    -> 打开登录页只看到"已登录"，不会重新认证
所以"打开登录页没用、必须退出账号重登"的根因就在这里。
本工具检测到"门户说在线、实际没网"时，会先 thirddauth 注销再 firstauth 登录。
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

# --------------------------------------------------------------------------- #
# 配置
# --------------------------------------------------------------------------- #

def app_dir() -> Path:
    """配置文件的落地目录。

    - 源码运行：脚本所在目录
    - PyInstaller 打包后：**exe 所在目录**

    注意不能用 sys._MEIPASS：那是 onefile 模式每次启动重新解包的临时目录，
    写在那里 config.json 一退出就没了。资源文件才放在 _MEIPASS。
    """
    if getattr(sys, "frozen", False):
        return Path(sys.executable).resolve().parent
    return Path(__file__).resolve().parent


PORTAL = os.environ.get("CAMPUSFLOW_PORTAL", "http://10.102.250.36")
PROFILE = os.environ.get("CAMPUSFLOW_PROFILE", "iFudan.stu")
DEFAULT_CHANNEL = os.environ.get("CAMPUSFLOW_CHANNEL", "中国电信")
CONFIG_PATH = Path(os.environ.get("CAMPUSFLOW_CONFIG",
                                  app_dir() / "config.json"))

UA = ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
      "(KHTML, like Gecko) Chrome/120.0 Safari/537.36")

# 联网探测：(url, 期望状态码)
PROBES = [
    ("http://connect.rom.miui.com/generate_204", 204),
    ("http://www.gstatic.com/generate_204", 204),
    ("http://captive.apple.com/hotspot-detect.html", 200),
]

TIMEOUT = 6.0


# --------------------------------------------------------------------------- #
# HTTP 小工具（只用标准库，绕开系统代理）
# --------------------------------------------------------------------------- #

def _opener() -> urllib.request.OpenerDirector:
    # 门户必须在校园网内直连，绝不能走系统代理
    return urllib.request.build_opener(urllib.request.ProxyHandler({}))


def _decode(raw: bytes) -> str:
    for enc in ("gbk", "utf-8"):
        try:
            return raw.decode(enc)
        except UnicodeDecodeError:
            continue
    return raw.decode("utf-8", errors="replace")


def _request(path: str, payload: dict | None = None,
             method: str = "GET", timeout: float = TIMEOUT) -> dict:
    """请求门户，返回解析后的 JSON。"""
    body = None
    if payload is not None:
        # 必须紧凑！门户后端对请求体做的是朴素子串匹配，
        # json.dumps 默认 `": "` 里的空格会让它把请求判成非法，
        # 直接返回 useronlinestate="off"（并附带 option82 字段）。
        body = json.dumps(payload, ensure_ascii=False,
                          separators=(",", ":")).encode("utf-8")

    req = urllib.request.Request(PORTAL + path, data=body, method=method)
    req.add_header("Content-Type", "application/json;charset=gbk")
    req.add_header("Access-Control-Allow-Origin", "*")
    req.add_header("User-Agent", UA)
    if body is None:
        req.add_header("Accept", "application/json, text/plain, */*")

    with _opener().open(req, timeout=timeout) as resp:
        text = _decode(resp.read())

    try:
        return json.loads(text)
    except json.JSONDecodeError:
        raise RuntimeError(f"门户返回了非 JSON 内容: {text[:200]!r}")


# --------------------------------------------------------------------------- #
# 门户 API 封装
# --------------------------------------------------------------------------- #

def get_ip(timeout: float = TIMEOUT) -> str:
    """获取本机在校园网内的 IPv4（由门户的 X-Real-IP 判定，最可靠）。"""
    r = _request("/api/v1/ip", timeout=timeout)
    ip = r.get("data")
    if not isinstance(ip, str) or not ip:
        raise RuntimeError(f"获取 IP 失败: {r}")
    return ip


def pre_login(ip: str, timeout: float = TIMEOUT) -> dict:
    """查询门户侧记录的在线状态。返回 data 字典。"""
    r = _request("/api/v1/pre_login",
                 {"getuseronlinestate": "on_or_off", "user_ipadress": ip},
                 method="POST", timeout=timeout)
    if r.get("code") != 200:
        raise RuntimeError(f"pre_login 失败: {r}")
    return r.get("data") or {}


def _auth_payload(ip: str, username: str, password: str,
                  channel: str, pagesign: str,
                  ifautologin: str = "0") -> dict:
    return {
        "username": username,
        "password": password,
        "ifautologin": ifautologin,
        "channel": channel,
        "pagesign": pagesign,
        "usripadd": ip,
    }


def login_step1(ip: str, username: str, password: str,
                timeout: float = TIMEOUT) -> dict:
    """第一步 firstauth：校验账号密码。

    若该账号有多个运营商出口，返回的 data 里会带 channels 列表
    （如 校园网/中国移动/中国电信/中国联通），此时**尚未真正认证**，
    必须再调 login_step2 选定通道。
    """
    return _request("/api/v1/login",
                    _auth_payload(ip, username, password, "_GET", "firstauth"),
                    method="POST", timeout=timeout)


def login_step2(ip: str, username: str, password: str, channel: str,
                timeout: float = TIMEOUT) -> dict:
    """第二步：带上选定的运营商通道，真正下发认证。

    channel == "0" 表示「关闭网络」，前端此时用 pagesign=thirdauth。
    """
    pagesign = "thirdauth" if str(channel) == "0" else "secondauth"
    return _request("/api/v1/login",
                    _auth_payload(ip, username, password, str(channel), pagesign),
                    method="POST", timeout=timeout)


def logout(ip: str, username: str, timeout: float = TIMEOUT) -> dict:
    """注销（前端「注销」按钮：pagesign=thirddauth，密码随便填）。"""
    return _request("/api/v1/login",
                    _auth_payload(ip, username, "123", "0", "thirddauth",
                                  ifautologin="1"),
                    method="POST", timeout=timeout)


def resolve_channel(channels: list, wanted: str | None) -> tuple[str, str]:
    """在 channels 里找目标运营商。wanted 可以是 id（"1"）或名称（"中国电信"）。

    返回 (channel_id, channel_name)，找不到返回 ("", "")。
    """
    if not channels:
        return "", ""
    wanted = str(wanted) if wanted not in (None, "") else DEFAULT_CHANNEL
    for c in channels:
        if str(c.get("id")) == wanted or c.get("name") == wanted:
            return str(c.get("id")), str(c.get("name", ""))
    return "", ""


def describe(info: dict) -> str:
    """把 login() 的返回整理成一行可读文本。"""
    r = info.get("resp") or {}
    data = r.get("data")
    if isinstance(data, dict):
        txt = data.get("text") or data.get("message") or data
    else:
        txt = data
    return f"code={r.get('code')} message={r.get('message')} data={txt}"


def login(ip: str, username: str, password: str, channel: str | None = None,
          timeout: float = TIMEOUT) -> tuple[bool, dict]:
    """完整的两步认证。返回 (是否成功, 详情)。"""
    r1 = login_step1(ip, username, password, timeout)
    if r1.get("code") != 200:
        return False, {"stage": "firstauth", "resp": r1}

    data = r1.get("data") or {}
    channels = data.get("channels")
    if not channels:
        # 不需要选运营商，第一步就已经认证成功
        return True, {"stage": "firstauth", "resp": r1, "data": data}

    cid, cname = resolve_channel(channels, channel)
    if not cid:
        return False, {"stage": "select", "resp": r1, "channels": channels,
                       "wanted": channel or DEFAULT_CHANNEL}

    r2 = login_step2(ip, username, password, cid, timeout)
    return (r2.get("code") == 200), {
        "stage": "secondauth", "resp": r2, "channel": cid,
        "channel_name": cname, "data": r2.get("data") or {},
    }


# --------------------------------------------------------------------------- #
# 连通性探测
# --------------------------------------------------------------------------- #

def check_internet(timeout: float = TIMEOUT) -> tuple[bool, str]:
    """返回 (是否真的能上外网, 说明)。

    多个探测点全部失败才算断网，避免单个站点抽风造成误判。
    """
    fails: list[str] = []
    for url, want in PROBES:
        host = urllib.parse.urlsplit(url).netloc
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with _opener().open(req, timeout=timeout) as resp:
                code = resp.getcode()
                body = resp.read(256)
            if code != want:
                # 被强制门户接管时，通常会返回 200 + 一个登录页
                fails.append(f"{host}: HTTP {code}（疑似门户劫持）")
                continue
            if want == 200 and b"Success" not in body:
                fails.append(f"{host}: 内容异常（疑似门户劫持）")
                continue
            return True, host
        except urllib.error.HTTPError as e:
            if e.code == want:
                return True, host
            fails.append(f"{host}: HTTP {e.code}")
        except Exception as e:                       # noqa: BLE001
            fails.append(f"{host}: {type(e).__name__}")
    return False, f"{len(fails)}/{len(PROBES)} 探测点失败（" + "；".join(fails) + "）"


def wifi_ssid() -> str | None:
    """当前连接的 WLAN SSID（非 Windows 或失败时返回 None）。"""
    if os.name != "nt":
        return None
    try:
        out = subprocess.run(
            ["netsh", "wlan", "show", "interfaces"],
            capture_output=True, timeout=10,
        ).stdout
    except Exception:                                # noqa: BLE001
        return None
    for raw in out.splitlines():
        line = _decode(raw) if isinstance(raw, bytes) else raw
        line = line.strip()
        if line.startswith("SSID") and "BSSID" not in line:
            return line.split(":", 1)[1].strip()
    return None


# --------------------------------------------------------------------------- #
# 配置读写
# --------------------------------------------------------------------------- #

DEFAULT_CONFIG = {
    "username": "",
    "password": "",
    "channel": DEFAULT_CHANNEL,      # 运营商出口："中国电信" 或 id "1"
    "portal": PORTAL,
    "profile": PROFILE,
    "watch_interval": 30,
    "on_stale_session": "relogin",   # relogin | login_only
}


def load_config() -> dict:
    cfg = dict(DEFAULT_CONFIG)
    if CONFIG_PATH.exists():
        try:
            cfg.update(json.loads(CONFIG_PATH.read_text(encoding="utf-8")))
        except Exception as e:                       # noqa: BLE001
            raise SystemExit(f"配置文件损坏 {CONFIG_PATH}: {e}")
    # 环境变量优先级最高
    cfg["username"] = os.environ.get("CAMPUSFLOW_USER", cfg["username"])
    cfg["password"] = os.environ.get("CAMPUSFLOW_PASS", cfg["password"])
    return cfg


def save_config(cfg: dict) -> None:
    CONFIG_PATH.write_text(
        json.dumps(cfg, ensure_ascii=False, indent=2), encoding="utf-8"
    )


# --------------------------------------------------------------------------- #
# 业务动作
# --------------------------------------------------------------------------- #

def cmd_status(cfg: dict, as_json: bool = False) -> int:
    ok, why = check_internet()
    info: dict = {
        "internet": ok,
        "probe": why,
        "ssid": wifi_ssid(),
        "ip": None,
        "portal_state": None,
        "portal_user": None,
    }
    try:
        info["ip"] = get_ip()
        data = pre_login(info["ip"])
        info["portal_state"] = data.get("useronlinestate")
        info["portal_user"] = data.get("username")
        info["portal_raw"] = data
    except Exception as e:                           # noqa: BLE001
        info["portal_error"] = f"{type(e).__name__}: {e}"

    if as_json:
        print(json.dumps(info, ensure_ascii=False, indent=2))
        return 0

    print(f"WiFi        : {info['ssid'] or '(未知)'}")
    print(f"内网 IP     : {info['ip'] or '(未获取)'}")
    print(f"门户状态    : {info['portal_state'] or '(未知)'}"
          + (f"   账号 {info['portal_user']}" if info["portal_user"] else ""))
    if "portal_error" in info:
        print(f"门户访问    : 失败 — {info['portal_error']}")
    if ok:
        print(f"公网连通    : ✅ 正常（{why}）")
    else:
        print(f"公网连通    : ❌ 不通（{why}）")

    if ok and info["portal_state"] == "on":
        print("\n结论: 一切正常。")
    elif not ok and info["portal_state"] == "on":
        print("\n结论: 门户显示在线但实际没网 —— 典型的网关侧会话失效，"
              "需要先注销再重新认证。跑 `fix` 即可。")
    elif not ok:
        print("\n结论: 未认证或认证已过期。跑 `fix` 即可。")
    return 0


def cmd_login(cfg: dict) -> int:
    ip = get_ip()
    ok, info = login(ip, cfg["username"], cfg["password"], cfg.get("channel"))
    if info.get("stage") == "select":
        names = "  ".join(f"{c.get('id')}={c.get('name')}"
                           for c in info["channels"])
        print(f"账号有多个运营商出口，配置里没匹配到 {info['wanted']!r}")
        print(f"可选: {names}")
        return 1
    print(f"[{info.get('stage')}] {describe(info)}")
    if info.get("channel_name"):
        print(f"运营商: {info['channel_name']} (id={info['channel']})")
    return 0 if ok else 1


def cmd_logout(cfg: dict) -> int:
    ip = get_ip()
    user = cfg["username"]
    try:
        data = pre_login(ip)
        user = data.get("username") or user
    except Exception:                                # noqa: BLE001
        pass
    r = logout(ip, user)
    print(f"logout -> code={r.get('code')} msg={r.get('message')} data={r.get('data')}")
    return 0 if r.get("code") == 200 else 1


def fix(cfg: dict, verbose: bool = True, on_log=None) -> bool:
    """把网络修好。返回是否（最终）可上网。

    on_log: 可选回调 on_log(message)，用于把日志实时推给 Web 前端。
    """
    def log(msg: str) -> None:
        if on_log is not None:
            on_log(msg)
        if verbose:
            print(msg, flush=True)

    ok, why = check_internet()
    if ok:
        log(f"[ok] 网络正常（{why}）")
        return True

    log(f"[..] 公网不通: {why}")

    try:
        ip = get_ip()
    except Exception as e:                           # noqa: BLE001
        log(f"[!!] 连门户都访问不到: {type(e).__name__}: {e}")
        log("     请检查是否已连上 WiFi，或门户地址是否变更。")
        return False

    state, user = None, cfg["username"]
    try:
        data = pre_login(ip)
        state = data.get("useronlinestate")
        user = data.get("username") or user
        log(f"[..] 门户状态: {state}  账号: {user}  IP: {ip}")
    except Exception as e:                           # noqa: BLE001
        log(f"[!!] pre_login 失败: {type(e).__name__}: {e}（继续尝试登录）")

    # 门户说在线但实际没网 -> 僵尸会话，必须先注销
    if state == "on" and cfg.get("on_stale_session", "relogin") == "relogin":
        log("[..] 门户认为在线但实际无网 -> 注销僵尸会话…")
        try:
            logout(ip, user)
        except Exception as e:                       # noqa: BLE001
            log(f"[??] 注销请求异常（忽略）: {e}")
        time.sleep(1.0)

    log("[..] 提交认证（firstauth → secondauth）…")
    try:
        ok, info = login(ip, cfg["username"], cfg["password"],
                         cfg.get("channel"))
    except Exception as e:                           # noqa: BLE001
        log(f"[!!] 认证请求失败: {type(e).__name__}: {e}")
        return False

    if info.get("stage") == "select":
        names = "  ".join(f"{c.get('id')}={c.get('name')}"
                           for c in info["channels"])
        log(f"[!!] 账号有多个运营商出口，但没匹配到 {info['wanted']!r}")
        log(f"     可选: {names}")
        log("     请在 config.json 的 \"channel\" 填名称或 id。")
        return False

    log(f"[..] {info.get('stage')} 响应: {describe(info)}")
    if info.get("channel_name"):
        log(f"[..] 运营商: {info['channel_name']} (id={info['channel']})")
    if not ok:
        return False

    # 认证成功后等一小会儿再验证，网关下发策略需要时间
    for i in range(6):
        time.sleep(1.5)
        ok, why = check_internet()
        if ok:
            log(f"[ok] 认证成功，网络已恢复（{why}）")
            return True
        log(f"[..] 第 {i + 1} 次复检仍未通（{why}）")

    log("[!!] 认证返回成功，但外网仍不通。可能账号欠费/被限制/需要二次验证。")
    return False


def cmd_fix(cfg: dict) -> int:
    return 0 if fix(cfg) else 1


def cmd_watch(cfg: dict, interval: int | None = None) -> int:
    interval = interval or int(cfg.get("watch_interval", 30))
    print(f"CampusFlow 守护中，每 {interval}s 检查一次（Ctrl+C 退出）")
    last_state: bool | None = None
    while True:
        try:
            ok, _ = check_internet()
            if not ok:
                print(f"[{time.strftime('%H:%M:%S')}] 掉线，尝试修复…")
                ok = fix(cfg)
                print(f"[{time.strftime('%H:%M:%S')}] "
                      f"{'已恢复' if ok else '修复失败'}")
            if ok != last_state:
                if ok and last_state is not None:
                    pass
                last_state = ok
        except KeyboardInterrupt:
            print("\n已退出。")
            return 0
        except Exception as e:                       # noqa: BLE001
            print(f"[{time.strftime('%H:%M:%S')}] 异常: {type(e).__name__}: {e}")
        time.sleep(interval)


def cmd_open(cfg: dict) -> int:
    """在默认浏览器打开认证门户（带 wlanuserfirsturl，和网关劫持时一致）。"""
    url = f"{PORTAL}/#/login?wlanuserfirsturl=http://www.msftconnecttest.com/redirect"
    print(f"打开: {url}")
    if os.name == "nt":
        os.startfile(url)                            # type: ignore[attr-defined]
    else:
        import webbrowser
        webbrowser.open(url)
    return 0


def cmd_init(cfg: dict) -> int:
    cfg = dict(cfg)
    if not cfg["username"]:
        cfg["username"] = input("学号/用户名: ").strip()
    if not cfg["password"]:
        import getpass
        cfg["password"] = getpass.getpass("密码: ")
    save_config(cfg)
    print(f"已写入 {CONFIG_PATH}")
    return 0


# --------------------------------------------------------------------------- #
# CLI
# --------------------------------------------------------------------------- #

def main(argv: list[str] | None = None) -> int:
    cfg = load_config()

    p = argparse.ArgumentParser(
        prog="campusflow",
        description="复旦 iFudan.stu 校园网自动认证",
    )
    sub = p.add_subparsers(dest="cmd")

    sp = sub.add_parser("status", help="查看联网/认证状态（只读，安全）")
    sp.add_argument("--json", action="store_true")

    sub.add_parser("fix", help="诊断并修复（推荐）")
    sub.add_parser("login", help="直接提交认证")
    sub.add_parser("logout", help="注销当前会话")
    sub.add_parser("open", help="在浏览器打开认证页")
    sub.add_parser("init", help="交互式写入 config.json")

    sw = sub.add_parser("watch", help="常驻守护，掉线自动重连")
    sw.add_argument("-i", "--interval", type=int, default=None,
                    help="检查间隔秒数，默认取配置")

    sw2 = sub.add_parser("web", help="启动 Web 控制台（浏览器图形界面）")
    sw2.add_argument("-p", "--port", type=int, default=8765)
    sw2.add_argument("--host", default="127.0.0.1")
    sw2.add_argument("--no-browser", action="store_true", help="不自动开浏览器")
    sw2.add_argument("--watch", action="store_true", help="启动时同时开启自动守护")

    args = p.parse_args(argv)

    # 双击 exe（无参数）时直接起 Web 控制台，不然只会闪一下黑窗就没了
    double_clicked = args.cmd is None and getattr(sys, "frozen", False)
    cmd = args.cmd or ("web" if double_clicked else "status")

    if cmd == "status":
        return cmd_status(cfg, as_json=getattr(args, "json", False))
    if cmd == "fix":
        return cmd_fix(cfg)
    if cmd in ("login", "logout") and not cfg["username"]:
        print("还没配置账号，先跑: campusflow init", file=sys.stderr)
        return 2
    if cmd == "login":
        return cmd_login(cfg)
    if cmd == "logout":
        return cmd_logout(cfg)
    if cmd == "open":
        return cmd_open(cfg)
    if cmd == "init":
        return cmd_init(cfg)
    if cmd == "watch":
        return cmd_watch(cfg, getattr(args, "interval", None))
    if cmd == "web":
        import cfweb
        return cfweb.serve(
            cfg,
            host=getattr(args, "host", "127.0.0.1"),
            port=getattr(args, "port", 8765),
            open_browser=not getattr(args, "no_browser", False),
            autostart_watch=bool(getattr(args, "watch", False)) or double_clicked,
        )

    p.print_help()
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
