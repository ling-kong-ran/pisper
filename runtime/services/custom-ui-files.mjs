// 原生组件的磁盘操作：安全遍历与内容哈希、目录内路径解析、暂存 + rename 交换的写入器、
// 版本快照、移入回收站，以及“每文件一条 Promise 链”的 JSON 文档。
//
// 边界：
// - 只处理组件根目录下名字合法的子目录；点开头的段（暂存目录、隐藏文件）一律跳过。
// - 遍历不跟随符号链接；按路径读取时 realpath 后仍须落在组件目录内（沿用旧 resolveAssetPath 的防护）。
// - 版本快照与回收站放在被监听、被服务的 custom-ui/ 之外，避免热重载循环（计划 §3.1）。
import { createHash, randomBytes } from 'node:crypto'
import {
  copyFile,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rename,
  rm,
  rmdir,
  stat,
  writeFile,
} from 'node:fs/promises'
import { dirname, extname, isAbsolute, join, normalize, resolve, sep } from 'node:path'
import { isSafeComponentPath } from '../../shared/custom-ui-manifest.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

// 组件 id 即目录名：只允许安全的文件名字符，避免路径与 URL 编码问题。
export const COMPONENT_ID_PATTERN = /^[a-z0-9][a-z0-9._-]{0,63}$/
// 资产大小上限：组件应是轻量静态文件，防止把目录当成文件服务器滥用。
export const MAX_ASSET_BYTES = 8 * 1024 * 1024
export const MAX_COMPONENT_FILES = 128
export const MAX_COMPONENT_BYTES = 16 * 1024 * 1024
export const MAX_COMPONENT_DEPTH = 4

// 原生模块路由可服务的类型；不含 .html / .htm：原生组件同源执行，HTML 文档不能当成组件资源打开。
export const COMPONENT_MODULE_MIME = Object.freeze({
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
})

/** @param {string} path */
export function componentFileType(path) {
  return /** @type {Record<string, string>} */ (COMPONENT_MODULE_MIME)[extname(path).toLowerCase()]
}

/** @param {unknown} error @returns {string | undefined} */
function errorCode(error) {
  return typeof error === 'object' && error !== null && 'code' in error
    ? String(error.code)
    : undefined
}

/**
 * 组件目录内的文件解析：拒绝绝对路径、.. 穿越、隐藏段、manifest.json 与符号链接逃逸。
 * 从旧 CustomUiService.resolveAssetPath 迁来，逻辑不变；旧服务的非内置分支也改为调用这里。
 * @param {string} root
 * @param {string} id
 * @param {unknown} relativePath
 * @param {{ maxBytes?: number }} [options]
 * @returns {Promise<{ file: string, size: number } | null>}
 */
export async function resolveComponentFile(
  root,
  id,
  relativePath,
  { maxBytes = MAX_ASSET_BYTES } = {},
) {
  if (!COMPONENT_ID_PATTERN.test(id)) return null
  const rawPath = String(relativePath || '')
  if (isAbsolute(rawPath) || rawPath.split(/[/\\]/).some((part) => part === '..')) return null
  const relative = normalize(rawPath)
  if (!relative || relative.startsWith('..') || isAbsolute(relative)) return null
  const segments = relative.split(sep)
  if (segments.some((segment) => segment.startsWith('.'))) return null
  // manifest 属于配置元数据，不作为静态资产提供。
  if (segments.length === 1 && segments[0] === 'manifest.json') return null
  const dir = resolve(root, id)
  const file = resolve(dir, relative)
  if (file !== dir && !file.startsWith(`${dir}${sep}`)) return null
  // 符号链接防护：realpath 后仍须落在组件目录内。
  const info = await stat(file).catch(() => null)
  if (!info?.isFile() || info.size > maxBytes) return null
  const [realFile, realDir, realRoot] = await Promise.all([
    realpath(file).catch(() => ''),
    realpath(dir).catch(() => ''),
    realpath(root).catch(() => ''),
  ])
  if (!realRoot || !realDir.startsWith(`${realRoot}${sep}`)) return null
  if (!realFile || (realFile !== realDir && !realFile.startsWith(`${realDir}${sep}`))) return null
  return { file, size: info.size }
}

/**
 * @typedef {{ path: string, file: string, size: number, mtimeMs: number }} ComponentFileEntry
 * @typedef {{ files: ComponentFileEntry[], error: string | null }} ComponentFileList
 */

/**
 * 安全遍历组件目录：不跟随符号链接、跳过点开头的条目；超过 4 层、128 个文件或 16 MiB 时报 component_too_large。
 * @param {string} dir
 * @returns {Promise<ComponentFileList>}
 */
