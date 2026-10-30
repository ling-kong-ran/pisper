// 侧栏会话分组的纯数据模型（规格 v2 第 2.1、2.2 节）：从看板派生的卡片（buildBoardItems 的结果）推导
// 需要你 → 运行中 → 排队 → 置顶 → 今天完成五组，以及窄轨计数、⌥A / ⌥↑⌥↓ 用的顺序。
// 泳道、状态、危险等级都沿用 shared/session-board-status.mjs 与 board-model.ts 的推导，这里只做分组、
// 组内排序、截断和“需要你”原因；不读写任何 store，不含文案（原因以 i18n 键给出），方便侧栏、手机抽屉和测试共用。
import {
  isWithinDoneWindow,
  type BoardCapabilities,
  type BoardStatus,
} from '@shared/session-board-status.mjs'
import type { StatusGlyphKind } from '@/components/app/status-glyph-kinds'
import type { BoardItem } from './board-model'
import { boardStatusGlyph } from './card-presentation'

export const SIDEBAR_GROUPS = ['attention', 'running', 'queued', 'pinned', 'done'] as const
export type SidebarGroupId = (typeof SIDEBAR_GROUPS)[number]

// 规格 2.2 的长列表规则：运行中超过 8 个显示前 8 个和“还有 N 个 →”（去全部代理）；
// 今天完成最多 10 行，之后是“全部 →”（去历史会话）。需要你永不折叠也不截断。
export const SIDEBAR_RUNNING_LIMIT = 8
export const SIDEBAR_DONE_LIMIT = 10

// 侧栏只用到卡片的这些字段；question / queuePosition 是 A5 / A9 期才有的数据，缺省时按“未知”处理。
export type SidebarSourceItem = Pick<
  BoardItem,
  | 'id'
  | 'title'
  | 'lane'
  | 'status'
  | 'since'
  | 'startedAt'
  | 'pinned'
  | 'unread'
  | 'approvals'
  | 'approvalsState'
  | 'severity'
> & {
  // 已归档的会话不出现（看板推导已经排除，这里再兜一层，免得上游换数据源时漏掉）。
  session?: { archived?: boolean } | null
  // 等你回答的问题数（ask_user，A5）；不知道时为空，原因只写“等你回答”。
  questionCount?: number | null
  // 队列位置（sessionQueue，A9）；没有时按组内顺序编号。
  queuePosition?: number | null
}

// “需要你”行第二行的中文原因：可以组合（“高风险批准 · 2 个问题”）。只放原因，不放命令或输出。
export type SidebarReason =
  | { kind: 'failed' }
  | { kind: 'approval'; critical: boolean }
  | { kind: 'question'; count: number | null }

// 快捷处理：普通批准一步“拒绝 / 允许”；高风险、提问、失败以及审批详情还没到的，一律“打开”进会话处理。
export type SidebarQuickAction = { kind: 'decide'; approvalId: string } | { kind: 'open' }

type SidebarRowBase = {
  id: string
  title: string
  glyph: StatusGlyphKind
  status: BoardStatus
}

export type SidebarAttentionRow = SidebarRowBase & {
  group: 'attention'
  // 等待起点（毫秒）；界面显示等待时长“18m”。
  waitingSince: number | null
  reasons: SidebarReason[]
  quickAction: SidebarQuickAction
}

export type SidebarRunningRow = SidebarRowBase & {
  group: 'running'
  // 实时计时的起点：确知的开始时间优先，退回首次观察到运行的时间。
  startedAt: number | null
  // 运行中弧的相位（0–1）：同组多行错开，看起来是各自在转。
  phase: number
}

export type SidebarQueuedRow = SidebarRowBase & {
  group: 'queued'
  // 队列序号（#1 起）。
  position: number
}

export type SidebarPinnedRow = SidebarRowBase & {
  group: 'pinned'
  // 相对时间的参照（最近一次活动或完成）。
  at: number | null
}

export type SidebarDoneRow = SidebarRowBase & {
  group: 'done'
  // 完成时刻（14:05）。
  finishedAt: number | null
  // 未读：标题后加小圆点。
  unread: boolean
}

export type SidebarRow =
  SidebarAttentionRow | SidebarRunningRow | SidebarQueuedRow | SidebarPinnedRow | SidebarDoneRow

export type SidebarGroup<Row extends SidebarRow = SidebarRow> = {
  id: SidebarGroupId
  // 组内全部数量（标题上的计数），不受截断影响。
  count: number
  rows: Row[]
  // 截断后没显示的数量：运行中“还有 N 个 →”、今天完成“全部 →”。
  hidden: number
}

