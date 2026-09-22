# 认证协议说明

本文记录复旦 `iFudan.stu` 校园网认证协议的分析结果，以及实现时必须避开的坑。
内容来自对认证门户前端 JavaScript 的逆向（门户页是 Vue SPA，逻辑全在打包后的 JS 里）。

> **门户地址**：`http://10.102.250.36`
> **网关**：`10.115.127.254`（iFudan.stu 网段 `10.115.0.0/17`）
> **自助服务系统**：`http://10.108.255.18:8800/`

---

## 1. 三层状态，互不同步

这是"打开认证页显示已登录但没网"的根本原因：

| 层 | 作用 | 失效时的表现 |
|---|---|---|
| WLAN 关联 | 连上 `iFudan.stu`（开放网络，无密码） | 一直连着，图标正常 |
| 认证网关 **AC** | **真正决定能不能上网** | 会话超时/被踢 → 没网 |
| 认证门户 | 记录"你在线"这个状态 | **可能仍显示 `on`** |

AC 侧会话没了、门户侧还是 `on` 时，门户一看你在线就直接跳过认证，什么都不做。
**必须先注销清掉那条僵尸记录，重新认证才会真正下发到 AC。**

---

## 2. 接口列表

全部挂在门户根下。密码**明文 JSON** 传输，无客户端加密；响应体是 **GBK** 编码。

### `GET /api/v1/ip`

```json
{ "code": 200, "data": "10.115.17.191" }
```

### `POST /api/v1/pre_login`

请求：

```json
{ "getuseronlinestate": "on_or_off", "user_ipadress": "10.115.17.191" }
```

> 注意参数名就是拼错的 `ipadress`，不是 `ipaddress`。

响应：

```json
{
  "code": 200,
  "message": "ok",
  "data": {
    "useronlinestate": "on",
    "username": "ad57361477",
    "balance": "0",
    "duration": "731",
    "outport": "中国电信",
    "totaltimespan": "0",
    "usripadd": "10.115.17.191"
  }
}
```

### `POST /api/v1/login`

认证和注销**共用这一个接口**，靠 `pagesign` 区分阶段。

**第一步 `firstauth`** —— 校验账号密码：

```json
{
  "username": "ad57361477",
  "password": "******",
  "ifautologin": "0",
  "channel": "_GET",
  "pagesign": "firstauth",
  "usripadd": "10.115.17.191"
}
```

如果账号有多个运营商出口，返回的是一个**待选择列表**（此时**尚未认证**）：

```json
{
  "code": 200,
  "data": {
    "channels": [
      { "id": "3", "name": "校园网" },
      { "id": "2", "name": "中国移动" },
      { "id": "1", "name": "中国电信" },
      { "id": "4", "name": "中国联通" }
    ]
  }
}
```

如果没有 `channels` 字段，说明第一步就已经认证成功了。

**第二步 `secondauth`** —— 选定运营商，真正下发认证：

```json
{ "...": "...", "channel": "1", "pagesign": "secondauth" }
```

成功响应：

```json
{
  "code": 200,
  "data": {
    "reauth": false,
    "username": "ad57361477",
    "balance": "0.00",
    "duration": "0",
    "outport": "中国电信",
    "usripadd": "10.115.17.191"
  }
}
```

**注销**（对应前端「注销」按钮）：

```json
{
  "username": "ad57361477",
  "password": "123",
  "ifautologin": "1",
  "channel": "0",
  "pagesign": "thirddauth",
  "usripadd": "10.115.17.191"
}
```

> 注销时密码随便填，服务端不校验。

### `pagesign` 取值对照

| 值 | 含义 | channel |
|---|---|---|
| `firstauth` | 账号密码首次认证 | `_GET` |
| `secondauth` | 选定运营商后真正认证 | 运营商 id |
| `thirdauth` | 关闭网络（前端弹窗里选「关闭网络」） | `0` |
| `thirddauth` | 注销按钮 | `0` |

对应前端源码：`handleLogin()` → `firstauth`，`handleModalOk(t)` → `secondauth`。

