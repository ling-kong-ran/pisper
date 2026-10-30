// 看板领域 API：复用聊天领域的会话 API 与统一 HTTP 客户端，只做看板需要的字段裁剪与校验。
// /live 返回 60 条消息等重量级字段，看板只取审批卡需要的部分；响应一律按不可信数据归一化。
// 第 2 期 Runtime 的看板视图（view=board）与看板事件流（/api/board/events）带 card 摘要，
// 同样逐字段校验、按码点限长：未知字段忽略，未知枚举值按缺省处理，坏条目整条丢弃而不是拖垮看板。
import {
  approvalSeverity,
  BOARD_CARD_OUTCOMES,
  type ApprovalSeverity,
  type BoardCardOutcome,
} from '@shared/session-board-status.mjs'
import { chatApi } from '@/features/chat/chat-api'
import {
  updateSessionOrganization,
  type SessionOrganizationPatch,
} from '@/features/chat/session-organization-api'
import { consumeEventStream } from '@/lib/event-stream'
import { ApiError, requestJson, waitForMobileRuntimeReady } from '@/lib/http'
import type { SessionSummary } from '@/types/chat'

export type BoardActivity =
  | { type: 'model'; stage: string }
  | { type: 'tool'; name: string; summary: string; startedAt: string | null }

export type BoardApproval = {
  id: string
  toolName: string
  toolCallId: string
  // Runtime 已按 safeArgs 清洗过参数；批准前必须能看到完整命令，因此这里不再截断 command。
  args: Record<string, unknown>
  command: string
  path: string
  reason: string
  risk: string
  severity: ApprovalSeverity
  createdAt: string | null
  // 来自 card 的审批没有原始参数，只有 Runtime 按同一格式（“键: 值”逐行）给出的预览文本。
  argsText?: string
  // 预览被 Runtime 截断（card.argsTruncated）：完整参数必须从 /live 取到之后才能批准。
  truncated?: boolean
}

export type BoardChangeStats = {
  added: number | null
  removed: number | null
  files: number | null
  // known | partial | unavailable；客户端只在 known / partial 且有文件时展示。
  status: string
}

// card 的客户端形态：字段全部可缺省，缺了就不显示；boardStatus 只做参考，泳道由客户端重新推导。
export type BoardCardInfo = {
  boardStatus: string
  runId: string
  activity: BoardActivity | null
  startedAt: string | null
  lastActivityAt: string | null
  finishedAt: string | null
  outcome: BoardCardOutcome | null
  error: { message: string; kind: string } | null
  approvals: BoardApproval[]
  queuedInputCount: number
  contextPct: number | null
  changeStats: BoardChangeStats | null
  preview: string
  tail: string[]
  agents: { running: number; total: number } | null
  totalTokens: number | null
}

// 看板数据源里的一条：会话列表字段（与 GET /api/sessions 同形）+ card。
export type BoardFeedItem = { session: SessionSummary; card: BoardCardInfo }

export type BoardViewPage = {
  items: BoardFeedItem[]
  nextCursor: string | null
  revision: number | null
}

export type BoardEventHandler = (event: string, data: unknown) => boolean | void

export type BoardLiveDetail = {
  streaming: boolean
  approvals: BoardApproval[]
  error: string
  activity: BoardActivity | null
  startedAt: string | null
  lastActivityAt: string | null
  finishedAt: string | null
}

export type ApprovalResolution = 'resolved' | 'already-resolved' | 'missing'

export type BoardStreamEvent = (event: string, data: Record<string, unknown>) => boolean | void

const MAX_REASON_CHARS = 400
const MAX_ERROR_CHARS = 2_000
const MAX_SUMMARY_CHARS = 160
// card 字段的客户端上限比 Runtime 规格（8.1）略宽：只防异常数据撑爆界面，不重新定义截断规则。
const MAX_ARGS_PREVIEW_CHARS = 512
const MAX_CARD_ERROR_CHARS = 400
const MAX_PREVIEW_CHARS = 240
const MAX_TAIL_LINES = 3
const MAX_TAIL_LINE_CHARS = 240
const MAX_CARD_APPROVALS = 8
const MAX_LABEL_CHARS = 40
const MAX_RUN_ID_CHARS = 80
// 看板视图分页：每页上限 200，最多读 3 页（与事件流快照 500 条的上限同量级）。
export const BOARD_VIEW_PAGE_LIMIT = 200
export const BOARD_VIEW_MAX_ITEMS = 500
const BOARD_VIEW_MAX_PAGES = 3

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {}
}