export type SidebarModel = {
  // 只含要显示的分组：需要你与运行中永远在（计数为 0 也要让窄轨与标题可见），
  // 排队只在 sessionQueue 能力且有排队时、置顶只在有置顶且不在上面几组时、今天完成只在有完成时出现。
  groups: SidebarGroup[]
  counts: Record<SidebarGroupId, number>
  // 侧栏的可见顺序：⌥↑ / ⌥↓ 按它切换会话，↑ / ↓ 在行间移动。
  order: string[]
  // ⌥A 聚焦的行：等待最久的需要你。
  longestWaitingId: string | null
}

export type SidebarModelOptions = {
  now: number
  capabilities?: BoardCapabilities
  timeZoneOffsetMinutes?: number
}

function finite(value: number | null | undefined) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

// 缺少时间的行排在组末，不把“未知”当成最早或最新；同值按 id 保证稳定。
function byTime<T extends { id: string }>(time: (row: T) => number | null, descending = false) {
  return (left: T, right: T) => {
    const a = finite(time(left))
    const b = finite(time(right))
    if (a !== b) {
      if (a === null) return 1
      if (b === null) return -1
      return descending ? b - a : a - b
    }
    return left.id < right.id ? -1 : left.id > right.id ? 1 : 0
  }
}

export function sidebarReasons(item: SidebarSourceItem): SidebarReason[] {
  const reasons: SidebarReason[] = []
  if (item.status === 'failed') reasons.push({ kind: 'failed' })
  if (item.status === 'approval')
    reasons.push({ kind: 'approval', critical: item.severity === 'critical' })
  const questions = finite(item.questionCount)
  if (item.status === 'question' || (questions !== null && questions > 0))
    reasons.push({
      kind: 'question',
      count: questions !== null && questions > 0 ? questions : null,
    })
  return reasons
}

// 只有“审批详情已到、全部是普通批准、没有同时在等的问题”才能在侧栏一步处理（规格 1.4、3.5）。
// 详情还在加载或加载失败时不知道危险等级，按“打开”处理，绝不替用户放行看不到命令的请求。
export function sidebarQuickAction(item: SidebarSourceItem): SidebarQuickAction {
  const first = item.approvals[0]
  const questions = finite(item.questionCount)
  if (
    item.status === 'approval' &&
    item.approvalsState === 'ready' &&
    item.severity === 'standard' &&
    first &&
    !first.truncated &&
    !(questions !== null && questions > 0)
  )
    return { kind: 'decide', approvalId: first.id }
  return { kind: 'open' }
}

// 原因的 i18n 键与参数（侧栏行、手机抽屉、读屏播报共用）：组合时按“失败 → 批准 → 问题”的顺序用“ · ”连接。
// 问题单独出现写“等你回答 · 2 个问题”；跟在批准后面只写“2 个问题”（规格 2.2：“高风险批准 · 2 个问题”）。
export function sidebarReasonKeys(
  reasons: readonly SidebarReason[],
): { key: string; params?: Record<string, number> }[] {
  return reasons.map((reason, index) => {
    if (reason.kind === 'failed') return { key: 'board:sidebar.reason.failed' }
    if (reason.kind === 'approval')
      return {
        key: reason.critical
          ? 'board:sidebar.reason.criticalApproval'
          : 'board:sidebar.reason.approval',
      }
    if (reason.count === null) return { key: 'board:sidebar.reason.question' }
    return {
      key: index === 0 ? 'board:sidebar.reason.questions' : 'board:sidebar.reason.questionCount',
      params: { count: reason.count },
    }
  })
}

// 相位：黄金分割步进，任意多行都不会落在同一角度附近。
function runningPhase(index: number) {
  return Math.round(((index * 0.381966) % 1) * 1000) / 1000
}

