// 手机会话页代理条（A4 改为抽屉前仍在用）的纯推导：条目顺序与连按切换时的“当前会话”。
// 桌面泳道条的宽度预算与四步收缩已随泳道条退役（规格 v2：并行的代理在应用侧栏里）。
import type { BoardLane } from '@shared/session-board-status.mjs'

// 一次已发出、界面还没换过去的代理切换：路由在 transition 里提交，URL 已是 to，界面仍是 from。
export type PendingStripSwitch = { from: string; to: string }

// 切换代理时的“当前会话”：上一次切换还没提交（界面仍停在 from）就以它的目标为起点，
// 否则连按 ⌥↓⌥↑ 会从旧会话再算邻居（落到别的代理），回到原会话也会被当成“已在这里”丢掉。
export function effectiveStripCurrent(currentId: string, pending: PendingStripSwitch | null) {
  return pending && pending.from === currentId ? pending.to : currentId
}

// 手机代理条的顺序：需要你 → 运行中 → 排队 → 已完成（与原泳道条从左到右一致）。
const STRIP_LANE_ORDER: readonly BoardLane[] = ['attention', 'running', 'queued', 'done']

export function stripOrder<T extends { id: string; lane: BoardLane }>(items: readonly T[]): T[] {
  return STRIP_LANE_ORDER.flatMap((lane) => items.filter((item) => item.lane === lane))
}
