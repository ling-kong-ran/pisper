// 全部代理页的分组模型（规格 v2 第 3.2 节，纯函数）：看板卡片（buildBoardItems 的结果，未筛选）→
// 按状态（需要你 → 运行中 → 排队 → 今天完成）或按项目（每个项目一个分区，区内仍按状态）排好的分区，
// 外加页头统计、键盘顺序与逐个处理的范围。泳道、状态与危险等级沿用 board-model / session-board-status 的推导，
// 组内排序与侧栏一致（sidebar-model 的 byTime）：需要你等待最久在前、运行中按开始时间、排队按队列、
// 今天完成按完成时间倒序。不读写 store、不含文案，便于测试。
import {
  BOARD_LANES,
  isWithinDoneWindow,
  type BoardCapabilities,
  type BoardLane,
} from '@shared/session-board-status.mjs'
import type { BoardItem } from './board-model'
import { byTime } from './sidebar-model'

export type OverviewGrouping = 'lane' | 'project'

export type OverviewCounts = Record<BoardLane, number>

export type OverviewStatusSection = {
  kind: 'status'
  id: BoardLane
  items: BoardItem[]
}

export type OverviewProjectSection = {
  kind: 'project'
  key: string
  cwd: string
  counts: OverviewCounts
  items: BoardItem[]
}

export type OverviewSection = OverviewStatusSection | OverviewProjectSection

export type OverviewProject = { key: string; cwd: string }

export type OverviewModel = {
  // 页头统计（筛选前）：代理数、项目数、运行中数。
  total: number
  projectCount: number
  running: number
  // 筛选前各状态的数量（侧栏筛选与空状态判断用）。
  counts: OverviewCounts
  sections: OverviewSection[]
  // 键盘 J / K 与 ↑ / ↓ 的顺序：页面上看得见的行，自上而下（折叠的今天完成不算）。
  order: BoardItem[]
  // 当前筛选下的需要你（按显示顺序），逐个处理在它们之间走。
  attention: BoardItem[]
  // 筛选前的全部需要你（等待最久在前）。
  attentionAll: BoardItem[]
  // 有代理，但筛选后一行都没有。
  filteredEmpty: boolean
}

export type OverviewOptions = {
  now: number
  groupBy: OverviewGrouping
  capabilities?: BoardCapabilities
  // 状态筛选（空数组表示全部）与项目筛选（工作区键，空数组表示全部），都来自 pisper-board-view。
  lanes?: readonly BoardLane[]
  projects?: readonly string[]
  // 按项目分组时的项目顺序（workspace-order-store 排好的）；没列到的项目按出现顺序排在后面。
  projectOrder?: readonly OverviewProject[]
  // 今天完成折叠时，它的行不进键盘顺序。
  doneCollapsed?: boolean
  timeZoneOffsetMinutes?: number
}

// 超过这个行数改用虚拟列表（规格 3.3），其余直接渲染（行高不一，直接渲染更稳）。
export const OVERVIEW_VIRTUALIZE_THRESHOLD = 30

export function overviewShouldVirtualize(rows: number) {
  return rows > OVERVIEW_VIRTUALIZE_THRESHOLD
}

const STATUS_RANK: Record<BoardLane, number> = { attention: 0, running: 1, queued: 2, done: 3 }

function emptyCounts(): OverviewCounts {
  return { attention: 0, running: 0, queued: 0, done: 0 }
}

function finite(value: number | null | undefined) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

const byWait = byTime<BoardItem>((item) => item.since)
const byStart = byTime<BoardItem>((item) => finite(item.startedAt) ?? finite(item.since))
const byFinish = byTime<BoardItem>((item) => item.since, true)

// 组内固定排序（规格 3.2）；排队还没有队列位置（sessionQueue 在 A9），先按进入队列的时间。
export function compareOverviewItems(left: BoardItem, right: BoardItem) {
  const rank = STATUS_RANK[left.lane] - STATUS_RANK[right.lane]
  if (rank) return rank
  if (left.lane === 'running') return byStart(left, right)
  if (left.lane === 'done') return byFinish(left, right)
  return byWait(left, right)
}

