import { stat } from 'node:fs/promises'
import { resolve, win32 } from 'node:path'

/**
 * @param {unknown} value
 * @param {string} [platform]
 * @returns {string}
 */
export function normalizeWorkspacePath(value, platform = process.platform) {
  const path = String(value || '').trim()
  if (platform !== 'win32') return path
  if (/^\\\\\?\\UNC\\/i.test(path)) return `\\\\${path.slice(8)}`
  if (/^\\\\\?\\/.test(path)) return path.slice(4)
  return path
}

/**
 * @param {unknown} value
 * @param {string} [platform]
 * @returns {string}
 */
export function workspacePathKey(value, platform = process.platform) {
  const path = normalizeWorkspacePath(value, platform).replace(/[\\/]+$/, '')
  return platform === 'win32' ? path.toLowerCase() : path
}

/**
 * @param {unknown} input
 * @param {unknown} fallback
 * @returns {Promise<string>}
 */
export async function resolveWorkspaceDirectory(
  input,
  fallback,
  { platform = process.platform, inspectDirectory = stat } = {},
) {
  const requested = normalizeWorkspacePath(input || fallback, platform)
  const path = normalizeWorkspacePath(
    platform === 'win32' ? win32.resolve(requested) : resolve(requested),
    platform,
  )
  const info = await inspectDirectory(path).catch(() => null)
  if (!info?.isDirectory()) throw new Error('工作目录不存在或不是文件夹。')
  return path
}
