import { homedir } from 'node:os'
import { join } from 'node:path'
import process from 'node:process'

export function releasePlatform(platform = process.platform) {
  if (platform === 'win32') return 'windows'
  if (platform === 'darwin') return 'darwin'
  if (platform === 'linux') return 'linux'
  throw new Error(`unsupported operating system: ${platform}`)
}

export function releaseArchitecture(arch = process.arch) {
  if (arch === 'x64') return 'x86_64'
  if (arch === 'arm64') return 'aarch64'
  throw new Error(`unsupported CPU architecture: ${arch}`)
}

export function supportedTarget(platform = process.platform, arch = process.arch) {
  const target = `${releasePlatform(platform)}_${releaseArchitecture(arch)}`
  const supported = new Set(['windows_x86_64', 'darwin_x86_64', 'darwin_aarch64', 'linux_x86_64'])
  if (!supported.has(target)) {
    throw new Error(`Pisper does not publish a TUI package for ${target}`)
  }
  return target
}

export function executableName(platform = process.platform) {
  return platform === 'win32' ? 'pisper.exe' : 'pisper'
}

export function localDataDirectory(platform = process.platform, env = process.env) {
  if (env.PISPER_NPM_INSTALL_DIR) return env.PISPER_NPM_INSTALL_DIR
  if (platform === 'win32') {
    if (!env.LOCALAPPDATA) throw new Error('LOCALAPPDATA is unavailable')
    return env.LOCALAPPDATA
  }
  if (platform === 'darwin') return join(homedir(), 'Library', 'Application Support')
  return env.XDG_DATA_HOME || join(homedir(), '.local', 'share')
}

export function componentsRoot(platform = process.platform, env = process.env) {
  return join(localDataDirectory(platform, env), 'com.lingkongran.pisper', 'components')
}
