import { createHash, randomUUID } from 'node:crypto'
import { constants } from 'node:fs'
import { lstat, mkdir, open, readdir, rename, rm } from 'node:fs/promises'
import { dirname, join, parse, relative, resolve, sep } from 'node:path'
import { isIP } from 'node:net'
import { Readable, Transform } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import tar from 'tar-stream'
import tarHeaders from 'tar-stream/headers.js'
import unbzip2 from 'unbzip2-stream'

const MARKER = '.installation.json'
const MAX_MARKER_BYTES = 1024 * 1024
const MAX_FILE_DURATION_MS = 30 * 60_000
const MAX_ARCHIVE_ENTRIES = 4096
const MAX_ARCHIVE_EXTRA_BYTES = 32 * 1024 * 1024
const MAX_ARCHIVE_METADATA_BYTES = 64 * 1024
let downloadQueue = Promise.resolve()

class DownloadError extends Error {
  constructor(code, message, statusCode = 400) {
    super(message)
    this.code = code
    this.statusCode = statusCode
  }
}

function failure(code) {
  const messages = {
    catalog: 'Invalid speech model catalog.',
    unknown: 'Unknown speech model.',
    path: 'Unsafe speech model storage path.',
    integrity: 'Speech model integrity verification failed.',
    size: 'Speech model download size does not match the manifest.',
    range: 'Invalid speech model partial response.',
    http: 'Speech model CDN request failed.',
    redirect: 'Speech model CDN redirect is not allowed.',
    timeout: 'Speech model download timed out.',
    cancelled: 'Speech model download cancelled.',
    disposed: 'Speech model download service is disposed.',
    storage: 'Speech model installation failed.',
    missing: 'Speech model is not installed or failed verification.',
  }
  return new DownloadError(code, messages[code], code === 'unknown' ? 404 : 400)
}

function safeError(error) {
  return error instanceof DownloadError ? error : failure('storage')
}

function safeSegment(value) {
  return (
    typeof value === 'string' &&
    /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/.test(value) &&
    !/[. ]$/.test(value) &&
    !/^(con|prn|aux|nul|com[0-9]|lpt[0-9])(?:\.|$)/i.test(value)
  )
}