function text(value: unknown) {
  return typeof value === 'string' ? value : ''
}

function timestamp(value: unknown) {
  return typeof value === 'string' && Number.isFinite(Date.parse(value)) ? value : null
}

// 按码点截断，避免把代理对或多字节字符切成半个。
export function truncateCodePoints(value: string, limit: number) {
  const points = Array.from(value)
  return points.length > limit ? `${points.slice(0, limit - 1).join('')}…` : value
}

// 一行活动摘要：bash 取命令，文件类工具取路径，其他工具取第一个字符串参数。
export function summarizeToolArgs(toolName: string, args: unknown) {
  const values = record(args)
  const preferred =
    toolName === 'bash'
      ? values.command
      : (values.path ?? values.file_path ?? values.url ?? values.query ?? values.pattern)
  const first =
    typeof preferred === 'string'
      ? preferred
      : Object.values(values).find((value): value is string => typeof value === 'string')
  return truncateCodePoints((first || '').replace(/\s+/g, ' ').trim(), MAX_SUMMARY_CHARS)
}

export function parseBoardActivity(value: unknown): BoardActivity | null {
  const activity = record(value)
  if (activity.type === 'model') return { type: 'model', stage: text(activity.stage) || 'thinking' }
  if (activity.type === 'tool' && text(activity.name)) {
    const name = text(activity.name)
    return {
      type: 'tool',
      name,
      summary: summarizeToolArgs(name, activity.args),
      startedAt: timestamp(activity.startedAt),
    }
  }
  return null
}

export function parseBoardApproval(value: unknown): BoardApproval | null {
  const approval = record(value)
  const id = text(approval.id)
  if (!id) return null
  const args = record(approval.args)
  const fileChange = record(approval.fileChange)
  return {
    id,
    toolName: text(approval.toolName),
    toolCallId: text(approval.toolCallId),
    args,
    command: text(args.command),
    path: text(fileChange.path) || text(args.path) || text(args.file_path),
    reason: truncateCodePoints(text(approval.reason), MAX_REASON_CHARS),
    risk: text(approval.risk),
    severity: approvalSeverity(approval),
    createdAt: timestamp(approval.createdAt),
  }
}

export function parseBoardApprovals(value: unknown): BoardApproval[] {
  if (!Array.isArray(value)) return []
  const seen = new Set<string>()
  const approvals: BoardApproval[] = []
  for (const item of value) {
    const approval = parseBoardApproval(item)
    if (!approval || seen.has(approval.id)) continue
    seen.add(approval.id)
    approvals.push(approval)
  }
  return approvals
}

