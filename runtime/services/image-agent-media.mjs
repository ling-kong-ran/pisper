import { constants } from 'node:fs'
import { lstat, open, realpath } from 'node:fs/promises'
import { basename, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { assertRasterBounds, readRasterDimensions } from '../../shared/image/raster-image.mjs'

const MAX_BYTES = 8 * 1024 * 1024
/** @typedef {import('../../shared/workflow/workflow-inputs.mjs').WorkflowMedia} WorkflowMedia */
/** @typedef {{path:string, info:import('node:fs').Stats}} FileIdentity */

/** @param {string} code @param {number} [statusCode] */
function error(code, statusCode = 400) {
  return Object.assign(new Error(code), { code, statusCode })
}

/** @param {string} root @param {string} target */
function inside(root, target) {
  const path = relative(root, target)
  return path !== '' && path !== '..' && !path.startsWith(`..${sep}`) && !isAbsolute(path)
}

/** @param {import('node:fs').Stats} first @param {import('node:fs').Stats} second */
function sameFile(first, second) {
  return first.dev === second.dev && first.ino === second.ino && first.mode === second.mode
}

/**
 * 逐层拒绝工作区下的链接，避免合法末级文件通过父目录链接越界。
 * cwd 自身可为平台提供的真实工作区别名；入口先将其解析成唯一根目录。
 * @param {string} root @param {string} path @returns {Promise<FileIdentity[]>}
 */
async function inspectPath(root, path) {
  if (!inside(root, path)) throw error('image_tools_source_outside_workspace', 403)
  const rootInfo = await lstat(root)
  if (!rootInfo.isDirectory() || rootInfo.isSymbolicLink())
    throw error('image_tools_source_invalid')
  const identities = [{ path: root, info: rootInfo }]
  const components = relative(root, path).split(sep)
  let current = root
  for (let index = 0; index < components.length; index++) {
    current = join(current, components[index])
    const info = await lstat(current)
    if (
      info.isSymbolicLink() ||
      (index === components.length - 1 ? !info.isFile() : !info.isDirectory())
    )
      throw error('image_tools_source_invalid')
    identities.push({ path: current, info })
  }
  const canonical = await realpath(path)
  if (
    !inside(root, canonical) ||
    !sameFile(identities[identities.length - 1].info, await lstat(canonical))
  )
    throw error('image_tools_source_invalid')
  return identities
}

/**
 * Agent 专用导入边界；不读取任意 URL，不把文件路径暴露到媒体协议。
 * 一旦交给 media.upload 即按原子提交完成，避免取消后丢掉已保存资源的引用。
 * @param {{
 * cwd:string, sourceImage:string,
 * media:{upload:(input:{name:string,mimeType:string,buffer:Uint8Array})=>Promise<WorkflowMedia>},
 * plugin:{assertAllowed:(consumer:'agent')=>Promise<void>}, signal?:AbortSignal
 * }} request
 * @returns {Promise<WorkflowMedia>}
 */
export async function importAgentImage({ cwd, sourceImage, media, plugin, signal }) {
  signal?.throwIfAborted()
  await plugin.assertAllowed('agent')
  signal?.throwIfAborted()
  if (
    typeof cwd !== 'string' ||
    !cwd.trim() ||
    cwd.length > 4096 ||
    typeof sourceImage !== 'string' ||
    !sourceImage.trim() ||
    sourceImage.length > 4096 ||
    [...cwd, ...sourceImage].some((character) => character.charCodeAt(0) < 32) ||
    /^[a-z][a-z0-9+.-]*:\/\//i.test(sourceImage) ||
    /^(?:file|data|https?|ftp):/i.test(sourceImage)
  )
    throw error('image_tools_source_invalid')
  let bytes
  let name
  try {
    const root = await realpath(resolve(cwd))
    const path = resolve(root, sourceImage)
    const identities = await inspectPath(root, path)
    const sourceInfo = identities[identities.length - 1].info
    if (sourceInfo.size > MAX_BYTES) throw error('image_tools_source_too_large', 413)
    if (sourceInfo.size < 1) throw error('image_tools_source_invalid')
    signal?.throwIfAborted()
    // NONBLOCK 防止检查后文件被替换为 FIFO 时，open 阻塞住整个导入。
    const file = await open(
      path,
      constants.O_RDONLY | (constants.O_NOFOLLOW || 0) | (constants.O_NONBLOCK || 0),
    )
    try {
      const opened = await file.stat()
      if (
        !opened.isFile() ||
        !sameFile(sourceInfo, opened) ||
        opened.size !== sourceInfo.size ||
        opened.mtimeMs !== sourceInfo.mtimeMs
      )
        throw error('image_tools_source_changed')
      bytes = Buffer.alloc(opened.size)
      let offset = 0
      while (offset < bytes.length) {
        signal?.throwIfAborted()
        const { bytesRead } = await file.read(
          bytes,
          offset,
          Math.min(64 * 1024, bytes.length - offset),
          offset,
        )
        if (!bytesRead) throw error('image_tools_source_changed')
        offset += bytesRead
      }
      const finished = await file.stat()
      if (
        !sameFile(opened, finished) ||
        opened.size !== finished.size ||
        opened.mtimeMs !== finished.mtimeMs
      )
        throw error('image_tools_source_changed')
      const after = await inspectPath(root, path)
      if (
        after.length !== identities.length ||
        after.some((item, index) => !sameFile(item.info, identities[index].info))
      )
        throw error('image_tools_source_changed')
      name = basename(path).slice(0, 150)
    } finally {
      await file.close()
    }
  } catch (cause) {
    signal?.throwIfAborted()
    if (
      cause instanceof Error &&
      'code' in cause &&
      typeof cause.code === 'string' &&
      cause.code.startsWith('image_tools_source_')
    )
      throw cause
    // ENOENT / EACCES 等系统错误含完整路径，不能跨工具协议返回。
    throw error('image_tools_source_unavailable', 404)
  }
  const dimensions = readRasterDimensions(bytes)
  if (!dimensions || !['image/png', 'image/jpeg', 'image/webp'].includes(dimensions.mimeType))
    throw error('image_tools_source_invalid')
  try {
    assertRasterBounds(dimensions)
  } catch {
    throw error('image_tools_source_too_large', 413)
  }
  signal?.throwIfAborted()
  await plugin.assertAllowed('agent')
  signal?.throwIfAborted()
  try {
    return await media.upload({ name, mimeType: dimensions.mimeType, buffer: bytes })
  } catch (cause) {
    if (
      cause instanceof Error &&
      'code' in cause &&
      typeof cause.code === 'string' &&
      /^workflow_media_[a-z_]+$/.test(cause.code)
    )
      throw error(
        cause.code,
        'statusCode' in cause && typeof cause.statusCode === 'number' ? cause.statusCode : 400,
      )
    throw error('image_tools_import_failed', 500)
  }
}
