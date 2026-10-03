// 本地处理引擎只是经过摘要验证的静态资源缓存；服务不执行 JS、WASM 或模型。
import { createHash, randomUUID } from 'node:crypto'
import { constants } from 'node:fs'
import { lstat, mkdir, open, readdir, realpath, rename, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { SPRITE_ENGINE_CATALOG, SpriteEngineError } from '../../shared/game/sprite-engine-catalog.mjs'
import { writeJsonAtomic } from '../storage/json-file.mjs'

/** @typedef {import('../../shared/game/sprite-engine-catalog.mjs').SpriteEngineDefinition} EngineDefinition */
/** @typedef {import('../../shared/game/sprite-engine-catalog.mjs').SpriteEngineFile} EngineFile */
/** @typedef {import('../../shared/game/sprite-engine-catalog.mjs').SpriteEngineStatus} EngineStatus */
/** @typedef {{ directory: string, version: string }} Installation */
/** @typedef {{ controller: AbortController, done: Promise<void> }} EngineJob */
/** @typedef {{ dataDir: string, fetchFn?: typeof fetch, definitions?: readonly EngineDefinition[], timeoutMs?: number, sourceTimeoutMs?: number }} EngineDependencies */

/** @param {string} code @param {number} [statusCode] */
function failure(code, statusCode = 400) {
  return new SpriteEngineError(code, '本地图片引擎资源不可用，请重试。', statusCode)
}
/** @param {unknown} error */
function missing(error) {
  return typeof error === 'object' && error !== null && 'code' in error && error.code === 'ENOENT'
}
/** @param {Uint8Array} bytes */
function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex')
}
/** @param {string} name */
function safePart(name) {
  return /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,159}$/.test(name) && name !== '..'
}
/** @param {string} path @param {boolean} [create] */
async function directory(path, create = false) {
  let info = await lstat(path).catch((error) => {
    if (!missing(error)) throw error
    return null
  })
  if (!info && create) {
    await mkdir(path, { mode: 0o700 })
    info = await lstat(path)
  }
  if (!info?.isDirectory() || info.isSymbolicLink() || (await realpath(path)) !== path)
    throw failure('sprite_engine_storage_unsafe', 409)
  return path
}

/** @param {string} path @param {number} maxBytes */
async function readBounded(path, maxBytes) {
  const info = await lstat(path)
  if (!info.isFile() || info.isSymbolicLink() || info.size > maxBytes)
    throw failure('sprite_engine_integrity')
  const file = await open(path, constants.O_RDONLY | (constants.O_NOFOLLOW || 0))
  try {
    const current = await file.stat()
    if (!current.isFile() || current.size > maxBytes) throw failure('sprite_engine_integrity')
    const bytes = Buffer.alloc(current.size)
    let offset = 0
    while (offset < bytes.length) {
      const { bytesRead } = await file.read(bytes, offset, bytes.length - offset, offset)
      if (!bytesRead) throw failure('sprite_engine_integrity')
      offset += bytesRead
    }
    return bytes
  } finally {
    await file.close()
  }
}

/** @param {Uint8Array} bytes @param {EngineFile} file */
function validateBytes(bytes, file) {
  if (bytes.byteLength !== file.bytes || sha256(bytes) !== file.sha256)
    throw failure('sprite_engine_integrity')
}

export class SpriteEngineService {
  /** @param {EngineDependencies} dependencies */
  constructor({
    dataDir,
    fetchFn = fetch,
    definitions = SPRITE_ENGINE_CATALOG,
    timeoutMs = 180_000,
    sourceTimeoutMs = 45_000,
  }) {
    this.dataDir = dataDir
    this.fetchFn = fetchFn
    this.definitions = definitions
    this.timeoutMs = timeoutMs
    this.sourceTimeoutMs = sourceTimeoutMs
    this.root = ''
    /** @type {Map<string, EngineStatus>} */
    this.states = new Map(
      definitions.map((definition) => {
        const total = definition.files.reduce((sum, file) => sum + file.bytes, 0)
        return [
          definition.id,
          {
            id: definition.id,
            name: definition.name,
            version: definition.version,
            bytes: total,
            status: 'missing',
            received: 0,
            total,
            error: '',
          },
        ]
      }),
    )
    /** @type {Map<string, Installation>} */
    this.installed = new Map()
    /** @type {Map<string, EngineJob>} */
    this.active = new Map()
    /** @type {Promise<void> | null} */
    this.initializing = null
    this.queue = Promise.resolve()
    this.closed = false
    /** @type {Promise<void> | null} */
    this.closing = null
  }

  /** @param {string} id */
  definition(id) {
    const definition = this.definitions.find((engine) => engine.id === id)
    if (!definition) throw failure('sprite_engine_not_found', 404)
    return definition
  }