function safeFileSegment(value) {
  return (
    typeof value === 'string' &&
    value.length > 0 &&
    value.length <= 255 &&
    value.isWellFormed() &&
    value !== '.' &&
    value !== '..' &&
    !/[\\/<>:"|?*\p{Cc}\p{Cf}]/u.test(value) &&
    !/[. ]$/.test(value) &&
    !/^(con|prn|aux|nul|conin\$|conout\$|clock\$|com[0-9\u00b9\u00b2\u00b3]|lpt[0-9\u00b9\u00b2\u00b3])(?:\.|$)/i.test(
      value,
    )
  )
}

function publicCdnHost(host) {
  return (
    typeof host === 'string' &&
    host.length <= 253 &&
    host.includes('.') &&
    !isIP(host.replace(/^\[|\]$/g, '')) &&
    host.split('.').every((part) => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i.test(part)) &&
    !/(?:^|\.)(localhost|local|internal|lan|home|arpa)$/i.test(host)
  )
}

function validCdnUrl(url) {
  return (
    url.protocol === 'https:' &&
    !url.port &&
    !url.username &&
    !url.password &&
    !url.hash &&
    publicCdnHost(url.hostname)
  )
}

function catalogUrls(values) {
  if (!Array.isArray(values) || !values.length || values.length > 2) throw failure('catalog')
  return values.map((value) => {
    try {
      if (typeof value !== 'string') throw failure('catalog')
      const url = new URL(value)
      if (!validCdnUrl(url)) throw failure('catalog')
      return url.href
    } catch {
      throw failure('catalog')
    }
  })
}

function validateTtsConfig(model, files) {
  if (model.kind !== 'tts') return
  const config = model.config
  const keys = new Set([
    'model',
    'tokens',
    'lexicon',
    'dictDir',
    'numThreads',
    'maxTextCodePoints',
    'noiseScale',
    'noiseScaleW',
    'lengthScale',
    'ruleFsts',
  ])
  if (model.engine !== 'vits' || !config || Object.keys(config).some((key) => !keys.has(key)))
    throw failure('catalog')
  const paths = new Set(files.map((file) => file.path))
  for (const key of ['model', 'tokens', 'lexicon', 'dictDir']) {
    const value = config[key]
    if (
      typeof value !== 'string' ||
      value.length > 512 ||
      value.includes(',') ||
      !value.split('/').every(safeFileSegment) ||
      (key === 'dictDir'
        ? paths.has(value) || !files.some((file) => file.path.startsWith(`${value}/`))
        : !paths.has(value))
    )
      throw failure('catalog')
  }
  if (
    config.ruleFsts !== undefined &&
    (!Array.isArray(config.ruleFsts) ||
      !config.ruleFsts.length ||
      config.ruleFsts.length > 16 ||
      config.ruleFsts.some(
        (path) => typeof path !== 'string' || path.includes(',') || !paths.has(path),
      ))
  )
    throw failure('catalog')
  for (const key of ['noiseScale', 'noiseScaleW', 'lengthScale']) {
    if (config[key] !== undefined && (!Number.isFinite(config[key]) || config[key] <= 0))
      throw failure('catalog')
  }
  if (
    (config.numThreads !== undefined &&
      (!Number.isInteger(config.numThreads) || config.numThreads < 1 || config.numThreads > 16)) ||
    (config.maxTextCodePoints !== undefined &&
      (!Number.isInteger(config.maxTextCodePoints) ||
        config.maxTextCodePoints < 1 ||
        config.maxTextCodePoints > 400))
  )
    throw failure('catalog')
}

function validateCatalog(catalog) {
  if (!Array.isArray(catalog)) throw failure('catalog')
  const models = new Map()
  const ids = new Set()
  for (const model of catalog) {
    if (
      !model ||
      !safeSegment(model.id) ||
      ids.has(model.id.toLowerCase()) ||
      !['asr', 'tts', 'vad'].includes(model.kind) ||
      typeof model.name !== 'string' ||
      !model.name.trim() ||
      !model.name.isWellFormed() ||
      !Array.isArray(model.files) ||
      !model.files.length ||
      model.files.length > 1024
    ) {
      throw failure('catalog')
    }
    let archive
    if (model.archive !== undefined) {
      const value = model.archive
      if (
        !value ||
        value.format !== 'tar.bz2' ||
        !Number.isSafeInteger(value.bytes) ||
        value.bytes < 1 ||
        !/^[a-fA-F0-9]{64}$/.test(value.sha256) ||
        typeof value.stripPrefix !== 'string' ||
        value.stripPrefix.length > 512 ||
        !value.stripPrefix.endsWith('/') ||
        !value.stripPrefix.slice(0, -1).split('/').every(safeFileSegment) ||
        value.stripPrefix.split('/').length > 17
      )
        throw failure('catalog')
      archive = {
        format: value.format,
        bytes: value.bytes,
        sha256: value.sha256.toLowerCase(),
        stripPrefix: value.stripPrefix,
        urls: catalogUrls(value.urls),
      }
    }
    const paths = new Set()
    const files = model.files.map((file) => {
      if (
        !file ||
        typeof file.path !== 'string' ||
        file.path.length > 512 ||
        !file.path.split('/').every(safeFileSegment) ||
        file.path.toLowerCase() === MARKER ||
        file.path.toLowerCase().startsWith(`${MARKER}/`) ||
        file.path.split('/').length > 16 ||
        !Number.isSafeInteger(file.bytes) ||
        file.bytes < 0 ||
        !/^[a-fA-F0-9]{64}$/.test(file.sha256)
      ) {
        throw failure('catalog')
      }
      const key = file.path.toLowerCase()
      for (const other of paths) {
        if (key === other || key.startsWith(`${other}/`) || other.startsWith(`${key}/`)) {
          throw failure('catalog')
        }
      }
      paths.add(key)
      const urls = archive && file.urls === undefined ? [] : catalogUrls(file.urls)
      return { path: file.path, bytes: file.bytes, sha256: file.sha256.toLowerCase(), urls }
    })
    validateTtsConfig(model, files)
    const filesBytes = files.reduce((total, file) => total + file.bytes, 0)
    if (!Number.isSafeInteger(filesBytes + MAX_ARCHIVE_EXTRA_BYTES)) throw failure('catalog')
    const totalBytes = archive ? archive.bytes : filesBytes
    const manifest = {
      id: model.id,
      kind: model.kind,
      files: files.map(({ path, bytes, sha256 }) => ({ path, bytes, sha256 })),
      ...(archive
        ? {
            archive: {
              format: archive.format,
              bytes: archive.bytes,
              sha256: archive.sha256,
              stripPrefix: archive.stripPrefix,
            },
          }
        : {}),
    }
    const fingerprint = createHash('sha256').update(JSON.stringify(manifest)).digest('hex')
    models.set(model.id, {
      ...manifest,
      name: model.name,
      files,
      ...(archive ? { archive } : {}),
      totalBytes,
      filesBytes,
      fingerprint,
    })
    ids.add(model.id.toLowerCase())
  }
  return models
}

// tar-stream 会内部缓冲扩展头；在交给解析器前限制物理条目和元数据，避免预算绕过。
function archiveBudget(model, signal) {
  const headerBuffer = Buffer.alloc(512)
  let headerBytes = 0
  let remaining = 0
  let expanded = 0
  let entries = 0
  let metadata
  let metadataBytes = 0
  let metadataType
  const limit = model.filesBytes + MAX_ARCHIVE_EXTRA_BYTES
  const entryLimit = Math.max(MAX_ARCHIVE_EXTRA_BYTES, ...model.files.map((file) => file.bytes))
  return new Transform({
    transform(chunk, _encoding, callback) {
      try {
        checkAbort(signal)
        expanded += chunk.length
        if (expanded > limit) throw failure('size')
        let offset = 0
        while (offset < chunk.length) {
          if (remaining) {
            const length = Math.min(remaining, chunk.length - offset)
            if (metadata && metadataBytes < metadata.length) {
              const copied = Math.min(length, metadata.length - metadataBytes)
              chunk.copy(metadata, metadataBytes, offset, offset + copied)
              metadataBytes += copied
              if (metadataBytes === metadata.length) validateArchiveMetadata(metadata, metadataType)
            }
            remaining -= length
            offset += length
          } else {
            const length = Math.min(512 - headerBytes, chunk.length - offset)
            chunk.copy(headerBuffer, headerBytes, offset, offset + length)
            headerBytes += length
            offset += length
            if (headerBytes !== 512) continue
            headerBytes = 0
            const header = tarHeaders.decode(headerBuffer, 'utf8', false)
            if (!header) continue
            entries += 1
            if (entries > MAX_ARCHIVE_ENTRIES) throw failure('size')
            if (!Number.isSafeInteger(header.size) || header.size < 0 || header.size > entryLimit)
              throw failure('size')
            const extended = ['pax-header', 'pax-global-header', 'gnu-long-path'].includes(
              header.type,
            )
            if (!extended && !['file', 'directory'].includes(header.type)) throw failure('path')
            if (header.type === 'directory' && header.size !== 0) throw failure('size')
            if (extended && (header.size < 1 || header.size > MAX_ARCHIVE_METADATA_BYTES))
              throw failure('size')
            metadata = extended ? Buffer.alloc(header.size) : null
            metadataBytes = 0
            metadataType = header.type
            remaining = Math.ceil(header.size / 512) * 512
          }
        }
        callback(null, chunk)
      } catch (error) {
        callback(error instanceof DownloadError ? error : failure('integrity'))
      }
    },
    flush(callback) {
      callback(headerBytes || remaining ? failure('size') : null)
    },
  })
}

function validateArchiveMetadata(buffer, type) {
  if (type === 'gnu-long-path') return
  // 先验证 PAX 记录长度，防止宽松解析器对负长度等输入无法前进；不接受改变数据边界的 size。
  let offset = 0
  while (offset < buffer.length) {
    const space = buffer.indexOf(32, offset)
    if (space < offset || space - offset > 8) throw failure('integrity')
    const digits = buffer.toString('ascii', offset, space)
    if (!/^[1-9][0-9]*$/.test(digits)) throw failure('integrity')
    const length = Number(digits)
    const end = offset + length
    if (end > buffer.length || end <= space + 2 || buffer[end - 1] !== 10)
      throw failure('integrity')
    const equals = buffer.indexOf(61, space + 1)
    if (equals < space + 2 || equals >= end - 1) throw failure('integrity')
    const key = buffer.toString('utf8', space + 1, equals)
    if (key === 'size' || key === 'linkpath' || (type === 'pax-global-header' && key === 'path'))
      throw failure('path')
    offset = end
  }
}

function archiveEntryPath(header, prefix) {
  if (!['file', 'directory'].includes(header.type) || header.linkname) throw failure('path')
  const name =
    header.type === 'directory' && header.name.endsWith('/')
      ? header.name.slice(0, -1)
      : header.name
  if (
    typeof name !== 'string' ||
    name.length > 1024 ||
    !name.split('/').every(safeFileSegment) ||
    name.split('/').length > 32
  )
    throw failure('path')
  if (header.type === 'directory' && name === prefix.slice(0, -1)) return ''
  if (!name.startsWith(prefix)) throw failure('path')
  const path = name.slice(prefix.length)
  if (!path || path.toLowerCase() === MARKER || path.toLowerCase().startsWith(`${MARKER}/`))
    throw failure('path')
  return path
}

async function statOrNull(path, options) {
  try {
    return await lstat(path, options)
  } catch (error) {
    if (error.code === 'ENOENT') return null
    throw error
  }
}

// 逐层检查而非仅检查最终 realpath，避免已有链接、Windows junction 和非目录祖先。
async function checkedDirectory(path, create = false, identities) {
  const absolute = resolve(path)
  const root = parse(absolute).root
  let current = root
  for (const part of absolute.slice(root.length).split(sep).filter(Boolean)) {
    current = join(current, part)
    let stat = await statOrNull(current, identities ? { bigint: true } : undefined)
    if (!stat && create) {
      await mkdir(current).catch((error) => {
        if (error.code !== 'EEXIST') throw error
      })
      stat = await statOrNull(current)
    }
    if (!stat) return false
    if (stat.isSymbolicLink() || !stat.isDirectory()) throw failure('path')
    if (identities) {
      // 共享祖先会因其他会话创建文件而变化；只核对身份，模型目录仍核对完整属性。
      const identity =
        current === absolute
          ? statIdentity(stat)
          : [stat.dev, stat.ino, stat.mode, stat.uid, stat.gid, stat.birthtimeNs].join(':')
      identities.push([current, identity])
    }
  }
  return true
}

async function checkedFile(path, flags = constants.O_RDONLY) {
  if (!(await checkedDirectory(dirname(path)))) throw failure('path')
  const before = await statOrNull(path)
  if (before && (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1)) {
    throw failure('path')
  }
  const handle = await open(path, flags | (constants.O_NOFOLLOW || 0), 0o600)
  try {
    const after = await handle.stat()
    if (
      !after.isFile() ||
      after.nlink !== 1 ||
      (before && (before.ino !== after.ino || before.dev !== after.dev))
    ) {
      throw failure('path')
    }
    return handle
  } catch (error) {
    await handle.close()
    throw error
  }
}

function checkAbort(signal) {
  if (signal?.aborted)
    throw signal.reason instanceof DownloadError ? signal.reason : failure('cancelled')
}

async function hashFile(path, expectedBytes, signal) {
  const handle = await checkedFile(path)
  try {
    const stat = await handle.stat()
    if (stat.size !== expectedBytes) throw failure('size')
    const hash = createHash('sha256')
    const buffer = Buffer.alloc(128 * 1024)
    let position = 0
    while (position < expectedBytes) {
      checkAbort(signal)
      const { bytesRead } = await handle.read(
        buffer,
        0,
        Math.min(buffer.length, expectedBytes - position),
        position,
      )
      if (!bytesRead) throw failure('size')
      hash.update(buffer.subarray(0, bytesRead))
      position += bytesRead
    }
    if ((await handle.stat()).size !== expectedBytes) throw failure('size')
    return hash
  } finally {
    await handle.close()
  }
}

function statIdentity(stat) {
  return [
    stat.dev,
    stat.ino,
    stat.mode,
    stat.nlink,
    stat.uid,
    stat.gid,
    stat.size,
    stat.mtimeNs,
    stat.ctimeNs,
    stat.birthtimeNs,
  ].join(':')
}

async function checkTree(directory, model, markerAllowed = true, prefix = '', identities) {
  const allowed = new Set(model.files.map((file) => file.path))
  const entries = await readdir(directory, { withFileTypes: true })
  entries.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
  for (const entry of entries) {
    const path = prefix ? `${prefix}/${entry.name}` : entry.name
    const absolute = join(directory, entry.name)
    const stat = await lstat(absolute, identities ? { bigint: true } : undefined)
    if (stat.isSymbolicLink()) throw failure('path')
    if (identities) identities.push([absolute, statIdentity(stat)])
    if (stat.isDirectory() && [...allowed].some((file) => file.startsWith(`${path}/`))) {
      await checkTree(absolute, model, markerAllowed, path, identities)
    } else if (
      !stat.isFile() ||
      stat.nlink !== (identities ? 1n : 1) ||
      !(allowed.has(path) || (markerAllowed && path === MARKER))
    ) {
      throw failure('path')
    } else if (identities) {
      // 缓存命中也要打开文件核对身份，保留链接及路径替换防护。
      const handle = await checkedFile(absolute)
      try {
        if (statIdentity(await handle.stat({ bigint: true })) !== statIdentity(stat)) {
          throw failure('path')
        }
      } finally {
        await handle.close()
      }
    }
  }
}

async function installationIdentity(directory, model) {
  const identities = []
  if (!(await checkedDirectory(directory, false, identities))) return null
  await checkTree(directory, model, true, '', identities)
  return JSON.stringify(identities)
}

async function checkLinks(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    const stat = await lstat(path)
    if (stat.isSymbolicLink() || (!stat.isDirectory() && (!stat.isFile() || stat.nlink !== 1))) {
      throw failure('path')
    }
    if (stat.isDirectory()) await checkLinks(path)
  }
}

