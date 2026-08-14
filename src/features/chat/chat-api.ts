import { consumeEventStream } from '@/lib/api'
import { requestJson } from '@/lib/http'
import type { ChatMessage, EntityRecord, ResourceInvocation, SessionSummary } from '@/types/chat'

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

export type SessionTreeNode = {
  id: string
  parentId: string | null
  type: string
  kind: string
  role: string
  text: string
  status: string
  label: string
  timestamp: string
  active: boolean
  leaf: boolean
  branchPoint: boolean
  children: SessionTreeNode[]
}

export type SessionTreeResponse = {
  sessionId: string
  leafId: string | null
  nodeCount: number
  branchCount: number
  streaming: boolean
  roots: SessionTreeNode[]
}

export type SessionTreeNavigationResponse = SessionTreeResponse & {
  cancelled: boolean
  editorText: string | null
}

export type SessionTreeLabelMatch = {
  sessionId: string
  sessionName: string
  sessionCreated: string
  sessionModified: string
  entryId: string
  label: string
  summary: string
  nodeTimestamp: string
  active: boolean
}

export type WorkspaceTrustStatus = {
  cwd: string
  decision: boolean | null
  trusted: boolean
  restricted: boolean
  requiresDecision: boolean
  decisionPath: string
  inherited: boolean
  resources: string[]
}

export type SessionCommand = {
  name: string
  invocation: string
  description: string
  argumentHint: string
  source: 'prompt' | 'skill'
  scope: 'user' | 'project' | 'package' | 'custom'
}

export type SessionCommandsResponse = {
  sessionId: string
  commands: SessionCommand[]
  counts: {
    total: number
    prompts: number
    skills: number
    diagnostics: number
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

  searchSessionTreeLabels: (query: string, limit = 20) => {
    const params = new URLSearchParams({ query, limit: String(limit) })
    return requestJson<{ labels: SessionTreeLabelMatch[] }>(`/api/session-labels?${params}`)
  },

  createSession: (name: string, cwd = '') =>
    requestJson<SessionSummary>('/api/sessions', {
      method: 'POST',
      data: { name, ...(cwd ? { cwd } : {}) },
    }),

  deriveSession: (sessionId: string, boundaryEntryId: string, name: string) =>
    requestJson<SessionSummary>(`${sessionPath(sessionId)}/derive`, {
      method: 'POST',
      data: { boundaryEntryId, name },
    }),

  getSessionTree: (sessionId: string) =>
    requestJson<SessionTreeResponse>(`${sessionPath(sessionId)}/tree`),

  navigateSessionTree: (sessionId: string, targetEntryId: string, summarize: boolean) =>
    requestJson<SessionTreeNavigationResponse>(`${sessionPath(sessionId)}/tree/navigate`, {
      method: 'POST',
      data: { targetEntryId, summarize },
      timeout: 180_000,
    }),

  setSessionTreeLabel: (sessionId: string, entryId: string, label: string) =>
    requestJson<SessionTreeResponse>(
      `${sessionPath(sessionId)}/tree/labels/${encodeURIComponent(entryId)}`,
      { method: 'PUT', data: { label } },
    ),

  getWorkspaceTrust: (sessionId: string) =>
    requestJson<WorkspaceTrustStatus>(`${sessionPath(sessionId)}/workspace-trust`),

  setWorkspaceTrust: (sessionId: string, trusted: boolean) =>
    requestJson<WorkspaceTrustStatus>(`${sessionPath(sessionId)}/workspace-trust`, {
      method: 'PUT',
      data: { trusted },
    }),

  getSessionCommands: (sessionId: string) =>
    requestJson<SessionCommandsResponse>(`${sessionPath(sessionId)}/commands`),

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
      invocation?: ResourceInvocation | null
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

  getSessionWorkflowRuns: (sessionId: string) =>
    requestJson<{ runs: EntityRecord[] }>(`${sessionPath(sessionId)}/workflow-runs`),

  resolveWorkflowApproval: (runId: string, nodeId: string, approved: boolean) =>
    requestJson<ApiRecord>(
      `/api/workflow-runs/${encodeURIComponent(runId)}/approvals/${encodeURIComponent(nodeId)}`,
      { method: 'POST', data: { approved } },
    ),

  stopWorkflowRun: (runId: string) =>
    requestJson<ApiRecord>(`/api/workflow-runs/${encodeURIComponent(runId)}/stop`, {
      method: 'POST',
      data: {},
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
