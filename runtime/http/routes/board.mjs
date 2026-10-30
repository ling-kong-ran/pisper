// 看板路由：GET /api/board/events（SSE）以及 GET /api/sessions?view=board 的查询校验。
// 查询串全部按不可信输入处理：多值参数限个数与长度，未知泳道与非法数字直接 400。
import { BOARD_LANES, BOARD_DONE_WINDOWS } from '../../../shared/session-board-status.mjs'
import { serializeSsePayload, sseSendGuarded } from '../response.mjs'
import { BoardEventsLimitError } from '../../services/board-event-hub.mjs'
import {
  BOARD_VIEW_DEFAULT_LIMIT,
  BOARD_VIEW_DEFAULT_WINDOW,
  BOARD_VIEW_MAX_LIMIT,
  BoardViewQueryError,
} from '../../services/session-board-view.mjs'

/**
 * @typedef {import('../../services/session-board-view.mjs').BoardViewQuery} BoardViewQuery
 * @typedef {import('../../../shared/session-board-status.mjs').BoardLane} BoardLane
 * @typedef {import('../../../shared/session-board-status.mjs').BoardDoneWindow} BoardDoneWindow
 */

const MAX_CWD_FILTERS = 64
const MAX_CWD_LENGTH = 4096
const MAX_CURSOR_LENGTH = 1024
// 时区偏移（分钟，本地 = UTC + 偏移）：真实世界的范围是 UTC-12 到 UTC+14。
const MAX_TIME_ZONE_OFFSET_MINUTES = 14 * 60

/** @param {string | null} value @param {string} name */
function integerParam(value, name) {
  if (value === null || value === '') return null
  if (!/^-?\d{1,6}$/.test(value)) throw new BoardViewQueryError(`${name} 必须是整数。`)
  return Number(value)
}

/**
 * GET /api/sessions?view=board 的查询参数：cwd（可重复）、lane（可重复或逗号分隔）、limit、cursor、
 * window（today | 7d）、tzOffset（分钟）。
 * @param {URLSearchParams} params
 * @returns {BoardViewQuery}
 */
export function parseBoardViewQuery(params) {
  const cwds = params.getAll('cwd')
  if (cwds.length > MAX_CWD_FILTERS) throw new BoardViewQueryError('cwd 筛选条件过多。')
  for (const cwd of cwds) {
    if (!cwd.trim() || cwd.length > MAX_CWD_LENGTH || cwd.includes('\0'))
      throw new BoardViewQueryError('cwd 筛选条件无效。')
  }
  /** @type {BoardLane[]} */
  const lanes = []
  for (const value of params.getAll('lane').flatMap((item) => item.split(','))) {
    const lane = value.trim()
    if (!lane) continue
    if (!(/** @type {readonly string[]} */ (BOARD_LANES).includes(lane)))
      throw new BoardViewQueryError('lane 只能是 attention、running、queued 或 done。')
    if (!lanes.includes(/** @type {BoardLane} */ (lane)))
      lanes.push(/** @type {BoardLane} */ (lane))
  }
  const requestedLimit = integerParam(params.get('limit'), 'limit')
  if (requestedLimit !== null && requestedLimit < 1)
    throw new BoardViewQueryError('limit 必须大于 0。')
  const limit = Math.min(BOARD_VIEW_MAX_LIMIT, requestedLimit ?? BOARD_VIEW_DEFAULT_LIMIT)
  const cursor = params.get('cursor') || ''
  if (cursor.length > MAX_CURSOR_LENGTH || !/^[A-Za-z0-9_-]*$/.test(cursor))
    throw new BoardViewQueryError('看板游标无效。', 'invalid_board_cursor')
  const window = params.get('window') || BOARD_VIEW_DEFAULT_WINDOW
  if (!(/** @type {readonly string[]} */ (BOARD_DONE_WINDOWS).includes(window)))
    throw new BoardViewQueryError('window 只能是 today 或 7d。')
  const offset = integerParam(params.get('tzOffset'), 'tzOffset')
  if (offset !== null && Math.abs(offset) > MAX_TIME_ZONE_OFFSET_MINUTES)
    throw new BoardViewQueryError('tzOffset 超出范围。')
  return {
    cwds,
    lanes,
    limit,
    cursor,
    doneWindow: /** @type {BoardDoneWindow} */ (window),
    ...(offset !== null ? { timeZoneOffsetMinutes: offset } : {}),
  }
}

/**
 * ?after= 重连参数：非法值按“没有 after”处理（发完整快照），不拒绝连接。
 * @param {string | null} value
 */
export function parseBoardEventsAfter(value) {
  if (!value || !/^\d{1,16}$/.test(value)) return null
  const after = Number(value)
  return Number.isSafeInteger(after) ? after : null
}

// 同一帧会写给所有连接：按数据对象缓存序列化结果，N 个连接只序列化一次。
/** @type {WeakMap<object, string>} */
const serializedFrames = new WeakMap()

/** @param {Record<string, unknown>} data */
function serializeFrame(data) {
  let payload = serializedFrames.get(data)
  if (payload === undefined) {
    payload = serializeSsePayload(data)
    serializedFrames.set(data, payload)
  }
  return payload
}

/**
 * @typedef {{
 *   runtime: { boardEvents?: import('../../services/board-event-hub.mjs').BoardEventHub },
 *   url: URL,
 *   res: import('node:http').ServerResponse,
 *   json: (status: number, value: unknown) => void,
 *   startSse: () => void,
 * }} BoardEventsContext
 */

export const boardRoutes = [
  {
    method: 'GET',
    path: '/api/board/events',
    /** @param {BoardEventsContext} context */
    async handler({ runtime, url, res, json, startSse }) {
      const hub = runtime.boardEvents
      if (!hub) {
        json(409, { error: '当前 Runtime 不支持看板事件流。', capability: 'board.events' })
        return
      }
      const after = parseBoardEventsAfter(url.searchParams.get('after'))
      /** @type {{ ready: Promise<void>, detach: () => void } | null} */
      let connection = null
      /** @type {() => void} */
      let finish = () => {}
      const closed = new Promise((resolve) => {
        finish = () => resolve(undefined)
      })
      try {
        connection = hub.connect({
          after,
          send: (event, data) => sseSendGuarded(res, event, data, null, serializeFrame(data)),
          // 集线器主动断开（连接卡住、Runtime 关闭）：结束响应，客户端带 after 重连。
          close: () => finish(),
        })
      } catch (error) {
        if (!(error instanceof BoardEventsLimitError)) throw error
        json(error.statusCode, { error: error.message, code: error.code })
        return
      }
      startSse()
      // 客户端断开：解除订阅即可，集线器的缓冲与修订号不受影响。
      res.on('close', () => {
        connection?.detach()
        finish()
      })
      if (res.destroyed) {
        connection.detach()
        return
      }
      await closed
      connection.detach()
    },
  },
]
