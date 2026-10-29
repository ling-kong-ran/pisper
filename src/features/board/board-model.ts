// 看板派生模型：权威会话目录 + 实时覆盖层 → 带泳道、状态、危险等级的卡片数据。
// 纯函数，不读写任何 store，方便看板、舰队计量和测试共用同一套推导。
import {
  BOARD_LANES,
  compareBoardItems,
  deriveBoardStatus,
  type ApprovalSeverity,
  type BoardCapabilities,
  type BoardDoneWindow,
  type BoardLane,
  type BoardStatus,
} from '@shared/session-board-status.mjs'
import { sessionWorkspaceKey } from '@/features/chat/session-workspaces'
import type { SessionSummary } from '@/types/chat'
import { truncateCodePoints, type BoardActivity, type BoardApproval } from './board-api'
import { patchedSession, type BoardOverlayState } from './board-store'

export type BoardApprovalsState = 'none' | 'loading' | 'ready' | 'error'

export type BoardItem = {
  id: string
  session: SessionSummary
  title: string
  cwd: string
  workspaceKey: string
  lane: BoardLane
  status: BoardStatus
  // 失败已被打开、标记已处理或重试过：“失败 · 已查看”，留在已完成泳道。
  handled: boolean
  // 等待 / 运行 / 完成的起点（毫秒），用于排序；运行中在降级模式下是首次观察到的时间。
  since: number | null
  // 只有确知的运行开始时间（本客户端发起或 /live 给出），界面据此决定是否显示已运行时长。
  startedAt: number | null
  pinned: boolean
  unread: boolean
  approvals: BoardApproval[]
  approvalsState: BoardApprovalsState
  // 本卡审批的最高危险等级；没有审批详情时为 null。
  severity: ApprovalSeverity | null
  activity: BoardActivity | null
  error: string
  // 运行流由本客户端持有（可以给出实时活动）。
  localRun: boolean
}

export type BoardBuildOptions = {
  now: number
  capabilities?: BoardCapabilities
  doneWindow?: BoardDoneWindow
  timeZoneOffsetMinutes?: number
}

export type BoardOverlaySnapshot = Pick<
  BoardOverlayState,
  'live' | 'localRuns' | 'runningSince' | 'patches'
>

export type BoardFilter = {
  lanes?: readonly BoardLane[]
  projects?: readonly string[]
}

export type BoardProject = { key: string; cwd: string; count: number }

export type FleetAttentionItem = {
  sessionId: string
  title: string
  status: BoardStatus
  since: number | null
  severity: ApprovalSeverity | null
  // 跳转浮层里的一行摘要：审批取完整命令或路径，失败取错误首行；界面自己加前缀文案。
  summary: string
}

export type FleetSummary = {
  running: number
  queued: number
  attention: number
  approvals: number
  questions: number
  failures: number
  longestWaiting: FleetAttentionItem | null
  latest: FleetAttentionItem | null
  attentionItems: FleetAttentionItem[]
}

const MAX_TITLE_CHARS = 120

function timestamp(value: string | null | undefined) {
  if (!value) return null
  const time = Date.parse(value)
  return Number.isFinite(time) ? time : null
}

// 标题：会话名优先；没有会话名时退回首条消息的首句。
export function boardSessionTitle(session: SessionSummary) {
  const name = typeof session.name === 'string' ? session.name.trim() : ''
  if (name) return truncateCodePoints(name, MAX_TITLE_CHARS)
  const first = typeof session.firstMessage === 'string' ? session.firstMessage.trim() : ''
  // 不用后行断言：构建目标是 Safari 16，旧版 WebKit 解析到它会让整个包失效。
  const end = first.search(/[。！？!?\n]|\.\s/u)
  const sentence = end < 0 ? first : first.slice(0, first[end] === '\n' ? end : end + 1)
  return truncateCodePoints(sentence.trim(), MAX_TITLE_CHARS)
}

function highestSeverity(approvals: readonly BoardApproval[]): ApprovalSeverity | null {
  if (!approvals.length) return null
  return approvals.some((approval) => approval.severity === 'critical') ? 'critical' : 'standard'
}

