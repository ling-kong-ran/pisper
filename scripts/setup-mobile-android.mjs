// 初始化 Android 工程（tauri android init）并应用 Pisper 的清单定制：
// - 相机与麦克风权限（扫码、附件与语音输入）
// - networkSecurityConfig：仅放行 127.0.0.1/localhost 明文（壳内本地代理）
// - Wry 媒体授权的实际代理来源校验，并保留原生文件选择等完整回调
// - WebView renderer 崩溃接管、宿主重建与当前路由恢复
// gen/ 不入库，定制必须可重放——本脚本是幂等的，可重复运行。
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawnSync } from 'node:child_process'
import { assertAndroidEnv, resolveAndroidEnv } from './android-env.mjs'
import {
  patchWryAndroidWebChromeClient,
  patchWryAndroidWebViewClient,
} from './patch-wry-android.mjs'
import { stageAndroidSpeechRuntime } from './stage-android-speech-runtime.mjs'
import { enableAndroidSpeechDesugaring } from './android-speech-desugaring.mjs'
import { enableAndroidReleaseNativeStripping } from './android-native-packaging.mjs'
import { ANDROID_WEBVIEW_RECOVERY } from './mobile-rust-env.mjs'
import { stageAndroidTls } from './stage-android-tls.mjs'

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
const rustPluginPath = join(
  androidDir,
  'buildSrc',
  'src',
  'main',
  'java',
  'com',
  'lingkongran',
  'pisper',
  'kotlin',
  'RustPlugin.kt',
)
const rustWebChromeClientPath = join(
  androidDir,
  'app',
  'src',
  'main',
  'java',
  'com',
  'lingkongran',
  'pisper',
  'generated',
  'RustWebChromeClient.kt',
)
const sherpaAar = join(
  root,
  'src-tauri',
  'mobile-device-plugin',
  'android',
  'libs',
  'sherpa-onnx.aar',
)

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

// Wry 的 Kotlin 文件由 cargo 构建脚本生成；先生成并应用安全补丁，再允许对应 variant 编译 Kotlin。
let rustPlugin = readFileSync(rustPluginPath, 'utf8')
const rustPluginEol = rustPlugin.includes('\r\n') ? '\r\n' : '\n'
const universalCompileDependency = [
  '                tasks["compileUniversal${profileCapitalized}Kotlin"].dependsOn(buildTask)',
].join(rustPluginEol)
if (!rustPlugin.includes(universalCompileDependency)) {
  rustPlugin = rustPlugin.replace(
    '                tasks["mergeUniversal${profileCapitalized}JniLibFolders"].dependsOn(buildTask)',
    [
      '                tasks["mergeUniversal${profileCapitalized}JniLibFolders"].dependsOn(buildTask)',
      universalCompileDependency,
    ].join(rustPluginEol),
  )
}
const targetCompileDependency = [
  '                    tasks["compile$targetArchCapitalized${profileCapitalized}Kotlin"].dependsOn(',
  '                        targetBuildTask',
  '                    )',
].join(rustPluginEol)
if (!rustPlugin.includes(targetCompileDependency)) {
  rustPlugin = rustPlugin.replace(
    [
      '                    tasks["merge$targetArchCapitalized${profileCapitalized}JniLibFolders"].dependsOn(',
      '                        targetBuildTask',
      '                    )',
    ].join(rustPluginEol),
    [
      '                    tasks["merge$targetArchCapitalized${profileCapitalized}JniLibFolders"].dependsOn(',
      '                        targetBuildTask',
      '                    )',
      targetCompileDependency,
    ].join(rustPluginEol),
  )
}
if (
  !rustPlugin.includes(universalCompileDependency) ||
  !rustPlugin.includes(targetCompileDependency)
) {
  throw new Error('无法让 Android Kotlin 编译等待 Wry 源码安全补丁。')
}
writeFileSync(rustPluginPath, rustPlugin, 'utf8')

