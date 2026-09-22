#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
CampusFlow Web 控制台 —— 只用 Python 标准库实现，零依赖。

设计前提：这个界面最需要被打开的时刻，恰恰是**没网**的时候。
所以页面、样式、脚本全部本地托管，不引用任何 CDN，不需要 npm / 构建步骤。

启动：
    python campusflow.py web            # http://127.0.0.1:8765/
    python campusflow.py web --watch    # 顺便开启掉线自动守护

HTTP API：
    GET  /                 前端页面
    GET  /style.css /app.js
    GET  /api/status       当前联网 + 认证状态
    GET  /api/logs         历史日志
    GET  /api/events       SSE 实时推日志
    GET  /api/config       读配置（不含密码）
    POST /api/config       写配置 {username,password,channel,watch_interval}
    POST /api/fix          一键修复
    POST /api/login        直接提交认证
    POST /api/logout       注销
    POST /api/watch        {action:"start|stop", interval:int}

只监听 127.0.0.1，不对外暴露。
"""

from __future__ import annotations

import collections
import json
import queue
import sys
import threading
import time
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import campusflow as cf


def _resource_dir() -> Path:
    """静态资源目录。

    PyInstaller onefile 会把 --add-data 的内容解包到 sys._MEIPASS，
    此时 __file__ 指向临时目录里的 cfweb.pyc，不能直接用。
    """
    base = getattr(sys, "_MEIPASS", None)
    if base:
        return Path(base)
    return Path(__file__).resolve().parent


WEB_DIR = _resource_dir() / "web"

# 端口占用时向后顺延的尝试次数
PORT_TRIES = 20

STATIC_TYPES = {
    ".html": "text/html; charset=utf-8",
    ".css": "text/css; charset=utf-8",
    ".js": "application/javascript; charset=utf-8",
    ".svg": "image/svg+xml",
    ".ico": "image/x-icon",
}

# 日志前缀 -> 前端配色等级
_LEVELS = {"[ok]": "ok", "[..]": "info", "[!!]": "error", "[??]": "warn"}


def _level_of(msg: str) -> str:
    for prefix, level in _LEVELS.items():
        if msg.startswith(prefix):
            return level
    return "info"


# --------------------------------------------------------------------------- #
# 日志总线：环形缓冲 + 多订阅者（给 SSE 用）
# --------------------------------------------------------------------------- #

class LogBus:
    def __init__(self, maxlen: int = 800) -> None:
        self._items: collections.deque = collections.deque(maxlen=maxlen)
        self._subs: set[queue.Queue] = set()
        self._lock = threading.Lock()

    def emit(self, msg: str, level: str | None = None) -> dict:
        ev = {"t": time.strftime("%H:%M:%S"),
              "level": level or _level_of(msg),
              "msg": msg}
        with self._lock:
            self._items.append(ev)
            subs = list(self._subs)
        for q in subs:
            try:
                q.put_nowait(ev)
            except queue.Full:
                pass
        return ev

    def history(self) -> list[dict]:
        with self._lock:
            return list(self._items)

    def clear(self) -> None:
        with self._lock:
            self._items.clear()

    def subscribe(self) -> queue.Queue:
        q: queue.Queue = queue.Queue(maxsize=1000)
        with self._lock:
            self._subs.add(q)
        return q

    def unsubscribe(self, q: queue.Queue) -> None:
        with self._lock:
            self._subs.discard(q)


# --------------------------------------------------------------------------- #
# 后台守护线程
# --------------------------------------------------------------------------- #

class Watcher(threading.Thread):
    """后台守护线程。

    注意：属性名千万不要用 self._stop ——
    threading.Thread 内部本来就有个 _stop() 方法，
    覆盖它会让 is_alive() 直接抛 TypeError。
    """

    def __init__(self, cfg: dict, bus: LogBus, interval: int = 30) -> None:
        super().__init__(daemon=True, name="cf-watcher")
        self.cfg = dict(cfg)
        self.bus = bus
        self.interval = max(5, int(interval))
        self._halt = threading.Event()
        self.state = {
            "running": True,
            "interval": self.interval,
            "checks": 0,
            "fixes": 0,
            "last_check": None,
            "last_result": None,
        }

    def run(self) -> None:
        self.bus.emit(f"[..] 自动守护已启动，每 {self.interval}s 检查一次")
        while not self._halt.is_set():
            try:
                ok, why = cf.check_internet()
            except Exception as e:                   # noqa: BLE001
                ok, why = False, f"{type(e).__name__}: {e}"

            self.state["checks"] += 1
            self.state["last_check"] = time.strftime("%H:%M:%S")
            self.state["last_result"] = "ok" if ok else "down"

            if not ok:
                self.bus.emit(f"[..] 检测到掉线（{why}），开始修复…")
                try:
                    good = cf.fix(self.cfg, verbose=False, on_log=self.bus.emit)
                except Exception as e:               # noqa: BLE001
                    good = False
                    self.bus.emit(f"[!!] 修复过程异常: {type(e).__name__}: {e}")
                if good:
                    self.state["fixes"] += 1
                    self.bus.emit("[ok] 自动修复成功")
                else:
                    self.bus.emit("[!!] 自动修复失败，稍后重试")

            self._halt.wait(self.interval)

    def stop(self) -> None:
        self._halt.set()
        self.state["running"] = False


# --------------------------------------------------------------------------- #
# 应用状态
# --------------------------------------------------------------------------- #

class App:
    def __init__(self, cfg: dict, bus: LogBus) -> None:
        self.cfg = dict(cfg)
        self.bus = bus
        self.watcher: Watcher | None = None
        self._busy = threading.Lock()

    # ---- 状态采集 ---- #
    def status(self) -> dict:
        out: dict = {
            "ssid": cf.wifi_ssid(),
            "channel": self.cfg.get("channel"),
            "username": self.cfg.get("username"),
            "portal": cf.PORTAL,
            "internet": None,
            "probe": None,
            "ip": None,
            "portal_state": None,
            "portal_user": None,
            "outport": None,
            "duration": None,
            "balance": None,
            "portal_error": None,
            "watcher": self.watcher.state if self.watcher else None,
        }

        ip = None
        try:
            ip = cf.get_ip(timeout=4)
            out["ip"] = ip
        except Exception as e:                       # noqa: BLE001
            out["portal_error"] = f"{type(e).__name__}: {e}"

        if ip:
            try:
                d = cf.pre_login(ip, timeout=4)
                out["portal_state"] = d.get("useronlinestate")
                out["portal_user"] = d.get("username")
                out["outport"] = d.get("outport")
                out["duration"] = d.get("duration")
                out["balance"] = d.get("balance")
            except Exception as e:                   # noqa: BLE001
                out["portal_error"] = f"{type(e).__name__}: {e}"

        ok, why = cf.check_internet(timeout=4)
        out["internet"] = ok
        out["probe"] = why
        return out

    # ---- 动作 ---- #
    def _guard(self):
        """同一时刻只允许一个认证类操作，避免并发打架。"""
        if not self._busy.acquire(blocking=False):
            return False
        return True

    def do_fix(self) -> dict:
        if not self._guard():
            return {"ok": False, "msg": "已有操作在执行中"}
        try:
            self.bus.emit("[..] ── 开始手动修复 ──")
            ok = cf.fix(self.cfg, verbose=False, on_log=self.bus.emit)
            return {"ok": ok, "msg": "修复成功" if ok else "修复失败"}
        finally:
            self._busy.release()

    def do_login(self) -> dict:
        if not self._guard():
            return {"ok": False, "msg": "已有操作在执行中"}
        try:
            ip = cf.get_ip()
            ok, info = cf.login(ip, self.cfg["username"], self.cfg["password"],
                                self.cfg.get("channel"))
            if info.get("stage") == "select":
                opts = "  ".join(f"{c.get('id')}={c.get('name')}"
                                 for c in info["channels"])
                self.bus.emit(f"[!!] 未匹配到运营商 {info['wanted']!r}，"
                              f"可选: {opts}")
                return {"ok": False, "msg": f"运营商 {info['wanted']!r} 不存在",
                        "channels": info["channels"]}
            msg = cf.describe(info)
            self.bus.emit(f"[{'ok' if ok else '!!'}] 认证: {msg}")
            if info.get("channel_name"):
                self.bus.emit(f"[..] 运营商: {info['channel_name']}"
                              f" (id={info['channel']})")
            return {"ok": ok, "msg": msg}
        except Exception as e:                       # noqa: BLE001
            self.bus.emit(f"[!!] 认证异常: {type(e).__name__}: {e}")
            return {"ok": False, "msg": f"{type(e).__name__}: {e}"}
        finally:
            self._busy.release()

    def do_logout(self) -> dict:
        if not self._guard():
            return {"ok": False, "msg": "已有操作在执行中"}
        try:
            ip = cf.get_ip()
            user = self.cfg["username"]
            try:
                user = cf.pre_login(ip).get("username") or user
            except Exception:                        # noqa: BLE001
                pass
            r = cf.logout(ip, user)
            ok = r.get("code") == 200
            self.bus.emit(f"[{'ok' if ok else '!!'}] 注销: code={r.get('code')}")
            return {"ok": ok, "msg": f"code={r.get('code')}"}
        except Exception as e:                       # noqa: BLE001
            self.bus.emit(f"[!!] 注销异常: {type(e).__name__}: {e}")
            return {"ok": False, "msg": f"{type(e).__name__}: {e}"}
        finally:
            self._busy.release()

    def watch_start(self, interval: int | None = None) -> dict:
        if self.watcher and self.watcher.is_alive():
            return {"ok": True, "msg": "守护已在运行"}
        iv = int(interval or self.cfg.get("watch_interval", 30))
        self.watcher = Watcher(self.cfg, self.bus, iv)
        self.watcher.start()
        return {"ok": True, "msg": f"守护已启动（{iv}s）"}

    def watch_stop(self) -> dict:
        if self.watcher and self.watcher.is_alive():
            self.watcher.stop()
            self.bus.emit("[..] 守护已停止")
            return {"ok": True, "msg": "守护已停止"}
        return {"ok": True, "msg": "守护本来就没开"}


# --------------------------------------------------------------------------- #
# HTTP server / handler
# --------------------------------------------------------------------------- #

class Server(ThreadingHTTPServer):
    daemon_threads = True

    def handle_error(self, request, client_address) -> None:
        # 浏览器/curl 断开 keep-alive 连接是常态，别刷一屏 traceback；
        # 其他异常照旧打出来，方便发现真 bug。
        import sys
        exc = sys.exc_info()[1]
        if isinstance(exc, (ConnectionAbortedError, ConnectionResetError,
                            BrokenPipeError, TimeoutError)):
            return
        super().handle_error(request, client_address)


class Handler(BaseHTTPRequestHandler):
    server_version = "CampusFlow"
    protocol_version = "HTTP/1.1"
    app: App = None                              # type: ignore[assignment]

    # ---- 基础工具 ---- #
    def log_message(self, fmt, *args) -> None:   # 静音，别刷屏
        pass

    def _json(self, obj, code: int = 200) -> None:
        raw = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(raw)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(raw)

    def _body(self) -> dict:
        """读取并解析请求体。解析不了就抛 ValueError，绝不能静默吞掉——
        否则前端会看到“保存成功”但其实什么都没改。

        编码先试 UTF-8，再退回 GBK：浏览器发的是 UTF-8，
        但 Windows 命令行（cp936）里的 curl 会把中文参数按 GBK 发出去。
        """
        try:
            n = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            n = 0
        if n <= 0:
            return {}

        raw = self.rfile.read(n)
        for enc in ("utf-8", "gbk"):
            try:
                obj = json.loads(raw.decode(enc))
            except (UnicodeDecodeError, json.JSONDecodeError):
                continue
            return obj if isinstance(obj, dict) else {}

        raise ValueError("请求体不是合法 JSON（已尝试 UTF-8 / GBK）")

    def _static(self, rel: str) -> None:
        if rel in ("", "/"):
            rel = "index.html"
        rel = rel.lstrip("/")
        target = (WEB_DIR / rel).resolve()
        # 防目录穿越
        if not str(target).startswith(str(WEB_DIR.resolve())) or not target.is_file():
            self.send_error(404, "Not Found")
            return
        raw = target.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type",
                         STATIC_TYPES.get(target.suffix, "application/octet-stream"))
        self.send_header("Content-Length", str(len(raw)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(raw)

    # ---- GET ---- #
    def do_GET(self) -> None:                    # noqa: N802
        path = self.path.split("?", 1)[0]

        if path == "/api/status":
            self._json(self.app.status())
        elif path == "/api/logs":
            self._json({"logs": self.app.bus.history()})
        elif path == "/api/config":
            c = self.app.cfg
            self._json({
                "username": c.get("username", ""),
                "channel": c.get("channel", cf.DEFAULT_CHANNEL),
                "watch_interval": c.get("watch_interval", 30),
                "portal": cf.PORTAL,
                "profile": cf.PROFILE,
                "on_stale_session": c.get("on_stale_session", "relogin"),
                "has_password": bool(c.get("password")),
            })
        elif path == "/api/events":
            self._sse()
        else:
            self._static(path)

    # ---- SSE ---- #
    def _sse(self) -> None:
        q = self.app.bus.subscribe()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "keep-alive")
        self.send_header("X-Accel-Buffering", "no")
        self.end_headers()
        try:
            self.wfile.write(b": connected\n\n")
            self.wfile.flush()
            while True:
                try:
                    ev = q.get(timeout=15)
                    payload = json.dumps(ev, ensure_ascii=False)
                    self.wfile.write(f"data: {payload}\n\n".encode("utf-8"))
                except queue.Empty:
                    self.wfile.write(b": ping\n\n")     # 心跳，防代理断连
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError, OSError):
            pass
        finally:
            self.app.bus.unsubscribe(q)

    # ---- POST ---- #
    def do_POST(self) -> None:                   # noqa: N802
        path = self.path.split("?", 1)[0]
        try:
            body = self._body()
        except ValueError as e:
            self._json({"ok": False, "msg": str(e)}, 400)
            return

        if path == "/api/fix":
            self._json(self.app.do_fix())
        elif path == "/api/login":
            self._json(self.app.do_login())
        elif path == "/api/logout":
            self._json(self.app.do_logout())
        elif path == "/api/clear-logs":
            self.app.bus.clear()
            self._json({"ok": True})
        elif path == "/api/watch":
            action = body.get("action")
            if action == "start":
                self._json(self.app.watch_start(body.get("interval")))
            elif action == "stop":
                self._json(self.app.watch_stop())
            else:
                self._json({"ok": False, "msg": "action 必须是 start/stop"}, 400)
        elif path == "/api/config":
            c = self.app.cfg
            changed = []
            for k in ("username", "channel"):
                if body.get(k) is not None and str(body[k]).strip() != c.get(k):
                    c[k] = str(body[k]).strip()
                    changed.append(k)
            # 密码留空表示不修改
            if body.get("password"):
                c["password"] = str(body["password"])
                changed.append("password")
            if body.get("watch_interval"):
                try:
                    iv = max(5, int(body["watch_interval"]))
                    if iv != c.get("watch_interval"):
                        c["watch_interval"] = iv
                        changed.append("watch_interval")
                except (TypeError, ValueError):
                    pass
            if not c.get("username") or not c.get("password"):
                self._json({"ok": False,
                            "msg": "用户名和密码都不能为空"}, 400)
                return
            cf.save_config(c)
            # 守护线程持有的是启动时的配置快照，改完要同步过去
            if self.app.watcher and self.app.watcher.is_alive():
                self.app.watcher.cfg = dict(c)
                self.app.bus.emit("[..] 守护已同步新配置")
            msg = ("已保存: " + "、".join(changed)) if changed else "没有变化"
            self.app.bus.emit("[ok] 配置已保存" if changed else "[..] 配置未变化")
            self._json({"ok": True, "msg": msg, "changed": changed})
        else:
            self._json({"ok": False, "msg": "not found"}, 404)


# --------------------------------------------------------------------------- #
# 启动
# --------------------------------------------------------------------------- #

def serve(cfg: dict, host: str = "127.0.0.1", port: int = 8765,
          open_browser: bool = True, autostart_watch: bool = False) -> int:
    bus = LogBus()
    app = App(cfg, bus)
    Handler.app = app

    httpd = None
    for p in range(port, port + PORT_TRIES):
        try:
            httpd = Server((host, p), Handler)
            port = p
            break
        except OSError:
            continue
    if httpd is None:
        print(f"[!!] {host}:{port}~{port + PORT_TRIES} 都占用，起不来")
        return 1

    url = f"http://{host}:{port}/"

    bus.emit(f"[ok] CampusFlow Web 控制台已启动: {url}")
    if not cfg.get("username") or not cfg.get("password"):
        bus.emit("[??] 还没配置账号密码，请在页面底部「设置」里填一下")

    if autostart_watch:
        app.watch_start()

    print(f"CampusFlow Web 控制台: {url}")
    print("按 Ctrl+C 停止")
    if open_browser:
        threading.Timer(0.6, lambda: webbrowser.open(url)).start()

    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\n已停止。")
    finally:
        if app.watcher and app.watcher.is_alive():
            app.watcher.stop()
        httpd.server_close()
    return 0


if __name__ == "__main__":
    import sys
    sys.exit(serve(cf.load_config()))
