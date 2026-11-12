// 桌面壳安装包状态的校验与派生：桥接返回值按不可信输入处理（旧壳、改过的壳都可能给出意外结构），
// 只留认得出的字段，界面据此决定轮询节奏和要显示的操作。
import type { ShellInstallerState, ShellInstallerStatus } from '@/types/update'

const STATES: ReadonlySet<string> = new Set<ShellInstallerState>([
  'unsupported',
  'idle',
  'checking',
  'current',
  'available',
  'downloading',
  'downloaded',
  'error',
])
const RELEASES_URL = 'https://github.com/ling-kong-ran/pisper/releases'
const VERSION = /^\d+\.\d+\.\d+$/
// 与壳侧 shell_installer 的命名一致：Pisper_<版本>_<平台>_<架构><后缀>。
const FILE_NAME =
  /^Pisper_\d+\.\d+\.\d+_(?:windows_x86_64-setup\.exe|darwin_aarch64\.dmg|linux_x86_64\.AppImage)$/
const MAX_NOTES = 64 * 1024
const MAX_MESSAGE = 500

// 检查与下载进行中时每秒刷新一次进度；其余时候低频刷新，好看到后台定时任务开始的下载。
export const SHELL_INSTALLER_ACTIVE_POLL_MS = 1_000
export const SHELL_INSTALLER_IDLE_POLL_MS = 60_000

function text(value: unknown, limit: number) {
  return typeof value === 'string' ? value.slice(0, limit) : ''
}

function count(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? Math.floor(value) : 0
}

function version(value: unknown) {
  return typeof value === 'string' && VERSION.test(value) ? value : ''
}

export function normalizeShellInstallerStatus(value: unknown): ShellInstallerStatus | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const raw = value as Record<string, unknown>
  if (typeof raw.state !== 'string' || !STATES.has(raw.state)) return null
  const releaseUrl = text(raw.releaseUrl, 300)
  const fileName = text(raw.fileName, 120)
  const size = count(raw.size)
  const retryAt = raw.retryAt
  return {
    supported: raw.supported === true,
    state: raw.state as ShellInstallerState,
    currentVersion: version(raw.currentVersion),
    availableVersion: version(raw.availableVersion),
    // 只放行本仓库的发布页，别的地址一律换回发布列表。
    releaseUrl:
      releaseUrl === RELEASES_URL || releaseUrl.startsWith(`${RELEASES_URL}/`)
        ? releaseUrl
        : RELEASES_URL,
    notes: text(raw.notes, MAX_NOTES),
    fileName: FILE_NAME.test(fileName) ? fileName : '',
    size,
    transferred: Math.min(count(raw.transferred), size || Number.MAX_SAFE_INTEGER),
    canDownload: raw.canDownload === true,
    // 缺字段时按壳的默认值（开启）显示。
    autoDownload: raw.autoDownload !== false,
    message: text(raw.message, MAX_MESSAGE),
    retryAt:
      typeof retryAt === 'number' && Number.isFinite(retryAt) && retryAt > 0 ? retryAt : null,
  }
}

export function shellInstallerPollDelay(state: ShellInstallerState) {
  return state === 'checking' || state === 'downloading'
    ? SHELL_INSTALLER_ACTIVE_POLL_MS
    : SHELL_INSTALLER_IDLE_POLL_MS
}