export function buildSidebarModel(
  items: readonly SidebarSourceItem[],
  { now, capabilities, timeZoneOffsetMinutes }: SidebarModelOptions,
): SidebarModel {
  const attention: SidebarSourceItem[] = []
  const running: SidebarSourceItem[] = []
  const queued: SidebarSourceItem[] = []
  const pinned: SidebarSourceItem[] = []
  const done: SidebarSourceItem[] = []
  const seen = new Set<string>()
  for (const item of items) {
    if (!item || typeof item.id !== 'string' || !item.id || seen.has(item.id)) continue
    if (item.session?.archived === true) continue
    seen.add(item.id)
    if (item.lane === 'attention') attention.push(item)
    else if (item.lane === 'running') running.push(item)
    else if (item.lane === 'queued') {
      // 没有 sessionQueue 能力时排队分组隐藏；看板推导本来就不会产生 queued，这里再守一层。
      if (capabilities?.sessionQueue === true) queued.push(item)
    } else if (item.lane === 'done') {
      // 置顶的会话不在上面几组时进“置顶”，不在“今天完成”里重复出现。
      if (item.pinned) pinned.push(item)
      else if (isWithinDoneWindow(item.since, { now, timeZoneOffsetMinutes, doneWindow: 'today' }))
        done.push(item)
    }
  }

  const attentionRows: SidebarAttentionRow[] = attention
    .sort(byTime((item) => item.since))
    .map((item) => ({
      group: 'attention',
      id: item.id,
      title: item.title,
      status: item.status,
      glyph: 'attention',
      waitingSince: finite(item.since),
      reasons: sidebarReasons(item),
      quickAction: sidebarQuickAction(item),
    }))

  // 运行中按开始时间先后（新开始的追加在后，正在看的行不跳位），置顶不改变组内顺序。
  const runningStart = (item: SidebarSourceItem) => finite(item.startedAt) ?? finite(item.since)
  const runningRows: SidebarRunningRow[] = running
    .sort(byTime(runningStart))
    .map((item, index) => ({
      group: 'running',
      id: item.id,
      title: item.title,
      status: item.status,
      glyph: 'run',
      startedAt: runningStart(item),
      phase: runningPhase(index),
    }))

  // 排队按队列位置；没有位置（旧数据）时按进入队列的时间。位置相同或都缺时再比时间，最后按 id 稳定。
  const bySince = byTime<SidebarSourceItem>((item) => item.since)
  const queuedRows: SidebarQueuedRow[] = queued
    .sort((left, right) => {
      const a = finite(left.queuePosition)
      const b = finite(right.queuePosition)
      if (a !== b) return a === null ? 1 : b === null ? -1 : a - b
      return bySince(left, right)
    })
    .map((item, index) => ({
      group: 'queued',
      id: item.id,
      title: item.title,
      status: item.status,
      glyph: 'queued',
      position: finite(item.queuePosition) ?? index + 1,
    }))

  const pinnedRows: SidebarPinnedRow[] = pinned
    .sort(byTime((item) => item.since, true))
    .map((item) => ({
      group: 'pinned',
      id: item.id,
      title: item.title,
      status: item.status,
      glyph: boardStatusGlyph(item),
      at: finite(item.since),
    }))

  const doneRows: SidebarDoneRow[] = done.sort(byTime((item) => item.since, true)).map((item) => ({
    group: 'done',
    id: item.id,
    title: item.title,
    status: item.status,
    glyph: boardStatusGlyph(item),
    finishedAt: finite(item.since),
    unread: item.unread === true,
  }))

  const counts: Record<SidebarGroupId, number> = {
    attention: attentionRows.length,
    running: runningRows.length,
    queued: queuedRows.length,
    pinned: pinnedRows.length,
    done: doneRows.length,
  }
  const group = <Row extends SidebarRow>(
    id: SidebarGroupId,
    rows: Row[],
    limit = Infinity,
  ): SidebarGroup<Row> => ({
    id,
    count: rows.length,
    rows: rows.slice(0, limit),
    hidden: Math.max(0, rows.length - limit),
  })
  const groups: SidebarGroup[] = [
    group('attention', attentionRows),
    group('running', runningRows, SIDEBAR_RUNNING_LIMIT),
  ]
  if (queuedRows.length) groups.push(group('queued', queuedRows))
  if (pinnedRows.length) groups.push(group('pinned', pinnedRows))
  if (doneRows.length) groups.push(group('done', doneRows, SIDEBAR_DONE_LIMIT))

  return {
    groups,
    counts,
    order: groups.flatMap((entry) => entry.rows.map((row) => row.id)),
    longestWaitingId: attentionRows[0]?.id ?? null,
  }
}

// ⌥↑ / ⌥↓：按侧栏可见顺序找上一个 / 下一个会话，到头不循环；当前会话不在侧栏里时从两端开始。
export function adjacentSidebarSession(
  order: readonly string[],
  currentId: string | null,
  direction: 1 | -1,
): string | null {
  if (!order.length) return null
  const index = currentId ? order.indexOf(currentId) : -1
  if (index < 0) return direction === 1 ? order[0] : order[order.length - 1]
  return order[index + direction] ?? null
}
