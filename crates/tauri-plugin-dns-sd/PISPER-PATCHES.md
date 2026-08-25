# Pisper 补丁说明

本目录基于 `tauri-plugin-dns-sd 0.1.0`，上游仓库为
<https://github.com/momics/dns-sd>，按 `MIT OR Apache-2.0` 许可保留。

Pisper 暂时 vendoring 该版本，是因为其 Android 原生参数模型与 Rust 移动桥不一致：
Rust 使用 `serde(flatten)` 将 `BrowseOptions` 序列化为顶层 `service` 和 `timeoutMs`，
但上游 Android `BrowseStartArgs` 错误地从 `options.service` 读取，因此会静默回退到
`_http._tcp`。本地补丁让 Android 与 Rust 及 iOS 使用同一展平协议。

上游发布包含等效修复的版本后，应恢复 crates.io 依赖，并删除本目录。
