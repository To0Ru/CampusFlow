# CampusFlow

复旦大学 **iFudan.stu** 校园网自动认证客户端。Rust + Tauri 桌面小程序，单文件 exe，托盘常驻，掉线自愈。

---

## 1. 为什么需要它

学校的校园网有三层状态，**互相不同步**——这就是"重启也不会自动连上、打开认证页显示已登录但没网、必须退出账号重登"的根本原因：

| 层 | 作用 | 不同步的表现 |
|---|---|---|
| WLAN 关联 | 连上 `iFudan.stu`（开放网络，无需密码） | 一直连着，图标正常 |
| 认证网关 AC | **真正决定你能不能上网** | 会话超时/被踢 → 没网 |
| 认证门户 | 记录"你在线"这个状态 | **可能仍显示在线** |

AC 侧会话已经没了，门户侧还是 `on`，于是门户一看你在线就直接跳过认证，什么都不做。
**只有先注销清掉那条僵尸记录，重新认证才会真正下发到 AC。**

CampusFlow 做的事就是：检测到"门户说在线、实际没网"时，自动 `注销 → 选运营商 → 认证`。

---

## 2. 逆向出来的协议

门户：`http://10.102.250.36`（Vue SPA，标题 `authenticate`）。
密码**明文 JSON** 传输，无客户端加密；响应体是 **GBK** 编码。

```
GET  /api/v1/ip
     -> {"code":200,"data":"10.115.17.191"}

POST /api/v1/pre_login
     {"getuseronlinestate":"on_or_off","user_ipadress":"<ip>"}
     -> {"code":200,"data":{"useronlinestate":"on|off","username":"ad57361477",
                            "balance":"0","duration":"731",
                            "outport":"中国电信","usripadd":"10.115.17.191"}}

POST /api/v1/login          // 认证和注销共用这个接口，靠 pagesign 区分
     第一步 firstauth : {"username":u,"password":p,"ifautologin":"0",
                        "channel":"_GET","pagesign":"firstauth","usripadd":"<ip>"}
         -> 多出口账号返回 channels（**此时尚未认证**）：
            {"code":200,"data":{"channels":[
                {"id":"3","name":"校园网"},{"id":"2","name":"中国移动"},
                {"id":"1","name":"中国电信"},{"id":"4","name":"中国联通"}]}}
         -> 没有 channels 说明第一步就已认证成功

     第二步 secondauth: {...,"channel":"1","pagesign":"secondauth"}
         -> {"code":200,"data":{"reauth":false,"outport":"中国电信",...}}
         注：channel="0" 表示「关闭网络」，此时 pagesign 用 "thirdauth"

     注销 : {"username":u,"password":"123","ifautologin":"1",
             "channel":"0","pagesign":"thirddauth","usripadd":"<ip>"}
```

请求头：`Content-Type: application/json;charset=gbk`

对应前端源码：`handleLogin()` → firstauth；`handleModalOk(t)` → secondauth。

### ⚠️ 坑一：请求体必须是紧凑 JSON

门户后端对请求体做的是**朴素子串匹配**。冒号后多一个空格它就解析不出来，
兜底返回 `"useronlinestate":"off"`（响应里会多一个 `"option82":"_OPTION82_"`）：

```
{"getuseronlinestate":"on_or_off","user_ipadress":"10.115.17.191"}      ✅ 返回真实状态
{"getuseronlinestate": "on_or_off", "user_ipadress": "10.115.17.191"}   ❌ 永远 "off"
```

`serde_json::to_string` 默认就是紧凑的，所以 Rust 这边天然安全——
**但千万别改成 `to_string_pretty`**（`portal.rs` 里有注释标注）。

### ⚠️ 坑二：响应是 GBK

不能直接 `from_utf8`，否则中文全是乱码。`portal.rs` 里按 `Content-Type` 判断，
没有则先试 UTF-8、失败再退 GBK。

### ⚠️ 坑三：探测"能不能上网"要防劫持

未认证时网关会把 HTTP 请求 302 到认证页，状态码往往还是 200。
所以探测时**不跟随跳转**，并且要校验返回内容（`captive.apple.com` 必须含 `Success`）。

其他地址：

* 自助服务系统：`http://10.108.255.18:8800/`
* 网关：`10.115.127.254`（iFudan.stu 网段 `10.115.0.0/17`）

---

## 3. 它是怎么工作的

```
fix 的判断流程
─────────────────────────────────────────────
能看到外网 ? ── 是 ──> 什么都不做
      │
      否
      ├─ 门户说 on  ──> 僵尸会话：先 thirddauth 注销，再走下面的认证
      └─ 门户说 off ──> 直接走下面的认证

认证（两步）
─────────────────────────────────────────────
firstauth ──> 返回 channels ?
                  │
                  ├─ 有 ──> 按配置找运营商（默认中国电信）
                  │          └─> secondauth(channel) ──> 完成
                  └─ 无 ──> 第一步已成功

最后每 1.5s 复检外网，最多 6 次。
```

---

## 4. 界面

原生桌面窗口（430×680），不是浏览器页面：

* **顶栏**：品牌 + 当前 SSID + 后端日志通道指示灯
* **状态页**：状态大卡（区分「外网不通」和「僵尸会话」）、一键修复、六格信息、
  自动守护开关、打开认证页 / 注销会话
* **日志页**：实时日志终端，Tauri 事件推送，能看到 `firstauth → 注销 → secondauth → 复检` 每一步
* **设置页**：账号 / 密码 / 出口运营商 / 守护间隔 / **开机自启**，以及版本、门户、配置路径
* **托盘**：显示窗口 / 一键修复 / 退出。关闭窗口 = 收进托盘，后台守护继续跑
* 快捷键：`R` 刷新，`F` 修复

