// 看板视图（GET /api/sessions?view=board）：在已投影卡片的会话上做筛选、排序与游标分页。
// 纯函数，不读写任何状态；HTTP 层负责把查询串校验成 BoardViewQuery 再交给这里。
import { BOARD_LANES, boardWorkspaceKey } from '../../shared/session-board-status.mjs'

/**
 * @typedef {import('../../shared/session-board-status.mjs').BoardLane} BoardLane
 * @typedef {import('../../shared/session-board-status.mjs').BoardDoneWindow} BoardDoneWindow
 * @typedef {{ id: string, cwd?: unknown, modified?: unknown, card: { boardStatus: string } }} BoardViewItem
 * @typedef {{
 *   cwds: string[],
 *   lanes: BoardLane[],
 *   limit: number,
 *   cursor: string,
 *   doneWindow: BoardDoneWindow,
 *   timeZoneOffsetMinutes?: number,
 * }} BoardViewQuery
 * @typedef {{ rank: number, modified: number, id: string }} BoardCursor
 */

export const BOARD_VIEW_DEFAULT_LIMIT = 100
export const BOARD_VIEW_MAX_LIMIT = 200
// 事件流快照的条数上限：超出时快照带 truncated，客户端再用 view=board 翻页补齐。
export const BOARD_SNAPSHOT_LIMIT = 500
export const BOARD_VIEW_DEFAULT_WINDOW = '7d'
const CURSOR_VERSION = 1
const MAX_CURSOR_ID_LENGTH = 256

export class BoardViewQueryError extends Error {
  /** @param {string} message @param {string} [code] */
  constructor(message, code = 'invalid_board_query') {
    super(message)
    this.name = 'BoardViewQueryError'
    this.code = code
    this.statusCode = 400
  }
}

/** @param {string} status */
function laneRank(status) {
  const index = /** @type {readonly string[]} */ (BOARD_LANES).indexOf(status)
  return index < 0 ? BOARD_LANES.length : index
}

/** @param {unknown} value */
function modifiedTime(value) {
  const time = typeof value === 'string' ? Date.parse(value) : Number.NaN
  return Number.isFinite(time) ? time : 0
}

/** @param {BoardViewItem} item @returns {BoardCursor} */
function sortKey(item) {
  return {
    rank: laneRank(item.card.boardStatus),
    modified: modifiedTime(item.modified),
    id: item.id,
  }
}

// 固定顺序：泳道（需要你 → 运行中 → 排队 → 已完成）、最近修改在前、id 升序。
// 泳道内的展示顺序由客户端用 compareBoardItems 重排；这里只要求分页稳定、需要你的卡片在第一页。
/** @param {BoardCursor} left @param {BoardCursor} right */
function compareKeys(left, right) {
  return (
    left.rank - right.rank ||
    right.modified - left.modified ||
    (left.id < right.id ? -1 : left.id > right.id ? 1 : 0)
  )
}

/** @param {BoardCursor} key */
export function encodeBoardCursor(key) {
  return Buffer.from(JSON.stringify([CURSOR_VERSION, key.rank, key.modified, key.id])).toString(
    'base64url',
  )
}

/**
 * 游标是服务端签发的不透明字符串，但仍来自客户端：解码失败或形状不对一律 400。
 * @param {string} value
 * @returns {BoardCursor}
 */
export function decodeBoardCursor(value) {
  /** @type {unknown} */
  let parsed
  try {
    parsed = JSON.parse(Buffer.from(value, 'base64url').toString('utf8'))
  } catch {
    throw new BoardViewQueryError('看板游标无效。', 'invalid_board_cursor')
  }
  if (
    !Array.isArray(parsed) ||
    parsed.length !== 4 ||
    parsed[0] !== CURSOR_VERSION ||
    !Number.isInteger(parsed[1]) ||
    parsed[1] < 0 ||
    parsed[1] > BOARD_LANES.length ||
    !Number.isFinite(parsed[2]) ||
    typeof parsed[3] !== 'string' ||
    !parsed[3] ||
    parsed[3].length > MAX_CURSOR_ID_LENGTH
  )
    throw new BoardViewQueryError('看板游标无效。', 'invalid_board_cursor')
  return { rank: parsed[1], modified: parsed[2], id: parsed[3] }
}

/**
 * 只保留上看板的条目（boardStatus 不是 idle），按固定顺序排好。
 * @template {BoardViewItem} T
 * @param {readonly T[]} items
 * @returns {T[]}
 */
export function sortBoardViewItems(items) {
  return items
    .filter((item) => item.card.boardStatus !== 'idle')
    .map((item) => ({ item, key: sortKey(item) }))
    .sort((left, right) => compareKeys(left.key, right.key))
    .map(({ item }) => item)
}

/**
 * @template {BoardViewItem} T
 * @param {readonly T[]} items 已投影卡片的会话（含 idle，函数内过滤）
 * @param {Pick<BoardViewQuery, 'cwds' | 'lanes' | 'limit' | 'cursor'>} query
 * @returns {{ sessions: T[], nextCursor: string | null, total: number }}
 */
export function buildBoardView(items, { cwds, lanes, limit, cursor }) {
  const cwdKeys = new Set(cwds.map((cwd) => boardWorkspaceKey(cwd)))
  const laneSet = new Set(lanes)
  const filtered = sortBoardViewItems(items).filter(
    (item) =>
      (!cwdKeys.size || cwdKeys.has(boardWorkspaceKey(item.cwd))) &&
      (!laneSet.size || laneSet.has(/** @type {BoardLane} */ (item.card.boardStatus))),
  )
  const after = cursor ? decodeBoardCursor(cursor) : null
  const start = after ? filtered.findIndex((item) => compareKeys(sortKey(item), after) > 0) : 0
  const remaining = start < 0 ? [] : filtered.slice(start)
  const page = remaining.slice(0, limit)
  const last = page.at(-1)
  return {
    sessions: page,
    nextCursor: last && remaining.length > page.length ? encodeBoardCursor(sortKey(last)) : null,
    total: filtered.length,
  }
}
