// 群聊页与侧栏群列表什么时候刷新：Runtime 推 room 帧（rooms.events）且事件流在线时，靠帧刷新，
// 只留慢速兜底轮询（帧丢了也不会一直停在旧状态）；旧 Runtime 或事件流断了时照旧勤轮询。纯函数，便于测试。
import type { RoomChange } from '@/features/board/room-changes'

// 推送在线时的兜底轮询间隔。
export const ROOM_FALLBACK_POLL_MS = 25_000
export const ROOM_LIST_FALLBACK_POLL_MS = 30_000
// 没有推送时（旧 Runtime、事件流断了）：群页面有 Bot 在干活 1.5 秒，否则 5 秒；侧栏 3 秒 / 10 秒。
export const ROOM_BUSY_POLL_MS = 1_500
export const ROOM_IDLE_POLL_MS = 5_000
export const ROOM_LIST_BUSY_POLL_MS = 3_000
export const ROOM_LIST_IDLE_POLL_MS = 10_000

type Cadence = { push: boolean; alive: boolean; busy: boolean }

/** 群页面两次轮询之间至少隔多久。 */
export function roomPollDelay({ push, alive, busy }: Cadence) {
  if (push && alive) return ROOM_FALLBACK_POLL_MS
  return busy ? ROOM_BUSY_POLL_MS : ROOM_IDLE_POLL_MS
}

/** 侧栏群列表两次刷新之间至少隔多久。 */
export function roomListPollDelay({ push, alive, busy }: Cadence) {
  if (push && alive) return ROOM_LIST_FALLBACK_POLL_MS
  return busy ? ROOM_LIST_BUSY_POLL_MS : ROOM_LIST_IDLE_POLL_MS
}

/** 这一帧要不要让这个群的页面去取增量：'*'（重连快照）总要；只改了侧栏列表的不用。 */
export function roomChangeAffectsPage(change: RoomChange | '*', roomId: string) {
  if (change === '*') return true
  if (change.roomId !== roomId) return false
  return change.deleted || change.scope.some((item) => item !== 'list') || !change.scope.length
}

// 收到帧后等这么久再取：Bot 连着跑工具时 Runtime 每 250ms 一帧，一帧取一次就成了一个接一个地请求。
// 卡片上的步骤最多晚这么久出现，看起来仍是实时的。
export const ROOM_FRAME_COALESCE_MS = 350

/** 这一帧之后要不要重读群详情（成员状态、读到哪、简报、用量）；只是消息变了的不用。 */
export function roomChangeNeedsDetail(change: RoomChange | '*') {
  if (change === '*') return true
  return change.deleted || change.scope.includes('detail') || !change.scope.length
}

/** 这一帧要不要刷新侧栏群列表。 */
export function roomChangeAffectsList(change: RoomChange | '*') {
  if (change === '*') return true
  return change.deleted || change.scope.includes('list')
}
