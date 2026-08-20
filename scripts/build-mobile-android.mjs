// 构建移动端 APK：先产出前端 dist，再调用 tauri android build。
// 移动端不打包 sidecar/外部资源，用 --config 覆盖掉桌面专属的产物声明。
// 用法：node scripts/build-mobile-android.mjs [--release] [--target x86_64|aarch64|...]
// 环境：需要 JAVA_HOME / ANDROID_HOME / NDK_HOME（Windows 上 symlink 需要开发者模式）。
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawnSync } from 'node:child_process'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const release = process.argv.includes('--release')
const targetIndex = process.argv.indexOf('--target')
const target = targetIndex >= 0 ? process.argv[targetIndex + 1] : 'x86_64'

const run = (command, args) => {
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: 'inherit',
    shell: process.platform === 'win32',
    env: process.env,
  })
  if (result.status !== 0) process.exit(result.status ?? 1)
}

console.log('==> 构建前端产物')
run('npm', ['run', 'build'])

console.log(`==> 构建 Android APK（${release ? 'release' : 'debug'}，${target}）`)
const args = [
  'tauri',
  'android',
  'build',
  '--apk',
  '--target',
  target,
  // 移动端不打 sidecar/更新器资源：覆盖桌面打包的产物清单。
  '--config',
  '{"bundle":{"externalBin":[],"resources":{}}}',
]
if (!release) args.push('--debug')
run('npx', args)

console.log('')
console.log('APK 输出目录：src-tauri/gen/android/app/build/outputs/apk/')
console.log('安装到 MuMu 模拟器：adb connect 127.0.0.1:7555 && adb install <apk 路径>')
