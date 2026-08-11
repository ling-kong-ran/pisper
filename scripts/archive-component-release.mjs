import { spawn } from 'node:child_process'
import { chmod, copyFile, cp, mkdir, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join, relative, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { assertReleaseComponent, readComponentVersion } from './release-components.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const component = assertReleaseComponent(process.argv[2])
const version = await readComponentVersion(root, component)
const platform =
  process.platform === 'win32' ? 'windows' : process.platform === 'darwin' ? 'darwin' : 'linux'
const arch = process.arch === 'x64' ? 'x86_64' : process.arch === 'arm64' ? 'aarch64' : process.arch
const executableSuffix = process.platform === 'win32' ? '.exe' : ''
const outputDir = join(root, 'release', 'component-artifacts')
const stageRoot = join(root, 'release', 'component-stage')
const directoryName = `pisper-${component}-${version}-${platform}-${arch}`
const stage = join(stageRoot, directoryName)
const label = component === 'desktop' ? 'Desktop' : component === 'tui' ? 'TUI' : 'Runtime'
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

async function stageDesktop() {
  await cp(join(root, 'dist'), join(stage, 'dist'), { recursive: true, force: true })
  await writeFile(
    join(stage, 'manifest.json'),
    `${JSON.stringify(
      {
        name: 'pisper-desktop',
        version,
        platform,
        arch,
        command: 'dist/index.html',
        layout: ['dist/'],
      },
      null,
      2,
    )}\n`,
    'utf8',
  )
}

async function stageRuntime() {
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
        name: 'pisper-runtime',
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

async function createArchive(sourceDirectory, destination) {
  await rm(destination, { force: true })
  const archiveArgument = relative(root, destination).replaceAll('\\', '/')
  const stageArgument = relative(root, stageRoot).replaceAll('\\', '/')
  await run('tar', ['-czf', archiveArgument, '-C', stageArgument, sourceDirectory])
  const bytes = (await stat(destination)).size
  if (bytes === 0) throw new Error(`Component archive is empty: ${destination}`)
  console.log(`Packaged Pisper ${label}: ${destination} (${bytes} bytes)`)
}

await rm(stage, { recursive: true, force: true })
await mkdir(stage, { recursive: true })
await mkdir(outputDir, { recursive: true })
if (component === 'desktop') await stageDesktop()
else if (component === 'runtime') await stageRuntime()
else await stageTui()
await createArchive(directoryName, archive)

if (component === 'runtime') {
  const nodeDirectoryName = `pisper-runtime-node-${version}-${platform}-${arch}`
  const nodeStage = join(stageRoot, nodeDirectoryName)
  const nodeArchive = join(outputDir, `Pisper_Runtime_Node_${version}_${platform}_${arch}.tar.gz`)
  await rm(nodeStage, { recursive: true, force: true })
  await mkdir(nodeStage, { recursive: true })
  await cp(join(stage, 'sidecar-runtime'), join(nodeStage, 'sidecar-runtime'), {
    recursive: true,
    force: true,
  })
  await writeFile(
    join(nodeStage, 'manifest.json'),
    `${JSON.stringify(
      {
        name: 'pisper-runtime-node',
        version,
        platform,
        arch,
        command: 'sidecar-runtime/runtime/sidecar.mjs',
        layout: ['sidecar-runtime/'],
      },
      null,
      2,
    )}\n`,
    'utf8',
  )
  await createArchive(nodeDirectoryName, nodeArchive)
}

if (component === 'tui') {
  const thinDirectoryName = `pisper-tui-component-${version}-${platform}-${arch}`
  const thinStage = join(stageRoot, thinDirectoryName)
  const thinArchive = join(outputDir, `Pisper_TUI_Component_${version}_${platform}_${arch}.tar.gz`)
  await rm(thinStage, { recursive: true, force: true })
  await mkdir(thinStage, { recursive: true })
  await copyFile(
    join(stage, `pisper${executableSuffix}`),
    join(thinStage, `pisper${executableSuffix}`),
  )
  if (process.platform !== 'win32') await chmod(join(thinStage, 'pisper'), 0o755)
  await writeFile(
    join(thinStage, 'manifest.json'),
    `${JSON.stringify(
      {
        name: 'pisper-tui-component',
        version,
        platform,
        arch,
        command: `pisper${executableSuffix}`,
        layout: ['pisper'],
      },
      null,
      2,
    )}\n`,
    'utf8',
  )
  await createArchive(thinDirectoryName, thinArchive)
}