  /** @param {string} id */
  state(id) {
    const state = this.states.get(id)
    if (!state) throw failure('sprite_engine_not_found', 404)
    return state
  }

  /** @param {string} id */
  async engineDirectory(id) {
    this.definition(id)
    await directory(this.root)
    return directory(join(this.root, id), true)
  }

  init() {
    this.initializing ??= (async () => {
      await mkdir(this.dataDir, { recursive: true, mode: 0o700 })
      this.root = join(await realpath(this.dataDir), 'sprite-engines')
      await directory(this.root, true)
      for (const definition of this.definitions) {
        const root = await this.engineDirectory(definition.id)
        // 上次崩溃留下的未发布下载不被识别为安装，也不会被自动续传。
        for (const item of await readdir(root, { withFileTypes: true })) {
          if (item.name.startsWith('staging-') && item.isDirectory())
            await rm(join(root, item.name), { recursive: true })
        }
        try {
          /** @type {unknown} */
          const value = JSON.parse(
            (await readBounded(join(root, 'installed.json'), 4096)).toString('utf8'),
          )
          if (
            !value ||
            typeof value !== 'object' ||
            !('directory' in value) ||
            typeof value.directory !== 'string' ||
            !/^install-[a-f0-9-]{36}$/.test(value.directory) ||
            !('version' in value) ||
            value.version !== definition.version
          )
            throw failure('sprite_engine_integrity')
          const installation = { directory: value.directory, version: definition.version }
          const path = await directory(join(root, installation.directory))
          for (const file of definition.files)
            validateBytes(await readBounded(join(path, file.name), file.bytes), file)
          this.installed.set(definition.id, installation)
          Object.assign(this.state(definition.id), {
            status: 'ready',
            received: this.state(definition.id).total,
          })
        } catch (error) {
          if (!missing(error))
            Object.assign(this.state(definition.id), {
              status: 'failed',
              error: 'sprite_engine_integrity',
            })
        }
      }
    })()
    return this.initializing
  }

  /** @template T @param {() => Promise<T> | T} operation @returns {Promise<T>} */
  async exclusive(operation) {
    await this.init()
    const pending = this.queue.then(() => {
      if (this.closed) throw failure('sprite_engine_closed', 503)
      return operation()
    })
    this.queue = pending.then(
      () => {},
      () => {},
    )
    return pending
  }

  async catalog() {
    await this.init()
    return { engines: [...this.states.values()].map((state) => ({ ...state })) }
  }

  /** @param {string} id */
  download(id) {
    const definition = this.definition(id)
    return this.exclusive(async () => {
      if (this.active.has(id) || this.state(id).status === 'ready') return this.catalog()
      const controller = new AbortController()
      const job = { controller, done: Promise.resolve() }
      this.active.set(id, job)
      Object.assign(this.state(id), { status: 'downloading', received: 0, error: '', file: '' })
      job.done = this.downloadFiles(definition, controller).finally(() => this.active.delete(id))
      void job.done.catch(() => {})
      return this.catalog()
    })
  }

  /** @param {EngineDefinition} definition @param {AbortController} controller */
  async downloadFiles(definition, controller) {
    let path = ''
    const { signal } = controller
    let timedOut = false
    const timer = setTimeout(() => {
      timedOut = true
      controller.abort()
    }, this.timeoutMs)
    try {
      const root = await this.engineDirectory(definition.id)
      const staging = `staging-${randomUUID()}`
      path = await directory(join(root, staging), true)
      for (const file of definition.files) {
        this.state(definition.id).file = file.name
        if (signal.aborted) throw failure('sprite_engine_cancelled')
        await this.downloadFile(definition.id, file, join(path, file.name), signal)
      }
      if (signal.aborted) throw failure('sprite_engine_cancelled')
      await this.publish(definition, staging)
    } catch (error) {
      Object.assign(this.state(definition.id), {
        status: signal.aborted && !timedOut ? 'missing' : 'failed',
        received: 0,
        error: timedOut
          ? 'sprite_engine_timeout'
          : signal.aborted
            ? ''
            : error instanceof SpriteEngineError
              ? error.code
              : 'sprite_engine_download_failed',
      })
    } finally {
      clearTimeout(timer)
      if (path) await rm(path, { recursive: true, force: true })
    }
  }

