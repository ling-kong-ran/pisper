// Agent 会话事件桥接：把 Pi 引擎的低层会话事件（消息增量/工具执行/回合生命周期等）
// 归一化为前端 SSE 事件与实时状态（live）更新。所有文本都经过清洗与长度限制，
// 保证经 HTTP 输出的 JSON 严格合法（serde_json 可解析）。
const MAX_EVENT_TEXT_CHARS = 240

// 事件类型 → 前端事件通道的映射；不在映射中的事件会被忽略。
export const AGENT_SESSION_EVENT_CHANNELS = Object.freeze({
  agent_start: 'agent_lifecycle',
  agent_end: 'agent_lifecycle',
  agent_settled: 'agent_lifecycle',
  turn_start: 'agent_lifecycle',
  turn_end: 'agent_lifecycle',
  message_start: 'agent_lifecycle',
  message_update: 'text_or_thinking_patch',
  message_end: 'agent_lifecycle',
  tool_execution_start: 'tool_start',
  tool_execution_update: 'tool_update',
  tool_execution_end: 'tool_end',
  queue_update: 'queue_update',
  compaction_start: 'compaction_start',
  compaction_end: 'compaction_end',
  auto_retry_start: 'retry',
  auto_retry_end: 'agent_lifecycle',
  entry_appended: 'session_tree_changed',
  session_info_changed: 'session_title',
  thinking_level_changed: 'thinking_level_changed',
})

// 需要“直接投影”到前端的事件：这些事件通过专用通道推送，不额外发 agent_lifecycle。
const DIRECT_EVENT_TYPES = new Set([
  'tool_execution_start',
  'tool_execution_update',
  'tool_execution_end',
  'queue_update',
  'compaction_start',
  'compaction_end',
  'auto_retry_start',
])

// 事件文本清洗：控制字符替换为空格、压缩空白、限长，防止注入控制序列或破坏 JSON。
function safeEventText(value, maxChars = MAX_EVENT_TEXT_CHARS) {
  return [...String(value || '')]
    .map((character) => {
      const code = character.charCodeAt(0)
      return code < 32 || code === 127 ? ' ' : character
    })
    .join('')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, maxChars)
}

// 安全的标识符：只保留字母数字与 . _ : -，其余清空。
function safeId(value) {
  const id = safeEventText(value, 160)
  return /^[A-Za-z0-9._:-]+$/.test(id) ? id : ''
}

function entryKind(entry) {
  const type = safeEventText(entry?.type, 40)
  if (type === 'custom') return 'runtime'
  return [
    'message',
    'compaction',
    'branch_summary',
    'model_change',
    'thinking_level_change',
    'label',
    'session_info',
  ].includes(type)
    ? type
    : 'runtime'
}

function hasDirectProjection(event) {
  if (event.type !== 'message_update') return DIRECT_EVENT_TYPES.has(event.type)
  return ['text_delta', 'text_end', 'thinking_delta', 'toolcall_delta'].includes(
    event.assistantMessageEvent?.type,
  )
}

// 计算事件对应的生命周期阶段（thinking/responding/using_tool/compacting…）。
function lifecyclePhase(event, current) {
  if (event.type === 'agent_start') return 'starting'
  if (event.type === 'turn_start') return 'thinking'
  if (event.type === 'turn_end') return 'processing_result'
  if (event.type === 'message_start')
    return event.message?.role === 'assistant' ? 'thinking' : current?.phase || 'starting'
  if (event.type === 'message_update') {
    const updateType = event.assistantMessageEvent?.type || ''
    if (updateType.startsWith('thinking')) return 'thinking'
    if (updateType.startsWith('text')) return 'responding'
    if (updateType.startsWith('toolcall') || updateType === 'done') return 'processing_result'
    if (updateType === 'error') return 'failed'
    return current?.phase || 'thinking'
  }
  if (event.type === 'message_end') {
    if (event.message?.role !== 'assistant') return current?.phase || 'thinking'
    return ['error', 'aborted'].includes(event.message.stopReason) || event.message.errorMessage
      ? 'failed'
      : 'processing_result'
  }
  if (event.type === 'tool_execution_start' || event.type === 'tool_execution_update')
    return 'using_tool'
  if (event.type === 'tool_execution_end') return 'processing_result'
  if (event.type === 'compaction_start') return 'compacting'
  if (event.type === 'compaction_end') return event.willRetry ? 'thinking' : 'processing_result'
  if (event.type === 'auto_retry_start') return 'retrying'
  if (event.type === 'auto_retry_end') return event.success ? 'processing_result' : 'failed'
  if (event.type === 'agent_end') return event.willRetry ? 'retrying' : 'settling'
  if (event.type === 'agent_settled') return 'settled'
  return current?.phase || 'starting'
}

