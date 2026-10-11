// 开发桌面与发布包共用 Rust 资源闭包，避免退回已删除的 Node sidecar。
import { execFileSync } from 'node:child_process'
import { dirname, join } from 'node:path'
import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { stageRustRuntime } from './stage-rust-runtime.mjs'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const run = (command, args, options = {}) =>
  execFileSync(command, args, { cwd: root, stdio: 'inherit', ...options })
const env = { ...process.env, CARGO_TARGET_DIR: join(root, 'runtime-rs/target') }
delete env.CARGO_BUILD_TARGET
run(
  process.env.CARGO || 'cargo',
  ['build', '--locked', '--manifest-path', 'runtime-rs/Cargo.toml'],
  { env },
)
await stageRustRuntime({
  root,
  runtimeDir: join(root, 'release/rust-dev'),
  runtimeExecutable: join(
    root,
    'runtime-rs/target/debug',
    `pisper-server${process.platform === 'win32' ? '.exe' : ''}`,
  ),
  runtimeVersion: JSON.parse(await readFile(join(root, 'package.json'), 'utf8')).version,
  sourceCommit: run('git', ['rev-parse', 'HEAD'], { encoding: 'utf8', stdio: 'pipe' }).trim(),
  sourceDirty: Boolean(
    run('git', ['status', '--porcelain'], { encoding: 'utf8', stdio: 'pipe' }).trim(),
  ),
})
for (const script of ['build-frontend.mjs', 'check-bundle-budget.mjs', 'check-dist-compat.mjs']) {
  run(process.execPath, [join(root, 'scripts', script)])
}
run(process.execPath, [
  join(root, 'node_modules/@tauri-apps/cli/tauri.js'),
  'dev',
  ...process.argv.slice(2),
])
