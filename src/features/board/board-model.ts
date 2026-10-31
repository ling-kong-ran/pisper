// 看板派生模型：权威会话目录 + 实时覆盖层 → 带泳道、状态、危险等级的卡片数据。
// 纯函数，不读写任何 store，方便看板、舰队计量和测试共用同一套推导。
// 第 2 期 Runtime 的看板条目（feed）就绪时，看板成员与 card 以条目为准（事件流 / 看板视图），
// 目录里只补上本客户端刚派发、条目还没到的会话；旧 Runtime 时完全按第 1 期规则从目录推导。
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
import { parsePendingQuestion, type PendingQuestion } from '@/features/chat/session-questions'
import type { SessionSummary } from '@/types/chat'
import {
  truncateCodePoints,
  type BoardActivity,
  type BoardApproval,
  type BoardCardInfo,
  type BoardChangeStats,
} from './board-api'
import type { BoardFeedState } from './board-feed'
import { boardActionKey, patchedSession, type BoardOverlayState } from './board-store'

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
  // 这张卡片有第 2 期 card 摘要；没有时界面按降级形态显示（不放空的尾巴框、退回“N 条消息”）。
  hasCard: boolean
  // 运行中与失败后的等宽尾巴（最后 ≤3 行，已去掉控制字符）。
  tail: string[]
  // 已完成卡片的结果摘要：最后一条回复的首行。
  preview: string
  // 改动统计（写入类工具结束时 Runtime 缓存）；没有时已完成卡片再按需读 /change-summary。
  changeStats: BoardChangeStats | null
  // 上下文用量百分比；没有时元信息行退回“N 条消息”。
  contextPct: number | null
  // 子代理汇总（排队、启动与运行中 / 总数）。
  agents: { running: number; total: number } | null
  // 代理的提问（ask_user，A5）：会话摘要里最早的一组待回答问题；只在状态是“等你回答”时有值。
  question: PendingQuestion | null
  // 这组提问里的问题数（侧栏原因“等你回答 · N 个问题”）。
  questionCount: number | null
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
> & {
  // 第 2 期看板条目；缺省或未就绪时按第 1 期规则从目录推导。
  feed?: Pick<BoardFeedState, 'ready' | 'entries'>
  // 进行中的动作：正在提交决定的审批不再从晚到的 card 里冒出来。
  pending?: BoardOverlayState['pending']
}

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

// card 里被截断的审批换成 /live 取到的完整详情（同一 id）；成员与顺序以 card 为准。
function mergeCardApprovals(card: BoardCardInfo, live: BoardOverlayState['live'][string]) {
  if (!live?.approvals.length || !card.approvals.some((approval) => approval.truncated))
    return card.approvals
  const details = new Map(live.approvals.map((approval) => [approval.id, approval]))
  return card.approvals.map((approval) =>
    approval.truncated ? (details.get(approval.id) ?? approval) : approval,
  )
}

function streamingPatch(patches: BoardOverlayState['patches'], sessionId: string) {
  return Boolean(patches[sessionId]?.some((entry) => entry.patch.streaming === true))
}

type BoardSource = { session: SessionSummary; card: BoardCardInfo | null }

// 看板成员：条目就绪时以条目为准，外加本客户端刚派发 / 重试、条目还没到的会话；否则整个目录。
function boardSources(
  sessions: readonly SessionSummary[],
  overlay: BoardOverlaySnapshot,
): BoardSource[] {
  const feed = overlay.feed
  if (!feed?.ready)
    return sessions
      .filter((session) => session && typeof session.id === 'string' && session.id)
      .map((session) => ({ session, card: null }))
  const sources: BoardSource[] = Object.values(feed.entries).map((entry) => ({
    session: entry.session,
    card: entry.card,
  }))
  for (const session of sessions) {
    if (!session || typeof session.id !== 'string' || !session.id || feed.entries[session.id])
      continue
    if (overlay.localRuns[session.id] || streamingPatch(overlay.patches, session.id))
      sources.push({ session, card: null })
  }
  return sources
}

export function buildBoardItems(
  sessions: readonly SessionSummary[],
  overlay: BoardOverlaySnapshot,
  { now, capabilities, doneWindow = 'today', timeZoneOffsetMinutes }: BoardBuildOptions,
): BoardItem[] {
  const items: BoardItem[] = []
  const pending = overlay.pending ?? {}
  for (const { session: source, card } of boardSources(sessions, overlay)) {
    const session = patchedSession(source, overlay.patches)
    const localRun = overlay.localRuns[session.id]
    const live = overlay.live[session.id]
    const exactStart =
      localRun?.startedAt ?? timestamp(card?.startedAt) ?? timestamp(live?.startedAt)
    // 正在提交决定的审批不显示：晚到的帧可能还带着它。
    const cardApprovals = card
      ? mergeCardApprovals(card, live).filter(
          (approval) => !pending[boardActionKey('approve', session.id, approval.id)],
        )
      : null
    const knownApprovals = cardApprovals?.length ? cardApprovals : live?.approvals
    const derived = deriveBoardStatus(
      {
        ...session,
        startedAt: exactStart ?? overlay.runningSince[session.id] ?? null,
        // 审批详情的 createdAt 比会话修改时间更接近真实的等待起点。
        approvals: knownApprovals,
        // card 只取泳道推导需要的结局（已停止、失败 · 已查看）。
        card: card ? { outcome: card.outcome ?? undefined } : undefined,
      },
      { now, capabilities, doneWindow, timeZoneOffsetMinutes },
    )
    if (!derived.lane) continue
    const approvals = derived.status === 'approval' ? (knownApprovals ?? []) : []
    const first = approvals[0]
    const approvalsState: BoardApprovalsState =
      derived.status !== 'approval'
        ? 'none'
        : first && !first.truncated
          ? 'ready'
          : live?.status === 'error'
            ? 'error'
            : 'loading'
    const running = derived.lane === 'running'
    const failed = derived.status === 'failed'
    const question =
      derived.status === 'question'
        ? parsePendingQuestion((session as Record<string, unknown>).question)
        : null
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
      startedAt: running ? exactStart : null,
      pinned: session.pinned === true,
      unread: session.unread === true,
      approvals,
      approvalsState,
      severity: highestSeverity(approvals),
      activity: running ? (localRun?.activity ?? card?.activity ?? live?.activity ?? null) : null,
      error: failed ? card?.error?.message || live?.error || '' : '',
      localRun: Boolean(localRun),
      hasCard: Boolean(card),
      // 尾巴只在运行中与失败时展示（失败的那一行帮助判断）。
      tail: card && (running || failed) ? card.tail : [],
      preview: card && derived.lane === 'done' && !failed ? card.preview : '',
      changeStats: card?.changeStats ?? null,
      contextPct: card?.contextPct ?? null,
      agents: card?.agents ?? null,
      question,
      questionCount: question ? question.questions.length : null,
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

// 项目顺序与启动台、管理项目对话框一致：先按 workspace-order-store 记住的位置，
// 没记过位置的排在后面，按卡片（或会话）数从多到少。
export function sortProjectsByOrder<T extends { key: string; count: number }>(
  projects: readonly T[],
  order: readonly string[],
) {
  const positions = new Map(order.map((key, index) => [key, index]))
  return [...projects].sort(
    (left, right) =>
      (positions.get(left.key) ?? Number.MAX_SAFE_INTEGER) -
        (positions.get(right.key) ?? Number.MAX_SAFE_INTEGER) || right.count - left.count,
  )
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
