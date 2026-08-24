#!/usr/bin/env bash
set -Eeuo pipefail

APK_PATH="${1:?用法：bash scripts/verify-android-page-size.sh <apk>}"
MIN_ALIGNMENT=$((16 * 1024))

if [[ ! -f "$APK_PATH" ]]; then
  echo "APK 不存在：$APK_PATH" >&2
  exit 1
fi

READELF="${ANDROID_READELF:-}"
if [[ -z "$READELF" ]]; then
  READELF="$(command -v llvm-readelf || command -v readelf || true)"
fi
if [[ -z "$READELF" ]]; then
  echo "缺少 llvm-readelf/readelf，无法校验 Android ELF 页对齐。" >&2
  exit 1
fi

ZIPALIGN="${ANDROID_ZIPALIGN:-}"
if [[ -z "$ZIPALIGN" ]]; then
  ZIPALIGN="$(command -v zipalign || true)"
fi
if [[ -z "$ZIPALIGN" && -n "${ANDROID_HOME:-}" ]]; then
  ZIPALIGN="$(find "$ANDROID_HOME/build-tools" -mindepth 2 -maxdepth 2 -type f \
    \( -name zipalign -o -name zipalign.exe \) -print 2>/dev/null | sort -V | tail -1)"
fi
if [[ -z "$ZIPALIGN" || ! -f "$ZIPALIGN" ]]; then
  echo "缺少支持 -P 16 的 zipalign；请安装 Android Build Tools 35+。" >&2
  exit 1
fi

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
unzip -q "$APK_PATH" 'lib/arm64-v8a/*.so' -d "$WORK_DIR"

for required in libnode.so libpisper_node_host.so libpisper_webview_lib.so; do
  if [[ ! -f "$WORK_DIR/lib/arm64-v8a/$required" ]]; then
    echo "APK 缺少 arm64 native 库：$required" >&2
    exit 1
  fi
done

mapfile -d '' LIBRARIES < <(find "$WORK_DIR/lib/arm64-v8a" -type f -name '*.so' -print0)
if (( ${#LIBRARIES[@]} == 0 )); then
  echo "APK 未包含 arm64 native 库。" >&2
  exit 1
fi

for library in "${LIBRARIES[@]}"; do
  mapfile -t ALIGNMENTS < <("$READELF" -lW "$library" | awk '$1 == "LOAD" { print $NF }')
  if (( ${#ALIGNMENTS[@]} == 0 )); then
    echo "无法读取 $(basename "$library") 的 ELF LOAD 段。" >&2
    exit 1
  fi
  for alignment in "${ALIGNMENTS[@]}"; do
    value=$((alignment))
    if (( value < MIN_ALIGNMENT )); then
      echo "$(basename "$library") 的 ELF LOAD 对齐仅为 $alignment，要求至少 0x4000。" >&2
      exit 1
    fi
  done
done

"$ZIPALIGN" -c -P 16 4 "$APK_PATH"
echo "Android 16 KB 页校验通过：${#LIBRARIES[@]} 个 arm64 native 库。"