// Wry 的扩展点在 Rust 构建时生成 WebViewClient；必须把环境变量放进 Gradle 启动 cargo 的进程。
const webViewClientExtension = ANDROID_WEBVIEW_RECOVERY
const rustBuildTask = readFileSync(rustBuildTaskPath, 'utf8')
const rustBuildTaskEol = rustBuildTask.includes('\r\n') ? '\r\n' : '\n'
const rustPageSizeBlock = [
  '            // Pisper：确保 Rust 宿主兼容 Android 16 KB 内存页。',
  '            environment(',
  '                "CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS",',
  '                System.getenv("CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS").orEmpty().let { flags ->',
  '                    val pageFlags = if (flags.contains("-Wl,-z,max-page-size=16384")) flags else ("-C link-arg=-Wl,-z,max-page-size=16384 " + flags).trim()',
  '                    listOf("-Clink-arg=-landroid", "-Clink-arg=-llog", "-Clink-arg=-lOpenSLES").fold(pageFlags) { result, flag ->',
  '                        if (result.split(" ").contains(flag)) result else "$result $flag"',
  '                    }',
  '                },',
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
const wryOriginPatchBlock = [
  '        // Pisper：Wry 每次 cargo 构建都会重写 Kotlin，随后重放来源校验。',
  '        project.exec {',
  '            workingDir(File(project.projectDir, rootDirRel).canonicalFile.parentFile)',
  '            executable("node")',
  '            args(',
  '                "scripts/patch-wry-android.mjs",',
  '                File(',
  '                    project.projectDir,',
  '                    "src/main/java/com/lingkongran/pisper/generated/RustWebChromeClient.kt",',
  '                ).absolutePath,',
  '            )',
  '        }.assertNormalExitValue()',
].join(rustBuildTaskEol)
const existingWryOriginPatchBlock =
  /        \/\/ Pisper：Wry 每次 cargo 构建都会重写 Kotlin，随后重放来源校验。\r?\n        project\.exec \{[\s\S]*?\r?\n        \}\.assertNormalExitValue\(\)/
if (existingWryOriginPatchBlock.test(updatedRustBuildTask)) {
  updatedRustBuildTask = updatedRustBuildTask.replace(
    existingWryOriginPatchBlock,
    wryOriginPatchBlock,
  )
} else {
  const taskEnd = ['        }.assertNormalExitValue()', '    }', '}'].join(rustBuildTaskEol)
  if (!updatedRustBuildTask.includes(taskEnd)) {
    throw new Error('无法在 Android Rust 构建后接入 Wry 媒体来源校验。')
  }
  updatedRustBuildTask = updatedRustBuildTask.replace(
    taskEnd,
    ['        }.assertNormalExitValue()', wryOriginPatchBlock, '    }', '}'].join(rustBuildTaskEol),
  )
}
if (!updatedRustBuildTask.includes('CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS')) {
  throw new Error('无法把 16 KB 页 linker 参数接入 Android Rust 构建。')
}
if (!updatedRustBuildTask.includes('WRY_RUSTWEBVIEWCLIENT_CLASS_EXTENSION')) {
  throw new Error('无法把 WebView renderer 恢复回调接入 Android Rust 构建。')
}
if (!updatedRustBuildTask.includes('scripts/patch-wry-android.mjs')) {
  throw new Error('无法把 Wry 媒体来源校验接入 Android Rust 构建。')
}
writeFileSync(rustBuildTaskPath, updatedRustBuildTask, 'utf8')
if (existsSync(rustWebChromeClientPath)) {
  patchWryAndroidWebChromeClient(rustWebChromeClientPath)
}
const rustWebViewClientPath = join(dirname(rustWebChromeClientPath), 'RustWebViewClient.kt')
if (existsSync(rustWebViewClientPath)) {
  patchWryAndroidWebViewClient(rustWebViewClientPath)
}
mkdirSync(dirname(mainActivityTargetPath), { recursive: true })
copyFileSync(mainActivitySourcePath, mainActivityTargetPath)

// 清理旧生成工程的 Node 接线，保留原生语音与 WebView 配置。
const oldGradlePath = join(androidDir, 'app/build.gradle.kts')
await stageAndroidTls(root, join(androidDir, 'app'))
// 保留 Android 原生 C++ 运行库；其来源是当前 NDK，与已移除的 Node 宿主无关。
const ndkHost =
  process.platform === 'darwin'
    ? 'darwin-x86_64'
    : process.platform === 'win32'
      ? 'windows-x86_64'
      : 'linux-x86_64'
const cppRuntime = join(
  env.NDK_HOME,
  'toolchains/llvm/prebuilt',
  ndkHost,
  'sysroot/usr/lib/aarch64-linux-android/libc++_shared.so',
)
if (!existsSync(cppRuntime)) throw new Error('Android NDK is missing the arm64 C++ runtime')
const jniDirectory = join(androidDir, 'app/src/main/jniLibs/arm64-v8a')
mkdirSync(jniDirectory, { recursive: true })
copyFileSync(cppRuntime, join(jniDirectory, 'libc++_shared.so'))
let oldGradle = readFileSync(oldGradlePath, 'utf8')
const tlsDependency = '    implementation(files("libs/rustls-platform-verifier.aar"))'
if (!oldGradle.includes(tlsDependency))
  oldGradle = oldGradle.replace('dependencies {', `dependencies {\n${tlsDependency}`)
const proguardPath = join(androidDir, 'app/proguard-rules.pro')
const tlsKeep = '-keep, includedescriptorclasses class org.rustls.platformverifier.** { *; }'
const proguard = existsSync(proguardPath) ? readFileSync(proguardPath, 'utf8') : ''
if (!proguard.includes(tlsKeep)) writeFileSync(proguardPath, `${proguard}\n${tlsKeep}\n`)
oldGradle = oldGradle.replace(
  /\n\s*externalNativeBuild\s*\{\s*cmake\s*\{[^{}]*pisper-node-host[^{}]*\}\s*\}/g,
  '',
)
oldGradle = oldGradle.replace(/,?\s*"pisper-node-host\.pro"/g, '')
oldGradle = enableAndroidReleaseNativeStripping(oldGradle)
writeFileSync(oldGradlePath, oldGradle)
for (const relative of [
  'app/src/main/cpp/pisper-node-host',
  'app/src/main/cpp/node-mobile',
  'app/src/main/java/com/lingkongran/pisper/EmbeddedNodeHost.kt',
  'app/src/main/jniLibs/arm64-v8a/libnode.so',
  'app/src/main/jniLibs/arm64-v8a/libpisper_node_host.so',
  'app/src/main/assets/pisper-embedded-node-android.json',
  'app/src/main/assets/LICENSE.nodejs-mobile',
  'app/pisper-node-host.pro',
]) {
  rmSync(join(androidDir, relative), { recursive: true, force: true })
}

const speechGradlePath = join(androidDir, 'app', 'build.gradle.kts')
const speechGradle = readFileSync(speechGradlePath, 'utf8')
const compatibleSpeechGradle = enableAndroidSpeechDesugaring(speechGradle)
if (speechGradle !== compatibleSpeechGradle)
  writeFileSync(speechGradlePath, compatibleSpeechGradle, 'utf8')

console.log('Android 工程已就绪（权限、网络安全与 WebView renderer 恢复已应用）。')
console.log('下一步：node scripts/build-mobile-android.mjs [--release] [--target aarch64]')
