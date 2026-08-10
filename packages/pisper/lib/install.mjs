import { chmod, mkdir, open, readFile, rename, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { PublicKey, Signature } from '@threema/wasm-minisign-verify'
import { x as extractTar } from 'tar'
import {
  componentsRoot,
  executableName,
  releaseArchitecture,
  releasePlatform,
  supportedTarget,
} from './platform.mjs'

const packageRoot = fileURLToPath(new URL('..', import.meta.url))
const packageManifest = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'))
const repository = packageManifest.pisper?.repository
const tuiVersion = packageManifest.pisper?.tuiVersion
const runtimeVersion = packageManifest.pisper?.runtimeVersion
const MAX_ARCHIVE_BYTES = 128 * 1024 * 1024
const INSTALL_WAIT_MS = 600_000

for (const [label, value] of [
  ['TUI', tuiVersion],
  ['Runtime', runtimeVersion],
]) {
  if (!/^\d+\.\d+\.\d+$/.test(value || '')) {
    throw new Error(`pisper package metadata does not contain a valid ${label} version`)
  }
}
if (!/^[\w.-]+\/[\w.-]+$/.test(repository || '')) {
  throw new Error('pisper package metadata does not contain a valid repository')
}

function decodeWrapped(value) {
  const trimmed = String(value).trim()
  if (trimmed.startsWith('untrusted comment:')) return trimmed
  if (!/^[A-Za-z0-9+/=\r\n]+$/.test(trimmed)) return trimmed
  const decoded = Buffer.from(trimmed.replace(/\s/g, ''), 'base64').toString('utf8').trim()
  return decoded.startsWith('untrusted comment:') ? decoded : trimmed
}

function componentAsset(component, version, platform, arch) {
  const target = supportedTarget(platform, arch)
  const label = component === 'tui' ? 'TUI_Component' : 'Runtime'
  return `Pisper_${label}_${version}_${target}.tar.gz`
}

function componentUrls({ component, version, platform, arch, baseUrl }) {
  const asset = componentAsset(component, version, platform, arch)
  const root = baseUrl
    ? String(baseUrl).replace(/\/$/, '')
    : `https://github.com/${repository}/releases/download/${component}-v${version}`
  return { archive: `${root}/${asset}`, signature: `${root}/${asset}.sig` }
}

export function releaseAsset(
  version = tuiVersion,
  platform = process.platform,
  arch = process.arch,
) {
  return componentAsset('tui', version, platform, arch)
}

export function releaseAssetUrls({
  version = tuiVersion,
  platform = process.platform,
  arch = process.arch,
  baseUrl = process.env.PISPER_CLI_DOWNLOAD_BASE_URL,
} = {}) {
  return componentUrls({ component: 'tui', version, platform, arch, baseUrl })
}

function compareVersions(left, right) {
  const a = left.split('.').map(Number)
  const b = right.split('.').map(Number)
  for (let index = 0; index < 3; index += 1) {
    if (a[index] !== b[index]) return a[index] - b[index]
  }
  return 0
}

async function isFile(path) {
  try {
    return (await stat(path)).isFile()
  } catch (error) {
    if (error?.code === 'ENOENT') return false
    throw error
  }
}

function commandName(component) {
  if (component === 'tui') return executableName()
  return process.platform === 'win32' ? 'pisper-sidecar.exe' : 'pisper-sidecar'
}

async function validateComponent(component, version, destination) {
  const executable = join(destination, commandName(component))
  if (!(await isFile(executable))) return ''
  if (
    component === 'runtime' &&
    !(await isFile(join(destination, 'sidecar-runtime', 'package.json')))
  ) {
    return ''
  }
  try {
    const manifest = JSON.parse(await readFile(join(destination, 'manifest.json'), 'utf8'))
    if (
      manifest.version !== version ||
      manifest.platform !== releasePlatform() ||
      manifest.arch !== releaseArchitecture() ||
      manifest.command !== commandName(component)
    ) {
      return ''
    }
  } catch {
    return ''
  }
  return executable
}

async function resolveCurrentComponent(root, component, minimumVersion) {
  try {
    const pointer = JSON.parse(await readFile(join(root, component, 'current.json'), 'utf8'))
    if (!/^\d+\.\d+\.\d+$/.test(pointer.version || '')) return null
    if (compareVersions(pointer.version, minimumVersion) < 0) return null
    const destination = join(root, component, 'versions', pointer.version)
    const executable = await validateComponent(component, pointer.version, destination)
    return executable ? { version: pointer.version, destination, executable } : null
  } catch (error) {
    if (error?.code === 'ENOENT' || error instanceof SyntaxError) return null
    throw error
  }
}

async function download(url, { label = url } = {}) {
  const response = await fetch(url, {
    headers: { 'User-Agent': `pisper/${packageManifest.version}` },
    redirect: 'follow',
    signal: AbortSignal.timeout(300_000),
  })
  if (!response.ok) throw new Error(`download failed with HTTP ${response.status}: ${url}`)
  const declaredSize = Number(response.headers.get('content-length') || 0)
  if (declaredSize > MAX_ARCHIVE_BYTES) {
    throw new Error(`download is larger than ${MAX_ARCHIVE_BYTES} bytes`)
  }
  if (!response.body) {
    const bytes = Buffer.from(await response.arrayBuffer())
    return validateDownloadBytes(bytes, url)
  }
  // Stream with visible progress so a large component download never looks
  // like an install that is stuck spinning.
  const reader = response.body.getReader()
  const chunks = []
  let received = 0
  let lastReported = 0
  while (true) {
    const { done, value } = await reader.read()
    if (done) break
    chunks.push(value)
    received += value.length
    const percent = declaredSize ? Math.floor((received / declaredSize) * 100) : 0
    if (percent - lastReported >= 10 || (declaredSize && received >= declaredSize)) {
      lastReported = percent
      const megabytes = (received / 1024 / 1024).toFixed(1)
      console.log(`  ${label}: ${megabytes} MB (${percent}%)`)
    }
  }
  const bytes = Buffer.concat(chunks)
  return validateDownloadBytes(bytes, url)
}

function validateDownloadBytes(bytes, url) {
  if (bytes.length === 0) throw new Error(`downloaded an empty file: ${url}`)
  if (bytes.length > MAX_ARCHIVE_BYTES) {
    throw new Error(`download is larger than ${MAX_ARCHIVE_BYTES} bytes`)
  }
  return bytes
}

async function downloadWithRetry(url, label) {
  const attempts = Math.max(1, Number(process.env.PISPER_CLI_DOWNLOAD_ATTEMPTS) || 3)
  let lastError
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      return await download(url, { label })
    } catch (error) {
      lastError = error
      const retryable =
        error?.name === 'AbortError' ||
        /ECONNRESET|ETIMEDOUT|EPIPE|ENOTFOUND|EAI_AGAIN|socket|network/i.test(error?.message || '')
      if (!retryable || attempt === attempts) throw lastError
      console.log(`  ${label}: download attempt ${attempt} failed (${error.message}); retrying…`)
      await wait(1000 * attempt)
    }
  }
  throw lastError
}

