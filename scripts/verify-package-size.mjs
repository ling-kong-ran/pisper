import { access, readdir, stat } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { join } from 'node:path'

const { Arch } = createRequire(import.meta.url)('builder-util')
const MAX_APP_RESOURCES_BYTES = 110 * 1024 * 1024

async function directorySize(path) {
  let total = 0
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const entryPath = join(path, entry.name)
    total += entry.isDirectory() ? await directorySize(entryPath) : (await stat(entryPath)).size
  }
  return total
}

function clipboardTarget(platform, arch) {
  if (platform === 'win32') return `clipboard-win32-${arch}-msvc`
  if (platform === 'darwin') return `clipboard-darwin-${arch}`
  if (platform === 'linux') return `clipboard-linux-${arch}-gnu`
  throw new Error(`Unsupported Electron target platform: ${platform}`)
}

function sandboxBinary(platform, arch) {
  if (platform === 'win32') return ['srt-win', arch, 'srt-win.exe']
  if (platform === 'linux') return ['seccomp', arch, 'apply-seccomp']
  return null
}

export async function afterPack(context) {
  const platform = context.electronPlatformName
  const arch = Arch[context.arch]
  const resourcesDir = context.packager.getResourcesDir(context.appOutDir)
  const unpackedDir = join(resourcesDir, 'app.asar.unpacked')
  const clipboardDir = join(unpackedDir, 'node_modules', '@mariozechner')
  const expectedClipboard = clipboardTarget(platform, arch)
  const clipboardPackages = (await readdir(clipboardDir))
    .filter((name) => name.startsWith('clipboard-'))
    .sort()

  if (clipboardPackages.length !== 1 || clipboardPackages[0] !== expectedClipboard) {
    throw new Error(`Packaged clipboard binaries do not match ${platform}-${arch}: ${clipboardPackages.join(', ') || 'none'}`)
  }

  const requiredSandboxBinary = sandboxBinary(platform, arch)
  if (requiredSandboxBinary) {
    await access(join(
      unpackedDir,
      'node_modules',
      '@anthropic-ai',
      'sandbox-runtime',
      'vendor',
      ...requiredSandboxBinary,
    ))
  }

  const asarBytes = (await stat(join(resourcesDir, 'app.asar'))).size
  const unpackedBytes = await directorySize(unpackedDir)
  const resourceBytes = asarBytes + unpackedBytes
  if (resourceBytes > MAX_APP_RESOURCES_BYTES) {
    throw new Error(`Packaged app resources exceed 110 MiB: ${(resourceBytes / 1024 / 1024).toFixed(1)} MiB`)
  }

  console.log(`Verified ${platform}-${arch} package resources: ${(resourceBytes / 1024 / 1024).toFixed(1)} MiB`)
}
