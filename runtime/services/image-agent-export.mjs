import { randomUUID } from 'node:crypto'
import { constants } from 'node:fs'
import { lstat, mkdir, open, realpath, rm } from 'node:fs/promises'
import { isAbsolute, join, relative, resolve, sep } from 'node:path'
import { parseImageOutput } from '../../shared/image/image-operations.mjs'
import { assertRasterBounds, readRasterDimensions } from '../../shared/image/raster-image.mjs'

/** @typedef {import('../../shared/workflow/workflow-inputs.mjs').WorkflowMedia} Media */
/** @typedef {{path:string,info:import('node:fs').Stats}} DirectoryIdentity */
const MAX_BYTES = 8 * 1024 * 1024

/** @param {string} code @param {number} [statusCode] */
function failure(code, statusCode = 400) {
  return Object.assign(new Error(code), { code, statusCode })
}
/** @param {AbortSignal|undefined} signal */
function active(signal) {
  if (signal?.aborted)
    throw Object.assign(failure('image_tools_export_cancelled', 409), { name: 'AbortError' })
}
/** @param {Media} expected @param {Media} actual */
function sameMedia(expected, actual) {
  return (
    expected.id === actual.id &&
    expected.name === actual.name &&
    expected.size === actual.size &&
    expected.mimeType === actual.mimeType
  )
}
/** @param {import('node:fs').Stats} first @param {import('node:fs').Stats} second */
function sameFile(first, second) {
  return first.dev === second.dev && first.ino === second.ino && first.mode === second.mode
}
/** @param {string} root @param {string} target */
function contained(root, target) {
  const suffix = relative(root, target)
  return suffix !== '' && suffix !== '..' && !suffix.startsWith(`..${sep}`) && !isAbsolute(suffix)
}
/** @param {string} root @param {string} path */
async function inspectDirectory(root, path) {
  if (path !== root && !contained(root, path)) throw failure('image_tools_export_invalid')
  const info = await lstat(path)
  if (!info.isDirectory() || info.isSymbolicLink() || (await realpath(path)) !== path)
    throw failure('image_tools_export_invalid')
  return { path, info }
}
/** @param {string} root @param {DirectoryIdentity[]} identities */
async function verifyDirectories(root, identities) {
  for (const entry of identities) {
    const current = await inspectDirectory(root, entry.path)
    if (!sameFile(entry.info, current.info)) throw failure('image_tools_export_changed')
  }
}
/** @param {string} root @param {DirectoryIdentity[]} identities @param {string} name */
async function childDirectory(root, identities, name) {
  await verifyDirectories(root, identities)
  const path = join(identities[identities.length - 1].path, name)
  await mkdir(path, { mode: 0o700 }).catch((error) => {
    if (!error || typeof error !== 'object' || !('code' in error) || error.code !== 'EEXIST')
      throw error
  })
  const directory = await inspectDirectory(root, path)
  identities.push(directory)
}
/** @param {string} root @param {DirectoryIdentity[]} identities @param {string} name @param {Uint8Array} bytes @param {AbortSignal|undefined} signal */
async function writeNewFile(root, identities, name, bytes, signal) {
  active(signal)
  await verifyDirectories(root, identities)
  const path = join(identities[identities.length - 1].path, name)
  const file = await open(
    path,
    constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | (constants.O_NOFOLLOW || 0),
    0o600,
  )
  try {
    const opened = await file.stat()
    if (!opened.isFile()) throw failure('image_tools_export_invalid')
    await file.writeFile(bytes)
    active(signal)
    await file.sync()
    await verifyDirectories(root, identities)
    const info = await lstat(path)
    if (
      !info.isFile() ||
      info.isSymbolicLink() ||
      !sameFile(opened, info) ||
      info.size !== bytes.byteLength ||
      (await realpath(path)) !== path
    )
      throw failure('image_tools_export_changed')
  } finally {
    await file.close()
  }
  return path
}

/**
 * 仅显式 Agent 导出可写入工作区；每次新建目录，返回路径属于用户请求的工具结果。
 * 项目原图与私有引用不出现在公开帧文件中，导出不修改工作流或工作台实体。
 * @param {{cwd:string,output:unknown,media:{read:(id:string)=>Promise<{media:Media,buffer:Uint8Array}>},plugin:{assertAllowed:(consumer:'agent')=>Promise<void>},signal?:AbortSignal}} input
 * @returns {Promise<{files:Array<{path:string,mimeType:string}>}>}
 */
