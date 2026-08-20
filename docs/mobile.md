# Pisper 移动端：第一阶段互联契约

本文档是移动端第一阶段（T0 局域网 + T1 IPv6 + T2 Tailscale 检测）的**实现契约**。
Runtime、Tauri 壳、前端三方均按本文档实现；任何一方需要偏离时，先改文档再改代码。

配套设计背景见各节引用；本文档只定义**协议与行为**，不约束内部实现。

## 1. 范围与非目标

### 范围内

- 移动端作为**桌面 Runtime 的远程客户端**（本机不跑 Agent Runtime）。
- 连通档位：T0 局域网直连（mDNS 发现）、T1 公网 IPv6 直连、T2 Tailscale 网卡检测与引导。
- 配对协议（A2）、发现与二维码（A5）、SSE 断线续传（A6）、TLS/指纹 pinning、版本握手。

### 非目标（后续阶段）

- 移动端本机运行 Runtime（阶段二/三）。
- 内置 P2P 隧道（T3，Iroh/WebRTC）与兜底中继（T4）。本文档的 endpoint 模型和壳内
  `Transport` 抽象为其预留扩展点，但不定义其行为。
- APNs/FCM 远程推送（阶段一用"回前台重连 + 未读标记"代替）。

## 2. 术语与角色

| 术语 | 含义 |
| --- | --- |
| **桌面端 / Runtime** | 运行在桌面设备上的 `runtime/` HTTP 服务，唯一事实数据源。 |
| **移动端 / 壳** | Tauri 2 Android/iOS 应用。内含静态 UI 包 + Rust 本地代理，**不启动 sidecar**。 |
| **本地代理** | 壳内 Rust 组件，监听 `127.0.0.1` 随机端口，向所选 endpoint 转发并做 TLS 指纹 pinning。WebView 只连本地代理。 |
| **endpoint** | 一个可连的 Runtime 地址，带类型标记：`lan` / `v6` / `ts` / `tunnel`（预留）。 |
| **配对码** | 一次性、短时效的配对凭据，由桌面端生成，扫码或手输交给移动端。 |
| **设备令牌** | 配对成功后移动端持有的长期凭据（Bearer token），可吊销。 |
| **run** | 一次 `POST /api/chat` 引发的流式执行单元，有唯一 `runId`，事件可重挂。 |
| **游标（cursor）** | SSE 事件帧的单调递增序号，用于断线续传。 |

## 3. 拓扑

```
┌──────────────────────── 移动端 ────────────────────────┐
│  WebView (内置 UI 包)                                   │
│    │ http://127.0.0.1:<随机端口>  (明文，仅回环)         │
│    ▼                                                    │
│  Rust 本地代理 ── TLS + 指纹 pinning ──► 所选 endpoint  │
└─────────────────────────────────────────────────────────┘
                           │  lan / v6 / ts / tunnel(预留)
                           ▼
┌──────────────────────── 桌面端 ────────────────────────┐
│  Runtime HTTP API（绑定 0.0.0.0，强制鉴权）              │
│  mDNS 广播 _pisper._tcp                                 │
└─────────────────────────────────────────────────────────┘
```

关键约束：

- WebView **永远不直接连远端地址**。Android WebView 拒绝自签证书且不允许以
  "忽略证书错误"方式上架；所有 TLS 终结在本地代理。
- 本地代理对 SSE 必须**关闭缓冲、禁用空闲超时**，逐字节透传。
- 本地代理是 `Transport` 抽象的插入点：`DirectTls | Tailscale | IrohTunnel(预留)`。

## 4. A2 配对协议

### 4.1 数据模型

Runtime 在数据目录持久化 `paired_devices`（JSON 或 sqlite，实现自定）：

