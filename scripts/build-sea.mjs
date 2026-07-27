import { execFile } from 'node:child_process'
import { copyFile, cp, mkdir, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'
import { extractAll } from '@electron/asar'

const run = promisify(execFile)
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const releaseDir = join(root, 'release')
const seaDir = join(releaseDir, 'sea')
const runtimeDir = join(seaDir, 'runtime')
const blobPath = join(seaDir, 'pisper-sidecar.blob')
const seaConfigPath = join(seaDir, 'sea-config.json')
const executableName = process.platform === 'win32' ? 'pisper-sidecar.exe' : 'pisper-sidecar'
const executablePath = join(seaDir, executableName)

async function copyContents(source, destination) {
  await mkdir(destination, { recursive: true })
  for (const entry of await readdir(source, { withFileTypes: true })) {
    await cp(join(source, entry.name), join(destination, entry.name), { recursive: true, force: true })
  }
}

function targetTriples() {
  const arch = process.arch === 'x64' ? 'x86_64' : process.arch === 'arm64' ? 'aarch64' : process.arch
  if (process.platform === 'win32') {
    return [`${arch}-pc-windows-msvc`, `${arch}-pc-windows-gnu`]
  }
  if (process.platform === 'darwin') return [`${arch}-apple-darwin`]
  return [`${arch}-unknown-linux-gnu`]
}

async function packageElectronDirectory() {
  const electronBuilder = join(root, 'node_modules', 'electron-builder', 'out', 'cli', 'cli.js')
  const args = [electronBuilder, '--dir', `--${process.platform === 'win32' ? 'win' : process.platform === 'darwin' ? 'mac' : 'linux'}`, `--${process.arch}`, '--publish', 'never']
  if (process.platform === 'win32') args.push('--config.electronDist=node_modules/electron/dist')
  await run(process.execPath, args, { cwd: root, maxBuffer: 10 * 1024 * 1024 })
}

async function electronResourcesPath() {
  const releaseEntries = await readdir(releaseDir, { withFileTypes: true })
  if (process.platform !== 'darwin') {
    const prefix = process.platform === 'win32' ? 'win' : 'linux'
    const directories = releaseEntries.filter(
      (entry) => entry.isDirectory() && entry.name.startsWith(prefix) && entry.name.endsWith('-unpacked'),
    )
    const preferred =
      directories.find((entry) => entry.name.includes(process.arch)) ||
      directories.find((entry) => entry.name === `${prefix}-unpacked`) ||
      directories[0]
    if (preferred) return join(releaseDir, preferred.name, 'resources')
    throw new Error(`Electron ${process.platform} application resources were not generated.`)
  }

  const macDirectories = releaseEntries
    .filter((entry) => entry.isDirectory() && entry.name.startsWith('mac'))
    .sort((left, right) => Number(right.name.includes(process.arch)) - Number(left.name.includes(process.arch)))
  for (const directory of macDirectories) {
    const directoryPath = join(releaseDir, directory.name)
    const app = (await readdir(directoryPath, { withFileTypes: true })).find(
      (entry) => entry.isDirectory() && entry.name.endsWith('.app'),
    )
    if (app) return join(directoryPath, app.name, 'Contents', 'Resources')
  }
  throw new Error('Electron macOS application resources were not generated.')
}

async function stageRuntime() {
  const electronResources = await electronResourcesPath()
  await rm(runtimeDir, { recursive: true, force: true })
  await mkdir(runtimeDir, { recursive: true })
  extractAll(join(electronResources, 'app.asar'), runtimeDir)
  const unpacked = join(electronResources, 'app.asar.unpacked')
  try {
    await copyContents(unpacked, runtimeDir)
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }

  await Promise.all([
    rm(join(runtimeDir, 'electron'), { recursive: true, force: true }),
    rm(join(runtimeDir, 'node_modules', 'electron-updater'), { recursive: true, force: true }),
  ])
  await mkdir(join(runtimeDir, 'electron'), { recursive: true })
  await copyFile(
    join(root, 'electron', 'desktop-pet-state.mjs'),
    join(runtimeDir, 'electron', 'desktop-pet-state.mjs'),
  )
  await cp(join(root, 'node_modules', 'playwright-core'), join(runtimeDir, 'node_modules', 'playwright-core'), {
    recursive: true,
    force: true,
  })
}

async function injectSea() {
  const config = {
    main: join(root, 'scripts', 'sea-bootstrap.cjs'),
    output: blobPath,
    disableExperimentalSEAWarning: true,
    useSnapshot: false,
    useCodeCache: false,
    execArgv: ['--no-warnings'],
    execArgvExtension: 'none',
  }
  await writeFile(seaConfigPath, `${JSON.stringify(config, null, 2)}\n`, 'utf8')
  await run(process.execPath, ['--experimental-sea-config', seaConfigPath], { cwd: root })
  await copyFile(process.execPath, executablePath)

  const postject = join(root, 'node_modules', 'postject', 'dist', 'cli.js')
  const args = [postject, executablePath, 'NODE_SEA_BLOB', blobPath, '--sentinel-fuse', 'NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2']
  if (process.platform === 'darwin') args.push('--macho-segment-name', 'NODE_SEA')
  await run(process.execPath, args, { cwd: root, maxBuffer: 10 * 1024 * 1024 })

  const binariesDir = join(root, 'src-tauri', 'binaries')
  await mkdir(binariesDir, { recursive: true })
  const tauriBinaries = targetTriples().map((triple) =>
    join(binariesDir, `pisper-sidecar-${triple}${process.platform === 'win32' ? '.exe' : ''}`),
  )
  await Promise.all(tauriBinaries.map((path) => copyFile(executablePath, path)))
  return tauriBinaries
}

await run(process.execPath, [join(root, 'scripts', 'generate-icons.mjs')], { cwd: root })
await run(process.execPath, [join(root, 'node_modules', 'vite', 'bin', 'vite.js'), 'build'], { cwd: root })
await packageElectronDirectory()
await stageRuntime()
const tauriBinaries = await injectSea()
const executableBytes = (await stat(executablePath)).size
console.log(`Built Node SEA sidecar: ${executablePath} (${(executableBytes / 1024 / 1024).toFixed(1)} MiB)`)
console.log(`Staged sidecar runtime: ${runtimeDir}`)
for (const path of tauriBinaries) console.log(`Prepared Tauri sidecar: ${path}`)
