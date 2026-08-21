// 初始化 Android 工程（tauri android init）并应用 Pisper 的清单定制：
// - CAMERA 权限（扫码配对）
// - networkSecurityConfig：仅放行 127.0.0.1/localhost 明文（壳内本地代理）
// gen/ 不入库，定制必须可重放——本脚本是幂等的，可重复运行。
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawnSync } from 'node:child_process'
import { assertAndroidEnv, resolveAndroidEnv } from './android-env.mjs'

const env = resolveAndroidEnv()

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const androidDir = join(root, 'src-tauri', 'gen', 'android')
const manifestPath = join(androidDir, 'app', 'src', 'main', 'AndroidManifest.xml')
const networkConfigPath = join(
  androidDir,
  'app',
  'src',
  'main',
  'res',
  'xml',
  'network_security_config.xml',
)

if (!existsSync(manifestPath)) {
  assertAndroidEnv(env)
  console.log('gen/android 不存在，运行 tauri android init …')
  const result = spawnSync('npx', ['tauri', 'android', 'init'], {
    cwd: root,
    stdio: 'inherit',
    shell: process.platform === 'win32',
    env,
  })
  if (result.status !== 0) {
    console.error('tauri android init 失败：请确认 JAVA_HOME / ANDROID_HOME / NDK_HOME 已配置。')
    process.exit(result.status ?? 1)
  }
}

// Tauri init 会写入模板 launcher，必须在工程生成后再覆盖移动端图标。
const iconResult = spawnSync(process.execPath, [join(root, 'scripts', 'sync-mobile-icons.mjs')], {
  cwd: root,
  stdio: 'inherit',
  env,
})
if (iconResult.status !== 0) process.exit(iconResult.status ?? 1)

const NETWORK_CONFIG = `<?xml version="1.0" encoding="utf-8"?>
<!--
  网络安全配置：仅放行回环地址的明文 HTTP（移动端本地代理 http://127.0.0.1:<port>）。
  出网流量仍然强制 TLS——代理转发到桌面端时做证书指纹锁定。
-->
<network-security-config>
    <base-config cleartextTrafficPermitted="false" />
    <domain-config cleartextTrafficPermitted="true">
        <domain includeSubdomains="false">127.0.0.1</domain>
        <domain includeSubdomains="false">localhost</domain>
    </domain-config>
</network-security-config>
`

mkdirSync(dirname(networkConfigPath), { recursive: true })
writeFileSync(networkConfigPath, NETWORK_CONFIG, 'utf8')

let manifest = readFileSync(manifestPath, 'utf8')
if (!manifest.includes('android.permission.CAMERA')) {
  manifest = manifest.replace(
    '<uses-permission android:name="android.permission.INTERNET" />',
    `<uses-permission android:name="android.permission.INTERNET" />
    <!-- 扫码配对需要相机权限（barcode-scanner 插件运行时申请） -->
    <uses-permission android:name="android.permission.CAMERA" />`,
  )
}
if (!manifest.includes('networkSecurityConfig')) {
  manifest = manifest.replace(
    'android:usesCleartextTraffic',
    'android:networkSecurityConfig="@xml/network_security_config"\n        android:usesCleartextTraffic',
  )
}
writeFileSync(manifestPath, manifest, 'utf8')

console.log('Android 工程已就绪（清单权限 + 网络安全配置已应用）。')
console.log('下一步：node scripts/build-mobile-android.mjs [--release] [--target aarch64]')
