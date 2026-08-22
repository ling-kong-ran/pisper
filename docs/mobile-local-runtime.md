# 移动端本机 Runtime 设计（M1）

Pisper 移动端最初是纯远程客户端：Agent Runtime、会话与 Provider 配置都留在桌面端。
本文档定义「本机运行」的第一个里程碑（M1）：在 Android/iOS 壳内嵌入一个**受限的
本机 Runtime**，让手机在没有桌面端在线时也能直接与模型 Provider 对话。

M1 刻意保持小边界。它不是桌面 Runtime 的移植，而是一套独立的、只覆盖对话主链路的
最小实现；与远程模式共用「服务器选择」入口统一切换。

## 能力矩阵

| 能力 | 本机 Runtime（M1） | 远程桌面 Runtime |
| --- | --- | --- |
| 流式对话（SSE） | ✅ OpenAI 兼容 Provider | ✅ 全部 Provider |
| 多会话与本地持久化 | ✅（有资源上限） | ✅ |
| 多 Provider 配置与切换 | ✅（密钥仅存本机） | ✅ |
| 工具调用 / 技能 / MCP / 记忆 | ❌（M2+ 评估） | ✅ |
| 文件与终端访问 | ❌ | ✅ |
| 二维码配对 / 远程访问 | 不涉及 | ✅ |

## 架构

```text
Mobile WebView
    |  http://127.0.0.1:<port>（仅回环，随机端口）
    v
本机 Runtime（Rust，嵌入移动端壳）
    |-- 内置对话页（/，单文件 HTML，同源调用 API）
    |-- /api/local/*（会话、Provider、状态）
    |-- POST /api/local/chat（SSE：meta/delta/done/error）
    |
    |  HTTPS（reqwest + rustls；仅 loopback 地址允许 http）
    v
OpenAI 兼容 Provider（/chat/completions 流式）
```

- 本机 Runtime 与现有回环代理（`mobile/proxy.rs`）是**并列**的两个回环服务：
  代理把流量转发到桌面端，本机 Runtime 直接在本机应答。两者互不知晓。
- WebView 地址即模式：`http://127.0.0.1:<proxy>` 是远程模式，
  `http://127.0.0.1:<local>` 是本机模式。切换就是导航，不涉及配对存储变更。
- 本机 Runtime 只监听回环，不暴露任何网络端口；它发起的唯一外网流量是到用户
  配置的 Provider 端点的 HTTPS 请求。

## 数据与密钥保管

应用私有目录下的 `local-runtime/`：

- `providers.json`：Provider 配置（baseUrl、model、activeId）。apiKey 经 `KeyCustody`
  加密后以 `apiKeyEnc` 密文落盘，API 读取时永远脱敏（仅返回 `keyHint` 末 4 位）。
- `sessions.json`：会话与消息。所有写入走「临时文件 + rename」原子替换。
- `master.key`：仅桌面开发/测试回退后端的主密钥文件（0600）。

**密钥保管后端（M2 已实现）**：

| 平台 | 后端 | 说明 |
| --- | --- | --- |
| Android | 系统 AndroidKeyStore | JNI 直连（无 Kotlin），AES-256/GCM 加解密在 Keystore 内完成，密钥本体不进入 Rust 进程，卸载即销毁 |
| iOS | Keychain + 进程内 AES-GCM | 随机主密钥以 `AfterFirstUnlockThisDeviceOnly` 存 Keychain，不随备份/iCloud 迁移 |
| 桌面 | 文件回退 | 仅供开发与测试，不承担真实对话 |

旧版明文 `providers.json` 在加载时自动迁移为密文并立刻重写。解密失败（Keystore 重置等）
保留 Provider 元数据、仅清空密钥，用户重新填 key 即可。

注：远程配对设备令牌（`pisper-mobile.json`）目前仍为应用私有目录明文，跟随沙箱保护；
迁入同一保管体系是后续硬化项。

## 资源上限

手机存储与内存受限，本机 Runtime 强制上限（超出即淘汰最旧会话）：

| 项 | 上限 |
| --- | --- |
| 会话数 | 50 |
| 单会话消息数 | 200 |
| 单条消息 | 32 KiB |
| 会话存储总量 | 4 MiB |
| API 请求体 | 1 MiB |
| Provider 连接 / 整体流 | 15 s / 180 s |

## 安全边界

- Provider baseUrl 必须是 `https://`；唯一例外是 loopback（`127.0.0.1` /
  `localhost` / `::1`）允许 `http://`，用于本机模型服务（如 Ollama）与开发调试。
- 错误信息不回显 apiKey；网络错误只做类别化描述。
- 内置对话页仅由本机 Runtime 同源提供，不加载任何远程资源。
- 本机模式不读取、不写入远程配对凭据；远程模式的 Bearer/指纹模型不受影响。

## 入口与切换

- 首次启动不再直接进入扫码流程：连接页平级呈现「本机运行」与「连接桌面端」两个选择，
  手动配对的地址/配对码/指纹输入框默认收纳进子面板，点击才展开。
- 远程 UI 的「服务器」设置页有「本机运行」行，本机对话页可「返回服务器」。
- 模式记忆（M2）：进入/离开本机模式会记录 `last_mode`，冷启动按记忆路由；
  显式选择远程服务器会把记忆切回远程。

## 协议

`POST /api/local/chat` 的 SSE 帧：

| event | data | 含义 |
| --- | --- | --- |
| `meta` | `{sessionId, messageId}` | 首帧，确认会话与助手消息 ID |
| `delta` | `{text}` | 增量文本，按到达顺序拼接 |
| `done` | `{messageId}` | 正常结束，消息已持久化 |
| `error` | `{message}` | 失败；已收到的增量会作为部分回复保留 |

该协议是 M1 私有协议，不承诺与桌面 Runtime 的 run/游标协议兼容；
当后续里程碑让完整 Web UI 跑在本机 Runtime 上时，再统一对齐。

## 后续里程碑

- **M2（已完成）**：apiKey 迁入 Android Keystore / iOS Keychain；模式记忆；
  Provider 模型列表拉取（测试连接成功后填入候选）。
- **M3**：受限工具能力（只读设备侧能力，如剪贴板/通知），逐项评审；
  远程配对设备令牌迁入系统安全存储。
- **M4**：评估完整 Web UI 直连本机 Runtime（需实现对齐的 sessions/chat/run API 面）。
