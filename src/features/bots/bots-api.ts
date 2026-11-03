// Bots 接口：/api/bots 与 /api/bot-tasks。响应按不可信数据校验，关键字段不对整体拒绝；
// 可选字段缺失时给安全的默认值，旧版 Runtime 多出或少了字段都不至于让页面崩掉。
import { requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'
import { isBotCharacter, isBotColor, type BotAvatarValue } from './avatar'

export type BotTaskStatus =
  'queued' | 'preparing' | 'running' | 'review' | 'done' | 'failed' | 'interrupted' | 'discarded'

export type BotActivity = {
  taskId: string
  status: BotTaskStatus
  title: string
  changedFiles: number
  needsYou: boolean
  // 运行中的任务在等你批准或回答；排队中、它的电脑不在线时是 environment（旧 Runtime 没有这个字段）。
  waiting: 'approval' | 'question' | 'environment' | null
  updatedAt: string
}

export type Bot = {
  id: string
  name: string
  avatar: BotAvatarValue
  enabled: boolean
  instructions: string
  executionMode: 'approval-required' | 'workspace-write' | 'full-access'
  delivery: 'reply-only' | 'diff' | 'commit'
  // 工具范围：null 表示用当前启用的全部工具。
  tools: string[] | null
  workspace: { environmentId: string; cwd: string; isolation: 'worktree' | 'shared' }
  threadSessionId: string
  activity: BotActivity | null
}

export type BotTurn = { text: string; at: string; reply?: string }

// 与看板卡片同一份审批摘要；argsTruncated 时完整参数只在会话里看得到。
export type BotWaitingApproval = {
  id: string
  toolName: string
  argsPreview: string
  argsTruncated: boolean
  severity: 'critical' | 'standard'
  reason: string
}

export type BotWaitingQuestion = {
  id: string
  prompt: string
  header: string
  options: string[]
  multiSelect: boolean
  count: number
}

export type BotWaiting = {
  approval: BotWaitingApproval | null
  question: BotWaitingQuestion | null
}

export type BotTask = {
  id: string
  botId: string
  status: BotTaskStatus
  title: string
  turns: BotTurn[]
  sessionId: string | null
  workspace: { kind: 'worktree' | 'shared'; branch?: string; path?: string; state: string } | null
  result: {
    summary: string
    changedFiles: number
    commit?: string
    applied?: boolean
    branch?: string
  } | null
  error: { code: string; message: string } | null
  waiting: BotWaiting | null
  createdAt: string
  updatedAt: string
}

export type BotDiff = {
  files: { status: string; path: string }[]
  diff: string
  truncated: boolean
}

const TASK_STATUSES = new Set([
  'queued',
  'preparing',
  'running',
  'review',
  'done',
  'failed',
  'interrupted',
  'discarded',
])

function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalidResponseError()
  return value as Record<string, unknown>
}

function text(value: unknown, fallback = '') {
  return typeof value === 'string' ? value : fallback
}

function status(value: unknown): BotTaskStatus {
  if (!TASK_STATUSES.has(String(value))) throw invalidResponseError()
  return value as BotTaskStatus
}

function parseActivity(value: unknown): BotActivity | null {
  if (!value) return null
  const input = object(value)
  return {
    taskId: text(input.taskId),
    status: status(input.status),
    title: text(input.title),
    changedFiles: Number(input.changedFiles) || 0,
    needsYou: input.needsYou === true,
    waiting:
      input.waiting === 'approval' ||
      input.waiting === 'question' ||
      input.waiting === 'environment'
        ? input.waiting
        : null,
    updatedAt: text(input.updatedAt),
  }
}

function parseWaiting(value: unknown): BotWaiting | null {
  if (!value) return null
  const input = object(value)
  const approval = input.approval ? object(input.approval) : null
  const question = input.question ? object(input.question) : null
  const waiting: BotWaiting = {
    approval:
      approval && text(approval.id)
        ? {
            id: text(approval.id),
            toolName: text(approval.toolName),
            argsPreview: text(approval.argsPreview),
            argsTruncated: approval.argsTruncated === true,
            // 不认识的等级按高风险处理：宁可多确认一步。
            severity: approval.severity === 'standard' ? 'standard' : 'critical',
            reason: text(approval.reason),
          }
        : null,
    question:
      question && text(question.id)
        ? {
            id: text(question.id),
            prompt: text(question.prompt),
            header: text(question.header),
            options: Array.isArray(question.options)
              ? question.options.filter((option): option is string => typeof option === 'string')
              : [],
            multiSelect: question.multiSelect === true,
            count: Number(question.count) || 1,
          }
        : null,
  }
  return waiting.approval || waiting.question ? waiting : null
}

