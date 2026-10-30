// 只同步真正需要跨随机回环端口保留的界面偏好；一次性请求和预览 URL 不进入持久化文件。
export const BROWSER_PREFERENCE_KEYS = Object.freeze([
  // ui-store 持有主题/排印；UI/代码数值字号分别默认14/12，旧三档恢复13.5/14/14.5；不保存输入草稿。
  // 旧快照里的 sidebarCollapsed / sidebarWidth 属于已退役的旧侧栏，读取时忽略、下次写入时自然丢弃。
  'pisper-ui',
  // 方向 A 侧栏：收起成窄轨、“今天完成”折叠（src/stores/sidebar-store.ts 是唯一所有者）。
  'pisper-sidebar',
  'pisper-session-context-layout',
  // 会话页检查器：开关、当前标签、宽度与改动 / 终端子视图（规格 4.4）。
  'pisper-session-inspector',
  'pisper-composer-toolbar',
  // 旧键保留在恢复白名单中，输入栏 Store 首次读取时迁到新名称。
  'pisper-zcode-composer-toolbar',
  'pisper-workspace-order',
  // 登记的项目（空项目）：经“新建项目…”选过、可能还没有会话的目录列表，与会话目录按工作区键合并。
  'pisper-registered-projects',
  // 看板视图（分组、项目/状态筛选、折叠泳道、已完成时间窗）与启动台上次的选择。
  'pisper-board-view',
  'pisper-launch-defaults',
  'pisper-floating-widgets',
  'pisper-floating-placement',
  'pisper-language',
  'pisper-shortcuts',
  // 看板单字母快捷键（J/K/H/L、A/D/R、T、G）与应用级绑定分表保存。
  'pisper-board-shortcuts',
  'pisper-active-session',
  // 会话主页：本设备上次停留的会话 id（进入真实会话时写入）。镜像只走本机 /api/local，远程监听返回 404，
  // 因此天然按设备记、不跨设备同步；读出时按不可信输入校验并限长（src/app/session-home.ts）。
  'pisper-last-session',
  'pisper-mobile-session-tabs',
  'pisper-terminal-panel',
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