export function buildBoardItems(
  sessions: readonly SessionSummary[],
  overlay: BoardOverlaySnapshot,
  { now, capabilities, doneWindow = 'today', timeZoneOffsetMinutes }: BoardBuildOptions,
): BoardItem[] {
  const items: BoardItem[] = []
  for (const source of sessions) {
    if (!source || typeof source.id !== 'string' || !source.id) continue
    const session = patchedSession(source, overlay.patches)
    const localRun = overlay.localRuns[session.id]
    const live = overlay.live[session.id]
    const exactStart = localRun?.startedAt ?? timestamp(live?.startedAt)
    const derived = deriveBoardStatus(
      {
        ...session,
        startedAt: exactStart ?? overlay.runningSince[session.id] ?? null,
        // 审批详情的 createdAt 比会话修改时间更接近真实的等待起点。
        approvals: live?.approvals,
      },
      { now, capabilities, doneWindow, timeZoneOffsetMinutes },
    )
    if (!derived.lane) continue
    const approvals = derived.status === 'approval' ? (live?.approvals ?? []) : []
    const approvalsState: BoardApprovalsState =
      derived.status !== 'approval'
        ? 'none'
        : !live || (live.status === 'loading' && !approvals.length)
          ? 'loading'
          : live.status === 'error' && !approvals.length
            ? 'error'
            : 'ready'
    items.push({
      id: session.id,
      session,
      title: boardSessionTitle(session),
      cwd: typeof session.cwd === 'string' ? session.cwd : '',
      workspaceKey: sessionWorkspaceKey(session),
      lane: derived.lane,
      status: derived.status,
      handled: derived.handled,
      since: derived.since,
      startedAt: derived.lane === 'running' ? exactStart : null,
      pinned: session.pinned === true,
      unread: session.unread === true,
      approvals,
      approvalsState,
      severity: highestSeverity(approvals),
      activity: derived.lane === 'running' ? (localRun?.activity ?? live?.activity ?? null) : null,
      error: derived.status === 'failed' ? (live?.error ?? '') : '',
      localRun: Boolean(localRun),
    })
  }
  return items.sort(compareBoardItems)
}

export function filterBoardItems(items: readonly BoardItem[], { lanes, projects }: BoardFilter) {
  const laneSet = lanes?.length ? new Set(lanes) : null
  const projectSet = projects?.length ? new Set(projects) : null
  if (!laneSet && !projectSet) return [...items]
  return items.filter(
    (item) =>
      (!laneSet || laneSet.has(item.lane)) && (!projectSet || projectSet.has(item.workspaceKey)),
  )
}

export function groupBoardLanes(items: readonly BoardItem[]): Record<BoardLane, BoardItem[]> {
  const lanes = Object.fromEntries(BOARD_LANES.map((lane) => [lane, [] as BoardItem[]])) as Record<
    BoardLane,
    BoardItem[]
  >
  for (const item of items) lanes[item.lane].push(item)
  return lanes
}

// 项目筛选与“按项目分组”的候选：按看板上出现的工作区统计，顺序由界面结合 workspace-order-store 决定。
export function boardProjects(items: readonly BoardItem[]): BoardProject[] {
  const projects = new Map<string, BoardProject>()
  for (const item of items) {
    const current = projects.get(item.workspaceKey)
    if (current) current.count += 1
    else projects.set(item.workspaceKey, { key: item.workspaceKey, cwd: item.cwd, count: 1 })
  }
  return [...projects.values()]
}

function attentionSummary(item: BoardItem) {
  if (item.status === 'failed') return item.error.split('\n')[0]?.trim() || ''
  const approval = item.approvals[0]
  if (!approval) return ''
  return approval.command || approval.path || approval.toolName
}

export function summarizeFleet(items: readonly BoardItem[]): FleetSummary {
  const summary: FleetSummary = {
    running: 0,
    queued: 0,
    attention: 0,
    approvals: 0,
    questions: 0,
    failures: 0,
    longestWaiting: null,
    latest: null,
    attentionItems: [],
  }
  for (const item of items) {
    if (item.lane === 'running') summary.running += 1
    else if (item.lane === 'queued') summary.queued += 1
    else if (item.lane === 'attention') {
      summary.attention += 1
      if (item.status === 'approval') summary.approvals += 1
      else if (item.status === 'question') summary.questions += 1
      else if (item.status === 'failed') summary.failures += 1
      summary.attentionItems.push({
        sessionId: item.id,
        title: item.title,
        status: item.status,
        since: item.since,
        severity: item.severity,
        summary: attentionSummary(item),
      })
    }
  }
  // items 已按泳道规则排好：需要你泳道内等待最久在前。
  summary.longestWaiting = summary.attentionItems[0] ?? null
  summary.latest =
    summary.attentionItems.reduce<FleetAttentionItem | null>(
      (latest, item) =>
        item.since !== null && (latest?.since == null || item.since > latest.since) ? item : latest,
      null,
    ) ?? summary.longestWaiting
  return summary
}
