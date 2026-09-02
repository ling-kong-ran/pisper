# Pisper 补丁说明

本目录基于 `tauri-plugin-dns-sd 0.1.0`，上游仓库为
<https://github.com/momics/dns-sd>，按 `MIT OR Apache-2.0` 许可保留。

Pisper 暂时 vendoring 该版本，是因为其 Android 原生参数模型与 Rust 移动桥不一致：
Rust 使用 `serde(flatten)` 将 `BrowseOptions` 序列化为顶层 `service` 和 `timeoutMs`，
但上游 Android `BrowseStartArgs` 错误地从 `options.service` 读取，因此会静默回退到
`_http._tcp`。本地补丁让 Android 与 Rust 及 iOS 使用同一展平协议。

Android `NsdManager` 会把回调中的服务类型规范化为 `_pisper._tcp.local` 等形式，
上游实现却再次与请求字符串 `_pisper._tcp` 做严格比较，导致已发现的服务被静默丢弃。
本地补丁移除这项冗余过滤，由 `NsdManager` 保证回调只包含请求的服务类型。
同时避免在 Android 主线程读取会触发反向 DNS 的 `InetAddress.hostName`，并在生成
FQDN 前规范化系统可能已经补上的 `.local` 后缀。

iOS `NWBrowser` 在用户拒绝局域网权限时会进入 `waiting` 或 `failed`，并返回
DNS policy-denied 或 POSIX 权限错误。本地补丁统一发送 `permission-denied` 停止原因，
让 App 显示明确的系统设置提示；Swift XCTest 覆盖这些错误映射及非权限错误边界，
在本仓库 CI 中经 `scripts/test-ios-dns-sd.sh` 运行。

上游发布包含等效修复的版本后，应恢复 crates.io 依赖，并删除本目录。
