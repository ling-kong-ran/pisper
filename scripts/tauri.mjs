// Gradle 通过 npm 回调 Tauri 时也必须恢复同一移动工具链环境。
import { spawnSync } from 'node:child_process'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { mobileRustEnv } from './mobile-rust-env.mjs'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const args = process.argv.slice(2)
const env = ['android', 'ios'].includes(args[0]) ? mobileRustEnv(args[0]) : process.env
const result = spawnSync(
  process.execPath,
  [join(root, 'node_modules/@tauri-apps/cli/tauri.js'), ...args],
  { cwd: root, env, stdio: 'inherit' },
)
if (result.error) throw result.error
process.exitCode = result.status ?? 1
