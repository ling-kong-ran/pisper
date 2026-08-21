// 移动端 App 组件的发布归属路径：与 desktop/tui/runtime 组件体系完全独立
// （桌面端发布不携带 App）。release-app.mjs 与 prepare-app-release.mjs 共用。
export const APP_TAG_PREFIX = 'app-v'
export const APP_VERSION_FILE = 'src-tauri/mobile-package.json'

// App 独有路径：这些变更只触发 App 发版，与 desktop 组件无关。
const APP_OWNED_PREFIXES = [
  'src-tauri/src/mobile/',
  'src-tauri/permissions/mobile.toml',
  'src-tauri/capabilities/mobile-bridge.json',
  'src-tauri/tauri.android.conf.json',
  'src-tauri/mobile-package.json',
  'public/connect.html',
  'public/connect.js',
  'scripts/android-env.mjs',
  'scripts/setup-mobile-android.mjs',
  'scripts/build-mobile-android.mjs',
  'scripts/app-paths.mjs',
  'scripts/release-app.mjs',
  'scripts/prepare-app-release.mjs',
  '.github/workflows/release-app.yml',
]

export function normalizeRepoPath(path) {
  return String(path || '')
    .trim()
    .replaceAll('\\', '/')
    .replace(/^\.\//, '')
}

export function isAppOwnedPath(input) {
  const path = normalizeRepoPath(input)
  if (!path) return false
  return APP_OWNED_PREFIXES.some((prefix) =>
    prefix.endsWith('/') ? path.startsWith(prefix) : path === prefix,
  )
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
