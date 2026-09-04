// 构建移动端 APK：先产出前端 dist，再调用 tauri android build。
// 移动端不打包 sidecar/外部资源，用 --config 覆盖掉桌面专属的产物声明。
// 用法：node scripts/build-mobile-android.mjs [--release] [--target x86_64|aarch64|...]
// 环境：需要 JAVA_HOME / ANDROID_HOME / NDK_HOME（Windows 上 symlink 需要开发者模式）。
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawnSync } from 'node:child_process'
import { assertAndroidEnv, resolveAndroidEnv } from './android-env.mjs'
import { stageAndroidSpeechModel } from './stage-android-speech-model.mjs'
import { stageAndroidSpeechRuntime } from './stage-android-speech-runtime.mjs'

const env = resolveAndroidEnv()

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const release = process.argv.includes('--release')
const mobileVersion = JSON.parse(
  readFileSync(join(root, 'src-tauri', 'mobile-package.json'), 'utf8'),
).version
const targetIndex = process.argv.indexOf('--target')
const target = targetIndex >= 0 ? process.argv[targetIndex + 1] : 'aarch64'

const run = (command, args, { shell = process.platform === 'win32' } = {}) => {
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: 'inherit',
    shell,
    env,
  })
  if (result.status !== 0) process.exit(result.status ?? 1)
}

assertAndroidEnv(env)

if (process.platform === 'win32') {
  // Cargo registry 与工程跨盘符时，Kotlin 增量缓存无法计算相对路径。
  const gradlePropertiesPath = join(root, 'src-tauri', 'gen', 'android', 'gradle.properties')
  if (!existsSync(gradlePropertiesPath)) {
    throw new Error('Android 工程尚未初始化，请先运行 npm run init:android。')
  }
  let gradleProperties = readFileSync(gradlePropertiesPath, 'utf8')
  const eol = gradleProperties.includes('\r\n') ? '\r\n' : '\n'
  for (const [name, value] of [
    ['kotlin.incremental', 'false'],
    ['kotlin.incremental.useClasspathSnapshot', 'false'],
  ]) {
    const pattern = new RegExp(`^${name.replaceAll('.', '\\.')}=.*$`, 'm')
    gradleProperties = pattern.test(gradleProperties)
      ? gradleProperties.replace(pattern, `${name}=${value}`)
      : `${gradleProperties.trimEnd()}${eol}${name}=${value}${eol}`
  }
  writeFileSync(gradlePropertiesPath, gradleProperties, 'utf8')
}

console.log('==> 构建前端产物')
run('npm', ['run', 'build'])

console.log('==> staging Android sherpa 原生 Runtime')
await stageAndroidSpeechRuntime({ root })
console.log('==> 重新 staging 当前移动嵌入 Runtime 与 Android 语音模型')
run(process.execPath, [join(root, 'scripts', 'build-mobile-runtime.mjs')], { shell: false })
await stageAndroidSpeechModel({
  sourceDir: join(root, 'release', 'mobile-speech-model'),
  targetDir: join(
    root,
    'src-tauri',
    'gen',
    'android',
    'app',
    'src',
    'main',
    'assets',
    'speech-model',
  ),
})
const androidAssetsDir = join(root, 'src-tauri', 'gen', 'android', 'app', 'src', 'main', 'assets')
mkdirSync(androidAssetsDir, { recursive: true })
copyFileSync(
  join(root, 'release', 'pisper-embedded-runtime.tar.gz'),
  join(androidAssetsDir, 'pisper-embedded-runtime.tgz'),
)

console.log(`==> 构建 Android APK（${release ? 'release' : 'debug'}，${target}）`)
// 桌面打包声明（sidecar/资源）用 --config 内联 JSON 覆盖。
// 注意：不能走 shell/npx（Windows 下会吞引号），直接以 node 跑 CLI 入口。
const tauriCli = join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js')
const args = [
  tauriCli,
  'android',
  'build',
  '--apk',
  '--target',
  target,
  '--config',
  JSON.stringify({ version: mobileVersion, bundle: { externalBin: [], resources: [] } }),
]
if (!release) args.push('--debug')
// node 直接执行：不走 shell，保证 --config 的内联 JSON 原样传递。
run(process.execPath, args, { shell: false })

console.log('')
console.log('APK 输出目录：src-tauri/gen/android/app/build/outputs/apk/')
console.log('安装到 MuMu 模拟器：adb connect 127.0.0.1:7555 && adb install <apk 路径>')