// 全部代理上出现的代理：未归档；排队只在 sessionQueue 能力下；完成的只要今天完成的
// （置顶但更早完成的会话在侧栏“置顶”分组，不冒充“今天完成”）。
export function overviewItems(
  items: readonly BoardItem[],
  {
    now,
    capabilities,
    timeZoneOffsetMinutes,
  }: Pick<OverviewOptions, 'now' | 'capabilities' | 'timeZoneOffsetMinutes'>,
) {
  const seen = new Set<string>()
  return items.filter((item) => {
    if (!item || !item.id || seen.has(item.id)) return false
    seen.add(item.id)
    if (item.session?.archived === true) return false
    if (item.lane === 'queued') return capabilities?.sessionQueue === true
    if (item.lane === 'done')
      return isWithinDoneWindow(item.since, { now, timeZoneOffsetMinutes, doneWindow: 'today' })
    return true
  })
}

export function buildOverviewModel(
  source: readonly BoardItem[],
  options: OverviewOptions,
): OverviewModel {
  const items = overviewItems(source, options).sort(compareOverviewItems)
  const laneFilter = options.lanes?.length ? new Set(options.lanes) : null
  const projectFilter = options.projects?.length ? new Set(options.projects) : null
  const visible = items.filter(
    (item) =>
      (!laneFilter || laneFilter.has(item.lane)) &&
      (!projectFilter || projectFilter.has(item.workspaceKey)),
  )
  const counts = emptyCounts()
  for (const item of items) counts[item.lane] += 1
  const projectKeys = new Set(items.map((item) => item.workspaceKey))

  const sections: OverviewSection[] = []
  if (options.groupBy === 'project') {
    const order = [...(options.projectOrder ?? [])]
    const known = new Set(order.map((project) => project.key))
    for (const item of visible)
      if (!known.has(item.workspaceKey)) {
        known.add(item.workspaceKey)
        order.push({ key: item.workspaceKey, cwd: item.cwd })
      }
    for (const project of order) {
      const rows = visible.filter((item) => item.workspaceKey === project.key)
      if (!rows.length) continue
      const projectCounts = emptyCounts()
      for (const item of rows) projectCounts[item.lane] += 1
      sections.push({
        kind: 'project',
        key: project.key,
        cwd: project.cwd,
        counts: projectCounts,
        items: rows,
      })
    }
  } else {
    for (const lane of BOARD_LANES) {
      const rows = visible.filter((item) => item.lane === lane)
      // 需要你与运行中永远有分组标题（为 0 时写一句安静的空状态，和侧栏一致）；排队、今天完成没有就不出现。
      const always = lane === 'attention' || lane === 'running'
      if (laneFilter && !laneFilter.has(lane)) continue
      if (!rows.length && !always) continue
      sections.push({ kind: 'status', id: lane, items: rows })
    }
  }

  const order = sections.flatMap((section) =>
    section.kind === 'status' && section.id === 'done' && options.doneCollapsed
      ? []
      : section.items,
  )
  return {
    total: items.length,
    projectCount: projectKeys.size,
    running: counts.running,
    counts,
    sections,
    order,
    attention: order.filter((item) => item.lane === 'attention'),
    attentionAll: items.filter((item) => item.lane === 'attention'),
    filteredEmpty: items.length > 0 && visible.length === 0,
  }
}

export type OverviewFilterCounts = {
  // 全部代理上出现的代理总数（侧栏“全部”）。
  total: number
  // 各状态的数量（侧栏状态筛选）。
  lanes: OverviewCounts
  // 每个项目（工作区键）的代理数（侧栏项目筛选）；没有代理的项目不在表里，按 0 显示。
  projects: Readonly<Record<string, number>>
}

// 全部代理页侧栏筛选上的计数（规格 2.3）：与主区同一份“出现在全部代理上的代理”，都按筛选前计——
// 筛选是“看哪一部分”，计数告诉你每一部分有多少，点了状态筛选后项目计数不跟着变，免得数字来回跳。
export function overviewFilterCounts(
  source: readonly BoardItem[],
  options: Pick<OverviewOptions, 'now' | 'capabilities' | 'timeZoneOffsetMinutes'>,
): OverviewFilterCounts {
  const lanes = emptyCounts()
  const projects: Record<string, number> = {}
  const items = overviewItems(source, options)
  for (const item of items) {
    lanes[item.lane] += 1
    projects[item.workspaceKey] = (projects[item.workspaceKey] ?? 0) + 1
  }
  return { total: items.length, lanes, projects }
}