```jsonc
{
  "devices": [
    {
      "id": "dev_01J...",            // 服务端生成，nanoid
      "name": "iPhone 15",           // 配对时移动端自报，可改
      "tokenHash": "sha256:...",     // 只存哈希，永不存明文令牌
      "createdAt": "2025-01-01T00:00:00Z",
      "lastSeenAt": "2025-01-02T00:00:00Z",
      "revokedAt": null
    }
  ],
  "pairingCode": {                   // 至多一个进行中的配对码
    "codeHash": "sha256:...",
    "expiresAt": "2025-01-01T00:05:00Z"
  }
}
```

### 4.2 API

所有路径以 `/api/remote/` 为前缀。错误统一为 `{ "error": "<message>", "code": "<machine_code>" }`。

#### `POST /api/remote/pairing-code`（桌面端本地调用）

桌面 WebView 调（走现有 Cookie 通道）生成配对码。**已有进行中配对码时作废旧码。**

响应 `200`：

```json
{
  "code": "ABCD-EFGH",          // 8 字符 Crockford Base32，无 0/O/1/I/L，连字符仅展示用
  "expiresAt": "2025-01-01T00:05:00Z",
  "qrPayload": { }               // 见 §5.2，桌面端可直接渲染
}
```

#### `POST /api/remote/pair`（移动端调用，无需任何既有凭据）

请求：

```json
{
  "code": "ABCDEFGH",            // 归一化：去连字符、大写
  "deviceName": "iPhone 15"
}
```

响应 `201`：

```json
{
  "deviceId": "dev_01J...",
  "token": "pst_01J...",         // 仅在此时返回一次，移动端须存入 Keychain/Keystore
  "serverName": "工作室台式机",
  "apiVersion": 1
}
```

错误：

| HTTP | code | 含义 |
| --- | --- | --- |
| 400 | `invalid_request` | 缺字段或格式非法 |
| 403 | `pairing_code_invalid` | 配对码错误或已被使用 |
| 410 | `pairing_code_expired` | 配对码已过期（TTL 5 分钟） |
| 429 | `pairing_rate_limited` | 同一来源 IP 连续失败 ≥5 次，冷却 60 秒 |

配对码验证用常量时间比较；验证成功即消费（一次性），与过期清理共用一个检查点。

#### `GET /api/remote/devices`（任意已认证通道）

响应 `200`：`{ "devices": [ { "id", "name", "createdAt", "lastSeenAt", "revokedAt", "current": true } ] }`。
`current` 标记发起本请求的设备自身。

#### `POST /api/remote/devices/:id/revoke`（Cookie 通道，或设备吊销自身）

响应 `204`。吊销即刻生效：该设备令牌的进行中请求允许完成，**新请求一律 401**；
SSE 长连接在 30 秒内被主动断开。

### 4.3 认证通道规则

| 通道 | 凭据 | 适用 | Origin 校验 |
| --- | --- | --- | --- |
| Cookie（现有 `desktop-sidecar-auth`） | HttpOnly Cookie | 桌面 WebView | 非幂等请求强制校验 |
| Bearer（新增） | `Authorization: Bearer pst_...` | 移动端 / 远程客户端 | 豁免（令牌不随浏览器自动携带，无 CSRF 面） |

- 两条通道共用同一套常量时间比较；Bearer 比对的是 `sha256(token)`。
- 远程模式（§7）下**未配置任何凭据时拒绝启动**，防止裸奔暴露。
- `lastSeenAt` 在每次 Bearer 认证成功时刷新（节流：最多每分钟一次）。

## 5. A5 发现与二维码

### 5.1 mDNS 广播（T0）

- 服务类型：`_pisper._tcp.local`，实例名 = 设备名。
- TXT 记录（全部小写键）：

| 键 | 含义 |
| --- | --- |
| `v` | 协议版本，当前为 `1` |
| `fp` | TLS 证书 SHA-256 指纹，`SHA256:<hex>` 大写无分隔 |
| `name` | 设备名（UTF-8） |
| `tls` | `1` 表示该端口要求 TLS |

- 依赖选纯 JS 实现（`@homebridge/ciao` 或 `bonjour-service`），禁止引入需要原生编译的 mDNS 模块。
- 远程模式关闭时不得广播。

