// 看板泳道推导：前端降级轮询与第 2 期 Runtime 的 card.boardStatus 共用这一份规则。
// 输入来自 /api/sessions 的会话摘要（以后可能附带 card、approvals 等字段），一律按不可信数据处理：
// 未知字段忽略、未知枚举值按缺省处理，任何输入都不抛错，避免一条坏数据拖垮整块看板。

export const BOARD_LANES = Object.freeze(
  /** @type {const} */ (['attention', 'running', 'queued', 'done']),
)
export const BOARD_STATUSES = Object.freeze(
  /** @type {const} */ (['approval', 'question', 'failed', 'running', 'queued', 'done', 'stopped']),
)
export const APPROVAL_SEVERITIES = Object.freeze(/** @type {const} */ (['standard', 'critical']))
export const BOARD_DONE_WINDOWS = Object.freeze(/** @type {const} */ (['today', '7d']))
// 命令守卫 warn 级拦截写入 approvals[].reason 的固定前缀，见
// runtime/services/session-permission-service.mjs 的 permissionRequirement()。
export const COMMAND_GUARD_REASON_PREFIX = '命令守卫拦截'

const DAY_MS = 86_400_000
const DONE_WINDOW_DAYS = { today: 1, '7d': 7 }

/**
 * @typedef {(typeof BOARD_LANES)[number]} BoardLane
 * @typedef {(typeof BOARD_STATUSES)[number]} BoardStatus
 * @typedef {(typeof APPROVAL_SEVERITIES)[number]} ApprovalSeverity
 * @typedef {(typeof BOARD_DONE_WINDOWS)[number]} BoardDoneWindow
 * @typedef {{ askUser?: boolean, sessionQueue?: boolean }} BoardCapabilities
 * @typedef {{
 *   now: number | string | Date,
 *   timeZoneOffsetMinutes?: number,
 *   capabilities?: BoardCapabilities,
 *   doneWindow?: BoardDoneWindow,
 * }} BoardStatusOptions
 * @typedef {{ lane: BoardLane | null, status: BoardStatus, handled: boolean, since: number | null }} BoardStatusResult
 * @typedef {{
 *   id: string,
 *   lane: BoardLane | null,
 *   since?: number | null,
 *   pinned?: boolean,
 *   queuePosition?: number | null,
 * }} BoardSortable
 */

/** @param {unknown} value @returns {Record<string, unknown>} */
function record(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? /** @type {Record<string, unknown>} */ (value)
    : {}
}

/** @param {unknown} value @returns {number | null} */
export function boardTimestamp(value) {
  if (value instanceof Date) {
    const time = value.getTime()
    return Number.isFinite(time) ? time : null
  }
  if (typeof value === 'number') return Number.isFinite(value) ? value : null
  if (typeof value !== 'string' || !value) return null
  const parsed = Date.parse(value)
  return Number.isFinite(parsed) ? parsed : null
}

// 本地日序号：显式偏移（分钟，本地时间 = UTC + 偏移，东八区为 480）让测试与时区无关；
// 未传时按该时刻所在的系统时区换算，跨夏令时也落在正确的本地日期。
/** @param {number} time @param {number | undefined} offsetMinutes */
function localDay(time, offsetMinutes) {
  const offset = Number.isFinite(offsetMinutes)
    ? /** @type {number} */ (offsetMinutes)
    : -new Date(time).getTimezoneOffset()
  return Math.floor((time + offset * 60_000) / DAY_MS)
}

/**
 * 时间戳是否落在“今天”或最近 7 个本地日内。未来时间（Runtime 与客户端时钟偏差）按最近处理。
 * @param {unknown} value
 * @param {{ now: number | string | Date, timeZoneOffsetMinutes?: number, doneWindow?: BoardDoneWindow }} options
 */
