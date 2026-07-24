import type { EntityRecord, TaskList, TaskListItem, ToolActivity } from '../../types/chat'

export const RUN_INACTIVITY_THRESHOLD_MS = 10_000
export const MAX_CURRENT_ACTIVITIES = 6

const RESEARCH_TOOLS = new Set([
  'read',
  'grep',
  'find',
  'ls',
  'memory_search',
  'get_task_list',
  'browser_automation',
])
const EDIT_TOOLS = new Set(['edit', 'write', 'memory_remember'])
const AGENT_TOOLS = new Set([
  'spawn_agent',
  'list_agents',
  'send_message',
  'followup_task',
  'wait_agent',
  'interrupt_agent',
])

export function agentActivityState(status?: string) {
  if (status === 'queued') return { titleKey: '{name} 等待调度', tone: 'waiting' }
  if (status === 'starting') return { titleKey: '{name} 正在启动', tone: 'running' }
  if (status === 'running') return { titleKey: '{name} 正在运行', tone: 'running' }
  if (status === 'completed') return { titleKey: '{name} 已完成', tone: 'completed' }
  if (status === 'interrupted') return { titleKey: '{name} 已中断', tone: 'stopped' }
  if (status === 'failed') return { titleKey: '{name} 执行失败', tone: 'failed' }
  return { titleKey: '{name} 状态已更新', tone: 'waiting' }
}

function timestamp(value: unknown) {
  const source =
    typeof value === 'string' || typeof value === 'number' || value instanceof Date ? value : 0
  const parsed = new Date(source).getTime()
  return Number.isFinite(parsed) ? parsed : 0
}

export function latestRunningTool(tools: ToolActivity[] = []) {
  return [...tools].reverse().find((tool) => tool?.status === 'running') || null
}

export function primaryRunActivity({
  currentActivity,
  compaction,
  text,
  thinkingText,
  lastActivityAt,
}: {
  currentActivity?: EntityRecord | null
  compaction?: EntityRecord | null
  text?: unknown
  thinkingText?: unknown
  lastActivityAt?: string | null
} = {}): EntityRecord {
  if (compaction?.active)
    return { type: 'compaction', compaction, updatedAt: compaction.startedAt || lastActivityAt }
  if (currentActivity && ['model', 'compaction', 'retry'].includes(currentActivity.type))
    return currentActivity
  const stage =
    currentActivity?.type === 'tool'
      ? currentActivity.status === 'running'
        ? 'working'
        : 'processing_result'
      : String(thinkingText || '').trim()
        ? 'thinking'
        : String(text || '').trim()
          ? 'responding'
          : 'thinking'
  return { type: 'model', stage, updatedAt: currentActivity?.updatedAt || lastActivityAt }
}

function activityKey(activity: EntityRecord | null | undefined) {
  if (!activity?.type) return ''
  if (activity.type === 'tool') return `tool:${activity.id || activity.name || ''}`
  if (activity.type === 'agent')
    return `agent:${activity.agent?.id || activity.agent?.canonicalName || ''}:${activity.agent?.status || ''}`
  if (activity.type === 'plan')
    return `plan:${activity.updatedAt || activity.taskList?.updatedAt || ''}`
  if (activity.type === 'model') return `model:${activity.stage || ''}`
  if (activity.type === 'compaction')
    return `compaction:${activity.compaction?.status || activity.compaction?.active || ''}`
  return `${activity.type}:${activity.id || activity.updatedAt || ''}`
}