// 即使注入的 fetch 不响应 AbortSignal，也保证自有任务按时结束并处理迟到拒绝。
async function interruptible(promise, signal, timeoutMs, onTimeout) {
  promise = Promise.resolve(promise)
  promise.catch(() => {})
  checkAbort(signal)
  let timer
  let listener
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        listener = () => reject(signal.reason || failure('cancelled'))
        signal.addEventListener('abort', listener, { once: true })
        if (timeoutMs) {
          timer = setTimeout(() => {
            onTimeout?.()
            reject(failure('timeout'))
          }, timeoutMs)
        }
      }),
    ])
  } finally {
    clearTimeout(timer)
    signal.removeEventListener('abort', listener)
  }
}

export class SpeechModelDownloadService {
  #installationProofs = new Map()
  #proofGenerations = new Map()

  invalidateInstallation(id) {
    this.#installationProofs.delete(id)
    this.#proofGenerations.set(id, (this.#proofGenerations.get(id) || 0) + 1)
  }

  constructor({
    dataDir,
    catalog,
    fetchImpl = globalThis.fetch,
    requestTimeoutMs = 30_000,
    trustedRedirectHosts = [],
  }) {
    if (typeof dataDir !== 'string' || !dataDir || typeof fetchImpl !== 'function') {
      throw failure('catalog')
    }
    if (
      !Number.isSafeInteger(requestTimeoutMs) ||
      requestTimeoutMs < 1 ||
      requestTimeoutMs > 300_000
    ) {
      throw failure('catalog')
    }
    if (!Array.isArray(trustedRedirectHosts) || !trustedRedirectHosts.every(publicCdnHost)) {
      throw failure('catalog')
    }
    this.trustedRedirectHosts = new Set(trustedRedirectHosts.map((host) => host.toLowerCase()))
    this.root = join(resolve(dataDir), 'speech-models')
    this.catalog = validateCatalog(catalog)
    this.fetchImpl = fetchImpl
    this.requestTimeoutMs = requestTimeoutMs
    this.states = new Map()
    this.inflight = new Map()
    this.disposed = false
  }