export async function listComponentFiles(dir) {
  /** @type {ComponentFileEntry[]} */
  const files = []
  let total = 0
  /** @type {string | null} */
  let error = null
  /** @param {string} current @param {string[]} prefix */
  async function walk(current, prefix) {
    if (error) return
    if (prefix.length > MAX_COMPONENT_DEPTH) {
      error = 'component_too_large'
      return
    }
    const entries = await readdir(current, { withFileTypes: true }).catch(() => [])
    entries.sort((left, right) => (left.name < right.name ? -1 : left.name > right.name ? 1 : 0))
    for (const entry of entries) {
      if (error) return
      if (entry.name.startsWith('.')) continue
      const path = [...prefix, entry.name]
      const absolute = join(current, entry.name)
      if (entry.isDirectory()) {
        await walk(absolute, path)
        continue
      }
      if (!entry.isFile()) continue
      const info = await lstat(absolute).catch(() => null)
      if (!info?.isFile()) continue
      total += info.size
      if (files.length >= MAX_COMPONENT_FILES || total > MAX_COMPONENT_BYTES) {
        error = 'component_too_large'
        return
      }
      files.push({ path: path.join('/'), file: absolute, size: info.size, mtimeMs: info.mtimeMs })
    }
  }
  await walk(dir, [])
  return { files, error }
}

/**
 * 组件内容哈希：对排序后的文件逐个计算 sha256(path\0size\0sha256(content)\n…)（含 manifest.json 原文），
 * rev 取前 24 位 hex。按 (mtimeMs, size) 缓存每个文件的摘要。
 */
export class ComponentHasher {
  constructor() {
    /** @type {Map<string, { mtimeMs: number, size: number, digest: string }>} */
    this.cache = new Map()
  }

  /**
   * @param {string} dir
   * @returns {Promise<{ rev: string | null, files: ComponentFileEntry[], error: string | null }>}
   */
  async compute(dir) {
    const listed = await listComponentFiles(dir)
    if (listed.error) return { rev: null, files: listed.files, error: listed.error }
    const hash = createHash('sha256')
    for (const entry of listed.files) {
      let cached = this.cache.get(entry.file)
      if (!cached || cached.mtimeMs !== entry.mtimeMs || cached.size !== entry.size) {
        const content = await readFile(entry.file).catch(() => null)
        if (!content) continue
        cached = {
          mtimeMs: entry.mtimeMs,
          size: entry.size,
          digest: createHash('sha256').update(content).digest('hex'),
        }
        this.cache.set(entry.file, cached)
      }
      hash.update(`${entry.path}\0${entry.size}\0${cached.digest}\n`)
    }
    return { rev: hash.digest('hex').slice(0, 24), files: listed.files, error: null }
  }

  /** @param {string} dir */
  invalidate(dir) {
    const prefix = `${dir}${sep}`
    for (const key of this.cache.keys()) if (key.startsWith(prefix)) this.cache.delete(key)
  }
}

/** @param {number} attempt */
function backoff(attempt) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, 20 * (attempt + 1)))
}

/**
 * Windows 上目录被扫描器或编辑器短暂占用时 rename 会报 EPERM / EBUSY / EACCES，短暂重试。
 * @param {string} from
 * @param {string} to
 * @param {(from: string, to: string) => Promise<void>} [renameDirectory]
 */
export async function renameWithRetry(from, to, renameDirectory = rename) {
  let lastError
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      await renameDirectory(from, to)
      return
    } catch (error) {
      lastError = error
      const code = errorCode(error)
      if (code !== 'EPERM' && code !== 'EBUSY' && code !== 'EACCES') throw error
      // 目标已存在时 Windows 也报 EPERM：交给调用方按“已存在”处理，不再重试。
      if (await lstat(to).catch(() => null)) throw error
      await backoff(attempt)
    }
  }
  throw lastError
}

/**
 * @typedef {{ path: string, content: string | Uint8Array }} ComponentFileWrite
 */

/**
 * 写入组件目录：先在 custom-ui/.component-write-* 里拼好完整目录，再 rename 交换，
 * 监听器与列表只会看到写入前或写入后的完整状态。
 * @param {{
 *   root: string,
 *   id: string,
 *   mode: 'create' | 'update',
 *   files: ComponentFileWrite[],
 *   deleteFiles?: string[],
 *   renameDirectory?: (from: string, to: string) => Promise<void>,
 * }} options
 * @returns {Promise<{ previous: Map<string, Buffer> }>} update 时返回被改写前的文件内容，便于统计改动行数
 */