export function taskListChanges(previous?: TaskList | null, next?: TaskList | null) {
  const previousItems = new Map<string, TaskListItem>(
    (previous?.items || []).map((item) => [String(item.id || ''), item]),
  )
  const nextItems = new Map<string, TaskListItem>(
    (next?.items || []).map((item) => [String(item.id || ''), item]),
  )
  const changes: EntityRecord[] = []
  for (const item of nextItems.values()) {
    const before = previousItems.get(String(item.id || ''))
    if (!before)
      changes.push({ id: item.id, title: item.title, status: item.status, kind: 'added' })
    else if (
      before.status !== item.status ||
      before.title !== item.title ||
      before.note !== item.note
    ) {
      changes.push({
        id: item.id,
        title: item.title,
        status: item.status,
        previousStatus: before.status,
        kind: 'updated',
      })
    }
  }
  for (const item of previousItems.values()) {
    if (!nextItems.has(String(item.id || '')))
      changes.push({ id: item.id, title: item.title, status: item.status, kind: 'removed' })
  }
  return changes
}

export function pushCurrentActivity(
  feed: EntityRecord[] = [],
  activity: EntityRecord,
  maximum = MAX_CURRENT_ACTIVITIES,
) {
  const current = Array.isArray(feed) ? feed : []
  if (!['tool', 'plan', 'agent'].includes(activity?.type)) return current
  let next = [...current]
  if (activity.type === 'plan')
    next = next.filter(
      (item) => item?.type !== 'tool' || !['get_task_list', 'update_task_list'].includes(item.name),
    )
  if (activity.type === 'agent')
    next = next.filter(
      (item) =>
        item?.type !== 'tool' ||
        ![
          'spawn_agent',
          'list_agents',
          'send_message',
          'followup_task',
          'wait_agent',
          'interrupt_agent',
        ].includes(item.name),
    )
  const key = activityKey(activity)
  const existingIndex = next.findIndex((item) => activityKey(item) === key)
  if (existingIndex >= 0) next[existingIndex] = { ...next[existingIndex], ...activity }
  else next.push(activity)
  return next.slice(-Math.max(1, Number(maximum) || MAX_CURRENT_ACTIVITIES))
}

export function settleToolCalls(
  tools: ToolActivity[] = [],
  {
    finishedAt = new Date().toISOString(),
    error = '',
  }: { finishedAt?: string; error?: string } = {},
) {
  return tools.map((tool) =>
    tool?.status === 'running'
      ? {
          ...tool,
          status: error ? 'error' : 'done',
          message: error || tool.message || '',
          updatedAt: finishedAt,
          finishedAt,
        }
      : tool,
  )
}

export function deriveRunActivity({
  streaming,
  text,
  tools = [],
  compaction,
  error,
  stopped,
  lastActivityAt,
  now = Date.now(),
}: {
  streaming?: boolean
  text?: unknown
  tools?: ToolActivity[]
  compaction?: EntityRecord | null
  error?: unknown
  stopped?: boolean
  lastActivityAt?: string | null
  now?: number
} = {}) {
  if (!streaming) {
    if (stopped) return { stage: 'stopped', inactiveMs: 0, activeTool: null }
    if (error) return { stage: 'failed', inactiveMs: 0, activeTool: null }
    return { stage: 'completed', inactiveMs: 0, activeTool: null }
  }

  const activeTool = latestRunningTool(tools)
  const lastActivity = timestamp(lastActivityAt)
  const inactiveMs = lastActivity ? Math.max(0, now - lastActivity) : 0
  if (compaction?.active) return { stage: 'compacting', inactiveMs, activeTool: null }
  if (inactiveMs >= RUN_INACTIVITY_THRESHOLD_MS) {
    return { stage: activeTool ? 'waiting_tool' : 'waiting_model', inactiveMs, activeTool }
  }
  if (activeTool?.name && AGENT_TOOLS.has(activeTool.name))
    return { stage: 'subagent', inactiveMs, activeTool }
  if (activeTool?.name === 'generate_visual')
    return { stage: 'generating_visual', inactiveMs, activeTool }
  if (activeTool?.name === 'bash') return { stage: 'validating', inactiveMs, activeTool }
  if (activeTool?.name && EDIT_TOOLS.has(activeTool.name))
    return { stage: 'editing', inactiveMs, activeTool }
  if (activeTool?.name && RESEARCH_TOOLS.has(activeTool.name))
    return { stage: 'researching', inactiveMs, activeTool }
  if (activeTool) return { stage: 'using_tool', inactiveMs, activeTool }
  if (String(text || '').trim()) return { stage: 'responding', inactiveMs, activeTool: null }
  return { stage: 'thinking', inactiveMs, activeTool: null }
}