export async function exportAgentImages({ cwd, output, media, plugin, signal }) {
  /** @type {DirectoryIdentity|null} */
  let ownDirectory = null
  let root = ''
  /** @type {DirectoryIdentity[]} */
  const identities = []
  try {
    active(signal)
    await plugin.assertAllowed('agent')
    active(signal)
    if (
      typeof cwd !== 'string' ||
      !cwd.trim() ||
      cwd.length > 4096 ||
      !isAbsolute(cwd) ||
      [...cwd].some((character) => character.charCodeAt(0) < 32)
    )
      throw failure('image_tools_export_invalid')
    let parsed
    try {
      parsed = parseImageOutput(output)
    } catch {
      throw failure('image_tools_export_invalid')
    }
    const atlas = parsed.atlas
    if (
      !atlas ||
      atlas.media.mimeType !== 'image/png' ||
      !atlas.frames.length ||
      atlas.frames.length !== parsed.frames.length ||
      !Number.isSafeInteger(atlas.width) ||
      !Number.isSafeInteger(atlas.height) ||
      atlas.frames.some(
        (frame) =>
          ![frame.x, frame.y, frame.width, frame.height].every(Number.isSafeInteger) ||
          frame.x + frame.width > atlas.width ||
          frame.y + frame.height > atlas.height,
      )
    )
      throw failure('image_tools_export_invalid')
    if (atlas.media.size > MAX_BYTES) throw failure('image_tools_export_too_large', 413)
    const stored = await media.read(atlas.media.id)
    if (
      !sameMedia(atlas.media, stored.media) ||
      !(stored.buffer instanceof Uint8Array) ||
      stored.buffer.byteLength !== atlas.media.size
    )
      throw failure('image_tools_export_invalid')
    if (stored.buffer.byteLength > MAX_BYTES) throw failure('image_tools_export_too_large', 413)
    const dimensions = readRasterDimensions(stored.buffer)
    if (
      !dimensions ||
      dimensions.mimeType !== 'image/png' ||
      dimensions.width !== atlas.width ||
      dimensions.height !== atlas.height
    )
      throw failure('image_tools_export_invalid')
    assertRasterBounds(dimensions)
    active(signal)
    await plugin.assertAllowed('agent')
    active(signal)
    // cwd 可为宿主提供的工作区别名；固定 canonical 根后，所有子路径逐层禁止链接。
    root = await realpath(resolve(cwd))
    identities.push(await inspectDirectory(root, root))
    await childDirectory(root, identities, 'generated')
    await childDirectory(root, identities, 'image-assets')
    await verifyDirectories(root, identities)
    const path = join(identities[identities.length - 1].path, randomUUID())
    await mkdir(path, { mode: 0o700 })
    ownDirectory = await inspectDirectory(root, path)
    identities.push(ownDirectory)
    const pngPath = await writeNewFile(root, identities, 'atlas.png', stored.buffer, signal)
    const metadata = new TextEncoder().encode(
      `${JSON.stringify({ image: 'atlas.png', width: atlas.width, height: atlas.height, frames: atlas.frames }, null, 2)}\n`,
    )
    const jsonPath = await writeNewFile(root, identities, 'frames.json', metadata, signal)
    await plugin.assertAllowed('agent')
    active(signal)
    await verifyDirectories(root, identities)
    return {
      files: [
        { path: pngPath, mimeType: 'image/png' },
        { path: jsonPath, mimeType: 'application/json' },
      ],
    }
  } catch (error) {
    let cleanupFailed = false
    if (ownDirectory) {
      let unchanged = false
      try {
        await verifyDirectories(root, identities)
        const current = await inspectDirectory(root, ownDirectory.path)
        unchanged = sameFile(ownDirectory.info, current.info)
      } catch {
        // 路径身份已变化时不能沿新路径删除，避免失败清理伤及其他目录。
      }
      if (unchanged) {
        try {
          await rm(ownDirectory.path, { recursive: true })
        } catch {
          cleanupFailed = true
        }
      }
    }
    if (cleanupFailed) throw failure('image_tools_export_cleanup_failed', 500)
    active(signal)
    if (error && typeof error === 'object' && 'code' in error && typeof error.code === 'string') {
      if (error.code === 'image_tools_agent_disabled') throw failure(error.code, 403)
      if (
        [
          'image_tools_export_invalid',
          'image_tools_export_changed',
          'image_tools_export_too_large',
        ].includes(error.code)
      )
        throw failure(error.code, error.code === 'image_tools_export_too_large' ? 413 : 400)
      if (error.code === 'workflow_image_invalid' || error.code === 'workflow_media_invalid')
        throw failure('image_tools_export_invalid')
    }
    // 系统和媒体错误可能包含完整路径或上游内容，不进入 Agent 可见消息。
    throw failure('image_tools_export_failed', 500)
  }
}