export async function writeComponentDirectory({
  root,
  id,
  mode,
  files,
  deleteFiles = [],
  renameDirectory = rename,
}) {
  if (!COMPONENT_ID_PATTERN.test(id)) throw new Error('component id invalid')
  for (const path of [...files.map((file) => file.path), ...deleteFiles]) {
    if (!isSafeComponentPath(path)) throw new Error('component path invalid')
  }
  await mkdir(root, { recursive: true, mode: 0o700 })
  const target = join(root, id)
  const existing = await lstat(target).catch(() => null)
  if (mode === 'create' && existing) throw Object.assign(new Error('exists'), { code: 'EEXIST' })
  if (mode === 'update' && !existing?.isDirectory())
    throw Object.assign(new Error('missing'), { code: 'ENOENT' })
  const stage = await mkdtemp(join(root, '.component-write-'))
  /** @type {Map<string, Buffer>} */
  const previous = new Map()
  /** @type {string | null} */
  let old = null
  try {
    if (mode === 'update') {
      const listed = await listComponentFiles(target)
      for (const entry of listed.files) {
        const content = await readFile(entry.file)
        previous.set(entry.path, content)
        const destination = join(stage, ...entry.path.split('/'))
        await mkdir(dirname(destination), { recursive: true, mode: 0o700 })
        await writeFile(destination, content, { flag: 'wx', mode: 0o600 })
      }
    }
    for (const path of deleteFiles) {
      const segments = path.split('/')
      await rm(join(stage, ...segments), { force: true })
      // 删掉文件后留下的空目录一并清掉；rmdir 遇到非空目录会失败，正好在那里停下。
      for (let depth = segments.length - 1; depth > 0; depth -= 1) {
        const removed = await rmdir(join(stage, ...segments.slice(0, depth))).then(
          () => true,
          () => false,
        )
        if (!removed) break
      }
    }
    for (const file of files) {
      const destination = join(stage, ...file.path.split('/'))
      await mkdir(dirname(destination), { recursive: true, mode: 0o700 })
      await writeFile(destination, file.content, { mode: 0o600 })
    }
    if (mode === 'update') {
      old = join(root, `.component-old-${randomBytes(6).toString('hex')}`)
      await renameWithRetry(target, old, renameDirectory)
      try {
        await renameWithRetry(stage, target, renameDirectory)
      } catch (error) {
        // 换入失败：把原目录放回去，不能让组件凭空消失。
        await renameWithRetry(old, target, renameDirectory).catch(() => {})
        old = null
        throw error
      }
    } else {
      try {
        await renameWithRetry(stage, target, renameDirectory)
      } catch (error) {
        const code = errorCode(error)
        if (code === 'EEXIST' || code === 'ENOTEMPTY' || code === 'EPERM') {
          if (await lstat(target).catch(() => null))
            throw Object.assign(new Error('exists'), { code: 'EEXIST' })
        }
        throw error
      }
    }
    return { previous }
  } finally {
    await rm(stage, { recursive: true, force: true }).catch(() => {})
    if (old) await rm(old, { recursive: true, force: true }).catch(() => {})
  }
}

/**
 * 写入前的版本快照：custom-ui-versions/<id>/<n>/，index.json 记录序号，最多保留 max 份（删最旧的）。
 * 调用方负责按组件串行。
 * @param {{ versionsRoot: string, id: string, dir: string, max?: number, now?: () => number, summary?: string }} options
 * @returns {Promise<{ n: number } | null>} 组件目录不存在时返回 null
 */
export async function snapshotComponent({
  versionsRoot,
  id,
  dir,
  max = 20,
  now = Date.now,
  summary = '',
}) {
  if (!COMPONENT_ID_PATTERN.test(id)) throw new Error('component id invalid')
  const listed = await listComponentFiles(dir)
  if (!listed.files.length) return null
  const base = join(versionsRoot, id)
  const indexPath = join(base, 'index.json')
  const raw = await readJson(indexPath, null).catch(() => null)
  /** @type {{ n: number, createdAt: string, summary: string }[]} */
  const snapshots = Array.isArray(raw?.snapshots)
    ? raw.snapshots.filter((/** @type {any} */ item) => Number.isSafeInteger(item?.n) && item.n > 0)
    : []
  const n = snapshots.reduce((highest, item) => Math.max(highest, item.n), 0) + 1
  const target = join(base, String(n))
  await rm(target, { recursive: true, force: true })
  for (const entry of listed.files) {
    const destination = join(target, ...entry.path.split('/'))
    await mkdir(dirname(destination), { recursive: true, mode: 0o700 })
    await copyFile(entry.file, destination)
  }
  snapshots.push({ n, createdAt: new Date(now()).toISOString(), summary })
  const removed = snapshots.splice(0, Math.max(0, snapshots.length - max))
  for (const item of removed) await rm(join(base, String(item.n)), { recursive: true, force: true })
  await writeJsonAtomic(indexPath, { version: 1, snapshots })
  return { n }
}