// 生命周期阶段 → 前端活动类型映射。
function lifecycleActivity(lifecycle) {
  const stages = {
    starting: 'starting',
    thinking: 'thinking',
    responding: 'responding',
    processing_result: 'processing_result',
    retrying: 'waiting_retry',
    settling: 'finalizing',
    settled: 'finalizing',
    failed: 'processing_result',
  }
  const stage = stages[lifecycle?.phase]
  return stage ? { type: 'model', stage, updatedAt: lifecycle.updatedAt } : null
}

// 初始生命周期状态。
export function initialAgentLifecycle(updatedAt) {
  return {
    phase: 'starting',
    event: 'prompt_submitted',
    turn: 0,
    updatedAt,
  }
}

// 构造一次运行（run）的实时状态快照，供前端通过 SSE 增量更新。
export function createLiveRunState({
  startedAt,
  goal,
  plan,
  agents,
  queuedInputs,
  contextUsage,
  sessionUsage,
  promptCache,
}) {
  return {
    streaming: true,
    text: '',
    thinkingText: '',
    tools: [],
    assets: [],
    error: '',
    goal,
    plan,
    agents,
    currentActivity: { type: 'model', stage: 'thinking', updatedAt: startedAt },
    activityFeed: [],
    queuedInputs,
    contextUsage,
    sessionUsage,
    promptCache,
    compaction: null,
    lifecycle: initialAgentLifecycle(startedAt),
    startedAt,
    lastActivityAt: startedAt,
  }
}

// 运行结束：标记生命周期为完成/失败。
export function finishAgentLifecycle(current, error, updatedAt) {
  return {
    ...(current || initialAgentLifecycle(updatedAt)),
    phase: error ? 'failed' : 'completed',
    event: error ? 'runtime_error' : 'runtime_done',
    ...(error ? { message: safeEventText(error) } : {}),
    updatedAt,
  }
}

// 核心桥接函数：把单个会话事件应用（apply）到 live 状态并 emit 对应前端事件。
export function bridgeAgentSessionEvent(event, live, emit) {
  if (!event?.type || !AGENT_SESSION_EVENT_CHANNELS[event.type]) return null
  const updatedAt = live.lastActivityAt || new Date().toISOString()

  if (event.type === 'entry_appended') {
    live.sessionTreeRevision = Number(live.sessionTreeRevision || 0) + 1
    const entry = {
      id: safeId(event.entry?.id),
      parentId: safeId(event.entry?.parentId),
      kind: entryKind(event.entry),
    }
    emit('session_tree_changed', { revision: live.sessionTreeRevision, entry })
    return { event: event.type, entry }
  }
  if (event.type === 'session_info_changed') {
    const name = safeEventText(event.name, 200)
    emit('session_title', { name, source: 'pi_session' })
    return { event: event.type, name }
  }
  if (event.type === 'thinking_level_changed') {
    const level = safeEventText(event.level, 20)
    emit('thinking_level_changed', { level })
    return { event: event.type, level }
  }
  if (event.type === 'queue_update') return { event: event.type }

  const current = live.lifecycle || initialAgentLifecycle(updatedAt)
  const lifecycle = {
    ...current,
    phase: lifecyclePhase(event, current),
    event: event.type,
    turn: event.type === 'turn_start' ? Number(current.turn || 0) + 1 : Number(current.turn || 0),
    updatedAt,
  }
  if (event.message?.role) lifecycle.messageRole = safeEventText(event.message.role, 20)
  if (!event.type.startsWith('tool_execution_')) delete lifecycle.tool
  if (event.type === 'turn_start' && lifecycle.retry?.success) delete lifecycle.retry
  if (event.toolCallId) {
    lifecycle.tool = {
      id: safeId(event.toolCallId),
      name: safeEventText(event.toolName, 120),
      ...(event.type === 'tool_execution_end' ? { error: Boolean(event.isError) } : {}),
    }
  } else if (event.type === 'tool_execution_end') delete lifecycle.tool
  if (event.type === 'auto_retry_start') {
    lifecycle.retry = {
      attempt: Number(event.attempt || 0),
      maxAttempts: Number(event.maxAttempts || 0),
    }
  } else if (event.type === 'auto_retry_end') {
    lifecycle.retry = {
      attempt: Number(event.attempt || 0),
      success: Boolean(event.success),
      ...(event.finalError ? { message: safeEventText(event.finalError) } : {}),
    }
  } else if (event.type === 'agent_end' && !event.willRetry) delete lifecycle.retry

  live.lifecycle = lifecycle
  const activity = lifecycleActivity(lifecycle)
  if (activity) live.currentActivity = activity
  if (!hasDirectProjection(event))
    emit('agent_lifecycle', {
      lifecycle,
      currentActivity: activity || live.currentActivity || null,
    })
  return { lifecycle, currentActivity: activity }
}
