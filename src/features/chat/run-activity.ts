// 运行活动（run activity）归并：把 SSE 流式事件压缩成当前活动与
// 活动轨迹，限制同时展示的活动数，计划工具/读工具不计入普通活动。
import { isPlanReadTool, isPlanTool, planFromActivity } from '@/lib/plan-protocol'
import type { EntityRecord, Plan, PlanItem, ToolActivity } from '@/types/chat'

export const RUN_INACTIVITY_THRESHOLD_MS = 10_000
export const MAX_CURRENT_ACTIVITIES = 6

const RESEARCH_TOOLS = new Set([
  'read',
  'grep',
  'find',
  'ls',
  'memory_search',
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

// Agent 状态 → 展示文案与色调的映射（排队/启动/运行/完成/中断/失败）。
export function agentActivityState(status?: string) {
  if (status === 'queued') return { titleKey: '{name} 等待调度', tone: 'waiting' }
  if (status === 'starting') return { titleKey: '{name} 正在启动', tone: 'running' }
  if (status === 'running') return { titleKey: '{name} 正在运行', tone: 'running' }
  if (status === 'completed') return { titleKey: '{name} 已完成', tone: 'completed' }
  if (status === 'interrupted') return { titleKey: '{name} 已中断', tone: 'stopped' }
  if (status === 'failed') return { titleKey: '{name} 执行失败', tone: 'failed' }
  return { titleKey: '{name} 状态已更新', tone: 'waiting' }
}

// 时间戳归一化：非法/缺失值返回 0，避免 NaN 破坏排序与时长计算。
function timestamp(value: unknown) {
  const source =
    typeof value === 'string' || typeof value === 'number' || value instanceof Date ? value : 0
  const parsed = new Date(source).getTime()
  return Number.isFinite(parsed) ? parsed : 0
}

// 最近一个仍在运行的工具（按列表倒序找第一个 running）。
export function latestRunningTool(tools: ToolActivity[] = []) {
  return [...tools].reverse().find((tool) => tool?.status === 'running') || null
}

// 推导“主活动”：压缩进行中优先，其次当前 agent 活动，
// 否则按 工具运行 → 处理结果 → 思考 → 回复 的优先级合成 model 阶段。
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

// 活动流滚动版本：把活动列表编码为字符串，用于触发滚动定位的依赖比较。
export function activityScrollVersion(feed: EntityRecord[] = []) {
  return feed
    .map(
      (activity) =>
        `${activity.type}:${activity.id || activity.agent?.id || ''}:${activity.status || activity.stage || activity.agent?.status || ''}`,
    )
    .join('|')
}

// 活动渲染 key：按类型取稳定标识（工具 id/agent id/plan 时间戳），
// 供 React key 使用，避免同类活动混淆导致复用错误。
export function activityRenderKey(activity: EntityRecord, index = 0) {
  if (activity.type === 'tool') return `tool:${activity.id || activity.name || index}`
  if (activity.type === 'agent')
    return `agent:${activity.agent?.id || activity.agent?.canonicalName || index}:${activity.agent?.status || activity.status || ''}`
  if (activity.type === 'plan')
    return `plan:${activity.id || planFromActivity(activity)?.updatedAt || activity.updatedAt || index}`
  return `${activity.type || 'activity'}:${activity.id || activity.startedAt || activity.createdAt || index}`
}

// 活动的稳定去重 key（不含状态，用于在活动流中定位同一条活动）。
function activityKey(activity: EntityRecord | null | undefined) {
  if (!activity?.type) return ''
  if (activity.type === 'tool') return `tool:${activity.id || activity.name || ''}`
  if (activity.type === 'agent')
    return `agent:${activity.agent?.id || activity.agent?.canonicalName || ''}`
  if (activity.type === 'plan')
    return `plan:${activity.updatedAt || planFromActivity(activity)?.updatedAt || ''}`
  if (activity.type === 'model') return `model:${activity.stage || ''}`
  if (activity.type === 'compaction')
    return `compaction:${activity.compaction?.status || activity.compaction?.active || ''}`
  return `${activity.type}:${activity.id || activity.updatedAt || ''}`
}

// 计划变更 diff：对比新旧计划项，输出 added/updated/removed 变更列表，
// 供计划面板高亮变化并触发动画。
export function planChanges(previous?: Plan | null, next?: Plan | null) {
  const previousItems = new Map<string, PlanItem>(
    (previous?.items || []).map((item) => [String(item.id || ''), item]),
  )
  const nextItems = new Map<string, PlanItem>(
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

// 推入当前活动：同 key 活动合并更新，计划活动会清掉同类计划工具活动，
// agent 活动清掉 spawn/send 等代理工具活动，最终截断到最大条数。
export function pushCurrentActivity(
  feed: EntityRecord[] = [],
  activity: EntityRecord,
  maximum = MAX_CURRENT_ACTIVITIES,
) {
  const current = Array.isArray(feed) ? feed : []
  if (!['tool', 'plan', 'agent'].includes(activity?.type)) return current
  let next = [...current]
  if (activity.type === 'plan')
    next = next.filter((item) => item?.type !== 'tool' || !isPlanTool(item.name))
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

// 结算工具调用：把仍处于 running 的工具统一标记为 done/error 并补时间戳，
// 用于一次回复结束或出错时兜底收尾。
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

// 推导运行阶段：非流式按 停止/失败/完成；流式中按当前工具类型细分
// （子代理/生成视觉/编辑/研究/普通工具），无工具时按文本推断 回复/思考，
// 长时间无活动则标记为等待（模型或工具）。
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
  if (activeTool?.name && (RESEARCH_TOOLS.has(activeTool.name) || isPlanReadTool(activeTool.name)))
    return { stage: 'researching', inactiveMs, activeTool }
  if (activeTool) return { stage: 'using_tool', inactiveMs, activeTool }
  if (String(text || '').trim()) return { stage: 'responding', inactiveMs, activeTool: null }
  return { stage: 'thinking', inactiveMs, activeTool: null }
}

// 找“尚未被后续进展覆盖”的最近工具错误：若该错误之后还有成功的工具
// 或更新的活动时间，视为已恢复不展示，避免残留报错误导用户。
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

// 工具调用分组：运行中 / 出错 / 已完成（按工具名聚合计数，便于折叠展示）。
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

// 运行时长（毫秒）：开始时间非法返回 0，未结束用当前时间补足。
export function runDurationMs(startedAt: unknown, finishedAt: unknown, now = Date.now()) {
  const start = timestamp(startedAt)
  if (!start) return 0
  const end = timestamp(finishedAt) || now
  return Math.max(0, end - start)
}

// 活动时长：优先活动的开始/结束时间，未结束的 agent/tool 用完成态时间或
// 当前时间兜底，保证运行时活动也能显示持续时长。
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

// 时长格式化：毫秒 → 中文/英文友好文案，超 1 分钟用 m:ss、超 1 小时 h:mm:ss。
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
