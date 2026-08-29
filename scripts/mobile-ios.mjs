// iOS 的外层 CLI 与 Xcode build script 都必须收到字面量版本；传版本文件路径会在内层解析失败。
import { spawnSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const command = process.argv[2]
if (command !== 'init' && command !== 'build') {
  throw new Error('用法：node scripts/mobile-ios.mjs <init|build> [Tauri 参数]')
}

const mobileVersion = JSON.parse(
  readFileSync(join(root, 'src-tauri', 'mobile-package.json'), 'utf8'),
).version
if (
  typeof mobileVersion !== 'string' ||
  !/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(mobileVersion)
) {
  throw new Error('src-tauri/mobile-package.json 缺少有效的 semver 版本。')
}

const passthrough = process.argv.slice(3)
const separator = passthrough.indexOf('--')
const options = separator === -1 ? passthrough : passthrough.slice(0, separator)
const runnerArgs = separator === -1 ? [] : passthrough.slice(separator)
const tauriCli = join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js')
const configPath = join(root, 'src-tauri', 'tauri.mobile-ios.conf.json')
const args = [
  tauriCli,
  'ios',
  command,
  ...options,
  '--config',
  configPath,
  '--config',
  JSON.stringify({ version: mobileVersion }),
  ...runnerArgs,
]

const result = spawnSync(process.execPath, args, { cwd: root, stdio: 'inherit' })
if (result.status !== 0) process.exit(result.status ?? 1)

if (command === 'init') {
  const setup = spawnSync(process.execPath, [join(root, 'scripts', 'setup-mobile-ios.mjs')], {
    cwd: root,
    stdio: 'inherit',
  })
  if (setup.status !== 0) process.exit(setup.status ?? 1)
}
