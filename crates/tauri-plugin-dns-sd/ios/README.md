# tauri-plugin-dns-sd（iOS）

DNS-SD 插件的 iOS 侧 Swift Package（`Package.swift`，依赖 Tauri 构建器生成的
`../.tauri/tauri-api`）。

- `Sources/DnsSdPlugin.swift` — `NWBrowser` / `NWListener`（Network.framework）
  实现浏览与通告，`NetService` 逐实例解析主机名 / 端口 / 地址，入口为
  `init_plugin_dns_sd`。
- `Sources/LocalNetworkPermission.swift` — 局域网权限拒绝的错误判定
  （DNS policy-denied / POSIX `EACCES` / `EPERM`）。
- `Tests/DnsSdPluginTests/` — 权限错误映射与非权限错误边界的 XCTest。

权限映射测试在本仓库通过 `scripts/test-ios-dns-sd.sh` 运行：脚本把
`LocalNetworkPermission.swift` 与测试隔离成独立 SwiftPM 包，在 macOS CI host
上执行（避免 Tauri UIKit 依赖）。完整插件需 Xcode 构建移动端 App 验证。