export function latestUnrecoveredToolError(
  tools: ToolActivity[] = [],
  {
    streaming = true,
    lastActivityAt,
  }: { streaming?: boolean; lastActivityAt?: string | null } = {},
) {
  if (!streaming) return null
  let errorIndex = -1
  for (let index = tools.length - 1; index >= 0; index -= 1) {
    if (tools[index]?.status === 'error') {
      errorIndex = index
      break
    }
  }
  if (errorIndex < 0) return null

  const error = tools[errorIndex]
  const hasLaterToolProgress = tools.slice(errorIndex + 1).some((tool) => tool?.status !== 'error')
  const errorAt = timestamp(error.finishedAt || error.updatedAt || error.startedAt)
  const activityAt = timestamp(lastActivityAt)
  if (hasLaterToolProgress || (errorAt && activityAt > errorAt)) return null
  return error
}

export function groupToolCalls(tools: ToolActivity[] = []) {
  const running: ToolActivity[] = []
  const errors: ToolActivity[] = []
  const completedByName = new Map<
    string,
    { name: string; count: number; tools: ToolActivity[]; message: string }
  >()
  for (const tool of tools) {
    if (!tool?.name) continue
    if (tool.status === 'running') {
      running.push(tool)
    } else if (tool.status === 'error') {
      errors.push(tool)
    } else {
      const existing = completedByName.get(tool.name) || {
        name: tool.name,
        count: 0,
        tools: [],
        message: '',
      }
      existing.count += 1
      existing.tools.push(tool)
      existing.message = tool.message || existing.message
      completedByName.set(tool.name, existing)
    }
  }
  return { running, errors, completed: [...completedByName.values()] }
}

export function runDurationMs(startedAt: unknown, finishedAt: unknown, now = Date.now()) {
  const start = timestamp(startedAt)
  if (!start) return 0
  const end = timestamp(finishedAt) || now
  return Math.max(0, end - start)
}

export function activityDurationMs(
  activity: EntityRecord | null | undefined,
  runStartedAt: unknown,
  now = Date.now(),
) {
  const agent = activity?.type === 'agent' ? activity.agent || {} : {}
  const startedAt = activity?.startedAt || agent.startedAt || activity?.updatedAt || runStartedAt
  let finishedAt = activity?.finishedAt || agent.completedAt || ''
  if (!finishedAt && activity?.type === 'tool' && activity.status && activity.status !== 'running')
    finishedAt = activity.updatedAt
  if (
    !finishedAt &&
    activity?.type === 'agent' &&
    !['queued', 'starting', 'running'].includes(agent.status)
  )
    finishedAt = agent.lastActivityAt || activity.updatedAt
  if (!finishedAt && activity?.type === 'plan') finishedAt = activity.updatedAt
  return runDurationMs(startedAt, finishedAt, now)
}

export function formatRunDuration(milliseconds: unknown, language = 'zh-CN') {
  const totalMilliseconds = Math.max(0, Math.round(Number(milliseconds) || 0))
  if (totalMilliseconds < 1000)
    return language === 'en-US' ? `${totalMilliseconds}ms` : `${totalMilliseconds} 毫秒`
  const totalSeconds = Math.floor(totalMilliseconds / 1000)
  if (totalSeconds < 60) return language === 'en-US' ? `${totalSeconds}s` : `${totalSeconds} 秒`
  const minutes = Math.floor(totalSeconds / 60)
  const seconds = totalSeconds % 60
  if (minutes < 60) return `${minutes}:${String(seconds).padStart(2, '0')}`
  const hours = Math.floor(minutes / 60)
  return `${hours}:${String(minutes % 60).padStart(2, '0')}:${String(seconds).padStart(2, '0')}`
}
