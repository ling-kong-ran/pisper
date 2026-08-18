// 工作区路径工具：统一路径规范化、大小写归一化（Windows）与目录解析/列举。
// 前端目录选择器与运行时路径校验共用这套逻辑，保证路径比较口径一致。
import { readdir, stat } from 'node:fs/promises'
import { dirname, isAbsolute, join, resolve, win32 } from 'node:path'

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
 * @returns {Promise<string>}
 */
export async function resolveWorkspaceDirectory(
  input,
  fallback,
  { platform = process.platform, inspectDirectory = stat } = {},
) {
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
  const info = await inspectDirectory(path).catch(() => null)
  if (!info?.isDirectory()) throw new Error('工作目录不存在或不是文件夹。')
  return path
}

// 仅返回子目录列表（Web 目录浏览器的目录模式）。
/**
 * List the subdirectories of a workspace path for the Web directory browser.
 * @param {unknown} input
 * @param {unknown} fallback
 */
export async function listWorkspaceDirectories(
  input,
  fallback,
  { inspectDirectory = stat, readDirectory = readdir } = {},
) {
  const listing = await listWorkspaceEntries(input, fallback, {
    inspectDirectory,
    readDirectory,
  })
  return { path: listing.path, parent: listing.parent, directories: listing.directories }
}

// 列出目录与文件（不读内容、不查文件大小）：目录/文件分别按自然序排序并截断数量，
// 避免超大目录拖垮目录选择器。
/**
 * List directories and files without reading file contents or inspecting file sizes.
 * @param {unknown} input
 * @param {unknown} fallback
 */
export async function listWorkspaceEntries(
  input,
  fallback,
  { inspectDirectory = stat, readDirectory = readdir } = {},
) {
  const path = await resolveWorkspaceDirectory(input, fallback, { inspectDirectory })
  const entries = await readDirectory(path, { withFileTypes: true })
  /** @param {{ name: string }} left @param {{ name: string }} right */
  const byName = (left, right) =>
    left.name.localeCompare(right.name, undefined, { numeric: true, sensitivity: 'base' })
  const directories = entries
    .filter((entry) => entry.isDirectory())
    .map((entry) => ({ name: entry.name, path: join(path, entry.name) }))
    .sort(byName)
    .slice(0, 300)
  const files = entries
    .filter((entry) => entry.isFile())
    .map((entry) => ({ name: entry.name, path: join(path, entry.name) }))
    .sort(byName)
    .slice(0, 500)
  const parent = dirname(path)
  return { path, parent: parent === path ? null : parent, directories, files }
}
