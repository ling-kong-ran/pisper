import { spawn } from 'node:child_process'
import { chmod, copyFile, cp, mkdir, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const packageJson = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'))
const tuiManifest = await readFile(join(root, 'src-tui', 'Cargo.toml'), 'utf8')
const tuiVersion = tuiManifest.match(/\[package\][\s\S]*?\r?\nversion\s*=\s*"([^"]+)"/)?.[1]
if (!tuiVersion) throw new Error('Unable to resolve the Pisper TUI version.')
const platform =
  process.platform === 'win32' ? 'windows' : process.platform === 'darwin' ? 'macos' : 'linux'
const arch = process.arch === 'x64' ? 'x86_64' : process.arch === 'arm64' ? 'aarch64' : process.arch
const executableSuffix = process.platform === 'win32' ? '.exe' : ''
const cliSource = join(root, 'src-tui', 'target', 'release', `pisper${executableSuffix}`)
const seaRoot = join(root, 'release', 'sea')
const sidecarSource = join(seaRoot, `pisper-sidecar${executableSuffix}`)
const runtimeSource = join(seaRoot, 'runtime')
const stage = resolve(
  root,
  process.env.PISPER_TUI_STAGE_DIR ||
    join('release', 'tui', `pisper-${tuiVersion}-${platform}-${arch}`),
)

function run(command, args) {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, args, { cwd: root, env: process.env, stdio: 'inherit' })
    child.once('error', rejectRun)
    child.once('exit', (code, signal) => {
      if (code === 0) resolveRun()
      else rejectRun(new Error(`${command} exited with ${signal || code}.`))
    })
  })
}

async function requirePath(path, message) {
  try {
    await stat(path)
  } catch (error) {
    if (error?.code === 'ENOENT') throw new Error(message)
    throw error
  }
}

await run('cargo', [
  'build',
  '--locked',
  '--release',
  '--manifest-path',
  join(root, 'src-tui', 'Cargo.toml'),
])
await Promise.all([
  requirePath(cliSource, 'Pisper TUI release binary was not produced.'),
  requirePath(
    sidecarSource,
    'SEA sidecar is missing. Run npm run sidecar:sea before npm run tui:package.',
  ),
  requirePath(
    runtimeSource,
    'SEA runtime is missing. Run npm run sidecar:sea before npm run tui:package.',
  ),
])

await rm(stage, { recursive: true, force: true })
await mkdir(stage, { recursive: true })
await Promise.all([
  copyFile(cliSource, join(stage, `pisper${executableSuffix}`)),
  copyFile(sidecarSource, join(stage, `pisper-sidecar${executableSuffix}`)),
  cp(runtimeSource, join(stage, 'sidecar-runtime'), { recursive: true, force: true }),
])
if (process.platform !== 'win32') {
  await Promise.all([
    chmod(join(stage, 'pisper'), 0o755),
    chmod(join(stage, 'pisper-sidecar'), 0o755),
  ])
}
await writeFile(
  join(stage, 'manifest.json'),
  `${JSON.stringify(
    {
      name: 'pisper',
      version: tuiVersion,
      runtimeVersion: packageJson.version,
      platform,
      arch,
      command: `pisper${executableSuffix}`,
      layout: ['pisper', 'pisper-sidecar', 'sidecar-runtime/'],
    },
    null,
    2,
  )}\n`,
  'utf8',
)
console.log(`Packaged Pisper TUI: ${stage}`)