### 5.2 二维码 payload（版本化 JSON）

```json
{
  "v": 1,
  "name": "工作室台式机",
  "endpoints": [
    { "t": "lan", "url": "https://192.168.1.5:5173" },
    { "t": "v6",  "url": "https://[240e::xxxx]:5173" },
    { "t": "ts",  "url": "http://100.64.x.x:5173" }
  ],
  "fp": "SHA256:AB12...",
  "code": "ABCD-EFGH"
}
```

规则：

- `v` 未知或大于客户端支持版本 → 提示"请升级 App"，**不得**尝试猜测解析。
- `endpoints` 按桌面端建议优先级排列；移动端仍需逐个探测，选择策略见 §5.3。
- `ts` 类型 endpoint 允许 `http://`（WireGuard 已加密），其余类型必须 `https://`。
- 二维码只是载体；同一 payload 也提供"复制文本"，移动端支持手输配对码 + 手动填地址的兜底路径。
- 未知顶层字段必须忽略（前向兼容）。

### 5.3 地址枚举与选择

桌面端枚举规则：

1. LAN IPv4：所有非回环私网地址。
2. 公网 IPv6：**必须探测入站可达性**（如向公网 echo 服务发起回连验证），只有地址存在不算数；
   不可达的 v6 地址不得写入 payload。
3. Tailscale：优先 `tailscale status --json`（超时 2 秒，失败视为未安装）；
   检测到 tailnet IPv4（`100.64.0.0/10`）即加入 payload；未安装但桌面端检测到移动端场景时，
   在设置页给安装引导。

移动端选择策略：按 `lan → v6 → ts` 顺序并发探测（`GET /api/health`，超时 3 秒），
取第一个通过 TLS 指纹校验的；全部失败进入" unreachable "状态页并给出排查清单
（同网/AP 隔离/防火墙/Tailscale 引导）。

## 6. A6 SSE 断线续传契约

### 6.1 背景

现状是**每请求流**：`POST /api/chat` 直接返回当轮事件流。移动网络下连接必断，
因此引入 run 模型：**流与请求解耦，事件可重挂**。

### 6.2 run 模型

`POST /api/chat` 行为变更（对旧客户端保持兼容，见 §6.6）：

- 响应仍为 `text/event-stream`，但**第一帧必须是**：

```
event: run
data: {"runId":"run_01J...","sessionId":"...","cursor":0}
```

- 此后每帧携带 SSE 标准 `id:` 行，值为**该 run 内单调递增的整数游标**（从 1 开始）：

```
id: 1
event: snapshot
data: {...}

id: 2
event: message_patch
data: {...}
```

- run 终态帧：`done` 或 `error`（现有语义不变），终态后游标停止增长。

### 6.3 重挂端点

`GET /api/runs/:runId/events?after=<cursor>`

- 返回 `text/event-stream`；先发 `after < cursor ≤ latest` 之间的全部缓存事件，然后继续实时推送。
- `after` 缺省或为 0：等同于全量重放（从缓存最早事件开始）。
- run 不存在或缓存已清理：返回 `409`，body 为 `{ "error": "...", "code": "run_not_resumable" }`，
  客户端应回退到重新拉取会话快照（现有 `GET /api/sessions/:id/tree` 等只读接口）。

### 6.4 服务端缓冲策略

- 每个 run 一个 ring buffer：上限 **512 事件或 1 MiB**（先到为准）；溢出时丢弃最旧事件，
  并在重挂响应开头插入：

```
event: resync_required
data: {"reason":"buffer_overflow","runId":"run_01J..."}
```

  客户端收到 `resync_required` 必须放弃本地增量状态，走快照重建。
- run 终态后缓存保留 **10 分钟**，随后清理（清理后重挂返回 409）。
- 所有帧序列化继续走现有 `jsonReplacer`（孤立代理项清洗），保证 `serde_json` 可解析——
  这是 TUI 兼容红线，新增帧类型同样适用。

### 6.5 客户端语义