export function isWithinDoneWindow(value, { now, timeZoneOffsetMinutes, doneWindow = 'today' }) {
  const time = boardTimestamp(value)
  const current = boardTimestamp(now)
  if (time === null || current === null) return false
  const days = DONE_WINDOW_DAYS[doneWindow] ?? DONE_WINDOW_DAYS.today
  return localDay(current, timeZoneOffsetMinutes) - localDay(time, timeZoneOffsetMinutes) < days
}

/** @param {unknown} value @returns {value is BoardLane} */
export function isBoardLane(value) {
  return typeof value === 'string' && /** @type {readonly string[]} */ (BOARD_LANES).includes(value)
}

/** @param {unknown} value @returns {value is BoardStatus} */
export function isBoardStatus(value) {
  return (
    typeof value === 'string' && /** @type {readonly string[]} */ (BOARD_STATUSES).includes(value)
  )
}

// 规格 3.5 列为 critical 的非 bash 请求：安装全局插件、全局写入（全局技能）、可能提交表单的浏览器交互。
// 旧 Runtime 没有 severity 字段，按工具名与参数降级判断，与第 2 期 Runtime 的 severity 判定保持一致。
const BROWSER_SUBMIT_ACTIONS = new Set(['click', 'type'])

/** @param {string} toolName @param {Record<string, unknown>} args */
function criticalTool(toolName, args) {
  if (toolName === 'plugin_create') return true
  if (toolName === 'skill_create') return args.scope === 'global'
  if (toolName === 'browser_automation')
    return BROWSER_SUBMIT_ACTIONS.has(String(args.action ?? ''))
  return false
}

/**
 * 审批危险等级：新 Runtime 直接给 severity；旧 Runtime 降级判断——命令守卫的 reason 前缀，
 * 或者规格 3.5 列出的高风险工具（plugin_create、全局 skill_create、浏览器点击 / 输入）。
 * 判断不了一律 standard（界面仍须展示完整命令或参数）。未知 severity 取值同样走降级判断。
 * @param {unknown} approval
 * @returns {ApprovalSeverity}
 */
export function approvalSeverity(approval) {
  const value = record(approval)
  if (value.severity === 'critical' || value.severity === 'standard') return value.severity
  const reason = typeof value.reason === 'string' ? value.reason.trimStart() : ''
  if (reason.startsWith(COMMAND_GUARD_REASON_PREFIX)) return 'critical'
  const toolName = typeof value.toolName === 'string' ? value.toolName : ''
  return criticalTool(toolName, record(value.args)) ? 'critical' : 'standard'
}

// 多条审批里取最早的创建时间作为等待起点；没有可用时间时返回 null。
/** @param {unknown} approvals */
function earliestCreatedAt(approvals) {
  if (!Array.isArray(approvals)) return null
  let earliest = null
  for (const approval of approvals) {
    const time = boardTimestamp(record(approval).createdAt)
    if (time !== null && (earliest === null || time < earliest)) earliest = time
  }
  return earliest
}

/** @param {Record<string, unknown>} session @param {Record<string, unknown>} card */
function queueEntry(session, card) {
  const queue = record(card.queue ?? session.queue)
  return Object.keys(queue).length ? queue : null
}

/**
 * 推导单个会话在看板上的泳道与状态。
 * - 已归档，或既不需要你、也没在运行、时间窗内既没完成也没活动、又没置顶的旧会话：lane 为 null（不上看板）。
 * - 没有 askUser / sessionQueue 能力时永不产生 question / queued，即使 Runtime 发来了对应字段。
 * - 失败在打开或标记已处理（unread 变为 false）后离开“需要你”，以“失败 · 已查看”留在已完成泳道。
 * - 当前没有字段能区分中止与正常完成，stopped 只保留在枚举里，暂不产生。
 * @param {unknown} input
 * @param {BoardStatusOptions} options
 * @returns {BoardStatusResult}
 */
