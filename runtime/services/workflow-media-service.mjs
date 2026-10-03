// 工作流素材按随机 ID 存入私有目录；协议只交换引用，文件路径仅在 Agent 调用边界生成。
import { createHash, randomUUID } from 'node:crypto'
import { constants } from 'node:fs'
import { lstat, mkdir, open, realpath, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { readRasterDimensions, assertRasterBounds } from '../../shared/image/raster-image.mjs'
import {
  parseWorkflowMedia,
  validateWorkflowInputDefinitions,
  WorkflowInputError,
} from '../../shared/workflow/workflow-inputs.mjs'
import { writeJsonAtomic } from '../storage/json-file.mjs'

/** @typedef {import('../../shared/workflow/workflow-inputs.mjs').WorkflowMedia} WorkflowMedia */
/** @typedef {{version: 1, media: WorkflowMedia, sha256: string}} StoredMedia */
/** @typedef {{kind:'image',name:string,mimeType:string,data:string,size:number}} MediaAttachment */
const MAX_BYTES = 64 * 1024 * 1024
/** @param {string} code @param {number} [statusCode] */
function error(code, statusCode = 400) {
  return Object.assign(new WorkflowInputError(code), { statusCode })
}
/** @param {unknown} value */
function safeId(value) {
  if (typeof value !== 'string' || !/^[a-zA-Z0-9][a-zA-Z0-9_-]{0,79}$/.test(value))
    throw error('workflow_media_invalid')
  return value
}
/** @param {Uint8Array} bytes */
function digest(bytes) {
  return createHash('sha256').update(bytes).digest('hex')
}
/** @param {unknown} value */
function isMissing(value) {
  return typeof value === 'object' && value !== null && 'code' in value && value.code === 'ENOENT'
}
/** @param {string} path @param {boolean} [create] */
async function directory(path, create = false) {
  if (create)
    await mkdir(path, { mode: 0o700 }).catch((value) => {
      if (!(
        typeof value === 'object' &&
        value !== null &&
        'code' in value &&
        value.code === 'EEXIST'
      ))
        throw value
    })
  const info = await lstat(path)
  if (!info.isDirectory() || info.isSymbolicLink() || (await realpath(path)) !== path)
    throw error('workflow_media_invalid')
  return path
}
/** @param {string} path @param {number} maximum */
async function readBytes(path, maximum) {
  const info = await lstat(path).catch((value) => {
    if (isMissing(value)) throw error('workflow_media_missing', 404)
    throw value
  })
  if (!info.isFile() || info.isSymbolicLink() || info.size > maximum)
    throw error('workflow_media_invalid')
  const file = await open(path, constants.O_RDONLY | (constants.O_NOFOLLOW || 0))
  try {
    const current = await file.stat()
    if (!current.isFile() || current.size < 1 || current.size > maximum)
      throw error('workflow_media_invalid')
    const buffer = Buffer.alloc(current.size)
    let offset = 0
    while (offset < buffer.length) {
      const { bytesRead } = await file.read(buffer, offset, buffer.length - offset, offset)
      if (!bytesRead) throw error('workflow_media_invalid')
      offset += bytesRead
    }
    return buffer
  } finally {
    await file.close()
  }
}

/** @param {Uint8Array} bytes @param {string} mimeType */
export function validateWorkflowMediaBytes(bytes, mimeType) {
  if (
    !bytes.byteLength ||
    bytes.byteLength > (mimeType.startsWith('image/') ? 8 : 64) * 1024 * 1024
  )
    throw error('workflow_media_too_large', 413)
  if (mimeType.startsWith('image/')) {
    const dimensions = readRasterDimensions(bytes)
    if (!dimensions || dimensions.mimeType !== mimeType) throw error('workflow_media_invalid')
    try {
      assertRasterBounds(dimensions)
    } catch {
      throw error('workflow_media_too_large', 413)
    }
  } else if (mimeType === 'video/mp4') {
    if (bytes.length < 16 || new TextDecoder().decode(bytes.subarray(4, 8)) !== 'ftyp')
      throw error('workflow_media_invalid')
  } else if (mimeType === 'video/webm') {
    if (
      bytes.length < 8 ||
      bytes[0] !== 0x1a ||
      bytes[1] !== 0x45 ||
      bytes[2] !== 0xdf ||
      bytes[3] !== 0xa3
    )
      throw error('workflow_media_invalid')
  } else throw error('workflow_media_invalid', 415)
}

/** @param {unknown} value @returns {StoredMedia} */
function storedMedia(value) {
  if (
    !value ||
    typeof value !== 'object' ||
    !('version' in value) ||
    value.version !== 1 ||
    !('media' in value) ||
    !('sha256' in value) ||
    typeof value.sha256 !== 'string' ||
    !/^[a-f0-9]{64}$/.test(value.sha256)
  )
    throw error('workflow_media_invalid')
  return { version: 1, media: parseWorkflowMedia(value.media), sha256: value.sha256 }
}

export class WorkflowMediaService {
  /** @param {{dataDir:string}} options */
  constructor({ dataDir }) {
    this.dataDir = dataDir
    this.root = ''
    /** @type {Promise<void>|null} */
    this.initializing = null
    this.queue = Promise.resolve()
    this.closed = false
    /** @type {WeakMap<object, string[]>} */
    this.importedMappings = new WeakMap()
  }

  init() {
    this.initializing ??= (async () => {
      await mkdir(this.dataDir, { recursive: true, mode: 0o700 })
      this.root = join(await realpath(this.dataDir), 'workflow-media')
      await directory(this.root, true)
    })()
    return this.initializing
  }

  /** @template T @param {()=>Promise<T>} operation @returns {Promise<T>} */
  async exclusive(operation) {
    await this.init()
    const pending = this.queue.then(() => {
      if (this.closed) throw error('workflow_media_closed', 503)
      return operation()
    })
    this.queue = pending.then(
      () => {},
      () => {},
    )
    return pending
  }

  /** @param {string} id */
  async mediaDirectory(id) {
    await directory(this.root)
    return directory(join(this.root, safeId(id))).catch((value) => {
      if (isMissing(value)) throw error('workflow_media_missing', 404)
      throw value
    })
  }

  /** @param {{name:string,mimeType:string,buffer:Uint8Array}} input */
  upload({ name, mimeType, buffer }) {
    validateWorkflowMediaBytes(buffer, mimeType)
    const id = randomUUID()
    const media = parseWorkflowMedia({
      id,
      name: name.split(/[\\/]/).pop()?.trim().slice(0, 160) || 'media',
      mimeType,
      size: buffer.byteLength,
    })
    return this.exclusive(() => this.write(media, buffer))
  }

  /** @param {WorkflowMedia} media @param {Uint8Array} buffer */
  async write(media, buffer) {
    await directory(this.root)
    const path = await directory(join(this.root, safeId(media.id)), true)
    try {
      await writeFile(join(path, 'data.bin'), buffer, { mode: 0o600, flag: 'wx' })
      await writeJsonAtomic(
        join(path, 'metadata.json'),
        { version: 1, media, sha256: digest(buffer) },
        { mode: 0o600 },
      )
      return media
    } catch (failure) {
      await rm(path, { recursive: true })
      throw failure
    }
  }

  /** @param {string} id */
  async load(id) {
    const path = await this.mediaDirectory(id)
    let metadata
    try {
      metadata = storedMedia(
        JSON.parse((await readBytes(join(path, 'metadata.json'), 4096)).toString('utf8')),
      )
    } catch (failure) {
      if (failure instanceof WorkflowInputError) throw failure
      throw error('workflow_media_invalid')
    }
    const buffer = await readBytes(join(path, 'data.bin'), MAX_BYTES)
    if (
      metadata.media.id !== id ||
      buffer.length !== metadata.media.size ||
      digest(buffer) !== metadata.sha256
    )
      throw error('workflow_media_invalid')
    validateWorkflowMediaBytes(buffer, metadata.media.mimeType)
    return { metadata, buffer, path: join(path, 'data.bin') }
  }

  /** @param {string} id */
  read(id) {
    return this.exclusive(async () => {
      const { metadata, buffer } = await this.load(id)
      return { media: metadata.media, buffer }
    })
  }

  /** @param {Record<string, unknown>} inputs */
  resolveInputs(inputs) {
    return this.exclusive(async () => {
      /** @type {MediaAttachment[]} */
      const attachments = []
      const context = []
      let imageBytes = 0
      for (const [name, value] of Object.entries(inputs)) {
        if (!value || typeof value !== 'object') continue
        const media = parseWorkflowMedia(value)
        const stored = await this.load(media.id)
        if (
          media.mimeType !== stored.metadata.media.mimeType ||
          media.name !== stored.metadata.media.name ||
          media.size !== stored.metadata.media.size
        )
          throw error('workflow_media_invalid')
        if (media.mimeType.startsWith('image/')) {
          imageBytes += media.size
          if (attachments.length >= 8 || imageBytes > 20 * 1024 * 1024)
            throw error('workflow_media_too_large', 413)
          attachments.push({
            kind: 'image',
            name: media.name,
            mimeType: media.mimeType,
            size: media.size,
            data: stored.buffer.toString('base64'),
          })
        }
        context.push(
          `${name}: ${media.name}\nLocal media path: ${JSON.stringify(stored.path)}${media.mimeType.startsWith('video/') ? '\nThis is a video file reference. Use available file/media tools; it is not a native video model attachment.' : ''}`,
        )
      }
      return { attachments, context: context.join('\n\n') }
    })
  }

  /** @param {unknown} workflow */
  exportFilesForWorkflow(workflow) {
    if (!workflow || typeof workflow !== 'object' || !('inputs' in workflow))
      throw error('workflow_media_invalid')
    const inputs = validateWorkflowInputDefinitions(workflow.inputs)
    return this.exclusive(async () => {
      /** @type {Record<string, Uint8Array>} */
      const files = {}
      /** @type {Map<string, WorkflowMedia>} */
      const included = new Map()
      let total = 0
      for (const input of inputs) {
        if (!input.defaultValue || typeof input.defaultValue !== 'object') continue
        const media = parseWorkflowMedia(input.defaultValue)
        const existing = included.get(media.id)
        if (existing) {
          if (JSON.stringify(media) !== JSON.stringify(existing))
            throw error('workflow_media_invalid')
          continue
        }
        total += media.size
        if (total > 128 * 1024 * 1024) throw error('workflow_media_too_large', 413)
        const { metadata, buffer } = await this.load(media.id)
        if (JSON.stringify(media) !== JSON.stringify(metadata.media))
          throw error('workflow_media_invalid')
        included.set(media.id, metadata.media)
        files[`media/${media.id}/metadata.json`] = new TextEncoder().encode(
          JSON.stringify(metadata),
        )
        files[`media/${media.id}/data.bin`] = buffer
      }
      return files
    })
  }

  /** @param {Record<string, Uint8Array>} files */
  validateBundleFiles(files) {
    const ids = new Set()
    for (const key of Object.keys(files)) {
      const parts = key.split('/')
      if (
        parts.length !== 3 ||
        parts[0] !== 'media' ||
        !['metadata.json', 'data.bin'].includes(parts[2])
      )
        throw error('workflow_media_invalid')
      ids.add(safeId(parts[1]))
    }
    /** @type {Array<{metadata:StoredMedia,buffer:Uint8Array}>} */
    const parsed = []
    for (const id of ids) {
      const json = files[`media/${id}/metadata.json`]
      const buffer = files[`media/${id}/data.bin`]
      if (
        !(json instanceof Uint8Array) ||
        json.byteLength > 4096 ||
        !(buffer instanceof Uint8Array)
      )
        throw error('workflow_media_missing')
      let metadata
      try {
        metadata = storedMedia(JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(json)))
      } catch {
        throw error('workflow_media_invalid')
      }
      if (
        metadata.media.id !== id ||
        metadata.media.size !== buffer.byteLength ||
        digest(buffer) !== metadata.sha256
      )
        throw error('workflow_media_invalid')
      validateWorkflowMediaBytes(buffer, metadata.media.mimeType)
      parsed.push({ metadata, buffer })
    }
    return parsed
  }

  /** @param {Record<string, Uint8Array>} files */
  importBundleFiles(files) {
    const parsed = this.validateBundleFiles(files)
    return this.exclusive(async () => {
      /** @type {Record<string,WorkflowMedia>} */
      const mapping = {}
      try {
        for (const { metadata, buffer } of parsed) {
          const media = { ...metadata.media, id: randomUUID() }
          mapping[metadata.media.id] = await this.write(media, buffer)
        }
        this.importedMappings.set(
          mapping,
          Object.values(mapping).map((media) => media.id),
        )
        return mapping
      } catch (failure) {
        for (const media of Object.values(mapping))
          await rm(await this.mediaDirectory(media.id), { recursive: true })
        throw failure
      }
    })
  }

  /** @param {Record<string, WorkflowMedia>} mapping */
  discardImported(mapping) {
    return this.exclusive(async () => {
      const ids = this.importedMappings.get(mapping)
      if (!ids) throw error('workflow_media_invalid')
      for (const id of ids) await rm(await this.mediaDirectory(id), { recursive: true })
      this.importedMappings.delete(mapping)
    })
  }

  async dispose() {
    this.closed = true
    await this.initializing
    await this.queue
  }
}