function finite(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function count(value: unknown) {
  const number = finite(value)
  return number === null ? null : Math.max(0, Math.floor(number))
}

// 尾巴与预览只展示可见字符：去掉控制字符（含 ANSI 转义的 ESC），Runtime 已清洗过，这里只是兜底。
// eslint-disable-next-line no-control-regex
const CONTROL_CHARACTERS = /[\u0000-\u001f\u007f-\u009f]/g

function visibleLine(value: unknown, limit: number) {
  return truncateCodePoints(text(value).replace(CONTROL_CHARACTERS, ' ').trimEnd(), limit)
}

// 非 bash 审批的预览是“键: 值”逐行；文件类工具取 path 行作为命令框里的主要内容，与 /live 的展示一致。
function previewPath(preview: string) {
  const match = /^(?:path|file_path): (.+)$/m.exec(preview)
  return match ? match[1].trim() : ''
}

export function parseBoardCardApproval(value: unknown): BoardApproval | null {
  const approval = record(value)
  const id = text(approval.id)
  if (!id) return null
  const toolName = text(approval.toolName)
  const raw = text(approval.argsPreview)
  const preview = truncateCodePoints(raw, MAX_ARGS_PREVIEW_CHARS)
  const truncated = approval.argsTruncated === true || preview !== raw
  const bash = toolName === 'bash'
  return {
    id,
    toolName,
    toolCallId: '',
    args: {},
    command: bash ? preview : '',
    path: bash ? '' : previewPath(preview),
    reason: truncateCodePoints(text(approval.reason), MAX_REASON_CHARS),
    risk: text(approval.risk),
    // Runtime 已按同一张规则表给出 severity；缺失或未知取值时按 reason / 工具名降级判断。
    severity: approvalSeverity({ severity: approval.severity, reason: approval.reason, toolName }),
    createdAt: timestamp(approval.createdAt),
    argsText: bash ? '' : preview,
    truncated,
  }
}

function parseCardActivity(value: unknown): BoardActivity | null {
  const activity = record(value)
  if (activity.type === 'model')
    return { type: 'model', stage: truncateCodePoints(text(activity.stage), MAX_LABEL_CHARS) }
  if (activity.type === 'tool' && text(activity.name))
    return {
      type: 'tool',
      name: truncateCodePoints(text(activity.name), MAX_LABEL_CHARS),
      summary: truncateCodePoints(
        text(activity.argsSummary).replace(/\s+/g, ' ').trim(),
        MAX_SUMMARY_CHARS,
      ),
      startedAt: timestamp(activity.startedAt),
    }
  return null
}

function parseChangeStats(value: unknown): BoardChangeStats | null {
  const stats = record(value)
  const status = text(stats.status)
  if (!status) return null
  return {
    added: count(stats.added),
    removed: count(stats.removed),
    files: count(stats.files),
    status: truncateCodePoints(status, MAX_LABEL_CHARS),
  }
}

export function parseBoardCard(value: unknown): BoardCardInfo {
  const card = record(value)
  const error = record(card.error)
  const errorMessage = visibleLine(error.message, MAX_CARD_ERROR_CHARS)
  const agents = record(card.agents)
  const running = count(agents.running)
  const total = count(agents.total)
  const contextPct = finite(card.contextPct)
  const outcome = (BOARD_CARD_OUTCOMES as readonly string[]).includes(text(card.outcome))
    ? (card.outcome as BoardCardOutcome)
    : null
  const approvals: BoardApproval[] = []
  const seen = new Set<string>()
  for (const item of Array.isArray(card.approvals) ? card.approvals : []) {
    const approval = parseBoardCardApproval(item)
    if (!approval || seen.has(approval.id)) continue
    seen.add(approval.id)
    approvals.push(approval)
    if (approvals.length >= MAX_CARD_APPROVALS) break
  }
  return {
    boardStatus: truncateCodePoints(text(card.boardStatus), MAX_LABEL_CHARS),
    runId: truncateCodePoints(text(card.runId), MAX_RUN_ID_CHARS),
    activity: parseCardActivity(card.activity),
    startedAt: timestamp(card.startedAt),
    lastActivityAt: timestamp(card.lastActivityAt),
    finishedAt: timestamp(card.finishedAt),
    outcome,
    error: errorMessage
      ? {
          message: errorMessage,
          kind: truncateCodePoints(text(error.kind), MAX_LABEL_CHARS) || 'runtime',
        }
      : null,
    approvals,
    queuedInputCount: count(card.queuedInputCount) ?? 0,
    contextPct: contextPct === null ? null : Math.min(100, Math.max(0, contextPct)),
    changeStats: parseChangeStats(card.changeStats),
    preview: visibleLine(card.lastAssistantPreview, MAX_PREVIEW_CHARS),
    tail: (Array.isArray(card.tail) ? card.tail : [])
      .map((line) => visibleLine(line, MAX_TAIL_LINE_CHARS))
      .filter((line) => line.trim())
      .slice(-MAX_TAIL_LINES),
    agents:
      running !== null && total !== null && total > 0
        ? { running: Math.min(running, total), total }
        : null,
    totalTokens: count(record(card.usage).totalTokens),
  }
}

// 会话行：与默认列表同形（不在这里重新定义列表字段），只要求有 id，并去掉 card 以免把实时摘要写进目录缓存。
export function parseBoardSessionRow(value: unknown): SessionSummary | null {
  const row = record(value)
  const id = text(row.id)
  if (!id) return null
  const session = { ...row, id } as SessionSummary
  delete session.card
  return session
}

// view=board 条目与事件流快照条目同形：{...会话列表字段, card}。
export function parseBoardFeedItem(value: unknown): BoardFeedItem | null {
  const session = parseBoardSessionRow(value)
  return session ? { session, card: parseBoardCard(record(value).card) } : null
}

export function parseBoardRevision(value: unknown) {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null
}

export function parseBoardViewPage(value: unknown): BoardViewPage {
  const page = record(value)
  const items: BoardFeedItem[] = []
  const seen = new Set<string>()
  for (const entry of Array.isArray(page.sessions) ? page.sessions : []) {
    const item = parseBoardFeedItem(entry)
    if (!item || seen.has(item.session.id)) continue
    seen.add(item.session.id)
    items.push(item)
  }
  const cursor = text(page.nextCursor)
  return {
    items,
    nextCursor: cursor && cursor.length <= 1024 ? cursor : null,
    revision: parseBoardRevision(page.revision),
  }
}

// 本地时区偏移（分钟，本地 = UTC + 偏移）：view=board 用它按本地日计算“今天 / 7 天”。
function timeZoneOffsetMinutes() {
  return -new Date().getTimezoneOffset()
}

export function parseBoardLiveDetail(value: unknown): BoardLiveDetail {
  const live = record(value)
  return {
    streaming: live.streaming === true,
    approvals: parseBoardApprovals(live.approvals),
    error: truncateCodePoints(text(live.error), MAX_ERROR_CHARS),
    activity: parseBoardActivity(live.currentActivity),
    startedAt: timestamp(live.startedAt),
    lastActivityAt: timestamp(live.lastActivityAt),
    finishedAt: timestamp(live.finishedAt),
  }
}

export const boardApi = {
  // 显式刷新：与其他读者共用同一个在途请求和同一份缓存。
  listSessions: () => chatApi.listSessions(),

  getLiveDetail: async (sessionId: string, signal?: AbortSignal) =>
    parseBoardLiveDetail(await chatApi.getLiveSession(sessionId, { signal, timeout: 15_000 })),

  // 审批已在别处（TUI、其他设备）处理时，Runtime 返回 alreadyResolved 或 404；两者都不是失败。
  resolveApproval: async (
    sessionId: string,
    approvalId: string,
    approved: boolean,
  ): Promise<ApprovalResolution> => {
    try {
      const result = record(await chatApi.resolveApproval(sessionId, approvalId, approved))
      return result.alreadyResolved === true ? 'already-resolved' : 'resolved'
    } catch (error) {
      if (error instanceof ApiError && error.status === 404) return 'missing'
      throw error
    }
  },

  // 重试会开一条可重挂的 SSE；消费由看板 store 持有并负责中断。
  retry: (sessionId: string, onEvent: BoardStreamEvent, signal: AbortSignal) =>
    chatApi.retryLastTurn(sessionId, onEvent, { signal }),

  updateOrganization: (sessionId: string, patch: SessionOrganizationPatch) =>
    updateSessionOrganization(sessionId, patch),

  // GET /api/sessions?view=board：固定取 7 天窗口（客户端再按自己的时间窗收窄），按游标最多读 3 页。
  listBoardView: async (signal?: AbortSignal): Promise<BoardViewPage> => {
    const items: BoardFeedItem[] = []
    const seen = new Set<string>()
    let cursor = ''
    let revision: number | null = null
    for (let page = 0; page < BOARD_VIEW_MAX_PAGES; page += 1) {
      const params = new URLSearchParams({
        view: 'board',
        window: '7d',
        limit: String(BOARD_VIEW_PAGE_LIMIT),
        tzOffset: String(timeZoneOffsetMinutes()),
      })
      if (cursor) params.set('cursor', cursor)
      const result = await requestJson(`/api/sessions?${params}`, {
        signal,
        timeout: 15_000,
        parse: parseBoardViewPage,
      })
      // 修订号取第一页（读取最早）的：之后接事件流时宁可多补发，也不漏掉翻页期间的变化。
      if (page === 0) revision = result.revision
      for (const item of result.items) {
        if (seen.has(item.session.id)) continue
        seen.add(item.session.id)
        items.push(item)
      }
      cursor = result.nextCursor ?? ''
      if (!cursor || items.length >= BOARD_VIEW_MAX_ITEMS) break
    }
    return { items: items.slice(0, BOARD_VIEW_MAX_ITEMS), nextCursor: cursor || null, revision }
  },

  // GET /api/board/events：长连接 SSE，事件原样交给看板 store 校验。正常结束（EOF）resolve，
  // 连接失败或 HTTP 错误 reject（ApiError 带状态码，429 表示连接数已满，409/404 表示不支持）。
  openBoardEvents: async (
    after: number | null,
    onEvent: BoardEventHandler,
    signal: AbortSignal,
  ): Promise<void> => {
    await waitForMobileRuntimeReady(signal)
    const query = after === null ? '' : `?after=${encodeURIComponent(String(after))}`
    let response: Response
    try {
      response = await fetch(`/api/board/events${query}`, {
        signal,
        headers: { Accept: 'text/event-stream' },
      })
    } catch (error) {
      if (signal.aborted) throw error
      throw new ApiError(error instanceof Error ? error.message : 'network', { kind: 'network' })
    }
    await consumeEventStream<unknown>(response, (event, data) => onEvent(event, data))
  },
}