### 请求头

```
Content-Type: application/json;charset=gbk
```

---

## 3. ⚠️ 坑一：请求体必须是紧凑 JSON

门户后端对请求体做的是**朴素子串匹配**。冒号后多一个空格就解析不出来，
兜底返回 `"useronlinestate":"off"`，并且响应里会多一个 `"option82":"_OPTION82_"` 字段：

```jsonc
// ✅ 返回真实状态
{"getuseronlinestate":"on_or_off","user_ipadress":"10.115.17.191"}

// ❌ 永远返回 "off"
{"getuseronlinestate": "on_or_off", "user_ipadress": "10.115.17.191"}
```

这个坑很隐蔽，因为**两边都是合法 JSON**，用 Python 的 `json.dumps()` 默认就这么输出
（`separators` 默认是 `(', ', ': ')`）。

Rust 侧用 `serde_json::to_string()`，默认就是紧凑的，天然安全——
**但千万别改成 `to_string_pretty`**。`portal.rs` 里有注释标注。

判断依据：失败的响应里会带 `"option82":"_OPTION82_"`。

---

## 4. ⚠️ 坑二：响应是 GBK 编码

不能直接按 UTF-8 解，否则 `中国电信` 会变成乱码。

处理顺序：

1. 看 `Content-Type` 里有没有 `gbk` / `gb2312` / `gb18030`，有就直接用 GBK
2. 没有的话先试 UTF-8，失败再退回 GBK

`netsh wlan show interfaces` 的输出在中日文 Windows 上也是 GBK，同样要处理。

Rust 用 `encoding_rs::GBK`。

---

## 5. ⚠️ 坑三：探测"能不能上网"要防劫持

未认证时，网关会把 HTTP 请求 **302 到认证页**。如果你跟随了跳转，最终会拿到
认证页的 `200 OK`，于是**误判为"网络正常"，导致永远不会触发重连**。

所以探测时必须：

1. **不跟随跳转**（`redirects(0)`）
2. **校验返回内容**，不只是看状态码

用到的探测点：

| 地址 | 期望 |
|---|---|
| `http://connect.rom.miui.com/generate_204` | 204 |
| `http://www.gstatic.com/generate_204` | 204 |
| `http://captive.apple.com/hotspot-detect.html` | 200 且正文含 `Success` |

三个点**全部失败**才算断网，避免单个站点抽风造成误判。

---

## 6. 认证流程总结

```
fix():
    check_internet() ── 通 ──> 直接返回
        │
        └─ 不通
            │
            get_ip()                    GET /api/v1/ip
            pre_login(ip)               → useronlinestate
            │
            ├─ state == "on" ──> logout()    thirddauth，清僵尸会话
            │                     sleep 1s
            │
            login():
                first_auth()            firstauth  → channels ?
                    ├─ 无 channels  ──> 已成功
                    └─ 有 channels  ──> 按配置找运营商
                                        └─ second_auth(id)  → 完成
            │
            └─ 每 1.5s 复检外网，最多 6 次
```

---

## 7. 复现分析方法

如果你想自己验证或适配，步骤大致是：

1. 连上 WiFi，确保当前**未认证**状态（或直接打开门户页）
2. 门户页是 Vue SPA，逻辑都在打包后的 JS 里。把页面源码里的 `js/*.js` 全下下来：
   ```bash
   curl -s http://10.102.250.36/ | grep -oE 'js/[^"]+\.js'
   ```
3. 在 JS 里搜 `url:"/api/` 或 `\$API\.` 就能定位所有接口
4. 搜 `pagesign` 能看全认证阶段
5. 浏览器开发者工具 Network 面板直接看一次完整登录的请求体最省事

另外，**浏览器历史记录里会留着门户 URL**（含 `wlanuserfirsturl` 参数），
这是最初定位到门户 IP 的线索：

```
http://10.102.250.36/#/login?wlanuserfirsturl=http://www.msftconnecttest.com/redirect
```
