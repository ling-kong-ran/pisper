#!/usr/bin/env bash
set -Eeuo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
PLUGIN_IOS="$ROOT/crates/tauri-plugin-dns-sd/ios"
CARGO_REGISTRY="${CARGO_HOME:-$HOME/.cargo}/registry/src"

cargo fetch --manifest-path "$ROOT/src-tauri/Cargo.toml"
TAURI_API_PACKAGE=$(find "$CARGO_REGISTRY" -path '*/tauri-*/mobile/ios-api/Package.swift' -print \
  | tail -1)
if [[ -z "$TAURI_API_PACKAGE" ]]; then
  echo '未找到 Tauri iOS API Swift package。' >&2
  exit 1
fi

# 测试使用与当前 Cargo 依赖一致的 Tauri Swift API，避免依赖已生成的 Xcode 工程。
TEST_PACKAGE=$(mktemp -d "${TMPDIR:-/tmp}/pisper-dns-sd.XXXXXX")
trap 'rm -rf "$TEST_PACKAGE"' EXIT
cp -R "$PLUGIN_IOS/." "$TEST_PACKAGE/"
rm -rf "$TEST_PACKAGE/.tauri"
TAURI_API_TARGET="$TEST_PACKAGE/.tauri/tauri-api"
mkdir -p "$TAURI_API_TARGET"
cp -R "$(dirname "$TAURI_API_PACKAGE")/." "$TAURI_API_TARGET/"
test -f "$TAURI_API_TARGET/Package.swift"
swift test --package-path "$TEST_PACKAGE"
