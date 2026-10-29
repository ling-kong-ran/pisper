// 看板领域 API：复用聊天领域的会话 API 与统一 HTTP 客户端，只做看板需要的字段裁剪与校验。
// /live 返回 60 条消息等重量级字段，看板只取审批卡需要的部分；响应一律按不可信数据归一化。
import { approvalSeverity, type ApprovalSeverity } from '@shared/session-board-status.mjs'
import { chatApi } from '@/features/chat/chat-api'
import {
  updateSessionOrganization,
  type SessionOrganizationPatch,
} from '@/features/chat/session-organization-api'
import { ApiError } from '@/lib/http'

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
}

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
}