export function deriveBoardStatus(input, options) {
  const session = record(input)
  const card = record(session.card)
  const capabilities = record(options?.capabilities)
  const windowOptions = {
    now: options?.now ?? Date.now(),
    timeZoneOffsetMinutes: options?.timeZoneOffsetMinutes,
    doneWindow: options?.doneWindow,
  }
  const archived = session.archived === true
  const pinned = session.pinned === true
  const modified = boardTimestamp(session.modified)
  const completedAt = boardTimestamp(session.lastCompletedAt)
  const reason = session.needsAttention === false ? null : session.attentionReason

  /**
   * @param {BoardLane | null} lane @param {BoardStatus} status @param {number | null} since
   * @param {boolean} [handled]
   * @returns {BoardStatusResult}
   */
  const result = (lane, status, since, handled = false) => ({
    lane: archived ? null : lane,
    status,
    handled,
    since,
  })

  if (reason === 'approval') {
    return result(
      'attention',
      'approval',
      earliestCreatedAt(card.approvals ?? session.approvals) ?? modified,
    )
  }
  if (reason === 'question' && capabilities.askUser === true) {
    const question = record(card.question ?? session.question)
    return result('attention', 'question', boardTimestamp(question.createdAt) ?? modified)
  }
  // 重试失败会话时失败标记要到本轮结束才清除；运行中必须优先于失败。
  if (session.streaming === true) {
    const startedAt = boardTimestamp(card.startedAt ?? session.startedAt)
    return result('running', 'running', startedAt)
  }
  const queue = capabilities.sessionQueue === true ? queueEntry(session, card) : null
  if (queue) return result('queued', 'queued', boardTimestamp(queue.createdAt))

  const failed = reason === 'failure'
  if (failed && session.unread !== false) {
    return result('attention', 'failed', completedAt ?? modified)
  }
  // 空会话（从未发出消息）的 modified 只是创建时间，不算“今天有活动”。
  const messageCount = session.messageCount
  const hasContent = typeof messageCount !== 'number' || messageCount > 0 || completedAt !== null
  const recent =
    isWithinDoneWindow(completedAt, windowOptions) ||
    (hasContent && isWithinDoneWindow(modified, windowOptions))
  return result(
    recent || pinned ? 'done' : null,
    failed ? 'failed' : 'done',
    completedAt ?? modified,
    failed,
  )
}

/** @param {BoardLane | null} lane */
function laneRank(lane) {
  const index = lane === null ? -1 : BOARD_LANES.indexOf(lane)
  return index < 0 ? BOARD_LANES.length : index
}

/** @param {number | null | undefined} value */
function finite(value) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

// 缺少时间的卡片总是排在本泳道末尾，避免把“未知”当成最早或最新。
/** @param {number | null | undefined} left @param {number | null | undefined} right */
function nullsLast(left, right, descending = false) {
  const a = finite(left)
  const b = finite(right)
  if (a === b) return 0
  if (a === null) return 1
  if (b === null) return -1
  return descending ? b - a : a - b
}

/**
 * 泳道内的固定排序（用户不能改）：
 * - 需要你：等待最久在前，不受置顶影响；
 * - 运行中：按开始时间先后，新卡片追加在后，正在看的卡片不跳位；
 * - 排队：按队列位置；
 * - 已完成：按完成时间倒序。
 * 除“需要你”外，同泳道置顶在前。跨泳道按泳道顺序，null 泳道最后；最终以 id 保证稳定。
 * @param {BoardSortable} a
 * @param {BoardSortable} b
 */
export function compareBoardItems(a, b) {
  const lane = laneRank(a.lane) - laneRank(b.lane)
  if (lane) return lane
  if (a.lane !== 'attention') {
    const pinned = Number(b.pinned === true) - Number(a.pinned === true)
    if (pinned) return pinned
  }
  const order =
    a.lane === 'queued'
      ? nullsLast(a.queuePosition, b.queuePosition) || nullsLast(a.since, b.since)
      : nullsLast(a.since, b.since, a.lane === 'done')
  if (order) return order
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0
}

/**
 * @template {BoardSortable} T
 * @param {readonly T[]} items
 * @returns {T[]}
 */
export function sortBoardItems(items) {
  return [...items].sort(compareBoardItems)
}
