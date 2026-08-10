import { createRequire } from 'node:module'
import { chmod, mkdir, open, readFile, rename, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { PublicKey, Signature } from '@threema/wasm-minisign-verify'
import { x as extractTar } from 'tar'
import { npmPlatformAlias, npmPlatformTarget, npmPlatformVersion } from './npm-platform.mjs'
import {
  componentsRoot,
  executableName,
  releaseArchitecture,
  releasePlatform,
  supportedTarget,
} from './platform.mjs'

const require = createRequire(import.meta.url)
const packageRoot = fileURLToPath(new URL('..', import.meta.url))
const packageManifest = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'))
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

function decodeWrapped(value) {
  const trimmed = String(value).trim()
  if (trimmed.startsWith('untrusted comment:')) return trimmed
  if (!/^[A-Za-z0-9+/=\r\n]+$/.test(trimmed)) return trimmed
  const decoded = Buffer.from(trimmed.replace(/\s/g, ''), 'base64').toString('utf8').trim()
  return decoded.startsWith('untrusted comment:') ? decoded : trimmed
}

function componentAsset(component, version, platform = process.platform, arch = process.arch) {
  const target = supportedTarget(platform, arch)
  const label = component === 'tui' ? 'TUI_Component' : 'Runtime'
  return `Pisper_${label}_${version}_${target}.tar.gz`
}

export function releaseAsset(
  version = tuiVersion,
  platform = process.platform,
  arch = process.arch,
) {
  return componentAsset('tui', version, platform, arch)
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

async function installedComponent(component, version, destination) {
  const executable = await validateComponent(component, version, destination)
  return executable ? { version, destination, executable } : null
}

async function resolveCurrentComponent(root, component, minimumVersion) {
  try {
    const pointer = JSON.parse(await readFile(join(root, component, 'current.json'), 'utf8'))
    if (!/^\d+\.\d+\.\d+$/.test(pointer.version || '')) return null
    if (compareVersions(pointer.version, minimumVersion) < 0) return null
    const destination = join(root, component, 'versions', pointer.version)
    return installedComponent(component, pointer.version, destination)
  } catch (error) {
    if (error?.code === 'ENOENT' || error instanceof SyntaxError) return null
    throw error
  }
}

async function resolvePlatformBundle() {
  const target = npmPlatformTarget()
  const alias = npmPlatformAlias()
  let manifestPath
  try {
    manifestPath = require.resolve(`${alias}/package.json`)
  } catch (error) {
    throw new Error(
      `the npm platform package ${alias} is missing; reinstall pisper without --omit=optional`,
      { cause: error },
    )
  }
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
  const metadata = manifest.pisperBundle
  const expectedVersion = npmPlatformVersion(packageManifest.version)
  if (
    manifest.name !== 'pisper' ||
    manifest.version !== expectedVersion ||
    metadata?.target !== target.release ||
    metadata?.tuiVersion !== tuiVersion ||
    metadata?.runtimeVersion !== runtimeVersion
  ) {
    throw new Error(
      `npm platform package ${alias} does not match pisper@${packageManifest.version}`,
    )
  }
  return dirname(manifestPath)
}

async function readBundledAsset(bundleRoot, component, version) {
  const asset = componentAsset(component, version)
  const archivePath = join(bundleRoot, 'components', component, asset)
  const signaturePath = `${archivePath}.sig`
  let size
  try {
    size = (await stat(archivePath)).size
  } catch (error) {
    throw new Error(`npm platform package is missing ${component} archive ${asset}`, {
      cause: error,
    })
  }
  if (size <= 0 || size > MAX_ARCHIVE_BYTES) {
    throw new Error(`npm platform package contains an invalid ${component} archive size: ${size}`)
  }
  const [archive, signature] = await Promise.all([readFile(archivePath), readFile(signaturePath)])
  return { archive, signature }
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

async function installComponent({
  root,
  bundleRoot,
  component,
  version,
  destination,
  activate = false,
}) {
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

    console.log(`pisper: installing ${component} ${version} from the npm platform package`)
    await rm(staging, { recursive: true, force: true })
    await mkdir(staging, { recursive: true })
    const { archive, signature } = await readBundledAsset(bundleRoot, component, version)
    console.log(`pisper: verifying ${component} ${version} signature`)
    await verifyArchive(component, archive, signature)
    console.log(`pisper: extracting ${component} ${version}`)
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
      throw new Error(`bundled ${component} archive has an invalid package layout`)
    }
    await mkdir(dirname(destination), { recursive: true })
    await rm(destination, { recursive: true, force: true })
    await rename(staging, destination)
    if (activate) await activateComponent(root, component, version)
    console.log(`pisper: ${component} ${version} is ready`)
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
      (current) => current || installedComponent('tui', tuiVersion, npmTuiDestination),
    ),
    resolveCurrentComponent(root, 'runtime', runtimeVersion).then(
      (current) => current || installedComponent('runtime', runtimeVersion, runtimeDestination),
    ),
  ])
  if (tui && runtime) {
    return {
      executable: tui.executable,
      sidecar: runtime.executable,
      appRoot: join(runtime.destination, 'sidecar-runtime'),
    }
  }

  const bundleRoot = await resolvePlatformBundle()
  const [installedTui, installedRuntime] = await Promise.all([
    tui ||
      installComponent({
        root,
        bundleRoot,
        component: 'tui',
        version: tuiVersion,
        destination: npmTuiDestination,
      }),
    runtime ||
      installComponent({
        root,
        bundleRoot,
        component: 'runtime',
        version: runtimeVersion,
        destination: runtimeDestination,
        activate: true,
      }),
  ])
  return {
    executable: installedTui.executable,
    sidecar: installedRuntime.executable,
    appRoot: join(installedRuntime.destination, 'sidecar-runtime'),
  }
}

export async function ensurePisperExecutable() {
  return (await ensurePisperInstallation()).executable
}