export function parseBot(value: unknown): Bot {
  const input = object(value)
  const avatar = object(input.avatar ?? {})
  const workspace = object(input.workspace ?? {})
  if (typeof input.id !== 'string' || typeof input.name !== 'string') throw invalidResponseError()
  return {
    id: input.id,
    name: input.name,
    avatar: {
      character: isBotCharacter(avatar.character) ? avatar.character : 'blob',
      color: isBotColor(avatar.color) ? avatar.color : 'blue',
    },
    enabled: input.enabled !== false,
    instructions: text(input.instructions),
    executionMode:
      input.executionMode === 'workspace-write' || input.executionMode === 'full-access'
        ? input.executionMode
        : 'approval-required',
    delivery:
      input.delivery === 'reply-only' || input.delivery === 'commit' ? input.delivery : 'diff',
    tools: Array.isArray(input.tools)
      ? input.tools.filter((tool): tool is string => typeof tool === 'string')
      : null,
    workspace: {
      environmentId: text(workspace.environmentId, 'local'),
      cwd: text(workspace.cwd),
      isolation: workspace.isolation === 'shared' ? 'shared' : 'worktree',
    },
    threadSessionId: text(input.threadSessionId),
    activity: parseActivity(input.activity),
  }
}

export function parseBotTask(value: unknown): BotTask {
  const input = object(value)
  if (typeof input.id !== 'string' || typeof input.botId !== 'string') throw invalidResponseError()
  const turns = Array.isArray(input.turns)
    ? input.turns.map((turn) => {
        const item = object(turn)
        return {
          text: text(item.text),
          at: text(item.at),
          ...(typeof item.reply === 'string' ? { reply: item.reply } : {}),
        }
      })
    : [{ text: text(input.request), at: text(input.createdAt) }]
  const workspace = input.workspace ? object(input.workspace) : null
  const result = input.result ? object(input.result) : null
  const error = input.error ? object(input.error) : null
  return {
    id: input.id,
    botId: input.botId,
    status: status(input.status),
    title: text(input.title),
    turns,
    sessionId: typeof input.sessionId === 'string' && input.sessionId ? input.sessionId : null,
    workspace: workspace
      ? {
          kind: workspace.kind === 'shared' ? 'shared' : 'worktree',
          branch: text(workspace.branch) || undefined,
          path: text(workspace.path) || undefined,
          state: text(workspace.state),
        }
      : null,
    result: result
      ? {
          summary: text(result.summary),
          changedFiles: Number(result.changedFiles) || 0,
          commit: text(result.commit) || undefined,
          applied: result.applied === true,
          branch: text(result.branch) || undefined,
        }
      : null,
    error: error ? { code: text(error.code), message: text(error.message) } : null,
    waiting: parseWaiting(input.waiting),
    createdAt: text(input.createdAt),
    updatedAt: text(input.updatedAt),
  }
}

function parseDiff(value: unknown): BotDiff {
  const input = object(value)
  if (!Array.isArray(input.files) || typeof input.diff !== 'string') throw invalidResponseError()
  return {
    files: input.files.map((file) => {
      const item = object(file)
      return { status: text(item.status), path: text(item.path) }
    }),
    diff: input.diff,
    truncated: input.truncated === true,
  }
}

function one<T>(key: string, parse: (value: unknown) => T) {
  return (value: unknown) => parse(object(value)[key])
}

export function fetchBots(signal?: AbortSignal) {
  return requestJson('/api/bots', {
    signal,
    parse: (value) => {
      const bots = object(value).bots
      if (!Array.isArray(bots)) throw invalidResponseError()
      return bots.map(parseBot)
    },
  })
}

export type BotDraft = {
  name: string
  avatar: BotAvatarValue
  instructions?: string
  workspace: { cwd: string; isolation?: 'worktree' | 'shared' }
}

export function createBot(draft: BotDraft) {
  return requestJson('/api/bots', { method: 'POST', data: draft, parse: one('bot', parseBot) })
}

