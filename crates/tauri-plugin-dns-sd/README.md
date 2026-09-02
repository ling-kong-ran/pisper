# tauri-plugin-dns-sd（Pisper vendored copy）

DNS-SD（mDNS / Bonjour / Zeroconf）服务发现与通告的 Tauri v2 插件，覆盖桌面
（Linux / macOS / Windows）与移动端（iOS + Android）。本目录是上游
[`tauri-plugin-dns-sd 0.1.0`](https://github.com/momics/dns-sd) 的 vendored
副本，仅包含 Rust 插件与 iOS / Android 原生源码；上游的 guest-js 绑定、示例
应用与 CI 不在本仓库内。本地改动见 [PISPER-PATCHES.md](./PISPER-PATCHES.md)。

## 工作原理

移动端（iOS / Android）无法打开原始 UDP 多播套接字，发现必须经过系统解析器
（Apple Bonjour `NWBrowser` / `NWListener`，Android `NsdManager`）。桌面端同样
不驱动共享 mDNS 引擎，而是委托给 Rust 插件内的 [`mdns-sd`][mdns-sd] crate ——
由 OS（或 `mdns-sd`）拥有协议本身。

| 层 | 位置 | 职责 |
| --- | --- | --- |
| Rust 插件 | `src/` | 命令、模型、桌面 `mdns-sd` 实现 |
| iOS | `ios/Sources/DnsSdPlugin.swift` | `NWBrowser` / `NWListener`（Network.framework） |
| Android | `android/.../DnsSdPlugin.kt` | `NsdManager` |

命令集：`browse_start`、`browse_stop`、`advertise_start`、`advertise_stop`
（见 `build.rs` 与 `src/lib.rs`）。事件通过 Tauri IPC `Channel` 以
`ServiceRecord` / browse-stopped 消息形式回传。

## 在 Pisper 中的接线

- Rust 依赖：`src-tauri/Cargo.toml` 中 `tauri-plugin-dns-sd = { path = "../crates/tauri-plugin-dns-sd" }`。
- 注册：仅移动端构建（`src-tauri/src/mobile/mod.rs` 的 `tauri_plugin_dns_sd::init()`）。
- 权限：`src-tauri/capabilities/mobile-bridge.json` 授予 `dns-sd:default`，即
  `allow-browse-start` / `allow-browse-stop` / `allow-advertise-start` /
  `allow-advertise-stop`（定义见 `permissions/default.toml`）。
- 前端：`MobilePairingDialog.tsx` 直接 `invoke('plugin:dns-sd|browse_start', …)`
  并通过 `Channel` 接收发现事件。

## TXT 记录

DNS-SD TXT 条目有三种状态，桌面端全部支持往返：

| 状态 | 契约值 | IPC 传输 | 网络上 |
| --- | --- | --- | --- |
| 裸键（flag） | `true` | `true` | `key` |
| 存在但为空 | `null` | `null` | `key=` |
| 字节值 | `Uint8Array` | `number[]` | `key=<bytes>` |

`advertise` 的纯 `string` 输入按 UTF-8 编码（RFC 6763 §6.5）。

## 平台矩阵与限制

| 能力 | 桌面（`mdns-sd`） | iOS（`NWBrowser`+`NetService`） | Android（`NsdManager`） |
| --- | --- | --- | --- |
| 浏览 / 通告 | ✅ | ✅ | ✅ |
| TXT 记录 | ✅（3 态） | ✅（3 态） | ⚠️ 裸键与空值合并 |
| 子类型（`_sub`） | ✅ | ⚠️ 接受但不按其过滤 | ⚠️ 通告需 API 35 |
| 自定义 `host` | ✅ | ⚠️ 受限 | ⚠️ 数字 IP，API 34+ |
| 自定义 `domain` | ✅（非 `local`） | ⚠️ 受限 | ❌ 仅 `local` |
| 浏览时主机/地址解析 | ✅ | ✅ | ✅（全部地址，API 34+） |
| 浏览超时 / 中止 | ✅（共享层） | ✅ | ✅ |
| `removed`（isActive:false） | ✅ | ✅ | ✅ |

**iOS：** `NWBrowser` 只发现端点与 TXT，自身不给出主机/地址（Network.framework
在连接内惰性解析）。为达到桌面/Android 对等，插件对每个发现的实例再经
`NetService`（Bonjour）解析：先发 `found`，主机名、端口与 IP 地址就绪后发
`resolved`，TXT 数据全程携带。

**Android：** 发现把每个实例解析为端口与**全部** IP 地址。Android 14+（API 34）
使用 `NsdManager.registerServiceInfoCallback`（返回所有地址并以 `updated` 事件
流式推送后续地址/TXT 变化）；更早版本回退到已废弃的 `resolveService`（单地址）。
自定义通告 `host` 仅在 Android 14+ 且为数字 IP 字面量时生效（经
`setHostAddresses`）；自定义主机名始终由 OS 决定，且只支持 `local` 域。
`NsdManager` 无法区分裸 TXT 键与空值（两者都表现为 `true`），通告 TXT 值按
UTF-8 编码（`setAttribute` 只接受 `String`）。通告子类型需要 `setSubtypes`
（Android 15 / API 35）；当前 `compileSdk`（34）下仅为 API 对等而接受、并不注册。

## 测试

| 覆盖 | 平台 | 自动化 |
| --- | --- | --- |
| Rust 命令实现：browse↔advertise、TXT、goodbye、超时 | 桌面 | ✅ `cargo test` |
| 真实网络发现（同机双实例） | 桌面 | ✅ 需环境开关 |
| iOS 权限错误映射 | macOS host | ✅ `scripts/test-ios-dns-sd.sh`（CI） |
| iOS / Android 原生路径 | 移动端 | ⚠️ 手动（需 Xcode / Android SDK） |

- **Rust 测试**（`src/desktop/commands.rs` 的 `#[cfg(test)]`）通过 `tauri::test`
  mock 应用直接驱动桌面命令实现。TXT 三态往返始终断言；**网络**测试（真实回环
  mDNS browse↔advertise、goodbye、超时）由环境开关控制以保持 CI 封闭：

  ```bash
  cargo test                          # 单元 + 对等测试
  DNS_SD_NETWORK_TESTS=1 cargo test   # + 真实网络端到端测试
  ```

  > **注意：** 网络测试依赖活跃网段与主机 mDNS 栈。macOS 应用防火墙可能对**新
  > 构建**的测试二进制首次运行静默丢弃入站 mDNS（直到二进制被批准），冷跑可能
  > 超时；重跑即可。这正是它们被挡在 CI 之外的原因。

- **iOS 权限映射测试**：`scripts/test-ios-dns-sd.sh` 把 `LocalNetworkPermission.swift`
  与对应 XCTest 隔离成独立 SwiftPM 包后在 macOS CI host 上运行（避免 Tauri
  UIKit 依赖）。

## 命名

| 概念 | 值 |
| --- | --- |
| Rust crate | `tauri-plugin-dns-sd` |
| Tauri 插件 id | `dns-sd` |
| iOS 类 / init | `DnsSdPlugin` / `init_plugin_dns_sd` |
| Android 包名 | `com.momics.dnssd` |

## 许可

MIT OR Apache-2.0

[mdns-sd]: https://crates.io/crates/mdns-sd
