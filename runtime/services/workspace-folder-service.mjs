// 新建项目文件夹：在已存在的上级目录里建一个新的空文件夹，供新任务直接选为项目。
// 只建一层：上级目录必须已经存在（沿用工作目录的统一校验），名称必须是单个路径段，
// 不能借名称里的分隔符或 .. 跳到别处，也不会因为上级路径打错而凭空建出一串目录。
// 列出子文件夹：手机、配对设备看不到主机的系统文件管理器，靠它在应用里逐级点选工作目录；
// 只给文件夹名，不列文件、不读内容。
import { mkdirSync, readdirSync, statSync } from 'node:fs'
import { mkdir, readdir, stat } from 'node:fs/promises'
import { posix, win32 } from 'node:path'
import { resolveWorkspaceDirectory } from '../runtime/workspace-directories.mjs'

const MAX_FOLDER_NAME_LENGTH = 255
const MAX_PARENT_PATH_LENGTH = 4096
// 一层里文件夹再多也只回这么多：选工作目录用不着翻几千项，也免得超大目录拖慢请求。
const MAX_LISTED_FOLDERS = 1000
// Windows 文件名里不允许的字符与保留设备名；其他平台只禁止分隔符，但统一按最严格的规则校验，
// 同一个项目名在桌面与配对设备之间搬动时不会在某一端变成非法名称。
const INVALID_NAME_CHARACTERS = /[<>:"/\\|?*]/
const RESERVED_WINDOWS_NAMES = /^(con|prn|aux|nul|com[0-9]|lpt[0-9])(\..*)?$/i

export class WorkspaceFolderError extends Error {
  /** @param {string} message @param {string} code @param {number} [statusCode] */
  constructor(message, code, statusCode = 400) {
    super(message)
    this.code = code
    this.statusCode = statusCode
  }
}

/** @param {unknown} value */
export function normalizeWorkspaceFolderName(value) {
  const name = typeof value === 'string' ? value.trim() : ''
  if (!name) throw new WorkspaceFolderError('Folder name is required.', 'folder_name_required')
  if (name.length > MAX_FOLDER_NAME_LENGTH)
    throw new WorkspaceFolderError('Folder name is too long.', 'folder_name_invalid')
  if (
    name === '.' ||
    name === '..' ||
    INVALID_NAME_CHARACTERS.test(name) ||
    [...name].some((character) => (character.codePointAt(0) || 0) < 32) ||
    /[. ]$/.test(name) ||
    RESERVED_WINDOWS_NAMES.test(name)
  )
    throw new WorkspaceFolderError(
      'Folder name cannot contain / \\ : * ? " < > | or end with a dot or space.',
      'folder_name_invalid',
    )
  return name
}

/** @param {string | undefined} profile */
function folderIo(profile) {
  // 与工作目录校验同一个原因：Node Mobile 的异步 FS worker 在部分 Android WebView 宿主中不会回调。
  if (profile === 'mobile-embedded' || profile === 'mobile-store')
    return {
      makeDirectory: async (/** @type {string} */ path) => mkdirSync(path),
      inspect: async (/** @type {string} */ path) => statSync(path),
      readDirectory: async (/** @type {string} */ path) =>
        readdirSync(path, { withFileTypes: true }),
    }
  return {
    makeDirectory: (/** @type {string} */ path) => mkdir(path),
    inspect: (/** @type {string} */ path) => stat(path),
    readDirectory: (/** @type {string} */ path) => readdir(path, { withFileTypes: true }),
  }
}

/**
 * 在 parent 下新建名为 name 的文件夹；parent 为空时用 Runtime 的默认工作目录。
 * 同名文件夹已经存在时直接返回它（created: false），同名的是文件时报错。
 * @param {{ parent?: unknown, name?: unknown }} input
 * @param {{ fallback?: string, platform?: NodeJS.Platform, profile?: string }} [options]
 * @returns {Promise<{ path: string, created: boolean }>}
 */
export async function createWorkspaceFolder(input, options = {}) {
  const {
    fallback = '',
    platform = process.platform,
    profile = process.env.PISPER_RUNTIME_PROFILE,
  } = options
  const name = normalizeWorkspaceFolderName(input?.name)
  const parentInput = typeof input?.parent === 'string' ? input.parent.trim() : ''
  if (parentInput.length > MAX_PARENT_PATH_LENGTH)
    throw new WorkspaceFolderError('Parent folder path is too long.', 'folder_parent_invalid')
  let parent
  try {
    parent = await resolveWorkspaceDirectory(parentInput, fallback, { platform, profile })
  } catch {
    throw new WorkspaceFolderError('Parent folder not found.', 'folder_parent_missing')
  }
  const path = (platform === 'win32' ? win32 : posix).join(parent, name)
  const io = folderIo(profile)
  try {
    await io.makeDirectory(path)
    return { path, created: true }
  } catch (error) {
    const code = /** @type {{ code?: string }} */ (error)?.code
    if (code === 'EEXIST') {
      const info = await io.inspect(path).catch(() => null)
      if (info?.isDirectory()) return { path, created: false }
      throw new WorkspaceFolderError(
        'A file with this name already exists.',
        'folder_exists_as_file',
        409,
      )
    }
    if (code === 'EACCES' || code === 'EPERM' || code === 'EROFS')
      throw new WorkspaceFolderError(
        'No permission to create a folder here.',
        'folder_permission_denied',
        403,
      )
    throw new WorkspaceFolderError('Folder creation failed.', 'folder_create_failed', 500)
  }
}

/**
 * 列出 path 下的子文件夹；path 为空时从 Runtime 的默认工作目录开始。
 * 隐藏文件夹（点开头）不列，指向文件夹的符号链接照常列出；parent 为空表示已经到根。
 * 子文件夹直接给出完整路径：客户端不必按主机平台自己拼分隔符。
 * @param {{ path?: unknown }} input
 * @param {{ fallback?: string, platform?: NodeJS.Platform, profile?: string }} [options]
 * @returns {Promise<{ path: string, parent: string, folders: { name: string, path: string }[], truncated: boolean }>}
 */
export async function listWorkspaceFolders(input, options = {}) {
  const {
    fallback = '',
    platform = process.platform,
    profile = process.env.PISPER_RUNTIME_PROFILE,
  } = options
  const pathInput = typeof input?.path === 'string' ? input.path.trim() : ''
  if (pathInput.length > MAX_PARENT_PATH_LENGTH)
    throw new WorkspaceFolderError('Folder path is too long.', 'folder_path_invalid')
  let path
  try {
    path = await resolveWorkspaceDirectory(pathInput, fallback, { platform, profile })
  } catch {
    throw new WorkspaceFolderError('Folder not found.', 'folder_missing', 404)
  }
  const io = folderIo(profile)
  const paths = platform === 'win32' ? win32 : posix
  let entries
  try {
    entries = await io.readDirectory(path)
  } catch (error) {
    const code = /** @type {{ code?: string }} */ (error)?.code
    if (code === 'EACCES' || code === 'EPERM')
      throw new WorkspaceFolderError(
        'No permission to open this folder.',
        'folder_permission_denied',
        403,
      )
    throw new WorkspaceFolderError('Folder could not be read.', 'folder_list_failed', 500)
  }
  /** @type {string[]} */
  const folders = []
  for (const entry of entries) {
    if (entry.name.startsWith('.')) continue
    if (entry.isDirectory()) folders.push(entry.name)
    else if (entry.isSymbolicLink()) {
      const info = await io.inspect(paths.join(path, entry.name)).catch(() => null)
      if (info?.isDirectory()) folders.push(entry.name)
    }
  }
  folders.sort((left, right) =>
    left.localeCompare(right, undefined, { numeric: true, sensitivity: 'base' }),
  )
  const parent = paths.dirname(path)
  return {
    path,
    parent: parent === path ? '' : parent,
    folders: folders
      .slice(0, MAX_LISTED_FOLDERS)
      .map((name) => ({ name, path: paths.join(path, name) })),
    truncated: folders.length > MAX_LISTED_FOLDERS,
  }
}
