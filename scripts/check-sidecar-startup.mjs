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

await run(process.execPath, ['--test', 'scripts/tests/native-process-inspection.test.mjs'])

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

// 同一新编译产物还必须能原样承接旧 Node Provider 配置。
await run(process.execPath, [
  'scripts/smoke-rust-provider-upgrade.mjs',
  path.join(
    root,
    'runtime-rs',
    'target',
    'debug',
    process.platform === 'win32' ? 'pisper-server.exe' : 'pisper-server',
  ),
])

// 用真实 HTTP 请求核对两种供应商协议的项目规则上下文。
await run(process.execPath, [
  'scripts/smoke-rust-project-context.mjs',
  path.join(
    root,
    'runtime-rs',
    'target',
    'debug',
    process.platform === 'win32' ? 'pisper-server.exe' : 'pisper-server',
  ),
])

// 会话读取缓存覆盖外部追加、改名、原子替换、分页与删除；不调用真实模型。
await run(process.execPath, [
  'scripts/smoke-rust-session-read-cache.mjs',
  path.join(
    root,
    'runtime-rs',
    'target',
    'debug',
    process.platform === 'win32' ? 'pisper-server.exe' : 'pisper-server',
  ),
])

// CLI 与 Runtime 的进程握手必须一起验证，不能只运行协议模型单元测试。
await run(process.env.CARGO || 'cargo', [
  'build',
  '--locked',
  '--manifest-path',
  'src-tui/Cargo.toml',
])
await run(process.execPath, ['scripts/smoke-rust-cli.mjs'])
