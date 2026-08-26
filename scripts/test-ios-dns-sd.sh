#!/usr/bin/env bash
set -Eeuo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
PLUGIN_IOS="$ROOT/crates/tauri-plugin-dns-sd/ios"

# 该测试只验证权限错误映射，隔离 Tauri UIKit 依赖后才能在 macOS CI 的 SwiftPM host 上稳定运行。
TEST_PACKAGE=$(mktemp -d "${TMPDIR:-/tmp}/pisper-dns-sd.XXXXXX")
trap 'rm -rf "$TEST_PACKAGE"' EXIT
cp -R "$PLUGIN_IOS/Sources" "$TEST_PACKAGE/"
cp -R "$PLUGIN_IOS/Tests" "$TEST_PACKAGE/"
cat > "$TEST_PACKAGE/Package.swift" <<'EOF'
// swift-tools-version:5.3
import PackageDescription

let package = Package(
    name: "tauri-plugin-dns-sd",
    platforms: [.macOS(.v10_13), .iOS(.v13)],
    products: [
        .library(name: "tauri-plugin-dns-sd", type: .static, targets: ["tauri-plugin-dns-sd"]),
    ],
    targets: [
        .target(
            name: "tauri-plugin-dns-sd",
            path: "Sources",
            sources: ["LocalNetworkPermission.swift"]
        ),
        .testTarget(
            name: "DnsSdPluginTests",
            dependencies: ["tauri-plugin-dns-sd"],
            path: "Tests/DnsSdPluginTests"
        ),
    ]
)
EOF
swift test --package-path "$TEST_PACKAGE"
