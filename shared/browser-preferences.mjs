// 只同步真正需要跨随机回环端口保留的界面偏好；一次性请求和预览 URL 不进入持久化文件。
export const BROWSER_PREFERENCE_KEYS = Object.freeze([
  'pisper-ui',
  'pisper-session-context-layout',
  'pisper-composer-toolbar',
  // 旧键保留在恢复白名单中，输入栏 Store 首次读取时迁到新名称。
  'pisper-zcode-composer-toolbar',
  'pisper-workspace-order',
  'pisper-floating-widgets',
  'pisper-floating-placement',
  'pisper-language',
  'pisper-shortcuts',
  'pisper-active-session',
  'pisper-mobile-session-tabs',
  'pisper-terminal-panel',
  'pisper-sponsor-dismissals',
  'pisper-model-onboarding-v1-dismissed',
  'pisper.config.manageConnectionsOpen',
  'pisper.config.visualConnectionsOpen',
  'pisper-web-desktop-pet-position',
])

const allowed = new Set(BROWSER_PREFERENCE_KEYS)
const RETIRED_CHAT_LAYOUT_KEY = 'pisper-chat-layout'
const MAX_VALUE_BYTES = 512 * 1024
export const BROWSER_PREFERENCE_MAX_TOTAL_BYTES = 2 * 1024 * 1024

export class BrowserPreferenceError extends Error {
  constructor() {
    super('browser_preferences_invalid')
    this.code = 'browser_preferences_invalid'
    this.statusCode = 400
  }
}

/** @param {unknown} value @returns {Record<string, unknown>} */
function record(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new BrowserPreferenceError()
  const prototype = Object.getPrototypeOf(value)
  if (prototype !== Object.prototype && prototype !== null) throw new BrowserPreferenceError()
  return /** @type {Record<string, unknown>} */ (value)
}

/** @param {unknown} value */
export function parseBrowserPreferenceUpdates(value) {
  const source = record(value)
  const entries = Object.entries(source)
  if (entries.length > BROWSER_PREFERENCE_KEYS.length) throw new BrowserPreferenceError()
  /** @type {Record<string, string | null>} */
  const updates = Object.create(null)
  let total = 0
  for (const [key, item] of entries) {
    if (!allowed.has(key) || (item !== null && typeof item !== 'string'))
      throw new BrowserPreferenceError()
    if (typeof item === 'string') {
      const bytes = new TextEncoder().encode(item).byteLength
      if (bytes > MAX_VALUE_BYTES) throw new BrowserPreferenceError()
      total += bytes
      if (total > BROWSER_PREFERENCE_MAX_TOTAL_BYTES) throw new BrowserPreferenceError()
    }
    updates[key] = item
  }
  return updates
}

/** @param {unknown} value @param {Record<string, string | null>} updates @param {boolean} [complete] */
export function parseBrowserPreferenceRevisions(value, updates, complete = false) {
  if (value === undefined) return {}
  const source = record(value)
  if (complete && Object.keys(source).length !== Object.keys(updates).length)
    throw new BrowserPreferenceError()
  /** @type {Record<string, number>} */
  const revisions = Object.create(null)
  for (const [key, revision] of Object.entries(source)) {
    if (
      !Object.hasOwn(updates, key) ||
      typeof revision !== 'number' ||
      !Number.isSafeInteger(revision) ||
      revision <= 0
    )
      throw new BrowserPreferenceError()
    revisions[key] = revision
  }
  return revisions
}

/** @param {unknown} value */
export function parseBrowserPreferenceSnapshot(value) {
  const source = record(value)
  if (Object.keys(source).some((key) => !['version', 'values', 'revisions'].includes(key)))
    throw new BrowserPreferenceError()
  if (source.version !== 1) throw new BrowserPreferenceError()
  // 已移除的布局曾保存超过单项上限的大型 JSON，读取旧快照时先剔除，
  // 不能让它阻断其他界面偏好的恢复。
  const rawValues = { ...record(source.values) }
  delete rawValues[RETIRED_CHAT_LAYOUT_KEY]
  const rawRevisions = source.revisions === undefined ? undefined : { ...record(source.revisions) }
  if (rawRevisions) delete rawRevisions[RETIRED_CHAT_LAYOUT_KEY]
  const values = parseBrowserPreferenceUpdates(rawValues)
  const revisions = parseBrowserPreferenceRevisions(rawRevisions, values)
  return { version: 1, values, revisions }
}