async function verifyArchive(component, archive, signatureBytes) {
  const publicKeyText = await readFile(join(packageRoot, 'updater.pubkey'), 'utf8')
  const publicKey = PublicKey.decode(decodeWrapped(publicKeyText))
  const signature = Signature.decode(decodeWrapped(signatureBytes.toString('utf8')))
  try {
    if (!publicKey.verify(archive, signature)) {
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

async function wait(milliseconds) {
  await new Promise((resolve) => setTimeout(resolve, milliseconds))
}

async function acquireLock(lockPath, validate) {
  const deadline = Date.now() + INSTALL_WAIT_MS
  while (true) {
    try {
      return await open(lockPath, 'wx')
    } catch (error) {
      if (error?.code !== 'EEXIST') throw error
      if (await validate()) return null
      if (Date.now() >= deadline) {
        throw new Error('timed out waiting for another Pisper installer')
      }
      await wait(250)
    }
  }
}

async function activateComponent(root, component, version) {
  const componentRoot = join(root, component)
  const pointer = join(componentRoot, 'current.json')
  const temporary = join(componentRoot, `current.json.tmp-${process.pid}`)
  await mkdir(componentRoot, { recursive: true })
  await writeFile(temporary, `${JSON.stringify({ version }, null, 2)}\n`, 'utf8')
  await rm(pointer, { force: true })
  await rename(temporary, pointer)
}

async function installComponent({ root, component, version, destination, activate = false }) {
  const lockRoot = join(root, 'npm')
  await mkdir(lockRoot, { recursive: true })
  const lockPath = join(lockRoot, `${component}-install.lock`)
  const validate = () => validateComponent(component, version, destination)
  const lock = await acquireLock(lockPath, validate)
  if (!lock) {
    if (activate) await activateComponent(root, component, version)
    return { version, destination, executable: await validate() }
  }

  const staging = `${destination}.tmp-${process.pid}-${Date.now()}`
  const archivePath = `${staging}.tar.gz`
  try {
    const existing = await validate()
    if (existing) {
      if (activate) await activateComponent(root, component, version)
      return { version, destination, executable: existing }
    }
    await rm(staging, { recursive: true, force: true })
    await mkdir(staging, { recursive: true })
    const urls = componentUrls({
      component,
      version,
      platform: process.platform,
      arch: process.arch,
      baseUrl: process.env.PISPER_CLI_DOWNLOAD_BASE_URL,
    })
    const [archive, signature] = await Promise.all([
      downloadWithRetry(urls.archive, `${component} archive`),
      downloadWithRetry(urls.signature, `${component} signature`),
    ])
    await verifyArchive(component, archive, signature)
    await writeFile(archivePath, archive, { mode: 0o600 })
    await extractTar({
      cwd: staging,
      file: archivePath,
      preservePaths: false,
      strict: true,
      strip: 1,
      filter: (_path, entry) => entry.type === 'File' || entry.type === 'Directory',
    })
    if (process.platform !== 'win32') await chmod(join(staging, commandName(component)), 0o755)
    const stagedExecutable = await validateComponent(component, version, staging)
    if (!stagedExecutable) {
      throw new Error(`downloaded ${component} archive has an invalid package layout`)
    }
    await mkdir(dirname(destination), { recursive: true })
    await rm(destination, { recursive: true, force: true })
    await rename(staging, destination)
    if (activate) await activateComponent(root, component, version)
    return { version, destination, executable: join(destination, commandName(component)) }
  } finally {
    await lock.close()
    await Promise.all([
      rm(lockPath, { force: true }),
      rm(staging, { recursive: true, force: true }),
      rm(archivePath, { force: true }),
    ])
  }
}

export async function ensurePisperInstallation() {
  const root = componentsRoot()
  const npmTuiDestination = join(root, 'npm', 'versions', tuiVersion)
  const runtimeDestination = join(root, 'runtime', 'versions', runtimeVersion)
  const [tui, runtime] = await Promise.all([
    resolveCurrentComponent(root, 'tui', tuiVersion).then(
      (current) =>
        current ||
        installComponent({
          root,
          component: 'tui',
          version: tuiVersion,
          destination: npmTuiDestination,
        }),
    ),
    resolveCurrentComponent(root, 'runtime', runtimeVersion).then(
      (current) =>
        current ||
        installComponent({
          root,
          component: 'runtime',
          version: runtimeVersion,
          destination: runtimeDestination,
          activate: true,
        }),
    ),
  ])
  return {
    executable: tui.executable,
    sidecar: runtime.executable,
    appRoot: join(runtime.destination, 'sidecar-runtime'),
  }
}

export async function ensurePisperExecutable() {
  return (await ensurePisperInstallation()).executable
}