---

## 5. 技术选型

| 选择 | 理由 |
|---|---|
| **Tauri v2** 而不是 egui / iced | egui 默认字体**没有中文字形**，要么内嵌 10MB 思源黑体，要么自己接系统字体；Tauri 走系统 WebView2，中文渲染免费且完美。窗口仍是货真价实的原生应用窗口（独立任务栏图标、托盘、无地址栏） |
| **`ureq`** 而不是 `reqwest` + `tokio` | 门户是纯 HTTP，没有 TLS。关掉 ureq 默认特性后不需要 async 运行时、不需要 rustls/OpenSSL，二进制小一大截 |
| **`encoding_rs`** | GBK 解码 |
| **不引 `tauri-plugin-autostart`** | 就写一个 `HKCU\...\Run` 键，调 `reg.exe` 足够，少一层依赖和权限配置 |
| **前端纯静态、零构建** | `frontendDist` 直接指向 `web/`，`withGlobalTauri: true` 让前端能用 `window.__TAURI__`，**不需要 npm、不需要打包器** |

依赖只有 6 个：`tauri` / `serde` / `serde_json` / `ureq` / `encoding_rs` / `tauri-plugin-*`（无）。

---

## 6. 仓库结构

```
.
├── src-tauri/                 Rust 后端
│   ├── src/
│   │   ├── main.rs            入口：状态装配、窗口事件、命令注册
│   │   ├── portal.rs          门户协议（两步认证 / 注销 / pre_login / ip）
│   │   ├── net.rs             联网探测 + netsh 读 SSID
│   │   ├── config.rs          配置读写（便携优先，退回 %APPDATA%）
│   │   ├── fixer.rs           修复编排 + 状态采集
│   │   ├── watcher.rs         后台守护线程
│   │   ├── logbus.rs          日志环形缓冲 + Tauri 事件广播
│   │   ├── commands.rs        前端可调用的 Tauri 命令
│   │   ├── tray.rs            托盘图标 + 菜单
│   │   └── autostart.rs       开机自启（注册表）
│   ├── capabilities/          权限配置
│   ├── icons/                 打包图标
│   ├── Cargo.toml
│   └── tauri.conf.json
├── web/                       前端（纯静态，无构建）
│   ├── index.html
│   ├── style.css
│   └── app.js
├── tools/make_icon.py         纯 stdlib 生成 ICO/PNG，不需要 Pillow
├── legacy/                    Python 实现（已归档，作为协议参考）
└── .github/workflows/build.yml
```

---

## 7. 编译与发布（全部在 GitHub 上跑，本机零环境）

本项目**不需要本机装 Rust / Visual Studio**，编译交给 GitHub Actions。

### 日常验证

推任意分支 → `cargo check` + `cargo build --release` 都会跑，
exe 作为 artifact 上传，在 Actions 页面直接下载来测。

### 发版

```bash
git tag v0.1.0
git push origin v0.1.0
```

CI 会额外：

1. 用 Tauri 打包 NSIS 安装包（**当前用户安装，不需要管理员**）
2. 发 GitHub Release，附带 `CampusFlow.exe`（绿色版）和安装包

### 本地编译（可选）

想在本机迭代的话：

```powershell
winget install --id Rustlang.Rustup -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e `
  --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

然后 `cd src-tauri && cargo run`（调试模式会自动开 devtools）。

---

## 8. 配置

首次启动在「设置」里填，写入 `config.json`：

```json
{
  "username": "ad57361477",
  "password": "你的密码",
  "channel": "中国电信",
  "portal": "http://10.102.250.36",
  "profile": "iFudan.stu",
  "watch_interval": 30,
  "on_stale_session": "relogin",
  "autostart": false
}
```

**落地位置**：优先 exe 同级目录（便携模式），写不进去才退回
`%APPDATA%\CampusFlow\`。所以绿色版能带着配置走，装到 Program Files 的版本
也不会因为没写权限而崩。实际路径在「设置」页底部能看到。

`channel` 可写名称也可写 id：`校园网=3`、`中国移动=2`、`中国电信=1`、`中国联通=4`。
写错了不会静默失败，日志里会列出可用选项。

`on_stale_session` 改成 `login_only` 可以关掉"僵尸会话先注销"的行为。

---

## 9. 排查

| 现象 | 原因 |
|---|---|
| 门户 `off` 但能上网 | 门户与 AC 不同步，无害；重新认证时会自动纠正 |
| 认证返回成功但仍不通 | 账号欠费 / 被限制 / 需要二次验证，去 `http://10.108.255.18:8800/` 看 |
| 日志提示「没匹配到运营商」 | 「设置」里的出口运营商写错了，日志里有可选项 |
| 连门户都访问不到 | 没连上 WiFi，或门户地址变了 |
| 托盘图标没出现 | 不影响主功能，日志里会有 `[??] 托盘图标创建失败` |

手动验证门户请求：

```bash
IP=$(curl -s http://10.102.250.36/api/v1/ip | python -c "import sys,json;print(json.load(sys.stdin)['data'])")
curl -s -X POST http://10.102.250.36/api/v1/pre_login \
     -H 'Content-Type: application/json;charset=gbk' \
     -d "{\"getuseronlinestate\":\"on_or_off\",\"user_ipadress\":\"$IP\"}"
```

> 若本机设了系统代理，请加 `--noproxy '*'`；程序内部始终绕过代理直连门户。

---

## 10. 免责声明

本项目通过逆向校园网认证页面的前端 JS 实现，仅供个人在自己账号上自动化重复操作。
公开发布等于把学校的认证协议一并公开，请自行评估学校相关规定的接受度。
