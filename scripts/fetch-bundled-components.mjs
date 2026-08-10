// Fetch the latest signed TUI and Runtime component archives and stage them
// into the desktop installer payload. The full installer then bundles the
// newest published components instead of locally built ones.
//
// Usage: node scripts/fetch-bundled-components.mjs [--tui=X.Y.Z] [--runtime=X.Y.Z]
// Without explicit versions the newest tui-v* / runtime-v* tags win.
//
// Outputs:
//   TUI     pisper(.exe)            -> src-tauri/binaries/pisper-cli-<triple>(.exe)
//   Runtime pisper-sidecar(.exe)    -> src-tauri/binaries/pisper-sidecar-<triple>(.exe)
//           sidecar-runtime/        -> release/sea/runtime/
//           runtime-size-manifest.json -> release/sea/runtime-size-manifest.json
// Also records the bundled versions in src-tauri/desktop-package.json.
import { chmod, cp, mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { PublicKey, Signature } from '@threema/wasm-minisign-verify'
import { x as extractTar } from 'tar'
import { execFileSync } from 'node:child_process'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const repository = 'ling-kong-ran/pisper'
const executableSuffix = process.platform === 'win32' ? '.exe' : ''
const platform =
  process.platform === 'win32' ? 'windows' : process.platform === 'darwin' ? 'darwin' : 'linux'
const arch = process.arch === 'x64' ? 'x86_64' : process.arch === 'arm64' ? 'aarch64' : process.arch
const target = `${platform}_${arch}`
const updaterPubKey = join(root, 'packages', 'pisper', 'updater.pubkey')
const MAX_ARCHIVE_BYTES = 128 * 1024 * 1024

const rawArgs = process.argv.slice(2)
const dryRun = rawArgs.includes('--dry-run')
const requestedTui = rawArgs.find((value) => value.startsWith('--tui='))?.slice('--tui='.length)
const requestedRuntime = rawArgs
  .find((value) => value.startsWith('--runtime='))
  ?.slice('--runtime='.length)

function runGit(args) {
  return execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim()
}

function latestVersionTag(prefix) {
  const tag = runGit(['tag', '--list', `${prefix}-v*`, '--sort=-version:refname'])
    .split(/\r?\n/)
    .filter(Boolean)[0]
  if (!tag) throw new Error(`No ${prefix} component tag found; cannot bundle latest components.`)
  return tag
}

function validVersion(value, label) {
  if (!/^\d+\.\d+\.\d+$/.test(value || '')) {
    throw new Error(`Invalid ${label} version: ${value || '(empty)'}`)
  }
  return value
}

function targetTriples() {
  if (process.platform === 'win32') {
    return [`${arch}-pc-windows-msvc`, `${arch}-pc-windows-gnu`]
  }
  if (process.platform === 'darwin') return [`${arch}-apple-darwin`]
  return [`${arch}-unknown-linux-gnu`]
}

function decodeWrapped(value) {
  const trimmed = String(value).trim()
  if (trimmed.startsWith('untrusted comment:')) return trimmed
  if (!/^[A-Za-z0-9+/=\r\n]+$/.test(trimmed)) return trimmed
  const decoded = Buffer.from(trimmed.replace(/\s/g, ''), 'base64').toString('utf8').trim()
  return decoded.startsWith('untrusted comment:') ? decoded : trimmed
}

async function download(url, destination) {
  const response = await fetch(url, {
    headers: { 'User-Agent': 'pisper-desktop-bundler' },
    redirect: 'follow',
    signal: AbortSignal.timeout(600_000),
  })
  if (!response.ok) throw new Error(`download failed with HTTP ${response.status}: ${url}`)
  const declaredSize = Number(response.headers.get('content-length') || 0)
  if (declaredSize > MAX_ARCHIVE_BYTES) {
    throw new Error(`download is larger than ${MAX_ARCHIVE_BYTES} bytes: ${url}`)
  }
  const bytes = Buffer.from(await response.arrayBuffer())
  if (bytes.length === 0 || bytes.length > MAX_ARCHIVE_BYTES) {
    throw new Error(`unexpected download size for ${url}`)
  }
  await writeFile(destination, bytes)
  return bytes
}

async function verifyArchive(component, archiveBytes, signatureBytes) {
  const publicKeyText = await readFile(updaterPubKey, 'utf8')
  const publicKey = PublicKey.decode(decodeWrapped(publicKeyText))
  const signature = Signature.decode(decodeWrapped(signatureBytes.toString('utf8')))
  try {
    if (!publicKey.verify(archiveBytes, signature)) {
      throw new Error('signature verification returned false')
    }
  } catch (error) {
    throw new Error(`${component} archive signature verification failed: ${error.message}`, {
      cause: error,
    })
  } finally {
    signature.free()
    publicKey.free()
  }
}

async function extract(destination, archivePath) {
  await rm(destination, { recursive: true, force: true })
  await mkdir(destination, { recursive: true })
  // Component archives include their top-level directory; strip it so the
  // payload files land directly under destination.
  await extractTar({ cwd: destination, file: archivePath, strip: 1 })
}

async function fetchComponent({ component, version, label, assetName, stageDir }) {
  const tag = `${component}-v${version}`
  const baseUrl = `https://github.com/${repository}/releases/download/${tag}`
  const archivePath = join(stageDir, `${component}.tar.gz`)
  const signaturePath = join(stageDir, `${component}.tar.gz.sig`)
  await mkdir(stageDir, { recursive: true })
  const archiveBytes = await download(`${baseUrl}/${assetName}`, archivePath)
  const signatureBytes = await download(`${baseUrl}/${assetName}.sig`, signaturePath)
  await verifyArchive(component, archiveBytes, signatureBytes)
  console.log(`Verified ${label} ${version} archive signature (${target}).`)
  return { archivePath, tag }
}

async function removeBinShims(rootDir) {
  const pending = [rootDir]
  let removed = 0
  while (pending.length) {
    const directory = pending.pop()
    let entries
    try {
      entries = await readdir(directory, { withFileTypes: true })
    } catch {
      continue
    }
    for (const entry of entries) {
      const fullPath = join(directory, entry.name)
      if (!entry.isDirectory()) continue
      if (entry.name === '.bin') {
        await rm(fullPath, { recursive: true, force: true })
        removed += 1
      } else {
        pending.push(fullPath)
      }
    }
  }
  return removed
}

async function main() {
  const tuiTag = requestedTui
    ? `tui-v${validVersion(requestedTui, 'TUI')}`
    : latestVersionTag('tui')
  const runtimeTag = requestedRuntime
    ? `runtime-v${validVersion(requestedRuntime, 'Runtime')}`
    : latestVersionTag('runtime')
  const tuiVersion = tuiTag.slice('tui-v'.length)
  const runtimeVersion = runtimeTag.slice('runtime-v'.length)

  const stageDir = join(root, 'release', 'bundled-components', target)
  const tuiAsset = `Pisper_TUI_Component_${tuiVersion}_${target}.tar.gz`
  const runtimeAsset = `Pisper_Runtime_${runtimeVersion}_${target}.tar.gz`
  if (dryRun) {
    console.log(`[dry-run] platform ${target}`)
    console.log(`[dry-run] TUI ${tuiTag} -> ${tuiAsset}`)
    console.log(`[dry-run] Runtime ${runtimeTag} -> ${runtimeAsset}`)
    console.log('[dry-run] targets:', targetTriples().join(', '))
    return
  }

  const { archivePath: tuiArchive } = await fetchComponent({
    component: 'tui',
    version: tuiVersion,
    label: 'TUI',
    assetName: tuiAsset,
    stageDir,
  })
  const { archivePath: runtimeArchive } = await fetchComponent({
    component: 'runtime',
    version: runtimeVersion,
    label: 'Runtime',
    assetName: runtimeAsset,
    stageDir,
  })

  // Stage TUI CLI payload for the Tauri external binaries.
  const tuiStage = join(stageDir, 'tui')
  await extract(tuiStage, tuiArchive)
  const cliSource = join(tuiStage, `pisper${executableSuffix}`)
  await stat(cliSource).catch((error) => {
    if (error?.code === 'ENOENT') throw new Error('TUI archive is missing the pisper executable.')
    throw error
  })
  const binariesDir = join(root, 'src-tauri', 'binaries')
  await mkdir(binariesDir, { recursive: true })
  for (const triple of targetTriples()) {
    const destination = join(binariesDir, `pisper-cli-${triple}${executableSuffix}`)
    await writeFile(destination, await readFile(cliSource))
    if (process.platform !== 'win32') await chmod(destination, 0o755)
    console.log(`Prepared Tauri CLI payload: ${destination}`)
  }

  // Stage Runtime backend payload: sidecar binary + sidecar-runtime closure.
  const runtimeStage = join(stageDir, 'runtime')
  await extract(runtimeStage, runtimeArchive)
  const sidecarSource = join(runtimeStage, `pisper-sidecar${executableSuffix}`)
  const sidecarRuntimeSource = join(runtimeStage, 'sidecar-runtime')
  await Promise.all([
    stat(sidecarSource).catch((error) => {
      if (error?.code === 'ENOENT') throw new Error('Runtime archive is missing pisper-sidecar.')
      throw error
    }),
    stat(join(sidecarRuntimeSource, 'package.json')).catch((error) => {
      if (error?.code === 'ENOENT') {
        throw new Error('Runtime archive is missing the sidecar-runtime closure.')
      }
      throw error
    }),
  ])
  for (const triple of targetTriples()) {
    const destination = join(binariesDir, `pisper-sidecar-${triple}${executableSuffix}`)
    await writeFile(destination, await readFile(sidecarSource))
    if (process.platform !== 'win32') await chmod(destination, 0o755)
    console.log(`Prepared Tauri sidecar payload: ${destination}`)
  }
  const seaRoot = join(root, 'release', 'sea')
  await rm(join(seaRoot, 'runtime'), { recursive: true, force: true })
  await cp(sidecarRuntimeSource, join(seaRoot, 'runtime'), { recursive: true })
  // npm bin shims are symlinks on POSIX and break Tauri resource mapping
  // ("resource path ... .bin/... doesn't exist"); the SEA runtime never
  // executes them, so remove every .bin directory in the closure.
  const removedBins = await removeBinShims(join(seaRoot, 'runtime'))
  if (removedBins > 0) {
    console.log(`Removed ${removedBins} npm .bin shim director${removedBins === 1 ? 'y' : 'ies'}.`)
  }
  try {
    await cp(
      join(runtimeStage, 'runtime-size-manifest.json'),
      join(seaRoot, 'runtime-size-manifest.json'),
    )
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }
  console.log('Staged sidecar-runtime closure under release/sea/runtime.')

  // Record the bundled component versions in the desktop package manifest.
  const desktopPackagePath = join(root, 'src-tauri', 'desktop-package.json')
  const desktopPackage = JSON.parse(await readFile(desktopPackagePath, 'utf8'))
  desktopPackage.bundled = { tui: tuiVersion, runtime: runtimeVersion }
  await writeFile(desktopPackagePath, `${JSON.stringify(desktopPackage, null, 2)}\n`, 'utf8')
  console.log(
    `Bundled TUI ${tuiVersion} and Runtime ${runtimeVersion} into desktop-package.json (${desktopPackage.version}).`,
  )
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