/** @param {number} time */
function compactTimestamp(time) {
  return new Date(time)
    .toISOString()
    .replace(/[-:]/g, '')
    .replace(/\.\d+Z$/, 'Z')
}

/**
 * 删除组件 = 移入回收站：<trashRoot>/<id>-<UTC 时间>/ 下放 component/（原目录）、可选 data.json（组件存储）
 * 与 meta.json（状态与实例快照）。7 天清理与恢复界面留到 A8。
 * @param {{
 *   root: string,
 *   trashRoot: string,
 *   dataRoot: string,
 *   id: string,
 *   now?: () => number,
 *   meta: Record<string, unknown>,
 *   renameDirectory?: (from: string, to: string) => Promise<void>,
 * }} options
 * @returns {Promise<string>} 回收站里的目录名
 */
export async function moveComponentToTrash({
  root,
  trashRoot,
  dataRoot,
  id,
  now = Date.now,
  meta,
  renameDirectory = rename,
}) {
  if (!COMPONENT_ID_PATTERN.test(id)) throw new Error('component id invalid')
  await mkdir(trashRoot, { recursive: true, mode: 0o700 })
  const stamp = compactTimestamp(now())
  let name = `${id}-${stamp}`
  let target = join(trashRoot, name)
  for (let suffix = 2; await lstat(target).catch(() => null); suffix += 1) {
    name = `${id}-${stamp}-${suffix}`
    target = join(trashRoot, name)
  }
  await mkdir(target, { mode: 0o700 })
  try {
    await renameWithRetry(join(root, id), join(target, 'component'), renameDirectory)
  } catch (error) {
    await rm(target, { recursive: true, force: true }).catch(() => {})
    throw error
  }
  const storage = join(dataRoot, `${id}.json`)
  if (await lstat(storage).catch(() => null)) {
    await rename(storage, join(target, 'data.json')).catch(() => {})
  }
  await writeJsonAtomic(join(target, 'meta.json'), {
    version: 1,
    id,
    deletedAt: new Date(now()).toISOString(),
    ...meta,
  })
  return name
}

/**
 * 每个 JSON 文件一条 Promise 链：writeJsonAtomic 的临时文件名精确到毫秒，并发写会撞名（计划 §3.1）。
 * 解析失败或形状不对时，先把原文件改名为 .corrupt-<时间> 再从空开始，不静默覆盖。
 * @template T
 */
export class SerialJsonDocument {
  /**
   * @param {{
   *   path: string,
   *   empty: () => T,
   *   parse: (value: unknown) => T | null,
   *   now?: () => number,
   *   maxBytes?: number,
   *   onCorrupt?: (backup: string) => void,
   * }} options
   */
  constructor({ path, empty, parse, now = Date.now, maxBytes = Infinity, onCorrupt }) {
    this.path = path
    this.empty = empty
    this.parse = parse
    this.now = now
    this.maxBytes = maxBytes
    this.onCorrupt = onCorrupt
    /** @type {T | null} */
    this.cache = null
    /** @type {Promise<unknown>} */
    this.queue = Promise.resolve()
  }

  /** @returns {Promise<T>} */
  async load() {
    if (this.cache) return this.cache
    let raw
    let corrupt = false
    try {
      raw = await readJson(this.path, undefined)
    } catch (error) {
      if (!(error instanceof SyntaxError)) throw error
      corrupt = true
    }
    let value = raw === undefined ? this.empty() : this.parse(raw)
    if (corrupt || value === null) {
      const backup = `${this.path}.corrupt-${compactTimestamp(this.now())}`
      await rename(this.path, backup).catch(() => {})
      this.onCorrupt?.(backup)
      value = this.empty()
    }
    this.cache = value
    return value
  }

  /** 排在所有已提交的写入之后读取，读到的总是最新落盘的值。 @returns {Promise<T>} */
  read() {
    const next = this.queue.then(() => this.load())
    this.queue = next.catch(() => {})
    return next
  }

  /**
   * @template R
   * @param {(current: T) => Promise<{ value?: T, result: R }> | { value?: T, result: R }} mutation
   *   返回 value 时原子写入；不返回 value 表示只读（例如校验失败）
   * @returns {Promise<R>}
   */
  mutate(mutation) {
    const next = this.queue.then(async () => {
      const current = await this.load()
      const { value, result } = await mutation(structuredClone(current))
      if (value !== undefined) {
        const bytes = Buffer.byteLength(JSON.stringify(value, null, 2))
        if (bytes > this.maxBytes)
          throw Object.assign(new Error('document too large'), { code: 'EFBIG' })
        await writeJsonAtomic(this.path, value, { mode: 0o600 })
        this.cache = value
      }
      return result
    })
    this.queue = next.catch(() => {})
    return next
  }
}
