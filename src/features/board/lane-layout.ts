// 泳道布局（纯函数）：哪些泳道展开、哪些收成 56px 竖条、各自分多少宽度。
// 非空泳道按权重分宽，空泳道收成竖条，避免大片空白；已完成泳道可以手动折叠。
// 视图只负责测量容器宽度并把结果写进 grid-template-columns，规则集中在这里便于测试。
import { BOARD_LANES, type BoardLane } from '@shared/session-board-status.mjs'

export const LANE_WEIGHTS: Readonly<Record<BoardLane, number>> = Object.freeze({
  attention: 1.24,
  running: 1.2,
  queued: 0.8,
  done: 0.9,
})
export const LANE_MIN_WIDTH = 260
// 窗口极窄时泳道最小宽度可以让到这里，保证 1080 下四条泳道同时展开也没有横向滚动。
export const LANE_MIN_WIDTH_FLOOR = 200
export const LANE_RAIL_WIDTH = 56
export const LANE_GAP = 12
// 超过这个数量的泳道改用虚拟列表，其余直接渲染（卡片高度不一，直接渲染更稳）。
export const LANE_VIRTUALIZE_THRESHOLD = 30
// 已完成泳道最多展示的卡片数；更多的去“全部历史会话”。
export const DONE_LANE_LIMIT = 20

export type LaneMode = 'expanded' | 'rail'

export type LaneLayoutEntry = {
  lane: BoardLane
  mode: LaneMode
  count: number
  // 竖条的成因：空泳道（点击临时展开）还是手动折叠（点击取消折叠并记住）。
  railReason: 'empty' | 'collapsed' | null
}

export type LaneLayoutInput = {
  counts: Readonly<Record<BoardLane, number>>
  // 当前可见的泳道（例如没有排队能力时不含 queued），按固定顺序。
  lanes?: readonly BoardLane[]
  // 用户手动折叠的泳道（持久化在 pisper-board-view，界面只开放已完成）。
  collapsed?: readonly BoardLane[]
  // 本次会话里点击竖条临时展开的空泳道。
  peeked?: readonly BoardLane[]
}

export function boardLaneLayout({
  counts,
  lanes = BOARD_LANES,
  collapsed = [],
  peeked = [],
}: LaneLayoutInput): LaneLayoutEntry[] {
  return lanes.map((lane) => {
    const count = Math.max(0, counts[lane] ?? 0)
    if (collapsed.includes(lane) && !peeked.includes(lane))
      return { lane, mode: 'rail', count, railReason: 'collapsed' }
    if (count === 0 && !peeked.includes(lane))
      return { lane, mode: 'rail', count, railReason: 'empty' }
    return { lane, mode: 'expanded', count, railReason: null }
  })
}

// 按容器宽度算出展开泳道的最小宽度：优先 260，空间不够时等分剩余空间，但不低于下限。
export function laneMinWidth(entries: readonly LaneLayoutEntry[], containerWidth: number) {
  const expanded = entries.filter((entry) => entry.mode === 'expanded').length
  if (!expanded || !Number.isFinite(containerWidth) || containerWidth <= 0) return LANE_MIN_WIDTH
  const rails = entries.length - expanded
  const available =
    containerWidth - rails * LANE_RAIL_WIDTH - Math.max(0, entries.length - 1) * LANE_GAP
  const share = Math.floor(available / expanded)
  return Math.max(LANE_MIN_WIDTH_FLOOR, Math.min(LANE_MIN_WIDTH, share))
}

// grid-template-columns：展开泳道 minmax(最小宽度, 权重fr)，竖条固定 56px。
// CSS 里各 fr 之和小于 1 时只分走同比例的剩余空间（只展开已完成泳道时 .9fr 会在右侧留白），
// 因此权重和不足 1 时等比放大到 1。
export function laneGridTemplate(entries: readonly LaneLayoutEntry[], containerWidth = 0) {
  const min = laneMinWidth(entries, containerWidth)
  const total = entries
    .filter((entry) => entry.mode === 'expanded')
    .reduce((sum, entry) => sum + LANE_WEIGHTS[entry.lane], 0)
  const scale = total > 0 && total < 1 ? 1 / total : 1
  return entries
    .map((entry) =>
      entry.mode === 'rail'
        ? `${LANE_RAIL_WIDTH}px`
        : `minmax(${min}px, ${Math.round(LANE_WEIGHTS[entry.lane] * scale * 1000) / 1000}fr)`,
    )
    .join(' ')
}

// 已完成泳道只展示前 20 张，其余由“全部历史会话”承接；其他泳道全部展示。
export function visibleLaneItems<T>(lane: BoardLane, items: readonly T[]) {
  return lane === 'done' ? items.slice(0, DONE_LANE_LIMIT) : [...items]
}

export function laneShouldVirtualize(count: number) {
  return count > LANE_VIRTUALIZE_THRESHOLD
}

// 溢出摘要：可视区底部以下还有几张卡片。卡片矩形与滚动容器矩形都用同一坐标系（视口）。
export function hiddenBelowCount(
  cardBottoms: readonly number[],
  viewportBottom: number,
  tolerance = 8,
) {
  let hidden = 0
  for (const bottom of cardBottoms) if (bottom > viewportBottom + tolerance) hidden += 1
  return hidden
}
