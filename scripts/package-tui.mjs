import { spawn } from 'node:child_process'
import { chmod, copyFile, cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { stageRustRuntime } from './stage-rust-runtime.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const tuiManifest = await readFile(join(root, 'src-tui', 'Cargo.toml'), 'utf8')
const tuiVersion = tuiManifest.match(/\[package\][\s\S]*?\r?\nversion\s*=\s*"([^"]+)"/)?.[1]
if (!tuiVersion) throw new Error('Unable to resolve the Pisper TUI version.')
const desktop = JSON.parse(await readFile(join(root, 'src-tauri/desktop-package.json'), 'utf8'))
const runtimeVersion = desktop.bundled?.runtime
if (typeof runtimeVersion !== 'string' || !runtimeVersion.trim()) {
  throw new Error('desktop-package.json must declare bundled.runtime before packaging.')
}
if (!(
  (process.platform === 'win32' && process.arch === 'x64') ||
  (process.platform === 'darwin' && process.arch === 'arm64')
)) {
  throw new Error('Rust CLI resources currently support Windows x64 and macOS Apple Silicon.')
}
const platform = process.platform === 'win32' ? 'windows' : 'darwin'
const arch = process.arch === 'x64' ? 'x86_64' : 'aarch64'
const suffix = process.platform === 'win32' ? '.exe' : ''
const cliSource = join(root, 'src-tui/target/release', `pisper${suffix}`)
const runtimeExecutable = join(root, 'runtime-rs/target/release', `pisper-server${suffix}`)
const stage = resolve(
  root,
  process.env.PISPER_TUI_STAGE_DIR ||
    join('release', 'tui', `pisper-${tuiVersion}-${platform}-${arch}`),
)

function run(command, args, { env = process.env, capture = false } = {}) {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, args, {
      cwd: root,
      env,
      stdio: capture ? ['ignore', 'pipe', 'inherit'] : 'inherit',
    })
    let output = ''
    if (capture)
      child.stdout.on('data', (chunk) => {
        output += chunk
      })
    child.once('error', rejectRun)
    child.once('close', (code, signal) =>
      code === 0
        ? resolveRun(output.trim())
        : rejectRun(new Error(`${command} exited with ${signal || code}.`)),
    )
  })
}
const cargo = process.env.CARGO || 'cargo'
const host = await run(process.env.RUSTC || 'rustc', ['-vV'], { capture: true })
const target = host.match(/^host:\s*(\S+)$/m)?.[1]
if (
  !(process.platform === 'darwin'
    ? target === 'aarch64-apple-darwin'
    : /^x86_64-.*-windows-(msvc|gnu)$/.test(target || ''))
) {
  throw new Error('Rust host target does not match the CLI packaging platform.')
}
for (const component of ['runtime-rs', 'src-tui']) {
  const env = { ...process.env, CARGO_TARGET_DIR: join(root, component, 'target') }
  delete env.CARGO_BUILD_TARGET
  await run(
    cargo,
    ['build', '--locked', '--release', '--manifest-path', join(root, component, 'Cargo.toml')],
    { env },
  )
}
const sourceCommit = await run('git', ['rev-parse', 'HEAD'], { capture: true })
if (!/^[0-9a-f]{40,64}$/i.test(sourceCommit)) throw new Error('Unable to resolve source commit.')
// 不再复制 Node SEA 闭包；原生资源由桌面与 CLI 的公共暂存函数准备。
await rm(stage, { recursive: true, force: true })
await mkdir(stage, { recursive: true })
await stageRustRuntime({
  root,
  runtimeDir: join(stage, 'sidecar-runtime'),
  runtimeExecutable,
  runtimeVersion,
  sourceCommit,
  sourceDirty: Boolean(
    await run('git', ['status', '--porcelain', '--untracked-files=no'], { capture: true }),
  ),
})
await copyFile(cliSource, join(stage, `pisper${suffix}`))
await copyFile(runtimeExecutable, join(stage, `pisper-sidecar${suffix}`))
if (process.platform !== 'win32') {
  await chmod(join(stage, 'pisper'), 0o755)
  await chmod(join(stage, 'pisper-sidecar'), 0o755)
}
// 本地包同时包含前端，使 pisper web 不必下载旧通道的前端组件。
await run(process.execPath, [join(root, 'scripts/build-frontend.mjs')])
await run(process.execPath, [join(root, 'scripts/check-bundle-budget.mjs')])
await run(process.execPath, [join(root, 'scripts/check-dist-compat.mjs')])
await cp(join(root, 'dist'), join(stage, 'sidecar-runtime/dist'), { recursive: true })
await writeFile(
  join(stage, 'manifest.json'),
  `${JSON.stringify(
    {
      name: 'pisper',
      version: tuiVersion,
      runtimeVersion,
      backend: 'rust',
      platform,
      arch,
      command: `pisper${suffix}`,
      layout: [`pisper${suffix}`, `pisper-sidecar${suffix}`, 'sidecar-runtime/'],
    },
    null,
    2,
  )}\n`,
)
console.log(`Packaged Pisper TUI: ${stage}`)
