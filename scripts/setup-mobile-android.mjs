// 初始化 Android 工程（tauri android init）并应用 Pisper 的清单定制：
// - CAMERA 权限（扫码配对）
// - networkSecurityConfig：仅放行 127.0.0.1/localhost 明文（壳内本地代理）
// - WebView renderer 崩溃接管、宿主重建与当前路由恢复
// gen/ 不入库，定制必须可重放——本脚本是幂等的，可重复运行。
import {
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawnSync } from 'node:child_process'
import { assertAndroidEnv, resolveAndroidEnv } from './android-env.mjs'
import { stageAndroidSpeechRuntime } from './stage-android-speech-runtime.mjs'

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
const mainActivitySourcePath = join(root, 'src-tauri', 'mobile', 'android', 'MainActivity.kt')
const mainActivityTargetPath = join(
  androidDir,
  'app',
  'src',
  'main',
  'java',
  'com',
  'lingkongran',
  'pisper',
  'MainActivity.kt',
)
const rustBuildTaskPath = join(
  androidDir,
  'buildSrc',
  'src',
  'main',
  'java',
  'com',
  'lingkongran',
  'pisper',
  'kotlin',
  'BuildTask.kt',
)
const nodeMobileDir = process.env.PISPER_NODE_MOBILE_ANDROID_DIR
  ? join(process.env.PISPER_NODE_MOBILE_ANDROID_DIR)
  : ''
const sherpaAar = join(
  root,
  'src-tauri',
  'mobile-device-plugin',
  'android',
  'libs',
  'sherpa-onnx.aar',
)
const requireEmbeddedNode = process.env.PISPER_REQUIRE_EMBEDDED_NODE === '1'

function resolveAndroidCppRuntime(ndkHome) {
  const prebuiltRoot = join(ndkHome || '', 'toolchains', 'llvm', 'prebuilt')
  if (!existsSync(prebuiltRoot)) return ''
  for (const entry of readdirSync(prebuiltRoot, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue
    const candidate = join(
      prebuiltRoot,
      entry.name,
      'sysroot',
      'usr',
      'lib',
      'aarch64-linux-android',
      'libc++_shared.so',
    )
    if (existsSync(candidate)) return candidate
  }
  return ''
}

if (!existsSync(sherpaAar)) {
  // CI 的干净检出没有本地缓存的 AAR；使用与发布构建相同的下载和校验路径。
  console.log('缺少 sherpa AAR，自动下载并校验官方 Android Runtime …')
  await stageAndroidSpeechRuntime({ root })
}

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
for (const permission of [
  'android.permission.RECORD_AUDIO',
  'android.permission.MODIFY_AUDIO_SETTINGS',
  'android.permission.READ_CONTACTS',
  'android.permission.ACCESS_COARSE_LOCATION',
  'android.permission.ACCESS_FINE_LOCATION',
  'android.permission.READ_EXTERNAL_STORAGE',
  'android.permission.READ_MEDIA_IMAGES',
  'android.permission.READ_MEDIA_VISUAL_USER_SELECTED',
  'android.permission.VIBRATE',
  'android.permission.POST_NOTIFICATIONS',
  'android.permission.ACCESS_LOCAL_NETWORK',
  'android.permission.ACCESS_NETWORK_STATE',
]) {
  if (!manifest.includes(permission)) {
    manifest = manifest.replace(
      '    <application',
      `    <uses-permission android:name="${permission}" />\n    <application`,
    )
  }
}
if (!manifest.includes('networkSecurityConfig')) {
  manifest = manifest.replace(
    'android:usesCleartextTraffic',
    'android:networkSecurityConfig="@xml/network_security_config"\n        android:usesCleartextTraffic',
  )
}
if (!manifest.includes('android:windowSoftInputMode="adjustResize"')) {
  if (manifest.includes('android:windowSoftInputMode=')) {
    manifest = manifest.replace(
      /android:windowSoftInputMode="[^"]*"/,
      'android:windowSoftInputMode="adjustResize"',
    )
  } else {
    manifest = manifest.replace(
      'android:launchMode="singleTask"',
      'android:launchMode="singleTask"\n            android:windowSoftInputMode="adjustResize"',
    )
  }
}
writeFileSync(manifestPath, manifest, 'utf8')