  /** @param {string} id @param {EngineFile} file @param {string} path @param {AbortSignal} signal */
  async downloadFile(id, file, path, signal) {
    const before = this.state(id).received
    const sources = [file.url, ...(file.fallbackUrls ?? [])]
    for (let index = 0; index < sources.length; index++) {
      signal.throwIfAborted()
      const controller = new AbortController()
      const abort = () => controller.abort(signal.reason)
      signal.addEventListener('abort', abort, { once: true })
      const timer = setTimeout(
        () => controller.abort(failure('sprite_engine_timeout')),
        this.sourceTimeoutMs,
      )
      let response
      try {
        response = await this.fetchFn(sources[index], {
          signal: controller.signal,
          redirect: 'follow',
          credentials: 'omit',
        })
        if (!response.ok || !response.body) throw failure('sprite_engine_download_failed')
        const declared = response.headers.get('content-length')
        if (declared !== null && Number(declared) > file.bytes)
          throw failure('sprite_engine_integrity')
        // 每次换源从头校验同一固定资源，不拼接不同来源的部分文件。
        const handle = await open(path, 'wx', 0o600)
        const reader = response.body.getReader()
        const hash = createHash('sha256')
        let bytes = 0
        try {
          while (true) {
            controller.signal.throwIfAborted()
            const chunk = await reader.read()
            if (chunk.done) break
            bytes += chunk.value.byteLength
            if (bytes > file.bytes) throw failure('sprite_engine_integrity')
            hash.update(chunk.value)
            await handle.writeFile(chunk.value)
            this.state(id).received = before + bytes
          }
          if (bytes !== file.bytes || hash.digest('hex') !== file.sha256)
            throw failure('sprite_engine_integrity')
        } finally {
          try {
            await reader.cancel().catch(() => {})
          } finally {
            reader.releaseLock()
            await handle.close()
          }
        }
        controller.signal.throwIfAborted()
        return
      } catch (error) {
        this.state(id).received = before
        await rm(path, { force: true })
        signal.throwIfAborted()
        const storageError =
          error &&
          typeof error === 'object' &&
          'code' in error &&
          [
            'EACCES',
            'EPERM',
            'EROFS',
            'ENOSPC',
            'EMFILE',
            'ENFILE',
            'EISDIR',
            'ENOTDIR',
            'EIO',
          ].includes(String(error.code))
        if (storageError) throw failure('sprite_engine_storage_unsafe')
        if (index === sources.length - 1)
          throw controller.signal.aborted ? failure('sprite_engine_timeout') : error
      } finally {
        clearTimeout(timer)
        signal.removeEventListener('abort', abort)
        // 网络中断后的流可能已出错，清理不能盖掉原始错误或阻止换源。
        if (response?.body && !response.body.locked) await response.body.cancel().catch(() => {})
      }
    }
  }

  /** @param {EngineDefinition} definition @param {string} staging */
  async publish(definition, staging) {
    const root = await this.engineDirectory(definition.id)
    const name = `install-${randomUUID()}`
    const old = this.installed.get(definition.id)
    await directory(join(root, staging))
    const metadata = join(root, 'installed.json')
    const info = await lstat(metadata).catch((error) => {
      if (!missing(error)) throw error
      return null
    })
    if (info && (!info.isFile() || info.isSymbolicLink()))
      throw failure('sprite_engine_storage_unsafe', 409)
    await rename(join(root, staging), join(root, name))
    try {
      await writeJsonAtomic(
        metadata,
        { version: definition.version, directory: name },
        { mode: 0o600 },
      )
    } catch (error) {
      await rm(join(root, name), { recursive: true })
      throw error
    }
    this.installed.set(definition.id, { version: definition.version, directory: name })
    Object.assign(this.state(definition.id), {
      status: 'ready',
      received: this.state(definition.id).total,
      error: '',
      file: '',
    })
    if (old && old.directory !== name)
      await rm(await directory(join(root, old.directory)), { recursive: true })
  }

  /** @param {string} id */
  cancel(id) {
    this.definition(id)
    return this.exclusive(async () => {
      const active = this.active.get(id)
      active?.controller.abort()
      await active?.done
      return this.catalog()
    })
  }

  /** @param {string} id */
  remove(id) {
    this.definition(id)
    return this.exclusive(async () => {
      const active = this.active.get(id)
      active?.controller.abort()
      await active?.done
      await rm(await this.engineDirectory(id), { recursive: true })
      this.installed.delete(id)
      Object.assign(this.state(id), { status: 'missing', received: 0, error: '' })
      return this.catalog()
    })
  }

  /** @param {string} id @param {string} fileName */
  file(id, fileName) {
    const definition = this.definition(id)
    const file = definition.files.find((file) => file.name === fileName && safePart(fileName))
    if (!file) throw failure('sprite_engine_file_not_found', 404)
    return this.exclusive(async () => {
      const installed = this.installed.get(id)
      if (!installed || this.state(id).status !== 'ready')
        throw failure('sprite_engine_missing', 409)
      const root = await this.engineDirectory(id)
      const path = await directory(join(root, installed.directory))
      const buffer = await readBounded(join(path, file.name), file.bytes)
      validateBytes(buffer, file)
      return { buffer, mimeType: file.mimeType }
    })
  }

