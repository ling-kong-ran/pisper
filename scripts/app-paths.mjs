// 移动端 App 保持独立版本和产物通道，但由统一 release 命令自动检测。
// 桌面端发布不携带 App；release 编排与 App 版本准备共用这些路径。
export const APP_TAG_PREFIX = 'app-v'
export const APP_VERSION_FILE = 'src-tauri/mobile-package.json'

// App 独有路径：这些变更只触发 App 发版，与 desktop 组件无关。
const APP_EXCLUSIVE_PREFIXES = [
  'src-tauri/src/mobile/',
  'src-tauri/mobile/',
  'src-tauri/mobile-device-plugin/',
  'src-tauri/icons/android/',
  'src-tauri/icons/ios/',
  'src-tauri/permissions/mobile.toml',
  'src-tauri/capabilities/mobile-bridge.json',
  'src-tauri/tauri.android.conf.json',
  'src-tauri/tauri.mobile-ios.conf.json',
  'src-tauri/Info.ios.plist',
  'src-tauri/mobile-package.json',
  'public/mobile-startup.html',
  'scripts/android-env.mjs',
  'scripts/setup-mobile-android.mjs',
  'scripts/build-mobile-android.mjs',
  'scripts/build-android-root-runtime.sh',
  'scripts/build-mobile-node-ios.sh',
  'scripts/build-mobile-runtime.mjs',
  'scripts/mobile-node-artifacts.json',
  'scripts/mobile-node-ios-smoke-view-controller.m',
  'scripts/smoke-mobile-node-ios.sh',
  'scripts/stage-mobile-node-android.mjs',
  'scripts/stage-mobile-node-ios.mjs',
  'scripts/sync-mobile-icons.mjs',
  'scripts/verify-android-page-size.sh',
  'scripts/verify-tauri-signature.mjs',
  'scripts/app-paths.mjs',
  'scripts/prepare-app-release.mjs',
  '.github/workflows/release-app.yml',
  '.github/workflows/build-store-app.yml',
]

// 这些 Rust 文件由 Desktop 与 App 共用，两个发布通道都必须看到它们的变更。
const APP_SHARED_PREFIXES = ['src/', 'public/', 'runtime/', 'shared/']

const APP_SHARED_FILES = new Set([
  'package.json',
  'package-lock.json',
  'index.html',
  'vite.config.ts',
  'scripts/build-sea.mjs',
  'scripts/patch-pi-mobile-compat.mjs',
  'scripts/sea-runtime.mjs',
  'scripts/stage-runtime-closure.mjs',
  'src-tauri/Cargo.lock',
  'src-tauri/Cargo.toml',
  'src-tauri/src/iroh_tunnel.rs',
  'src-tauri/src/lib.rs',
])

export function normalizeRepoPath(path) {
  return String(path || '')
    .trim()
    .replaceAll('\\', '/')
    .replace(/^\.\//, '')
}

export function isAppExclusivePath(input) {
  const path = normalizeRepoPath(input)
  if (!path) return false
  return APP_EXCLUSIVE_PREFIXES.some((prefix) =>
    prefix.endsWith('/') ? path.startsWith(prefix) : path === prefix,
  )
}

export function isAppOwnedPath(input) {
  const path = normalizeRepoPath(input)
  if (isAppExclusivePath(path) || APP_SHARED_FILES.has(path)) return true
  if (path.startsWith('runtime/tests/')) return false
  return APP_SHARED_PREFIXES.some((prefix) => path.startsWith(prefix))
}

export function appTagPattern() {
  return /^app-v\d+\.\d+\.\d+$/
}

export function appReleaseTag(version) {
  return `${APP_TAG_PREFIX}${version}`
}

export function appVersionFromTag(tag) {
  return appTagPattern().test(String(tag || '')) ? String(tag).slice(APP_TAG_PREFIX.length) : ''
}
