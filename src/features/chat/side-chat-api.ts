import { requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'
import type { SessionSummary } from '@/types/chat'

export type SideChatResponse = {
  session: SessionSummary | null
  expiresAt: string | null
  created: boolean
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

// 临时会话入口独立校验；未知扩展字段不进入前端状态。
export function decodeSideChatResponse(value: unknown): SideChatResponse {
  if (!record(value) || typeof value.created !== 'boolean') throw invalidResponseError()
  if (value.session === null) {
    if (value.expiresAt !== null || value.created) throw invalidResponseError()
    return { session: null, expiresAt: null, created: false }
  }
  const session = value.session
  if (
    !record(session) ||
    typeof session.id !== 'string' ||
    !session.id ||
    typeof value.expiresAt !== 'string' ||
    !Number.isFinite(Date.parse(value.expiresAt))
  )
    throw invalidResponseError()
  const result: SessionSummary = { id: session.id }
  for (const key of [
    'name',
    'cwd',
    'model',
    'thinkingLevel',
    'executionMode',
    'permissionMode',
    'runMode',
  ] as const) {
    const field = session[key]
    if (field !== undefined && field !== null && typeof field !== 'string')
      throw invalidResponseError()
    if (typeof field === 'string') result[key] = field
  }
  if (session.streaming !== undefined && typeof session.streaming !== 'boolean')
    throw invalidResponseError()
  if (typeof session.streaming === 'boolean') result.streaming = session.streaming
  return { session: result, expiresAt: value.expiresAt, created: value.created }
}

export async function getSideChat(parentId: string, signal?: AbortSignal) {
  return decodeSideChatResponse(
    await requestJson<unknown>(`/api/sessions/${encodeURIComponent(parentId)}/side-chat`, {
      signal,
    }),
  )
}

export async function ensureSideChat(parentId: string, signal?: AbortSignal) {
  const result = decodeSideChatResponse(
    await requestJson<unknown>(`/api/sessions/${encodeURIComponent(parentId)}/side-chat`, {
      method: 'POST',
      signal,
    }),
  )
  if (!result.session) throw invalidResponseError()
  return result
}
