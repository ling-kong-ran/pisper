// 原生组件的启用状态与来源：<dataDir>/custom-ui-state.json。
// 目录里有、状态里没有的组件是用户手放的（source:'local'），默认启用（负责人决定，规格 5.7）。
import { COMPONENT_ID_PATTERN, SerialJsonDocument } from './custom-ui-files.mjs'

export const COMPONENT_SOURCES = Object.freeze(
  /** @type {const} */ (['agent', 'template', 'local']),
)

/**
 * @typedef {(typeof COMPONENT_SOURCES)[number]} ComponentSource
 * @typedef {{
 *   enabled: boolean,
 *   source: ComponentSource,
 *   boundSessionId?: string,
 *   createdAt: string,
 *   updatedAt: string,
 * }} ComponentStateEntry
 * @typedef {{ version: 1, components: Record<string, ComponentStateEntry> }} ComponentStateFile
 */

/** @param {unknown} value @returns {value is string} */
function isTimestamp(value) {
  return typeof value === 'string' && value.length <= 40 && !Number.isNaN(Date.parse(value))
}

/** @param {unknown} raw @returns {ComponentStateFile | null} */
function parseStateFile(raw) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const source = /** @type {Record<string, unknown>} */ (raw)
  const components = source.components
  if (source.version !== 1 || !components || typeof components !== 'object') return null
  /** @type {ComponentStateFile} */
  const state = { version: 1, components: {} }
  for (const [id, value] of Object.entries(components)) {
    // 单条坏数据只丢这一条：整份文件仍然可用，不必当成损坏。
    if (!COMPONENT_ID_PATTERN.test(id) || !value || typeof value !== 'object') continue
    const entry = /** @type {Record<string, unknown>} */ (value)
    if (typeof entry.enabled !== 'boolean') continue
    const origin = COMPONENT_SOURCES.includes(/** @type {any} */ (entry.source))
      ? /** @type {ComponentSource} */ (entry.source)
      : 'local'
    /** @type {ComponentStateEntry} */
    const normalized = {
      enabled: entry.enabled,
      source: origin,
      createdAt: isTimestamp(entry.createdAt) ? entry.createdAt : new Date(0).toISOString(),
      updatedAt: isTimestamp(entry.updatedAt) ? entry.updatedAt : new Date(0).toISOString(),
    }
    if (typeof entry.boundSessionId === 'string' && /^[\w.:-]{1,200}$/.test(entry.boundSessionId))
      normalized.boundSessionId = entry.boundSessionId
    state.components[id] = normalized
  }
  return state
}

export class CustomUiStateStore {
  /** @param {{ path: string, now?: () => number }} options */
  constructor({ path, now = Date.now }) {
    this.now = now
    /** @type {SerialJsonDocument<ComponentStateFile>} */
    this.document = new SerialJsonDocument({
      path,
      now,
      empty: () => ({ version: 1, components: {} }),
      parse: parseStateFile,
    })
  }

  /** @returns {Promise<Record<string, ComponentStateEntry>>} */
  async all() {
    return (await this.document.read()).components
  }

  /** @param {string} id @returns {Promise<ComponentStateEntry | null>} */
  async get(id) {
    return (await this.all())[id] || null
  }

  /**
   * 合并写入一条状态；没有记录时以 defaults 为底（手放组件第一次被停用时补上 source:'local'）。
   * @param {string} id
   * @param {Partial<Omit<ComponentStateEntry, 'createdAt' | 'updatedAt'>>} patch
   * @returns {Promise<ComponentStateEntry>}
   */
  update(id, patch) {
    return this.document.mutate((state) => {
      const time = new Date(this.now()).toISOString()
      /** @type {Omit<ComponentStateEntry, 'updatedAt'>} */
      const current = state.components[id] || { enabled: true, source: 'local', createdAt: time }
      /** @type {ComponentStateEntry} */
      const next = { ...current, ...patch, updatedAt: time }
      state.components[id] = next
      return { value: state, result: next }
    })
  }

  /** @param {string} id @returns {Promise<ComponentStateEntry | null>} */
  remove(id) {
    return this.document.mutate((state) => {
      const current = state.components[id] || null
      if (!current) return { result: null }
      delete state.components[id]
      return { value: state, result: current }
    })
  }
}