  /** @param {Record<string, Uint8Array>} files */
  validateBundleFiles(files) {
    const present = new Set()
    for (const [path, bytes] of Object.entries(files)) {
      const parts = path.split('/')
      if (
        parts.length !== 3 ||
        parts[0] !== 'engines' ||
        !safePart(parts[1]) ||
        !safePart(parts[2])
      )
        throw failure('sprite_engine_bundle_invalid')
      const definition = this.definitions.find((engine) => engine.id === parts[1])
      const file = definition?.files.find((file) => file.name === parts[2])
      if (!definition || !file || !(bytes instanceof Uint8Array))
        throw failure('sprite_engine_bundle_invalid')
      validateBytes(bytes, file)
      present.add(definition.id)
    }
    for (const id of present)
      for (const file of this.definition(id).files) {
        if (!Object.hasOwn(files, `engines/${id}/${file.name}`))
          throw failure('sprite_engine_bundle_incomplete')
      }
  }

  /** 仅向内部短命执行器提供已重新验证的固定资源目录。 @param {string} id */
  getExecutionDirectory(id) {
    const definition = this.definition(id)
    return this.exclusive(async () => {
      const installed = this.installed.get(id)
      if (!installed || this.state(id).status !== 'ready')
        throw failure('sprite_engine_missing', 409)
      const root = await this.engineDirectory(id)
      const path = await directory(join(root, installed.directory))
      for (const file of definition.files)
        validateBytes(await readBounded(join(path, file.name), file.bytes), file)
      return path
    })
  }

  /** @param {Record<string, Uint8Array>} files */
  installBundleFiles(files) {
    this.validateBundleFiles(files)
    return this.exclusive(async () => {
      const definitions = this.definitions.filter((definition) =>
        definition.files.some((file) =>
          Object.hasOwn(files, `engines/${definition.id}/${file.name}`),
        ),
      )
      if (definitions.some((definition) => this.active.has(definition.id)))
        throw failure('sprite_engine_busy', 409)
      /** @type {Array<{definition:EngineDefinition, root:string, staging:string, path:string, state:EngineStatus}>} */
      const staged = []
      /** @type {Array<{id:string, root:string, directory:string}>} */
      const published = []
      try {
        for (const definition of definitions) {
          const root = await this.engineDirectory(definition.id)
          const current = this.installed.get(definition.id)
          if (current) {
            const path = await directory(join(root, current.directory))
            for (const file of definition.files)
              validateBytes(await readBounded(join(path, file.name), file.bytes), file)
            continue
          }
          const staging = `staging-${randomUUID()}`
          const path = await directory(join(root, staging), true)
          staged.push({ definition, root, staging, path, state: { ...this.state(definition.id) } })
          for (const file of definition.files) {
            const handle = await open(join(path, file.name), 'wx', 0o600)
            try {
              await handle.writeFile(files[`engines/${definition.id}/${file.name}`])
            } finally {
              await handle.close()
            }
          }
        }
        // 全套文件落入暂存目录后才发布；失败只撤销本次新安装，不碰原有可用引擎。
        for (const { definition, root, staging } of staged) {
          await this.publish(definition, staging)
          const installed = this.installed.get(definition.id)
          if (installed) published.push({ id: definition.id, root, directory: installed.directory })
        }
      } catch (error) {
        for (const installed of published.reverse()) {
          await rm(join(installed.root, 'installed.json'), { force: true })
          await rm(await directory(join(installed.root, installed.directory)), { recursive: true })
          this.installed.delete(installed.id)
        }
        for (const entry of staged) Object.assign(this.state(entry.definition.id), entry.state)
        throw error
      } finally {
        for (const { path } of staged) await rm(path, { recursive: true, force: true })
      }
      return this.catalog()
    })
  }

  async exportFiles() {
    return this.exclusive(async () => {
      /** @type {Record<string, Uint8Array>} */
      const files = {}
      for (const definition of this.definitions) {
        const installed = this.installed.get(definition.id)
        if (!installed || this.state(definition.id).status !== 'ready') continue
        const root = await this.engineDirectory(definition.id)
        const path = await directory(join(root, installed.directory))
        for (const file of definition.files) {
          const bytes = await readBounded(join(path, file.name), file.bytes)
          validateBytes(bytes, file)
          files[`engines/${definition.id}/${file.name}`] = bytes
        }
      }
      return files
    })
  }

  dispose() {
    this.closed = true
    this.closing ??= (async () => {
      await this.initializing
      for (const job of this.active.values()) job.controller.abort()
      await Promise.all([...this.active.values()].map((job) => job.done))
      await this.queue
    })()
    return this.closing
  }
}