// Wry 的扩展点在 Rust 构建时生成 WebViewClient；必须把环境变量放进 Gradle 启动 cargo 的进程。
const webViewClientExtension = `override fun onRenderProcessGone(
    view: WebView,
    detail: RenderProcessGoneDetail
): Boolean {
    return MainActivity.recoverFromRendererCrash(view, currentUrl, detail.didCrash())
}`
const rustBuildTask = readFileSync(rustBuildTaskPath, 'utf8')
const rustBuildTaskEol = rustBuildTask.includes('\r\n') ? '\r\n' : '\n'
const rustPageSizeBlock = [
  '            // Pisper：确保 Rust 宿主兼容 Android 16 KB 内存页。',
  '            environment(',
  '                "CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS",',
  '                (System.getenv("CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS").orEmpty() +',
  '                    " -C link-arg=-Wl,-z,max-page-size=16384").trim(),',
  '            )',
].join(rustBuildTaskEol)
const existingRustPageSizeBlock =
  /            \/\/ Pisper：确保 Rust 宿主兼容 Android 16 KB 内存页。\r?\n            environment\([\s\S]*?\r?\n            \)/
let updatedRustBuildTask
if (existingRustPageSizeBlock.test(rustBuildTask)) {
  updatedRustBuildTask = rustBuildTask.replace(existingRustPageSizeBlock, rustPageSizeBlock)
} else {
  updatedRustBuildTask = rustBuildTask.replace(
    /(            workingDir\(File\(project\.projectDir, rootDirRel\)\)\r?\n)/,
    `$1${rustPageSizeBlock}${rustBuildTaskEol}`,
  )
}

const rendererRecoveryBlock = [
  '            // Pisper：注入 renderer 崩溃恢复回调。',
  '            environment(',
  '                "WRY_RUSTWEBVIEWCLIENT_CLASS_EXTENSION",',
  '                """',
  ...webViewClientExtension.split('\n').map((line) => `                ${line}`),
  '                """.trimIndent(),',
  '            )',
].join(rustBuildTaskEol)
const existingRecoveryBlock =
  /            \/\/ Pisper：注入 renderer 崩溃恢复回调。\r?\n            environment\([\s\S]*?\r?\n            \)/
if (existingRecoveryBlock.test(updatedRustBuildTask)) {
  updatedRustBuildTask = updatedRustBuildTask.replace(existingRecoveryBlock, rendererRecoveryBlock)
} else {
  updatedRustBuildTask = updatedRustBuildTask.replace(
    /(            workingDir\(File\(project\.projectDir, rootDirRel\)\)\r?\n)/,
    `$1${rendererRecoveryBlock}${rustBuildTaskEol}`,
  )
}
if (!updatedRustBuildTask.includes('CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS')) {
  throw new Error('无法把 16 KB 页 linker 参数接入 Android Rust 构建。')
}
if (!updatedRustBuildTask.includes('WRY_RUSTWEBVIEWCLIENT_CLASS_EXTENSION')) {
  throw new Error('无法把 WebView renderer 恢复回调接入 Android Rust 构建。')
}
writeFileSync(rustBuildTaskPath, updatedRustBuildTask, 'utf8')
mkdirSync(dirname(mainActivityTargetPath), { recursive: true })
copyFileSync(mainActivitySourcePath, mainActivityTargetPath)

