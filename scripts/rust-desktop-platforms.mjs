// 桌面、CLI 暂存和发布矩阵共享此表；仅列出资源闭包已明确的平台。
export const RUST_DESKTOP_PLATFORMS = Object.freeze({
  'win32-x64': {
    platform: 'win32',
    arch: 'x64',
    rustTarget: 'x86_64-pc-windows-msvc',
    rustTargets: ['x86_64-pc-windows-msvc', 'x86_64-pc-windows-gnu'],
    runner: 'windows-latest',
    assetTarget: 'windows-x86_64',
    bundles: 'nsis',
    windowsOffline: true,
  },
  'darwin-arm64': {
    platform: 'darwin',
    arch: 'arm64',
    rustTarget: 'aarch64-apple-darwin',
    rustTargets: ['aarch64-apple-darwin'],
    runner: 'macos-15',
    assetTarget: 'darwin-aarch64',
    bundles: 'app,dmg',
    minimumSystemVersion: '14.0',
  },
  'darwin-x64': {
    platform: 'darwin',
    arch: 'x64',
    rustTarget: 'x86_64-apple-darwin',
    rustTargets: ['x86_64-apple-darwin'],
    runner: 'macos-15-intel',
    assetTarget: 'darwin-x86_64',
    bundles: 'app',
    // 锁定的 Intel ONNX Runtime 1.28.1 原生库要求 macOS 15.5。
    minimumSystemVersion: '15.5',
    simpleIntelMacDmg: true,
  },
  'linux-x64': {
    platform: 'linux',
    arch: 'x64',
    rustTarget: 'x86_64-unknown-linux-gnu',
    rustTargets: ['x86_64-unknown-linux-gnu'],
    runner: 'ubuntu-22.04',
    assetTarget: 'linux-x86_64',
    bundles: 'appimage,deb',
  },
})

export function resolveRustDesktopPlatform({
  platform = process.platform,
  arch = process.arch,
  rustTarget,
} = {}) {
  const profile = RUST_DESKTOP_PLATFORMS[`${platform}-${arch}`]
  if (!profile) throw new Error(`Unsupported Rust desktop platform: ${platform}-${arch}.`)
  const target = rustTarget ?? profile.rustTarget
  if (!profile.rustTargets.includes(target))
    throw new Error(`Rust host target ${target} does not match ${platform}-${arch}.`)
  return {
    ...profile,
    rustTarget: target,
    targetArgs: target.endsWith('-windows-gnu') ? ['--target', target] : [],
  }
}
