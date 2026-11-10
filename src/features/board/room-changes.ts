// @public Bot 群聊变化通知：看板事件流里的 room 帧（只带群 id、最后一条的序号和变化范围，不带正文）
// 被接受后通知监听者；断线重连收到 snapshot 时通知 '*'（期间可能漏掉了帧）。群页面与侧栏群列表据此
// 去取增量，只留慢速兜底轮询。也记下最近一次收到任何帧（含心跳）的时间：事件流断了时页面据此退回勤轮询。
// 这个文件不导入看板数据层；帧来自 Runtime，按不可信 unknown 解析，非法帧丢弃不抛错。

export type RoomChangeScope = 'messages' | 'detail' | 'list' | 'deleted'

export type RoomChange = {
  roomId: string
  lastSeq: number
  scope: RoomChangeScope[]
  deleted: boolean
}

export type RoomChangeListener = (change: RoomChange | '*') => void

const SCOPES: readonly RoomChangeScope[] = ['messages', 'detail', 'list', 'deleted']
// Runtime 每 15 秒发一次心跳：超过这么久什么帧都没收到，就当事件流不在线。
export const ROOM_FEED_STALE_MS = 40_000

const listeners = new Set<RoomChangeListener>()
let lastFrameAt = 0

export function parseRoomChange(data: unknown): RoomChange | null {
  if (data === null || typeof data !== 'object' || Array.isArray(data)) return null
  const input = data as Record<string, unknown>
  const roomId = input.roomId
  if (typeof roomId !== 'string' || !/^room_[A-Za-z0-9_-]{6,40}$/.test(roomId)) return null
  const lastSeq =
    typeof input.lastSeq === 'number' && Number.isSafeInteger(input.lastSeq) && input.lastSeq >= 0
      ? input.lastSeq
      : 0
  const scope = Array.isArray(input.scope)
    ? SCOPES.filter((item) => (input.scope as unknown[]).includes(item))
    : []
  const deleted = input.deleted === true || scope.includes('deleted')
  return { roomId, lastSeq, scope, deleted }
}

/** 订阅群聊变化；返回取消订阅函数。 */
export function subscribeRoomChanges(listener: RoomChangeListener) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** 事件流此刻像不像在线：最近 40 秒内收到过帧（心跳也算）。 */
export function roomFeedAlive(now = Date.now()) {
  return lastFrameAt > 0 && now - lastFrameAt < ROOM_FEED_STALE_MS
}

/** 只由看板 store 在处理每一帧时调用。单个监听者抛错不影响其他监听者与事件流本身。 */
export function notifyRoomFeedEvent(
  event: string,
  data: unknown,
  accepted: boolean,
  now = Date.now(),
) {
  lastFrameAt = now
  if (!accepted || !listeners.size) return
  const change = event === 'snapshot' ? '*' : event === 'room' ? parseRoomChange(data) : null
  if (!change) return
  for (const listener of [...listeners]) {
    try {
      listener(change)
    } catch {
      // 监听者自己的错误不能打断看板事件流的处理。
    }
  }
}

/** 事件流断开（页面隐藏、出错重连前）：不再算在线，页面立刻退回勤轮询。 */
export function markRoomFeedOffline() {
  lastFrameAt = 0
}
