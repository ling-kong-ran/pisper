import { consumeEventStream } from '@/lib/api'
import { requestJson } from '@/lib/http'
import type { ChatMessage, EntityRecord, SessionSummary } from '@/types/chat'

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

type CompactionPreferenceResponse = {
  thresholdPercent: number
  minPercent: number
  maxPercent: number
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

export type GitChangesResponse = EntityRecord & {
  vcs?: string
  isRepo: boolean
  gitAvailable?: boolean
  svnAvailable?: boolean
  cwd?: string
  branch?: string
  hasHead?: boolean
  files: Array<{ path: string; status: string }>
  diff: string
  diffTruncated?: boolean
  ahead?: number | null
  error?: string
}

const sessionPath = (sessionId: string) => `/api/sessions/${encodeURIComponent(sessionId)}`

export const chatApi = {
  listSessions: () => requestJson<SessionListResponse>('/api/sessions'),

  createSession: (name: string, cwd = '') =>
    requestJson<SessionSummary>('/api/sessions', {
      method: 'POST',
      data: { name, ...(cwd ? { cwd } : {}) },
    }),

  getConfig: () => requestJson<ChatConfigResponse>('/api/config'),

  updateCompactionPreference: (thresholdPercent: number) =>
    requestJson<CompactionPreferenceResponse>('/api/settings/compaction', {
      method: 'PATCH',
      data: { thresholdPercent },
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
      goalTokenBudget?: number | null
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

  compactSession: (sessionId: string) =>
    requestJson<ApiRecord>(`${sessionPath(sessionId)}/compact`, {
      method: 'POST',
      data: {},
      timeout: 180_000,
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

  setGoalBudget: (sessionId: string, tokenBudget: number) =>
    requestJson<ApiRecord>(`${sessionPath(sessionId)}/goal`, {
      method: 'PATCH',
      data: { action: 'set-budget', tokenBudget },
    }),

  getGitChanges: (sessionId: string) =>
    requestJson<GitChangesResponse>(`${sessionPath(sessionId)}/git/changes`),

  commitGitChanges: (sessionId: string, message: string) =>
    requestJson<GitChangesResponse>(`${sessionPath(sessionId)}/git/commit`, {
      method: 'POST',
      data: { message },
      timeout: 60_000,
    }),

  pushGitChanges: (sessionId: string) =>
    requestJson<GitChangesResponse>(`${sessionPath(sessionId)}/git/push`, {
      method: 'POST',
      data: {},
      timeout: 150_000,
    }),

  revertGitChanges: (sessionId: string) =>
    requestJson<GitChangesResponse>(`${sessionPath(sessionId)}/git/revert`, {
      method: 'POST',
      data: {},
      timeout: 60_000,
    }),

  getVcsChanges: (sessionId: string) =>
    requestJson<GitChangesResponse>(`${sessionPath(sessionId)}/vcs/changes`),

  commitVcsChanges: (sessionId: string, message: string) =>
    requestJson<GitChangesResponse>(`${sessionPath(sessionId)}/vcs/commit`, {
      method: 'POST',
      data: { message },
      timeout: 150_000,
    }),

  pushVcsChanges: (sessionId: string) =>
    requestJson<GitChangesResponse>(`${sessionPath(sessionId)}/vcs/push`, {
      method: 'POST',
      data: {},
      timeout: 150_000,
    }),

  revertVcsChanges: (sessionId: string) =>
    requestJson<GitChangesResponse>(`${sessionPath(sessionId)}/vcs/revert`, {
      method: 'POST',
      data: {},
      timeout: 60_000,
    }),

  updateModel: (sessionId: string, provider: string, model: string) =>
    requestJson<ApiRecord>(`${sessionPath(sessionId)}/model`, {
      method: 'PUT',
      data: { provider, model },
    }),

  getThinkingLevel: (sessionId: string) =>
    requestJson<ApiRecord>(`${sessionPath(sessionId)}/thinking-level`),

  setThinkingLevel: (sessionId: string, level: string) =>
    requestJson<ApiRecord>(`${sessionPath(sessionId)}/thinking-level`, {
      method: 'PUT',
      data: { level },
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
