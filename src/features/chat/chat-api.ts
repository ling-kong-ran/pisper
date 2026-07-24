import { consumeEventStream } from '../../lib/api'
import { requestJson } from '../../lib/http'
import type { ChatMessage, EntityRecord, SandboxStatus, SessionSummary } from '../../types/chat'

export type ApiRecord = EntityRecord
type StreamEventHandler = (event: string, data: ApiRecord) => boolean | void

type SessionListResponse = { sessions: SessionSummary[] }
type MessagePageResponse = EntityRecord & {
  messages: ChatMessage[]
  pageInfo: {
    start: number
    hasMore: boolean
    nextCursor: string | null
  }
}

type ChatConfigResponse = EntityRecord & {
  provider?: string
  model?: string
  providers: Array<
    EntityRecord & {
      id: string
      name?: string
      configured?: boolean
      enabled?: boolean
      models: Array<EntityRecord & { id: string; name?: string; kind?: string }>
    }
  >
}

const sessionPath = (sessionId: string) => `/api/sessions/${encodeURIComponent(sessionId)}`

export const chatApi = {
  listSessions: () => requestJson<SessionListResponse>('/api/sessions'),

  createSession: (name: string) =>
    requestJson<SessionSummary>('/api/sessions', {
      method: 'POST',
      data: { name },
    }),

  getConfig: () => requestJson<ChatConfigResponse>('/api/config'),

  getSandboxStatus: () => requestJson<SandboxStatus>('/api/sandbox/status'),

  installSandbox: () =>
    requestJson<SandboxStatus>('/api/sandbox/install', {
      method: 'POST',
      data: {},
    }),

  getLiveSession: (sessionId: string) =>
    requestJson<MessagePageResponse>(`${sessionPath(sessionId)}/live`),

  getMessages: (sessionId: string, options: { limit: number; before?: string }) => {
    const params = new URLSearchParams({ limit: String(options.limit) })
    if (options.before) params.set('before', options.before)
    return requestJson<MessagePageResponse>(`${sessionPath(sessionId)}/messages?${params}`)
  },

  openStream: async (
    input: {
      sessionId: string
      message: string
      attachments: unknown[]
      goalMode: boolean
    },
    onEvent: StreamEventHandler,
  ) => {
    const response = await fetch('/api/chat', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(input),
    })
    await consumeEventStream(response, onEvent)
  },

  queueInput: (sessionId: string, message: string, behavior: string) =>
    requestJson<ApiRecord>(`${sessionPath(sessionId)}/input`, {
      method: 'POST',
      data: { message, behavior },
    }),

  abort: (sessionId: string) =>
    requestJson<ApiRecord>(`${sessionPath(sessionId)}/abort`, {
      method: 'POST',
      data: {},
    }),

  pauseGoal: (sessionId: string) =>
    requestJson<ApiRecord>(`${sessionPath(sessionId)}/goal`, {
      method: 'PATCH',
      data: { action: 'pause' },
    }),

  updateModel: (sessionId: string, provider: string, model: string) =>
    requestJson<ApiRecord>(`${sessionPath(sessionId)}/model`, {
      method: 'PUT',
      data: { provider, model },
    }),

  updateExecutionMode: (sessionId: string, mode: string) =>
    requestJson<ApiRecord>(`${sessionPath(sessionId)}/execution-mode`, {
      method: 'PUT',
      data: { mode },
    }),

  resolveApproval: (sessionId: string, approvalId: string, approved: boolean) =>
    requestJson<ApiRecord>(
      `${sessionPath(sessionId)}/approvals/${encodeURIComponent(approvalId)}`,
      {
        method: 'POST',
        data: { approved },
      },
    ),

  updateCwd: (sessionId: string, cwd: string) =>
    requestJson<ApiRecord>(`${sessionPath(sessionId)}/cwd`, {
      method: 'PUT',
      data: { cwd },
    }),

  renameSession: (sessionId: string, name: string) =>
    requestJson<ApiRecord>(sessionPath(sessionId), {
      method: 'PATCH',
      data: { name },
    }),
}