export function updateBot(id: string, patch: Partial<Omit<Bot, 'id' | 'activity'>>) {
  return requestJson(`/api/bots/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    data: patch,
    parse: one('bot', parseBot),
  })
}

export function sendBotMessage(id: string, message: string) {
  return requestJson(`/api/bots/${encodeURIComponent(id)}/messages`, {
    method: 'POST',
    data: { text: message },
    parse: one('task', parseBotTask),
  })
}

export function fetchBotTasks(botId: string, signal?: AbortSignal) {
  return requestJson(`/api/bot-tasks?botId=${encodeURIComponent(botId)}&limit=50`, {
    signal,
    parse: (value) => {
      const tasks = object(value).tasks
      if (!Array.isArray(tasks)) throw invalidResponseError()
      // 接口按新到旧返回；对话从旧到新排。
      return tasks.map(parseBotTask).reverse()
    },
  })
}

export function fetchBotTaskDiff(taskId: string, signal?: AbortSignal) {
  return requestJson(`/api/bot-tasks/${encodeURIComponent(taskId)}/diff`, {
    signal,
    parse: parseDiff,
  })
}

export function botTaskAction(
  taskId: string,
  action: 'accept' | 'discard' | 'resume' | 'stop',
  data?: { apply?: boolean },
) {
  return requestJson(`/api/bot-tasks/${encodeURIComponent(taskId)}/${action}`, {
    method: 'POST',
    data: data ?? {},
    parse: one('task', parseBotTask),
  })
}

// 任务在 Bot 的会话里等审批或回答：直接调会话的接口，和会话页里处理是同一件事。
export function resolveBotApproval(sessionId: string, approvalId: string, approved: boolean) {
  return requestJson(
    `/api/sessions/${encodeURIComponent(sessionId)}/approvals/${encodeURIComponent(approvalId)}`,
    { method: 'POST', data: { approved }, parse: () => undefined },
  )
}

export function answerBotQuestion(
  sessionId: string,
  questionId: string,
  answer: { selected: string[]; other?: string } | { skip: true },
) {
  return requestJson(
    `/api/sessions/${encodeURIComponent(sessionId)}/questions/${encodeURIComponent(questionId)}`,
    {
      method: 'POST',
      data: 'skip' in answer ? { skip: true } : { answers: [answer] },
      parse: () => undefined,
    },
  )
}

// Bot 的电脑（B6）：本机与已配对的执行节点。
export type BotEnvironment = {
  id: string
  kind: 'local' | 'node'
  name: string
  online: boolean
  roots: string[]
  shell: boolean
  usedBy: { id: string; name: string }[]
  lastSeenAt: string | null
}

// 远程访问的地址与证书指纹：执行节点要连这里。没打开时节点连不上。
export type BotRemoteInfo = { enabled: boolean; endpoints: string[]; fingerprint: string | null }

function parseRemote(value: unknown): BotRemoteInfo {
  const input = value && typeof value === 'object' ? (value as Record<string, unknown>) : {}
  return {
    enabled: input.enabled === true,
    endpoints: Array.isArray(input.endpoints)
      ? input.endpoints.filter(
          (url): url is string => typeof url === 'string' && /^https:\/\//.test(url),
        )
      : [],
    fingerprint: typeof input.fingerprint === 'string' ? input.fingerprint : null,
  }
}

export function parseEnvironment(value: unknown): BotEnvironment {
  const input = object(value)
  if (typeof input.id !== 'string' || !input.id) throw invalidResponseError()
  const capabilities =
    input.capabilities && typeof input.capabilities === 'object'
      ? (input.capabilities as Record<string, unknown>)
      : {}
  return {
    id: input.id,
    kind: input.kind === 'node' ? 'node' : 'local',
    name: text(input.name),
    online: input.online === true,
    roots: Array.isArray(input.roots)
      ? input.roots.filter((root): root is string => typeof root === 'string')
      : [],
    shell: capabilities.shell === true,
    usedBy: Array.isArray(input.usedBy)
      ? input.usedBy.flatMap((item) => {
          const bot = item && typeof item === 'object' ? (item as Record<string, unknown>) : {}
          return typeof bot.id === 'string' && typeof bot.name === 'string'
            ? [{ id: bot.id, name: bot.name }]
            : []
        })
      : [],
    lastSeenAt: typeof input.lastSeenAt === 'string' ? input.lastSeenAt : null,
  }
}

export function fetchEnvironments(signal?: AbortSignal) {
  return requestJson('/api/environments', {
    signal,
    parse: (value) => {
      const input = object(value)
      if (!Array.isArray(input.environments)) throw invalidResponseError()
      return {
        environments: input.environments.map(parseEnvironment),
        remote: parseRemote(input.remote),
      }
    },
  })
}

export function createEnvironmentPairing() {
  return requestJson('/api/environments/pairing', {
    method: 'POST',
    data: {},
    parse: (value) => {
      const input = object(value)
      const pairing = object(input.pairing)
      if (typeof pairing.code !== 'string') throw invalidResponseError()
      return {
        code: pairing.code,
        expiresAt: text(pairing.expiresAt),
        remote: parseRemote(input.remote),
      }
    },
  })
}

export function removeEnvironment(id: string) {
  return requestJson(`/api/environments/${encodeURIComponent(id)}`, {
    method: 'DELETE',
    parse: () => undefined,
  })
}
