import { execFile } from 'node:child_process'
import { copyFile, cp, mkdir, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const run = promisify(execFile)
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const releaseDir = join(root, 'release')
const seaDir = join(releaseDir, 'sea')
const runtimeDir = join(seaDir, 'runtime')
const blobPath = join(seaDir, 'pisper-sidecar.blob')
const seaConfigPath = join(seaDir, 'sea-config.json')
const executableName = process.platform === 'win32' ? 'pisper-sidecar.exe' : 'pisper-sidecar'
const executablePath = join(seaDir, executableName)

function targetTriples() {
  const arch = process.arch === 'x64' ? 'x86_64' : process.arch === 'arm64' ? 'aarch64' : process.arch
  if (process.platform === 'win32') {
    return [`${arch}-pc-windows-msvc`, `${arch}-pc-windows-gnu`]
  }
  if (process.platform === 'darwin') return [`${arch}-apple-darwin`]
  return [`${arch}-unknown-linux-gnu`]
}

async function runNpm(args, options = {}) {
  const npmCli = String(process.env.npm_execpath || '').trim()
  if (npmCli) return run(process.execPath, [npmCli, ...args], options)
  const command = process.platform === 'win32' ? 'npm.cmd' : 'npm'
  return run(command, args, options)
}

const removableDirectoryNames = new Set([
  '.github',
  '__tests__',
  'docs',
  'example',
  'examples',
  'test',
  'tests',
])
const removableFileNames = new Set([
  'changelog',
  'changelog.md',
  'license',
  'license.md',
  'readme',
  'readme.md',
])
const removableFileSuffixes = ['.d.cts', '.d.mts', '.d.ts', '.map', '.tsbuildinfo']

async function prunePackageTree(directory) {
  let entries
  try {
    entries = await readdir(directory, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOENT') return
    throw error
  }
  await Promise.all(
    entries.map(async (entry) => {
      const entryPath = join(directory, entry.name)
      if (entry.isDirectory()) {
        if (removableDirectoryNames.has(entry.name.toLowerCase())) {
          await rm(entryPath, { recursive: true, force: true })
        } else {
          await prunePackageTree(entryPath)
        }
        return
      }
      const lowerName = entry.name.toLowerCase()
      if (
        removableFileNames.has(lowerName) ||
        removableFileSuffixes.some((suffix) => lowerName.endsWith(suffix))
      ) {
        await rm(entryPath, { force: true })
      }
    }),
  )
}

async function pruneRuntime() {
  const nodeModules = join(runtimeDir, 'node_modules')
  const removePaths = [
    ['tesseract.js'],
    ['tesseract.js-core'],
    ['zlibjs'],
    ['@napi-rs', 'canvas'],
    ['@earendil-works', 'pi-coding-agent', 'CHANGELOG.md'],
    ['@earendil-works', 'pi-coding-agent', 'containerization.md'],
    ['@larksuiteoapi', 'node-sdk', 'es'],
    ['@larksuiteoapi', 'node-sdk', 'types'],
    ['openai', 'src'],
    ['pdfjs-dist', 'image_decoders'],
    ['pdfjs-dist', 'web'],
    ['highlight.js', 'scss'],
    ['highlight.js', 'styles'],
  ]
  await Promise.all(removePaths.map((parts) => rm(join(nodeModules, ...parts), { recursive: true, force: true })))

  const pdfBuild = join(nodeModules, 'pdfjs-dist', 'build')
  for (const name of [
    'pdf.min.mjs',
    'pdf.sandbox.mjs',
    'pdf.sandbox.min.mjs',
    'pdf.worker.mjs',
    'pdf.worker.min.mjs',
  ]) {
    await rm(join(pdfBuild, name), { force: true })
  }

  const canvasScope = join(nodeModules, '@napi-rs')
  try {
    for (const entry of await readdir(canvasScope, { withFileTypes: true })) {
      if (entry.isDirectory() && entry.name.startsWith('canvas-')) {
        await rm(join(canvasScope, entry.name), { recursive: true, force: true })
      }
    }
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }

  const sandboxVendor = join(nodeModules, '@anthropic-ai', 'sandbox-runtime', 'vendor')
  for (const family of ['seccomp', 'srt-win']) {
    const familyDir = join(sandboxVendor, family)
    try {
      for (const entry of await readdir(familyDir, { withFileTypes: true })) {
        if (entry.isDirectory() && entry.name !== process.arch) {
          await rm(join(familyDir, entry.name), { recursive: true, force: true })
        }
      }
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
  }

  await prunePackageTree(nodeModules)
}

async function stageRuntime() {
  await rm(runtimeDir, { recursive: true, force: true })
  await mkdir(runtimeDir, { recursive: true })
  await mkdir(join(runtimeDir, 'docs'), { recursive: true })
  await Promise.all([
    cp(join(root, 'dist'), join(runtimeDir, 'dist'), { recursive: true, force: true }),
    cp(join(root, 'server'), join(runtimeDir, 'server'), { recursive: true, force: true }),
    cp(join(root, 'shared'), join(runtimeDir, 'shared'), { recursive: true, force: true }),
    copyFile(join(root, 'docs', 'sponsors.json'), join(runtimeDir, 'docs', 'sponsors.json')),
    copyFile(join(root, 'package.json'), join(runtimeDir, 'package.json')),
    copyFile(join(root, 'package-lock.json'), join(runtimeDir, 'package-lock.json')),
  ])
  await rm(join(runtimeDir, 'server', 'tests'), { recursive: true, force: true })

  await runNpm(['ci', '--omit=dev', '--ignore-scripts', '--no-audit', '--no-fund'], {
    cwd: runtimeDir,
    env: { ...process.env, NODE_ENV: 'production' },
    maxBuffer: 10 * 1024 * 1024,
  })
  await rm(join(runtimeDir, 'package-lock.json'), { force: true })
  await pruneRuntime()
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

await run(process.execPath, [join(root, 'node_modules', 'vite', 'bin', 'vite.js'), 'build'], { cwd: root })
await stageRuntime()
const tauriBinaries = await injectSea()
const executableBytes = (await stat(executablePath)).size
console.log(`Built Node SEA sidecar: ${executablePath} (${(executableBytes / 1024 / 1024).toFixed(1)} MiB)`)
console.log(`Staged sidecar runtime: ${runtimeDir}`)
for (const path of tauriBinaries) console.log(`Prepared Tauri sidecar: ${path}`)
