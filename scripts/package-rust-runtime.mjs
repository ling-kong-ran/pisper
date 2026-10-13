import { execFileSync } from 'node:child_process'
import { chmod, copyFile, cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { resolveRustDesktopPlatform } from './rust-desktop-platforms.mjs'
import { stageRustRuntime } from './stage-rust-runtime.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const runtimeVersion = JSON.parse(await readFile(join(root, 'package.json'), 'utf8')).version
const run = (command, args, options = {}) =>
  execFileSync(command, args, { cwd: root, stdio: 'inherit', ...options })
const capture = (command, args) => run(command, args, { encoding: 'utf8', stdio: 'pipe' }).trim()
const rustTarget = capture(process.env.RUSTC || 'rustc', ['-vV']).match(/^host:\s*(\S+)$/m)?.[1]
const profile = resolveRustDesktopPlatform({
  platform: process.platform,
  arch: process.arch,
  rustTarget,
})
const platform =
  process.platform === 'win32' ? 'windows' : process.platform === 'darwin' ? 'darwin' : 'linux'
const arch = process.arch === 'x64' ? 'x86_64' : 'aarch64'
const command = `pisper-sidecar${process.platform === 'win32' ? '.exe' : ''}`
const stage = join(root, 'release', 'rust-runtime-component')
const runtimeExecutable = join(
  root,
  'runtime-rs',
  'target',
  'release',
  `pisper-server${process.platform === 'win32' ? '.exe' : ''}`,
)
const env = { ...process.env, CARGO_TARGET_DIR: join(root, 'runtime-rs', 'target') }
delete env.CARGO_BUILD_TARGET
if (profile.minimumSystemVersion) env.MACOSX_DEPLOYMENT_TARGET = profile.minimumSystemVersion
run(
  process.env.CARGO || 'cargo',
  ['build', '--release', '--locked', '--manifest-path', 'runtime-rs/Cargo.toml'],
  { env },
)
const sourceCommit = capture('git', ['rev-parse', 'HEAD'])
if (!/^[0-9a-f]{40,64}$/i.test(sourceCommit)) throw new Error('Unable to resolve source commit.')
await rm(stage, { recursive: true, force: true })
await mkdir(stage, { recursive: true })
await stageRustRuntime({
  root,
  runtimeDir: join(stage, 'sidecar-runtime'),
  runtimeExecutable,
  runtimeVersion,
  sourceCommit,
  sourceDirty: Boolean(capture('git', ['status', '--porcelain', '--untracked-files=no'])),
})
run(process.execPath, [
  join(root, 'scripts/verify-rust-runtime-resources.mjs'),
  runtimeExecutable,
  join(stage, 'sidecar-runtime'),
])
await copyFile(runtimeExecutable, join(stage, command))
if (process.platform !== 'win32') await chmod(join(stage, command), 0o755)
for (const script of ['build-frontend.mjs', 'check-bundle-budget.mjs', 'check-dist-compat.mjs']) {
  run(process.execPath, [join(root, 'scripts', script)])
}
// Runtime 更新后仍须提供与当前协议匹配的 Web 资源；不能依赖上一版 Node 闭包。
await cp(join(root, 'dist'), join(stage, 'sidecar-runtime', 'dist'), { recursive: true })
await writeFile(
  join(stage, 'manifest.json'),
  `${JSON.stringify(
    {
      name: 'pisper-runtime',
      version: runtimeVersion,
      backend: 'rust',
      platform,
      arch,
      command,
      layout: [command, 'sidecar-runtime/'],
    },
    null,
    2,
  )}\n`,
)
run(process.execPath, [join(root, 'scripts', 'smoke-rust-data-paths.mjs'), join(stage, command)])
run(process.execPath, [
  join(root, 'scripts', 'smoke-rust-provider-upgrade.mjs'),
  join(stage, command),
])
console.log(`Packaged Rust Runtime: ${stage}`)
