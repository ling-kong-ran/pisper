// 工作区路径工具：统一路径规范化、大小写归一化（Windows）与目录校验。
// 保留工作目录设置所需的路径校验，不向客户端枚举宿主文件系统。
import { statSync } from 'node:fs'
import { stat } from 'node:fs/promises'
import { isAbsolute, resolve, win32 } from 'node:path'

/** @typedef {(path: string) => import('node:fs').Stats | Promise<import('node:fs').Stats>} InspectDirectory */
/**
 * @typedef {object} WorkspaceDirectoryOptions
 * @property {string} [profile]
 * @property {string} [platform]
 * @property {InspectDirectory} [inspectDirectory]
 */

/** @returns {{ inspectDirectory: InspectDirectory }} */
function workspaceDirectoryIo(profile = process.env.PISPER_RUNTIME_PROFILE) {
  // Node Mobile 的异步 FS worker 在部分 Android WebView 宿主中不会回调；
  // 本机模式改用同步 stat 可避免工作目录校验请求永久占住。
  if (profile === 'mobile-embedded' || profile === 'mobile-store') {
    return {
      inspectDirectory: (path) => statSync(path),
    }
  }
  return {
    inspectDirectory: (path) => stat(path),
  }
}

// 规范化路径：去掉 Windows 的 \\?\ 前缀（\\?\UNC\server\share 转回 \\server\share），
// 使路径可以在本地路径体系内使用。
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

// 路径比较键：去除尾部分隔符；Windows 下统一小写，避免大小写差异导致同一目录被当成两个。
/**
 * @param {unknown} value
 * @param {string} [platform]
 * @returns {string}
 */
export function workspacePathKey(value, platform = process.platform) {
  const path = normalizeWorkspacePath(value, platform).replace(/[\\/]+$/, '')
  return platform === 'win32' ? path.toLowerCase() : path
}

// 解析工作目录：绝对路径直接用；相对路径基于 fallback（或进程 cwd）解析，
// 并校验目标确实存在且是文件夹，否则抛错（前端据此提示非法目录）。
/**
 * @param {unknown} input
 * @param {unknown} fallback
 * @param {WorkspaceDirectoryOptions} [options]
 * @returns {Promise<string>}
 */
export async function resolveWorkspaceDirectory(input, fallback, options = {}) {
  const {
    platform = process.platform,
    inspectDirectory = workspaceDirectoryIo(options.profile).inspectDirectory,
  } = options
  const requested = normalizeWorkspacePath(input || fallback, platform)
  const base = fallback ? normalizeWorkspacePath(fallback, platform) : ''
  const path = normalizeWorkspacePath(
    platform === 'win32'
      ? win32.isAbsolute(requested)
        ? win32.resolve(requested)
        : win32.resolve(base, requested)
      : isAbsolute(requested)
        ? resolve(requested)
        : resolve(base || process.cwd(), requested),
    platform,
  )
  const info = await Promise.resolve()
    .then(() => inspectDirectory(path))
    .catch(() => null)
  if (!info?.isDirectory()) throw new Error('工作目录不存在或不是文件夹。')
  return path
}