  getModel(id) {
    const model = this.catalog.get(id)
    if (!model) throw failure('unknown')
    return model
  }

  directory(model) {
    const path = join(this.root, model.id)
    if (relative(this.root, path) !== model.id) throw failure('path')
    return path
  }

  snapshot(model) {
    return {
      id: model.id,
      status: 'not-installed',
      downloadedBytes: 0,
      totalBytes: model.totalBytes,
      ...(model.archive ? { filesBytes: model.filesBytes } : {}),
      ...this.states.get(model.id),
    }
  }

  async list() {
    return Promise.all(
      [...this.catalog.values()].map(async (model) => ({
        id: model.id,
        kind: model.kind,
        name: model.name,
        files: model.files.map(({ path, bytes, sha256 }) => ({ path, bytes, sha256 })),
        ...(await this.status(model.id)),
      })),
    )
  }

  async verifyInstallation(model, signal) {
    const generation = this.#proofGenerations.get(model.id) || 0
    const directory = this.directory(model)
    const catalogIdentity = JSON.stringify({
      id: model.id,
      kind: model.kind,
      fingerprint: model.fingerprint,
      files: model.files.map(({ path, bytes, sha256 }) => ({ path, bytes, sha256 })),
    })
    try {
      checkAbort(signal)
      const before = await installationIdentity(directory, model)
      if (!before) {
        this.invalidateInstallation(model.id)
        return false
      }
      const cached = this.#installationProofs.get(model.id)
      if (
        !this.disposed &&
        cached?.catalogIdentity === catalogIdentity &&
        cached.identity === before
      ) {
        checkAbort(signal)
        return true
      }
      this.#installationProofs.delete(model.id)
      if (!(await this.verifyInstallationContents(model, signal))) {
        this.invalidateInstallation(model.id)
        return false
      }
      // 只有整次读取前后身份和纳秒级属性均不变，才留下可复用的校验证明。
      const after = await installationIdentity(directory, model)
      checkAbort(signal)
      if (after !== before) {
        this.invalidateInstallation(model.id)
        return false
      }
      if (!this.disposed && generation === (this.#proofGenerations.get(model.id) || 0)) {
        this.#installationProofs.set(model.id, { catalogIdentity, identity: after })
      }
      return true
    } catch (error) {
      this.invalidateInstallation(model.id)
      throw error
    }
  }

  async verifyInstallationContents(model, signal) {
    const directory = this.directory(model)
    if (!(await checkedDirectory(directory))) return false
    const markerPath = join(directory, MARKER)
    const markerStat = await statOrNull(markerPath)
    if (!markerStat || markerStat.size > MAX_MARKER_BYTES) return false
    const handle = await checkedFile(markerPath)
    let marker
    try {
      const buffer = Buffer.alloc(MAX_MARKER_BYTES + 1)
      const { bytesRead } = await handle.read(buffer, 0, buffer.length, 0)
      if (bytesRead > MAX_MARKER_BYTES) return false
      marker = JSON.parse(buffer.subarray(0, bytesRead).toString('utf8'))
    } catch {
      return false
    } finally {
      await handle.close()
    }
    if (marker.version !== 1 || marker.id !== model.id || marker.fingerprint !== model.fingerprint)
      return false
    if (
      JSON.stringify(marker.files) !==
      JSON.stringify(model.files.map(({ path, bytes, sha256 }) => ({ path, bytes, sha256 })))
    )
      return false
    await checkTree(directory, model)
    for (const file of model.files) {
      checkAbort(signal)
      const hash = await hashFile(join(directory, file.path), file.bytes, signal)
      if (hash.digest('hex') !== file.sha256) return false
    }
    return true
  }

  async status(id) {
    const model = this.getModel(id)
    if (this.inflight.has(id)) return this.snapshot(model)
    let installed = false
    try {
      installed = await this.verifyInstallation(model)
    } catch {
      installed = false
    }
    if (this.inflight.has(id)) return this.snapshot(model)
    if (installed) {
      this.states.set(id, { status: 'installed', downloadedBytes: model.totalBytes })
    } else if (this.states.get(id)?.status === 'installed') {
      this.states.delete(id)
    }
    return this.snapshot(model)
  }

  async modelDirectory(id) {
    const model = this.getModel(id)
    try {
      if (await this.verifyInstallation(model)) return this.directory(model)
    } catch {
      // 不让磁盘路径、底层异常或不可信 marker 进入公共错误。
    }
    throw failure('missing')
  }

  ensureDownload(id) {
    const model = this.getModel(id)
    if (this.disposed) throw failure('disposed')
    const existing = this.inflight.get(id)
    if (existing) return existing
    this.invalidateInstallation(id)
    const controller = new AbortController()
    const task = { controller, promise: null }
    this.states.set(id, { status: 'downloading', downloadedBytes: 0 })
    const previous = downloadQueue
    // 只中断排队等待；开始落盘后必须等 finally 关闭句柄，取消才算真正完成。
    const turn = (async () => {
      await interruptible(previous, controller.signal, 0)
      checkAbort(controller.signal)
      return this.runDownload(model, controller.signal)
    })()
    task.promise = turn
      .catch((error) => {
        this.invalidateInstallation(id)
        const safe = safeError(controller.signal.aborted ? controller.signal.reason : error)
        this.states.set(id, {
          ...this.states.get(id),
          status: safe.code === 'cancelled' ? 'cancelled' : 'error',
          ...(safe.code === 'cancelled' ? {} : { error: safe.message }),
        })
        throw safe
      })
      .finally(() => {
        if (this.inflight.get(id) === task) this.inflight.delete(id)
      })
    // startDownload 的调用方不持有完成 Promise，必须在这里安装拒绝处理器。
    task.promise.catch(() => {})
    downloadQueue = Promise.all([previous, task.promise.catch(() => {})]).then(() => {})
    this.inflight.set(id, task)
    return task
  }

  async startDownload(id) {
    this.ensureDownload(id)
    return this.snapshot(this.getModel(id))
  }

  async download(id) {
    return this.ensureDownload(id).promise
  }

  async cancelDownload(id) {
    this.getModel(id)
    const task = this.inflight.get(id)
    if (task) {
      task.controller.abort(failure('cancelled'))
      await task.promise.catch(() => {})
    }
    return this.status(id)
  }

  async dispose() {
    this.disposed = true
    this.#installationProofs.clear()
    this.#proofGenerations.clear()
    const tasks = [...this.inflight.values()]
    for (const task of tasks) task.controller.abort(failure('cancelled'))
    await Promise.all(tasks.map((task) => task.promise.catch(() => {})))
  }

  async runDownload(model, signal) {
    checkAbort(signal)
    await checkedDirectory(this.root, true)
    try {
      if (await this.verifyInstallation(model, signal)) {
        this.states.set(model.id, { status: 'installed', downloadedBytes: model.totalBytes })
        return this.snapshot(model)
      }
    } catch (error) {
      if (error instanceof DownloadError && error.code === 'path') throw error
      checkAbort(signal)
    }
    const staging = join(this.root, `.${model.id}.${model.fingerprint}.partial`)
    await checkedDirectory(staging, true)
    await checkTree(staging, model)
    let completedBytes = 0
    const archivePath = model.archive
      ? join(this.root, `.${model.id}.${model.fingerprint}.tar.bz2`)
      : null
    if (model.archive) {
      await this.downloadFile(model, model.archive, archivePath, 0, signal)
      completedBytes = model.archive.bytes
      this.states.set(model.id, { status: 'verifying', downloadedBytes: completedBytes })
      // 仅清理已经通过白名单及链接检查的自有残缺输出；压缩缓存保留用于取消重试。
      await checkTree(staging, model)
      await rm(staging, { recursive: true })
      await checkedDirectory(staging, true)
      await this.extractArchive(model, archivePath, staging, signal)
    } else {
      for (const file of model.files) {
        checkAbort(signal)
        await checkedDirectory(dirname(join(staging, file.path)), true)
        await this.downloadFile(model, file, join(staging, file.path), completedBytes, signal)
        completedBytes += file.bytes
      }
    }
    this.states.set(model.id, { status: 'verifying', downloadedBytes: completedBytes })
    for (const file of model.files) {
      const hash = await hashFile(join(staging, file.path), file.bytes, signal)
      if (hash.digest('hex') !== file.sha256) throw failure('integrity')
    }
    await checkTree(staging, model)
    checkAbort(signal)
    const marker = await checkedFile(join(staging, MARKER), constants.O_WRONLY | constants.O_CREAT)
    try {
      await marker.truncate(0)
      await marker.writeFile(
        JSON.stringify({
          version: 1,
          id: model.id,
          fingerprint: model.fingerprint,
          files: model.files.map(({ path, bytes, sha256 }) => ({ path, bytes, sha256 })),
        }),
      )
      await marker.sync()
    } finally {
      await marker.close()
    }
    checkAbort(signal)
    await this.publish(model, staging)
    if (archivePath) {
      // 发布后清理失败不否定已安装状态，也不递归删除可疑缓存路径。
      const handle = await checkedFile(archivePath)
      await handle.close()
      await rm(archivePath).catch(() => {})
    }
    this.states.set(model.id, { status: 'installed', downloadedBytes: model.totalBytes })
    return this.snapshot(model)
  }

  async extractArchive(model, archivePath, staging, signal) {
    const handle = await checkedFile(archivePath)
    const controller = new AbortController()
    const abort = () => controller.abort(signal.reason)
    signal.addEventListener('abort', abort, { once: true })
    const timer = setTimeout(() => controller.abort(failure('timeout')), MAX_FILE_DURATION_MS)
    const extract = tar.extract()
    const seen = new Map()
    const found = new Set()
    let tasks = []
    try {
      checkAbort(signal)
      const stat = await handle.stat()
      if (stat.size !== model.archive.bytes) throw failure('size')
      const input = async function* () {
        const hash = createHash('sha256')
        let position = 0
        while (position < model.archive.bytes) {
          checkAbort(controller.signal)
          const buffer = Buffer.alloc(Math.min(64 * 1024, model.archive.bytes - position))
          const { bytesRead } = await handle.read(buffer, 0, buffer.length, position)
          if (!bytesRead) throw failure('size')
          position += bytesRead
          hash.update(buffer.subarray(0, bytesRead))
          yield buffer.subarray(0, bytesRead)
        }
        if (hash.digest('hex') !== model.archive.sha256) throw failure('integrity')
      }
      const consume = async () => {
        for await (const stream of extract) {
          checkAbort(controller.signal)
          const header = stream.header
          const path = archiveEntryPath(header, model.archive.stripPrefix)
          const key = path.toLowerCase()
          if (seen.has(key)) throw failure('path')
          for (const [other, type] of seen) {
            if (
              (type === 'file' && key.startsWith(`${other}/`)) ||
              (header.type === 'file' && other.startsWith(`${key}/`))
            )
              throw failure('path')
          }
          seen.set(key, header.type)
          if (!Number.isSafeInteger(header.size) || header.size < 0) throw failure('size')
          const file = model.files.find((entry) => entry.path === path)
          if (file && (header.type !== 'file' || header.size !== file.bytes)) throw failure('size')
          let output
          let bytes = 0
          const hash = createHash('sha256')
          try {
            if (file) {
              await checkedDirectory(dirname(join(staging, path)), true)
              output = await checkedFile(
                join(staging, path),
                constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL,
              )
            }
            for await (const chunk of stream) {
              checkAbort(controller.signal)
              bytes += chunk.length
              if (bytes > header.size) throw failure('size')
              if (!output) continue
              hash.update(chunk)
              let offset = 0
              while (offset < chunk.length) {
                checkAbort(controller.signal)
                const { bytesWritten } = await output.write(chunk, offset, chunk.length - offset)
                if (!bytesWritten) throw failure('storage')
                offset += bytesWritten
              }
            }
            if (bytes !== header.size) throw failure('size')
            if (file) {
              if (hash.digest('hex') !== file.sha256) throw failure('integrity')
              await output.sync()
              found.add(path)
            }
          } finally {
            await output?.close()
          }
        }
        if (found.size !== model.files.length) throw failure('integrity')
      }
      const stop = (error) => {
        controller.abort(error)
        extract.destroy(error)
        throw error
      }
      // legacy 解压流需要真正的 Readable 源；直接传 async iterator 会丢失 pipeline 收尾通知。
      tasks = [
        consume().catch(stop),
        pipeline(
          Readable.from(input()),
          unbzip2(),
          archiveBudget(model, controller.signal),
          extract,
          {
            signal: controller.signal,
          },
        ).catch(stop),
      ]
      await Promise.all(tasks)
      checkAbort(signal)
    } catch (error) {
      checkAbort(signal)
      const reason = controller.signal.reason || error
      throw reason instanceof DownloadError ? reason : failure('integrity')
    } finally {
      controller.abort(failure('cancelled'))
      // 不能仅等待 pipeline：entry 内可能仍在异步写入，取消返回前必须关闭所有自有句柄。
      await Promise.allSettled(tasks)
      clearTimeout(timer)
      signal.removeEventListener('abort', abort)
      await handle.close()
    }
  }

  async publish(model, staging) {
    this.invalidateInstallation(model.id)
    const destination = this.directory(model)
    await checkedDirectory(this.root)
    await checkedDirectory(staging)
    const exists = await checkedDirectory(destination)
    const backup = join(this.root, `.${model.id}.${randomUUID()}.previous`)
    if (exists) {
      await checkLinks(destination)
      await rename(destination, backup)
    }
    try {
      await rename(staging, destination)
    } catch (error) {
      if (exists) await rename(backup, destination)
      throw error
    }
    // 发布成功后清理旧目录失败，不应把已安装模型报告为失败。
    if (exists) await rm(backup, { recursive: true, force: true }).catch(() => {})
  }

  async downloadFile(model, file, path, completedBytes, signal) {
    let lastError
    for (const url of file.urls) {
      checkAbort(signal)
      try {
        await this.downloadFromUrl(model, file, path, completedBytes, signal, url)
        return
      } catch (error) {
        checkAbort(signal)
        lastError = safeError(error)
        if (lastError.code === 'path' || lastError.code === 'storage') throw lastError
      }
    }
    throw lastError
  }

  async downloadFromUrl(model, file, path, completedBytes, signal, initialUrl) {
    let offset = (await statOrNull(path))?.size || 0
    let hash = createHash('sha256')
    if (await statOrNull(path)) {
      if (offset <= file.bytes) hash = await hashFile(path, offset, signal)
      else {
        const handle = await checkedFile(path, constants.O_WRONLY)
        try {
          await handle.truncate(0)
        } finally {
          await handle.close()
        }
        offset = 0
      }
    }
    if (offset === file.bytes) {
      if (hash.copy().digest('hex') === file.sha256) {
        // 零字节文件也要真实创建，不能只凭空流的摘要认为已落盘。
        const handle = await checkedFile(path, constants.O_WRONLY | constants.O_CREAT)
        await handle.close()
        this.states.set(model.id, {
          status: 'downloading',
          downloadedBytes: completedBytes + offset,
        })
        return
      }
      const handle = await checkedFile(path, constants.O_WRONLY)
      try {
        await handle.truncate(0)
      } finally {
        await handle.close()
      }
      offset = 0
      hash = createHash('sha256')
    }
    this.states.set(model.id, { status: 'downloading', downloadedBytes: completedBytes + offset })
    const controller = new AbortController()
    const abort = () => controller.abort(signal.reason)
    checkAbort(signal)
    signal.addEventListener('abort', abort, { once: true })
    const timeout = () => controller.abort(failure('timeout'))
    const totalTimer = setTimeout(timeout, MAX_FILE_DURATION_MS)
    let response
    let reader
    let handle
    try {
      let url = initialUrl
      for (let redirects = 0; ; redirects += 1) {
        const pending = Promise.resolve().then(() =>
          this.fetchImpl(url, {
            signal: controller.signal,
            redirect: 'manual',
            credentials: 'omit',
            referrerPolicy: 'no-referrer',
            headers: {
              'Accept-Encoding': 'identity',
              ...(offset ? { Range: `bytes=${offset}-` } : {}),
            },
          }),
        )
        pending.then(
          (lateResponse) => {
            if (controller.signal.aborted) lateResponse.body?.cancel().catch(() => {})
          },
          () => {},
        )
        try {
          response = await interruptible(pending, controller.signal, this.requestTimeoutMs, timeout)
        } catch (error) {
          if (controller.signal.aborted) throw controller.signal.reason
          throw error instanceof DownloadError ? error : failure('http')
        }
        if (response.redirected) throw failure('redirect')
        if (response.status < 300 || response.status >= 400) break
        response.body?.cancel().catch(() => {})
        const location = response.headers.get('location')
        if (!location || redirects >= 2) throw failure('redirect')
        let next
        try {
          next = new URL(location, url)
        } catch {
          throw failure('redirect')
        }
        if (
          !validCdnUrl(next) ||
          (!file.urls.includes(next.href) &&
            !this.trustedRedirectHosts.has(next.hostname.toLowerCase()))
        ) {
          throw failure('redirect')
        }
        url = next.href
      }
      if (response.status !== 200 && response.status !== 206) throw failure('http')
      if (
        response.headers.get('content-encoding') &&
        response.headers.get('content-encoding') !== 'identity'
      )
        throw failure('size')
      if (response.status === 206) {
        const expected = `bytes ${offset}-${file.bytes - 1}/${file.bytes}`
        if (!offset || response.headers.get('content-range') !== expected) throw failure('range')
      } else {
        if (response.headers.get('content-range')) throw failure('range')
        offset = 0
        hash = createHash('sha256')
      }
      const length = response.headers.get('content-length')
      if (
        length !== null &&
        (!/^(0|[1-9][0-9]*)$/.test(length) || Number(length) !== file.bytes - offset)
      )
        throw failure('size')
      if (!response.body) throw failure('size')
      handle = await checkedFile(path, constants.O_WRONLY | constants.O_CREAT)
      if (!offset) await handle.truncate(0)
      this.states.set(model.id, { status: 'downloading', downloadedBytes: completedBytes + offset })
      reader = response.body.getReader()
      while (true) {
        checkAbort(controller.signal)
        let chunk
        try {
          chunk = await interruptible(
            reader.read(),
            controller.signal,
            this.requestTimeoutMs,
            timeout,
          )
        } catch (error) {
          throw error instanceof DownloadError ? error : failure('http')
        }
        const { done, value } = chunk
        if (done) break
        if (!(value instanceof Uint8Array) || value.byteLength > file.bytes - offset)
          throw failure('size')
        let written = 0
        while (written < value.byteLength) {
          checkAbort(controller.signal)
          const { bytesWritten } = await handle.write(
            value,
            written,
            value.byteLength - written,
            offset + written,
          )
          if (!bytesWritten) throw failure('storage')
          written += bytesWritten
        }
        hash.update(value)
        offset += value.byteLength
        this.states.set(model.id, {
          status: 'downloading',
          downloadedBytes: completedBytes + offset,
        })
      }
      if (offset !== file.bytes) throw failure('size')
      if (hash.digest('hex') !== file.sha256) {
        await handle.truncate(0)
        this.states.set(model.id, { status: 'downloading', downloadedBytes: completedBytes })
        throw failure('integrity')
      }
      await handle.sync()
    } finally {
      clearTimeout(totalTimer)
      signal.removeEventListener('abort', abort)
      controller.abort(failure('cancelled'))
      if (reader) {
        reader.cancel().catch(() => {})
        reader.releaseLock()
      } else response?.body?.cancel().catch(() => {})
      await handle?.close()
    }
  }
}
