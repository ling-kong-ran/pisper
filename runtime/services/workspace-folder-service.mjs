// 新建项目文件夹：在已存在的上级目录里建一个新的空文件夹，供新任务直接选为项目。
// 只建一层：上级目录必须已经存在（沿用工作目录的统一校验），名称必须是单个路径段，
// 不能借名称里的分隔符或 .. 跳到别处，也不会因为上级路径打错而凭空建出一串目录。
import { mkdirSync, statSync } from 'node:fs'
import { mkdir, stat } from 'node:fs/promises'
import { posix, win32 } from 'node:path'
import { resolveWorkspaceDirectory } from '../runtime/workspace-directories.mjs'

const MAX_FOLDER_NAME_LENGTH = 255
const MAX_PARENT_PATH_LENGTH = 4096
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
  if (!name) throw new WorkspaceFolderError('请填写文件夹名称。', 'folder_name_required')
  if (name.length > MAX_FOLDER_NAME_LENGTH)
    throw new WorkspaceFolderError('文件夹名称太长。', 'folder_name_invalid')
  if (
    name === '.' ||
    name === '..' ||
    INVALID_NAME_CHARACTERS.test(name) ||
    [...name].some((character) => (character.codePointAt(0) || 0) < 32) ||
    /[. ]$/.test(name) ||
    RESERVED_WINDOWS_NAMES.test(name)
  )
    throw new WorkspaceFolderError(
      '文件夹名称不能包含 / \\ : * ? " < > | 等字符，也不能以点或空格结尾。',
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
    }
  return {
    makeDirectory: (/** @type {string} */ path) => mkdir(path),
    inspect: (/** @type {string} */ path) => stat(path),
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
    throw new WorkspaceFolderError('上级文件夹路径太长。', 'folder_parent_invalid')
  let parent
  try {
    parent = await resolveWorkspaceDirectory(parentInput, fallback, { platform, profile })
  } catch {
    throw new WorkspaceFolderError('上级文件夹不存在或不是文件夹。', 'folder_parent_missing')
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
      throw new WorkspaceFolderError('同名的文件已经存在。', 'folder_exists_as_file', 409)
    }
    if (code === 'EACCES' || code === 'EPERM' || code === 'EROFS')
      throw new WorkspaceFolderError('没有权限在这里新建文件夹。', 'folder_permission_denied', 403)
    throw new WorkspaceFolderError('新建文件夹失败。', 'folder_create_failed', 500)
  }
}
