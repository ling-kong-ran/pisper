import { spawn } from 'node:child_process'
import { chmod, copyFile, cp, mkdir, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join, relative, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { assertReleaseComponent, readComponentVersion } from './release-components.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const component = assertReleaseComponent(process.argv[2])
if (component === 'desktop') {
  throw new Error('Desktop artifacts are staged by scripts/stage-tauri-artifacts.mjs.')
}
const version = await readComponentVersion(root, component)
const platform =
  process.platform === 'win32' ? 'windows' : process.platform === 'darwin' ? 'darwin' : 'linux'
const arch = process.arch === 'x64' ? 'x86_64' : process.arch === 'arm64' ? 'aarch64' : process.arch
const executableSuffix = process.platform === 'win32' ? '.exe' : ''
const outputDir = join(root, 'release', 'component-artifacts')
const stageRoot = join(root, 'release', 'component-stage')
const directoryName = `pisper-${component}-${version}-${platform}-${arch}`
const stage = join(stageRoot, directoryName)
const label = component === 'tui' ? 'TUI' : 'Server'
const archive = join(outputDir, `Pisper_${label}_${version}_${platform}_${arch}.tar.gz`)

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

async function stageServer() {
  const seaRoot = join(root, 'release', 'sea')
  const sidecar = join(seaRoot, `pisper-sidecar${executableSuffix}`)
  const runtime = join(seaRoot, 'runtime')
  await Promise.all([stat(sidecar), stat(runtime)])
  await Promise.all([
    copyFile(sidecar, join(stage, `pisper-sidecar${executableSuffix}`)),
    cp(runtime, join(stage, 'sidecar-runtime'), { recursive: true, force: true }),
    copyFile(
      join(seaRoot, 'runtime-size-manifest.json'),
      join(stage, 'runtime-size-manifest.json'),
    ),
  ])
  if (process.platform !== 'win32') {
    await chmod(join(stage, 'pisper-sidecar'), 0o755)
  }
  await writeFile(
    join(stage, 'manifest.json'),
    `${JSON.stringify(
      {
        name: 'pisper-server',
        version,
        platform,
        arch,
        command: `pisper-sidecar${executableSuffix}`,
        layout: ['pisper-sidecar', 'sidecar-runtime/'],
      },
      null,
      2,
    )}\n`,
    'utf8',
  )
}

async function stageTui() {
  const source = resolve(
    root,
    process.env.PISPER_TUI_STAGE_DIR ||
      join(
        'release',
        'tui',
        `pisper-${version}-${platform === 'darwin' ? 'macos' : platform}-${arch}`,
      ),
  )
  await stat(source)
  await cp(source, stage, { recursive: true, force: true })
}

await rm(stage, { recursive: true, force: true })
await mkdir(stage, { recursive: true })
await mkdir(outputDir, { recursive: true })
if (component === 'server') await stageServer()
else await stageTui()
await rm(archive, { force: true })
const archiveArgument = relative(root, archive).replaceAll('\\', '/')
const stageArgument = relative(root, stageRoot).replaceAll('\\', '/')
await run('tar', ['-czf', archiveArgument, '-C', stageArgument, directoryName])
const bytes = (await stat(archive)).size
if (bytes === 0) throw new Error(`Component archive is empty: ${archive}`)
console.log(`Packaged Pisper ${label}: ${archive} (${bytes} bytes)`)
