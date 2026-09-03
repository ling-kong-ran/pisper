import { execFile } from 'node:child_process'
import { copyFile, mkdir, readFile, stat, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'
import {
  assertSizeManifest,
  finalizeSizeManifest,
  runtimeTarget,
  writeSizeManifest,
} from './sea-runtime.mjs'
import { stageRuntimeClosure } from './stage-runtime-closure.mjs'

const run = promisify(execFile)
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const releaseDir = join(root, 'release')
const seaDir = join(releaseDir, 'sea')
const runtimeDir = join(seaDir, 'runtime')
const manifestPath = join(seaDir, 'runtime-size-manifest.json')
const blobPath = join(seaDir, 'pisper-sidecar.blob')
const seaConfigPath = join(seaDir, 'sea-config.json')
const executableName = process.platform === 'win32' ? 'pisper-sidecar.exe' : 'pisper-sidecar'
const executablePath = join(seaDir, executableName)
const target = runtimeTarget()
const packageJson = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'))

function targetTriples() {
  const arch =
    process.arch === 'x64' ? 'x86_64' : process.arch === 'arm64' ? 'aarch64' : process.arch
  if (process.platform === 'win32') {
    return [`${arch}-pc-windows-msvc`, `${arch}-pc-windows-gnu`]
  }
  if (process.platform === 'darwin') return [`${arch}-apple-darwin`]
  return [`${arch}-unknown-linux-gnu`]
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
  if (process.platform === 'darwin') {
    await run('codesign', ['--remove-signature', executablePath], { cwd: root })
  }

  const postject = join(root, 'node_modules', 'postject', 'dist', 'cli.js')
  const args = [
    postject,
    executablePath,
    'NODE_SEA_BLOB',
    blobPath,
    '--sentinel-fuse',
    'NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2',
  ]
  if (process.platform === 'darwin') args.push('--macho-segment-name', 'NODE_SEA')
  await run(process.execPath, args, { cwd: root, maxBuffer: 10 * 1024 * 1024 })
  if (process.platform === 'darwin') {
    await run('codesign', ['--force', '--sign', '-', executablePath], { cwd: root })
  }

  const binariesDir = join(root, 'src-tauri', 'binaries')
  await mkdir(binariesDir, { recursive: true })
  const tauriBinaries = targetTriples().map((triple) =>
    join(binariesDir, `pisper-sidecar-${triple}${process.platform === 'win32' ? '.exe' : ''}`),
  )
  await Promise.all(tauriBinaries.map((path) => copyFile(executablePath, path)))
  return tauriBinaries
}

await run(process.execPath, [join(root, 'node_modules', 'vite', 'bin', 'vite.js'), 'build'], {
  cwd: root,
})
let manifest = await stageRuntimeClosure({
  root,
  runtimeDir,
  manifestPath,
  target,
  appVersion: packageJson.version,
  includeSpeechModel: true,
})
const tauriBinaries = await injectSea()
const executableBytes = (await stat(executablePath)).size
manifest = finalizeSizeManifest(manifest, executableBytes)
await writeSizeManifest(manifestPath, manifest)
assertSizeManifest(manifest, { requireExecutable: true })

console.log(
  `Built Node SEA sidecar: ${executablePath} (${(executableBytes / 1024 / 1024).toFixed(1)} MiB)`,
)
console.log(
  `Staged sidecar runtime: ${runtimeDir} (${(manifest.runtime.afterPrune.bytes / 1024 / 1024).toFixed(1)} MiB, ${manifest.runtime.reduction.percent}% pruned)`,
)
console.log(`Runtime size audit: ${manifestPath} (budget pass: ${manifest.budget.pass})`)
for (const path of tauriBinaries) console.log(`Prepared Tauri sidecar: ${path}`)
