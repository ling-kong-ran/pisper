import { spawn } from 'node:child_process'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
function run(command, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: root, stdio: 'inherit', windowsHide: true })
    child.once('error', reject)
    child.once('exit', (code, signal) => {
      if (code === 0) resolve()
      else reject(new Error(`Rust startup gate failed (${signal || code}).`))
    })
  })
}

// Rust 分支的启动闸门使用真实独立进程和隔离的本机模型，覆盖鉴权、配置、消息与重启。
await run(process.env.CARGO || 'cargo', [
  'build',
  '--locked',
  '--manifest-path',
  'runtime-rs/Cargo.toml',
])
await run(process.execPath, [
  'scripts/smoke-rust-usability.mjs',
  path.join(
    root,
    'runtime-rs',
    'target',
    'debug',
    process.platform === 'win32' ? 'pisper-server.exe' : 'pisper-server',
  ),
  '--skip-ui',
])