// gen/ 不入库；只有固定产物已经过校验并 staged 时才接入 C++ Node 宿主。
const nodeLibrary = nodeMobileDir ? join(nodeMobileDir, 'arm64-v8a', 'libnode.so') : ''
const nodeHeaders = nodeMobileDir ? join(nodeMobileDir, 'include') : ''
if (nodeLibrary && existsSync(nodeLibrary) && existsSync(nodeHeaders)) {
  const cppRuntime = resolveAndroidCppRuntime(env.NDK_HOME)
  if (!cppRuntime) {
    throw new Error('固定 Android NDK 缺少 arm64 libc++_shared.so。')
  }
  const appDir = join(androidDir, 'app')
  const mainDir = join(appDir, 'src', 'main')
  const hostSource = join(root, 'src-tauri', 'mobile', 'node-host', 'android')
  const hostTarget = join(mainDir, 'cpp', 'pisper-node-host')
  const headersTarget = join(mainDir, 'cpp', 'node-mobile', 'include')
  const jniTarget = join(mainDir, 'jniLibs', 'arm64-v8a')
  const kotlinTarget = join(mainDir, 'java', 'com', 'lingkongran', 'pisper')
  const assetsTarget = join(mainDir, 'assets')
  for (const directory of [hostTarget, headersTarget, jniTarget, kotlinTarget, assetsTarget]) {
    mkdirSync(directory, { recursive: true })
  }
  cpSync(nodeHeaders, headersTarget, { recursive: true, force: true })
  for (const name of ['CMakeLists.txt', 'node_host.cpp']) {
    copyFileSync(join(hostSource, name), join(hostTarget, name))
  }
  copyFileSync(join(hostSource, 'EmbeddedNodeHost.kt'), join(kotlinTarget, 'EmbeddedNodeHost.kt'))
  // Rust 通过 JNI 字符串查找入口，R8 无法静态发现；release 构建必须保留类名和方法签名。
  copyFileSync(join(hostSource, 'pisper-node-host.pro'), join(appDir, 'pisper-node-host.pro'))
  copyFileSync(nodeLibrary, join(jniTarget, 'libnode.so'))
  // Node Mobile 动态依赖 NDK libc++；Android 系统镜像不会提供这个 App 私有 ABI 库。
  copyFileSync(cppRuntime, join(jniTarget, 'libc++_shared.so'))
  for (const [source, target] of [
    ['pisper-node-artifact.json', 'pisper-embedded-node-android.json'],
    ['LICENSE.nodejs', 'LICENSE.nodejs-mobile'],
  ]) {
    copyFileSync(join(nodeMobileDir, source), join(assetsTarget, target))
  }

  const buildGradlePath = join(appDir, 'build.gradle.kts')
  let buildGradle = readFileSync(buildGradlePath, 'utf8')
  if (!buildGradle.includes('src/main/cpp/pisper-node-host/CMakeLists.txt')) {
    const eol = buildGradle.includes('\r\n') ? '\r\n' : '\n'
    buildGradle = buildGradle.replace(
      /    buildFeatures \{\r?\n        buildConfig = true\r?\n    \}\r?\n\}/,
      [
        '    buildFeatures {',
        '        buildConfig = true',
        '    }',
        '    externalNativeBuild {',
        '        cmake {',
        '            path = file("src/main/cpp/pisper-node-host/CMakeLists.txt")',
        '            version = "3.22.1"',
        '        }',
        '    }',
        '}',
      ].join(eol),
    )
  }
  if (!buildGradle.includes('src/main/cpp/pisper-node-host/CMakeLists.txt')) {
    throw new Error('无法把嵌入式 Node CMake 宿主接入 Android Gradle 工程。')
  }
  writeFileSync(buildGradlePath, buildGradle, 'utf8')
  console.log('已接入固定 Node 24 Android arm64 宿主。')
} else if (requireEmbeddedNode) {
  throw new Error('缺少已校验的 Android embedded Node staging。')
}

// gen/ 不入库，重复构建时也要同步宿主入口，避免 Rust 调用到旧的 Kotlin ABI。
const hostSourcePath = join(
  root,
  'src-tauri',
  'mobile',
  'node-host',
  'android',
  'EmbeddedNodeHost.kt',
)
const hostTargetPath = join(
  androidDir,
  'app',
  'src',
  'main',
  'java',
  'com',
  'lingkongran',
  'pisper',
  'EmbeddedNodeHost.kt',
)
if (existsSync(hostSourcePath) && existsSync(hostTargetPath)) {
  copyFileSync(hostSourcePath, hostTargetPath)
}

console.log('Android 工程已就绪（权限、网络安全与 WebView renderer 恢复已应用）。')
console.log('下一步：node scripts/build-mobile-android.mjs [--release] [--target aarch64]')