- 移动端记录每个 run 已收到的最大游标；断线重连用 `?after=<maxCursor>` 重挂。
- 收到重复游标的事件必须幂等丢弃（以游标去重，不以内容去重）。
- 本地代理不得解析/重组 SSE 帧，只透传字节；游标逻辑只在 WebView 内的 UI 层。

### 6.6 兼容旧客户端（Web 桌面 / TUI）

- 旧客户端忽略 `id:` 行和未知 `run` 帧即可，无需改动——这是选 SSE 标准 `id:` 字段的原因。
- 验收时必须跑 `npm test` 与 `npm run tui:check`，确认 TUI 消费不受影响。

## 7. 远程模式与 TLS

- 开启：`--remote` 或 `PISPER_REMOTE=1`；绑定 `0.0.0.0`（可用 `HOST` 覆盖为具体网卡）。
- 远程模式强制：①存在至少一种有效凭据（桌面 token 或任一未吊销设备令牌），否则拒绝启动；
  ②默认要求 TLS。
- 证书：首次启动生成自签证书并持久化（指纹稳定，重新安装才变化）；
  `GET /api/health` 响应头携带 `X-Pisper-Cert-Fingerprint` 供调试，但**客户端只能信任配对时获得的指纹**。
- `remote.tls = auto | required | off`：`auto` 对 `ts`/tunnel 类型 endpoint 允许明文回环转发，其余强制 TLS。

## 8. 版本握手

- `GET /api/health` 响应体扩展：`{ "ok": true, "apiVersion": 1, "minClientVersion": 1, "engineVersion": "..." }`。
- 壳内置 UI 包带 `clientVersion`；`clientVersion < minClientVersion` → UI 显示"请升级 App"全屏页；
  `apiVersion > clientVersion` → 仅提示，不阻断（服务端承诺同大版本内前向兼容）。

## 9. 安全模型要点

1. 配对码：一次性、5 分钟 TTL、Crockford Base32（2⁴⁰ 空间）、常量时间比较、按源 IP 限流。
2. 设备令牌：服务端只存 SHA-256 哈希；移动端存 Keychain/Keystore；吊销即时生效。
3. TLS 指纹随配对二维码带外传递（TOFU）；此后一切连接校验指纹，不依赖系统 CA。
4. Bearer 通道豁免 Origin 校验；Cookie 通道维持现有 Origin 校验不变。
5. 配对接口是唯一无需凭据的远程入口，攻击面收敛于此：除限流外，错误响应不得区分
   "码不存在"与"码已过期"之外的任何内部状态。

## 10. 验收清单

### Runtime

- [ ] `npm test` 新增：配对码一次性/过期/限流/吊销后 401；重挂端点游标语义；buffer 溢出 `resync_required`。
- [ ] `npm run tui:check` 通过（SSE 帧变更对 TUI 透明）。
- [ ] 未配置凭据时 `--remote` 拒绝启动。

### Tauri 壳

- [ ] Android/iOS 真机：扫码 → 指纹确认 → 配对 → 聊天流式渲染。
- [ ] 切 4G 后自动按优先级重选 endpoint；锁屏 5 分钟回前台按游标续传不丢帧。
- [ ] 自签证书场景无任何"忽略证书错误"代码路径。

### 前端

- [ ] `npm run check`（含 i18n）通过；新增 key 双语齐全。
- [ ] 桌面端设置页可展示二维码、设备列表、吊销；移动端有配对/服务器管理/手动兜底页。

## 11. 开放问题

- 中继/T3 落地后，`endpoints` 增加 `{ "t": "tunnel", "url": "iroh:<nodeId>" }`，由本地代理解析——契约已预留，细节另文。
- 多桌面同时在线时的 mDNS 实例冲突处理（当前假设实例名唯一，冲突时追加序号）。
- run 缓存 10 分钟 TTL 是否覆盖"锁屏过夜早上继续看"的场景——若不够，需要终态事件落盘，
  属于阶段一之后的增强。
