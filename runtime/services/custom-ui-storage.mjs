// 组件存储：ctx.storage.get / set / remove 的落盘端（计划 §3.8）。
// 每个组件一个 custom-ui-data/<id>.json = {version:1, values:{[key]: JSON}}，放在被监听、被服务的
// custom-ui/ 之外（写存储不会触发热重载）。写入走原子写，按组件串行（SerialJsonDocument 一文件一条链）。
// 配额按整个文件序列化后的字节数计算，超出时拒绝且不落盘。
import { join } from 'node:path'
import { toWellFormed } from '../../shared/custom-ui-manifest.mjs'
import { CustomUiError } from './custom-ui-error.mjs'
import { SerialJsonDocument } from './custom-ui-files.mjs'

export const STORAGE_KEY_PATTERN = /^[a-zA-Z0-9][a-zA-Z0-9._:-]{0,127}$/
const MAX_KEYS = 1000
const MAX_DEPTH = 32

/** @typedef {{ version: 1, values: Record<string, unknown> }} StorageDocument */

/** @param {unknown} value @returns {value is Record<string, unknown>} */
function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

/**
 * 存储值来自 JSON.parse（请求体），这里再保证严格 JSON：孤立代理项替换为 U+FFFD、拒绝过深的嵌套。
 * @param {unknown} value
 * @param {number} [depth]
 * @returns {unknown}
 */
function strictJson(value, depth = 0) {
  if (depth > MAX_DEPTH) throw new CustomUiError('component_storage_value_invalid', 400)
  if (value === null || typeof value === 'boolean') return value
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) throw new CustomUiError('component_storage_value_invalid', 400)
    return value
  }
  if (typeof value === 'string') return toWellFormed(value)
  if (Array.isArray(value)) return value.map((item) => strictJson(item, depth + 1))
  if (isRecord(value)) {
    /** @type {Record<string, unknown>} */
    const result = {}
    for (const [key, item] of Object.entries(value))
      result[toWellFormed(key)] = strictJson(item, depth + 1)
    return result
  }
  throw new CustomUiError('component_storage_value_invalid', 400)
}

/** @param {unknown} raw @returns {StorageDocument | null} */
function parseDocument(raw) {
  if (!isRecord(raw) || raw.version !== 1 || !isRecord(raw.values)) return null
  /** @type {Record<string, unknown>} */
  const values = {}
  for (const [key, value] of Object.entries(raw.values)) {
    if (STORAGE_KEY_PATTERN.test(key)) values[key] = value
  }
  return { version: 1, values }
}

/** @param {StorageDocument} document */
function documentBytes(document) {
  // 与 writeJsonAtomic 落盘的格式一致（两空格缩进 + 末尾换行），配额按真实文件大小算。
  return Buffer.byteLength(JSON.stringify(document, null, 2)) + 1
}

/** @param {unknown} key @returns {string} */
export function parseStorageKey(key) {
  if (typeof key !== 'string' || !STORAGE_KEY_PATTERN.test(key))
    throw new CustomUiError('component_storage_key_invalid', 400)
  return key
}

export class CustomUiStorage {
  /** @param {{ root: string, now?: () => number }} options */
  constructor({ root, now = Date.now }) {
    this.root = root
    this.now = now
    /** @type {Map<string, SerialJsonDocument<StorageDocument>>} */
    this.documents = new Map()
  }

  /** @param {string} id */
  document(id) {
    let document = this.documents.get(id)
    if (!document) {
      document = new SerialJsonDocument({
        path: join(this.root, `${id}.json`),
        empty: () => ({ version: 1, values: {} }),
        parse: parseDocument,
        now: this.now,
      })
      this.documents.set(id, document)
    }
    return document
  }

  /** @param {string} id @param {unknown} rawKey */
  async get(id, rawKey) {
    const key = parseStorageKey(rawKey)
    const current = await this.document(id).read()
    if (!Object.hasOwn(current.values, key))
      throw new CustomUiError('component_storage_missing', 404)
    return { key, value: current.values[key] }
  }

  /**
   * @param {string} id
   * @param {unknown} rawKey
   * @param {unknown} input 请求体 {value}
   * @param {number} quotaBytes
   */
  async set(id, rawKey, input, quotaBytes) {
    const key = parseStorageKey(rawKey)
    if (!isRecord(input) || !Object.hasOwn(input, 'value') || Object.keys(input).length !== 1)
      throw new CustomUiError('component_request_invalid', 400)
    const value = strictJson(input.value)
    return this.document(id).mutate((current) => {
      if (!Object.hasOwn(current.values, key) && Object.keys(current.values).length >= MAX_KEYS)
        throw new CustomUiError('component_storage_quota', 413, { quotaBytes })
      const next = {
        version: /** @type {const} */ (1),
        values: { ...current.values, [key]: value },
      }
      const usedBytes = documentBytes(next)
      if (usedBytes > quotaBytes)
        throw new CustomUiError('component_storage_quota', 413, { usedBytes, quotaBytes })
      return { value: next, result: { ok: true, usedBytes, quotaBytes } }
    })
  }

  /** @param {string} id @param {unknown} rawKey */
  async remove(id, rawKey) {
    const key = parseStorageKey(rawKey)
    return this.document(id).mutate((current) => {
      if (!Object.hasOwn(current.values, key)) return { result: { ok: true } }
      const values = { ...current.values }
      delete values[key]
      return { value: { version: /** @type {const} */ (1), values }, result: { ok: true } }
    })
  }

  /** 组件删除（存储文件已随组件移入回收站）后丢掉缓存，避免旧值复活。 @param {string} id */
  forget(id) {
    this.documents.delete(id)
  }
}
