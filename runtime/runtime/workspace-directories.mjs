import { readdir, stat } from 'node:fs/promises'
import { dirname, isAbsolute, join, resolve, win32 } from 'node:path'

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
