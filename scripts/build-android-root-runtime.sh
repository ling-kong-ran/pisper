#!/usr/bin/env bash
# 为已 root 的 Android 设备构建最小 Linux 用户空间；仅支持在同架构 Linux 主机上构建。
set -Eeuo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ARCH=${PISPER_ROOT_RUNTIME_ARCH:-arm64}
APP_VERSION=${PISPER_APP_VERSION:-dev}
UBUNTU_VERSION=${PISPER_UBUNTU_BASE_VERSION:-24.04.3}
WORK_DIR=${PISPER_ROOT_RUNTIME_WORK_DIR:-"$ROOT_DIR/release/android-root-runtime-work"}
OUTPUT=${PISPER_ROOT_RUNTIME_OUTPUT:-"$ROOT_DIR/release/pisper-root-runtime-android-${ARCH}.tar.gz"}

case "$ARCH" in
  arm64)
    HOST_ARCH=aarch64
    BASE_ARCH=arm64
    ;;
  x86_64)
    HOST_ARCH=x86_64
    BASE_ARCH=amd64
    ;;
  *)
    echo "不支持的 Android root Runtime 架构：$ARCH" >&2
    exit 64
    ;;
esac

if [ "$(uname -m)" != "$HOST_ARCH" ]; then
  echo "必须在 $HOST_ARCH Linux 主机上构建 $ARCH root Runtime。" >&2
  exit 65
fi

SEA_DIR="$ROOT_DIR/release/sea"
SIDECAR="$SEA_DIR/pisper-sidecar"
RUNTIME_DIR="$SEA_DIR/runtime"
if [ ! -x "$SIDECAR" ] || [ ! -d "$RUNTIME_DIR" ] || [ ! -d "$ROOT_DIR/dist" ]; then
  echo "缺少 SEA 或前端产物；请先运行 npm run sidecar:sea。" >&2
  exit 66
fi

BASE_NAME="ubuntu-base-${UBUNTU_VERSION}-base-${BASE_ARCH}.tar.gz"
BASE_URL="https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release/${BASE_NAME}"
BASE_ARCHIVE="$WORK_DIR/$BASE_NAME"
ROOTFS="$WORK_DIR/rootfs"

rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR" "$(dirname "$OUTPUT")"
curl --fail --location --retry 3 --output "$BASE_ARCHIVE" "$BASE_URL"
mkdir -p "$ROOTFS"
sudo tar --numeric-owner -xpf "$BASE_ARCHIVE" -C "$ROOTFS"

# 构建机与 Android 运行时都不应继承 systemd-resolved 的回环 DNS 配置。
printf 'nameserver 1.1.1.1\nnameserver 8.8.8.8\n' | sudo tee "$ROOTFS/etc/resolv.conf" >/dev/null
sudo chroot "$ROOTFS" /usr/bin/env DEBIAN_FRONTEND=noninteractive apt-get update
sudo chroot "$ROOTFS" /usr/bin/env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
  bash ca-certificates curl fd-find git locales ripgrep util-linux
sudo ln -s /usr/bin/fdfind "$ROOTFS/usr/local/bin/fd"

sudo mkdir -p "$ROOTFS/opt/pisper" "$ROOTFS/data/agent" "$ROOTFS/workspace" \
  "$ROOTFS/proc" "$ROOTFS/dev" "$ROOTFS/sys"
sudo cp "$SIDECAR" "$ROOTFS/opt/pisper/pisper-sidecar"
sudo cp -a "$RUNTIME_DIR" "$ROOTFS/opt/pisper/sidecar-runtime"
sudo cp -a "$ROOT_DIR/dist" "$ROOTFS/opt/pisper/dist"
printf '{"appVersion":"%s","arch":"%s","ubuntu":"%s"}\n' \
  "$APP_VERSION" "$ARCH" "$UBUNTU_VERSION" | sudo tee "$ROOTFS/opt/pisper/root-runtime.json" >/dev/null

# 运行时根目录只读；可变数据仅放在 /data 与 /workspace，启动时再归属 App UID。
sudo chown -R 0:0 "$ROOTFS"
sudo chmod 0755 "$ROOTFS/opt/pisper/pisper-sidecar"
sudo chmod -R go-w "$ROOTFS/opt/pisper"
sudo chroot "$ROOTFS" apt-get clean
sudo rm -rf "$ROOTFS/var/lib/apt/lists/"* "$ROOTFS/tmp/"* "$ROOTFS/var/tmp/"* \
  "$ROOTFS/var/cache/apt/archives/"*.deb "$ROOTFS/etc/machine-id"
sudo find "$ROOTFS/var/log" -type f -exec truncate -s 0 {} +

rm -f "$OUTPUT"
sudo tar --numeric-owner --xattrs --acls -czf "$OUTPUT" -C "$ROOTFS" .
sudo chown "$(id -u):$(id -g)" "$OUTPUT"
gzip -t "$OUTPUT"
printf '已生成 %s（%s bytes）\n' "$OUTPUT" "$(stat -c %s "$OUTPUT")"
