# legacy — Python 实现（已归档）

这是 CampusFlow 的第一版：Python 标准库 + 浏览器页面。

**现在不再维护。** 保留它有两个原因：

1. **它是协议的可执行文档。** Rust 版的所有行为都是照着它 1:1 复刻的，
   出问题时可以对着读。
2. **它是兜底方案。** 万一 Rust 版在某个环境编不出来，这个版本只要有
   Python 3.8+ 就能直接跑。

## 文件

| 文件 | 说明 |
|---|---|
| `campusflow.py` | 单文件实现：协议 + CLI + `<cmd>` 命令 |
| `cfweb.py` | 本地 Web 控制台（`http.server` + SSE） |
| `config.example.json` | 配置样板 |
| `start-web.bat` | Windows 启动脚本 |

## 注意

这个版本配套的 `web/` 前端**已被 Rust 版覆盖**——仓库根目录的 `web/` 现在是
Tauri 桌面版的界面，用的是 `invoke()` / Tauri 事件，不是 `fetch()` / SSE。

所以 `cfweb.py` 现在**跑不起来界面**。真要用的话，把 `web/app.js` 里的传输层
换回 `fetch`/`EventSource` 即可，接口是一一对应的：

| Rust 命令 | 旧 HTTP 接口 |
|---|---|
| `invoke('status')` | `GET /api/status` |
| `invoke('do_fix')` | `POST /api/fix` |
| `invoke('get_config')` | `GET /api/config` |
| `invoke('save_config', {patch})` | `POST /api/config` |
| `invoke('watch_control', {action, interval})` | `POST /api/watch` |
| `listen('cf-log', ...)` | `GET /api/events`（SSE） |

## 用法（历史记录）

```bash
python campusflow.py init      # 写 config.json
python campusflow.py status    # 看状态
python campusflow.py fix       # 一键修复
python campusflow.py watch -i 30
python campusflow.py web       # 浏览器界面（需旧版 web/）
```
