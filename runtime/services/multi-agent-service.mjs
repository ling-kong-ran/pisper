// 多 Agent 服务：父会话可派生隔离上下文的子 Agent（spawn），子 Agent 独立运行、
// 通过邮箱（mailbox）收发消息；服务负责注册表持久化、并发限制、运行/中断/回收
// 生命周期管理，以及子 Agent 会话的定时释放（保留期后销毁）。
import { randomUUID } from 'node:crypto'
import { existsSync } from 'node:fs'
import { readdir, rm, unlink } from 'node:fs/promises'
import { dirname, isAbsolute, join } from 'node:path'
import {
  createAgentSession,
  createDefaultResourceLoader,
  DEFAULT_MAX_BYTES,
  estimateTokens,
  formatSize,
  SessionManager,
} from '../runtime/pi-coding-agent.mjs'
import {
  applyPisperSystemPrompt,
  PISPER_OPENING,
  pisperPromptExtension,
} from '../prompts/pisper-system-prompt.mjs'
import {
  createCompactionSettingsManager,
  pisperCompactionExtension,
} from '../runtime/compaction-policy.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { PLAN_WRITE_TOOL_NAMES } from '../tools/app/plan-tool-names.mjs'
import { extensionSafeSettingsManager } from './skills-service.mjs'
import { SUBAGENT_MODEL_TIERS } from './agent-profiles.mjs'

export const MAX_CONCURRENT_AGENTS = 4
export const MAX_AGENTS_PER_PARENT = 64
export const MAX_AGENT_RECORDS = 256
export const MAX_AGENT_TASK_CHARS = 12_000
export const TERMINAL_AGENT_SESSION_RETENTION_MS = 10 * 60 * 1000
export const AGENT_PROGRESS_HEARTBEAT_MS = 5_000

const AGENT_REGISTRY_VERSION = 4
const AGENT_COMPLETION_MARKER = '[Pisper internal agent completion]'

export const MULTI_AGENT_TOOL_NAMES = Object.freeze([
  'spawn_agent',
  'list_agents',
  'send_message',
  'followup_task',
  'wait_agent',
  'interrupt_agent',
  'update_team_task',
  'run_team_workflow',
])

export const PARENT_ONLY_TOOL_NAMES = new Set([
  ...MULTI_AGENT_TOOL_NAMES,
  'get_goal',
  'update_goal',
  ...PLAN_WRITE_TOOL_NAMES,
  'browser_automation',
  'computer_use',
  'mcp_list',
  'mcp_manage',
  // 定时任务与工作流会以完全访问权限无人值守运行：只有主会话能建或启动。
  'schedule_list',
  'schedule_manage',
  'workflow_list',
  'workflow_run',
])

const ACTIVE_AGENT_STATUSES = new Set(['queued', 'starting', 'running'])
const TERMINAL_AGENT_STATUSES = new Set(['completed', 'failed', 'interrupted'])
const RESTART_INTERRUPTION_REASON = 'Agent was interrupted because Pisper restarted.'

// 子 Agent 系统提示：强调隔离上下文、只做委派任务、可验证结果、遵守父会话约束。
export const MULTI_AGENT_SYSTEM_PROMPT = `You are a Pisper subagent working in an isolated context on one delegated task.
- Complete only the concrete task you were given and return a concise, evidence-based result.
- Inspect the relevant files before drawing conclusions or editing.
- Do not duplicate unrelated work or wait for additional instructions.
- You cannot spawn other agents.
- In Team mode, you may inspect teammates and send direct handoff messages through the restricted team communication tools.
- Respond in the language used by the delegated task.`

function textFromContent(content) {
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content
    .filter((part) => part?.type === 'text')
    .map((part) => part.text || '')
    .join('')
}

function usageTotal(messages = []) {
  return messages.reduce(
    (total, message) => {
      if (message?.role !== 'assistant' || !message.usage) return total
      const usage = message.usage
      total.input += Number(usage.input) || 0
      total.output += Number(usage.output) || 0
      total.cacheRead += Number(usage.cacheRead) || 0
      total.cacheWrite += Number(usage.cacheWrite) || 0
      total.reasoning += Number(usage.reasoning) || 0
      total.totalTokens += Number(usage.totalTokens ?? usage.total) || 0
      return total
    },
    { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, totalTokens: 0 },
  )
}

function usageDifference(total, baseline) {
  return Object.fromEntries(
    Object.keys(total).map((key) => [
      key,
      Math.max(0, (Number(total[key]) || 0) - (Number(baseline?.[key]) || 0)),
    ]),
  )
}

// 附加技能目录（Bot 自己写的技能）：只认绝对路径，最多 4 个。
function skillPathList(value) {
  return (Array.isArray(value) ? value : [])
    .map((item) => String(item || ''))
    .filter((item) => item && isAbsolute(item))
    .slice(0, 4)
}

function modelLabel(model) {
  return model?.provider && model?.id ? `${model.provider}/${model.id}` : ''
}

function modelFromLabel(value) {
  const label = String(value || '')
  const separator = label.indexOf('/')
  return separator > 0
    ? { provider: label.slice(0, separator), id: label.slice(separator + 1) }
    : null
}

const MODEL_SOURCES = new Set(['parent', 'definition', 'tier'])
const MODEL_FALLBACK_REASONS = new Set(['model-unavailable', 'tier-unset', 'tier-unavailable'])

// 子代理定义与档位带来的字段：用了哪份定义、模型从哪来（含实际用的档位）、没用上定义模型的原因；重启后也要能恢复。
function profileFields(value) {
  return {
    agentProfile: String(value?.agentProfile || '').slice(0, 48),
    profileInstructions: String(value?.profileInstructions || '').slice(0, 12_000),
    modelSource: MODEL_SOURCES.has(value?.modelSource) ? value.modelSource : 'parent',
    modelTier: SUBAGENT_MODEL_TIERS.includes(value?.modelTier) ? value.modelTier : '',
    modelFallbackReasons: (Array.isArray(value?.modelFallbackReasons)
      ? value.modelFallbackReasons
      : []
    ).filter((reason) => MODEL_FALLBACK_REASONS.has(reason)),
  }
}

function publicProfileFields(record) {
  return {
    agentProfile: record.agentProfile,
    modelSource: record.modelSource,
    modelTier: record.modelTier,
    modelFallbackReasons: [...record.modelFallbackReasons],
  }
}

function emptyUsage() {
  return { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, totalTokens: 0 }
}

// 记录持久化：只保留可序列化字段（去掉函数/会话对象），供重启后恢复列表。
function durableRecord(record) {
  return {
    id: record.id,
    taskName: record.taskName,
    title: record.title || '',
    ...(record.toolCallId ? { toolCallId: record.toolCallId } : {}),
    ...(record.parentLimit ? { parentLimit: record.parentLimit } : {}),
    ...(record.skillPaths?.length ? { skillPaths: record.skillPaths } : {}),
    canonicalName: record.canonicalName,
    parentSessionId: record.parentSessionId,
    cwd: record.cwd,
    model: modelLabel(record.model),
    thinkingLevel: record.thinkingLevel,
    ...profileFields(record),
    message: record.message,
    availableTools: [...record.availableTools],
    turnCount: record.turnCount,
    toolCallCount: record.toolCallCount,
    tools: record.tools.map((tool) => ({ ...tool })),
    output: record.output,
    outputTruncated: record.outputTruncated,
    usage: { ...record.usage },
    runUsage: { ...record.runUsage },
    runNumber: record.runNumber,
    resultVersion: record.resultVersion,
    error: record.error,
    startedAt: record.startedAt,
    lastActivityAt: record.lastActivityAt,
    completedAt: record.completedAt,
    durationMs: record.durationMs,
    status: record.status,
    currentActivity: record.currentActivity,
  }
}

function durableMailboxEntry(entry) {
  const { fullOutput: _fullOutput, ...durable } = entry
  return durable
}

function restoredMailboxEntry(value) {
  const mailboxId = String(value?.mailboxId || '').trim()
  const id = String(value?.id || '').trim()
  const parentSessionId = String(value?.parentSessionId || '').trim()
  const resultVersion = positiveInteger(value?.resultVersion)
  if (
    !mailboxId ||
    !id ||
    !parentSessionId ||
    !resultVersion ||
    !TERMINAL_AGENT_STATUSES.has(value?.status)
  )
    return null
  return {
    ...value,
    mailboxId,
    id,
    parentSessionId,
    resultVersion,
    fullOutput: String(value?.output || ''),
  }
}

function restoredRecord(value) {
  const id = String(value?.id || '').trim()
  const parentSessionId = String(value?.parentSessionId || '').trim()
  const model = modelFromLabel(value?.model)
  if (!id || !parentSessionId || !model) return null
  const status =
    TERMINAL_AGENT_STATUSES.has(value?.status) || ACTIVE_AGENT_STATUSES.has(value?.status)
      ? value.status
      : 'failed'
  const output = String(value?.output || '')
  const parentLimit = Number(value?.parentLimit)
  return {
    id,
    taskName: normalizeTaskName(value?.taskName),
    title: String(value?.title || '')
      .trim()
      .slice(0, 80),
    toolCallId: toolCallIdValue(value?.toolCallId),
    parentLimit: Number.isInteger(parentLimit) && parentLimit > 0 ? parentLimit : null,
    skillPaths: skillPathList(value?.skillPaths),
    canonicalName: String(
      value?.canonicalName || `/root/${normalizeTaskName(value?.taskName)}_restored`,
    ),
    parentSessionId,
    cwd: String(value?.cwd || ''),
    model,
    thinkingLevel: String(value?.thinkingLevel || 'medium'),
    ...profileFields(value),
    message: String(value?.message || ''),
    availableTools: childTools(value?.availableTools),
    customTools: [],
    createCustomTools: null,
    disposeCustomTools: null,
    turnCount: positiveInteger(value?.turnCount) || 0,
    toolCallCount: positiveInteger(value?.toolCallCount) || 0,
    tools: Array.isArray(value?.tools) ? value.tools.map((tool) => ({ ...tool })) : [],
    output,
    fullOutput: output,
    outputTruncated: Boolean(value?.outputTruncated),
    usage: { ...emptyUsage(), ...(value?.usage || {}) },
    runUsage: { ...emptyUsage(), ...(value?.runUsage || {}) },
    runNumber: positiveInteger(value?.runNumber) || 0,
    runGeneration: 0,
    slotActive: false,
    resultVersion: positiveInteger(value?.resultVersion) || 0,
    error: String(value?.error || ''),
    startedAt: value?.startedAt || new Date().toISOString(),
    lastActivityAt: value?.lastActivityAt || null,
    completedAt: value?.completedAt || null,
    durationMs: Number.isFinite(value?.durationMs) ? value.durationMs : null,
    status,
    currentActivity:
      value?.currentActivity && typeof value.currentActivity === 'object'
        ? { ...value.currentActivity }
        : null,
    session: null,
    unsubscribe: () => {},
    aborted: false,
    abortReason: '',
    runningPromise: Promise.resolve(),
    restartChain: Promise.resolve(),
    pendingMessages: [],
    onProgress: null,
    onSession: null,
    onCompleted: null,
    onTerminal: null,
  }
}

function positiveInteger(value) {
  const number = Number(value)
  return Number.isFinite(number) && number > 0 ? Math.floor(number) : null
}

function modelOutputTokenLimit(model) {
  const limits = [positiveInteger(model?.contextWindow), positiveInteger(model?.maxTokens)].filter(
    Boolean,
  )
  return limits.length ? Math.min(...limits) : null
}

function estimatedTextTokens(text) {
  return estimateTokens({ role: 'custom', content: [{ type: 'text', text }] })
}

function utf8Prefix(value, maxBytes) {
  const buffer = Buffer.from(value, 'utf8')
  if (buffer.length <= maxBytes) return value
  let end = maxBytes
  while (end > 0 && (buffer[end] & 0xc0) === 0x80) end -= 1
  return buffer.subarray(0, end).toString('utf8')
}

function contextLimitedText(value, model) {
  const text = String(value || '').trim()
  const tokenLimit = modelOutputTokenLimit(model)
  const estimatedTokens = estimatedTextTokens(text)
  const outputBytes = Buffer.byteLength(text, 'utf8')
  const tokenTruncated = Boolean(tokenLimit && estimatedTokens > tokenLimit)
  const byteTruncated = outputBytes > DEFAULT_MAX_BYTES
  if (!tokenTruncated && !byteTruncated) {
    return {
      text,
      fullText: text,
      truncated: false,
      estimatedTokens,
      tokenLimit,
      outputBytes,
      byteLimit: DEFAULT_MAX_BYTES,
    }
  }
  const reasons = [
    tokenTruncated ? 'model context' : '',
    byteTruncated ? `${formatSize(DEFAULT_MAX_BYTES)} Pisper tool-output limit` : '',
  ]
    .filter(Boolean)
    .join(' and ')
  const suffix = `\n\n[Output truncated for ${reasons}.]`
  const byteBudget = Math.max(0, DEFAULT_MAX_BYTES - Buffer.byteLength(suffix, 'utf8'))
  const charBudget = tokenTruncated ? Math.max(0, tokenLimit * 4 - suffix.length) : text.length
  return {
    text: `${utf8Prefix(text.slice(0, charBudget), byteBudget)}${suffix}`,
    fullText: text,
    truncated: true,
    estimatedTokens,
    tokenLimit,
    outputBytes,
    byteLimit: DEFAULT_MAX_BYTES,
  }
}

function childTools(allowedTools) {
  return [...new Set(Array.isArray(allowedTools) ? allowedTools.filter(Boolean) : [])].filter(
    (tool) => !PARENT_ONLY_TOOL_NAMES.has(tool),
  )
}

function inheritedCustomTools(tools, customTools) {
  const enabled = new Set(tools)
  return Array.isArray(customTools) ? customTools.filter((tool) => enabled.has(tool?.name)) : []
}

// 工具调用 id 来自模型输出，只当作不透明的短标识。
function toolCallIdValue(value) {
  const id = String(value || '').trim()
  return id.length <= 128 && ![...id].some((char) => char.charCodeAt(0) < 32) ? id : ''
}

// 子代理的对话记录放在 <transcriptDir>/<父会话>/<子代理>.jsonl：父会话 id 只保留安全字符，不会跳出目录。
function transcriptSegment(value) {
  return (
    String(value || '')
      .replace(/[^A-Za-z0-9_-]/g, '_')
      .slice(0, 128) || '_'
  )
}

function normalizeTaskName(value) {
  const normalized = String(value || '')
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, '_')
    .replace(/_+/g, '_')
    .replace(/^_|_$/g, '')
    .slice(0, 48)
  return normalized || 'task'
}

async function createAgentResourceLoader({
  cwd,
  agentDir,
  settingsManager,
  skillPaths = [],
  appendSystemPrompt = MULTI_AGENT_SYSTEM_PROMPT,
}) {
  process.env.PI_CODING_AGENT_DIR = agentDir || cwd
  const loader = await createDefaultResourceLoader({
    cwd,
    agentDir: agentDir || cwd,
    ...(settingsManager ? { settingsManager: extensionSafeSettingsManager(settingsManager) } : {}),
    extensionFactories: [pisperPromptExtension, pisperCompactionExtension],
    noExtensions: false,
    ...(skillPaths.length ? { additionalSkillPaths: skillPaths } : {}),
    systemPromptOverride: (base) => base ?? PISPER_OPENING,
    appendSystemPromptOverride: (base) => [...base, appendSystemPrompt],
  })
  await loader.reload()
  return loader
}

function summaryValue(value, depth = 0) {
  if (depth > 1) return '[omitted]'
  if (typeof value === 'string') return value.slice(0, 250)
  if (Array.isArray(value)) return value.slice(0, 4).map((item) => summaryValue(item, depth + 1))
  if (value && typeof value === 'object')
    return Object.fromEntries(
      Object.entries(value)
        .slice(0, 8)
        .map(([key, child]) => [key, summaryValue(child, depth + 1)]),
    )
  return value
}

function publicRecord(record) {
  return {
    id: record.id,
    taskName: record.taskName,
    title: record.title || '',
    canonicalName: record.canonicalName,
    parentSessionId: record.parentSessionId,
    status: record.status,
    message: record.message,
    model: modelLabel(record.model),
    thinkingLevel: record.thinkingLevel,
    ...publicProfileFields(record),
    availableTools: [...record.availableTools],
    startedAt: record.startedAt,
    lastActivityAt: record.lastActivityAt,
    completedAt: record.completedAt,
    durationMs: record.durationMs,
    turnCount: record.turnCount,
    toolCallCount: record.toolCallCount,
    tools: record.tools.map((tool) => ({ ...tool })),
    output: record.output,
    fullOutput: record.fullOutput,
    outputTruncated: record.outputTruncated,
    usage: { ...record.usage },
    runUsage: { ...record.runUsage },
    runNumber: record.runNumber,
    resultVersion: record.resultVersion,
    error: record.error,
    currentActivity: record.currentActivity ? { ...record.currentActivity } : null,
  }
}

function summaryRecord(record) {
  return {
    id: record.id,
    taskName: record.taskName,
    title: record.title || '',
    // 派出它的那次 spawn_agent 调用：父会话的工具行据此找到对应的子代理（同名任务可以派多次）。
    toolCallId: record.toolCallId || '',
    canonicalName: record.canonicalName,
    parentSessionId: record.parentSessionId,
    status: record.status,
    message: String(record.message || '').slice(0, 500),
    model: modelLabel(record.model),
    thinkingLevel: record.thinkingLevel,
    ...publicProfileFields(record),
    startedAt: record.startedAt,
    lastActivityAt: record.lastActivityAt,
    completedAt: record.completedAt,
    durationMs: record.durationMs,
    turnCount: record.turnCount,
    toolCallCount: record.toolCallCount,
    output: String(record.output || '').slice(0, 500),
    outputTruncated: record.outputTruncated,
    runNumber: record.runNumber,
    resultVersion: record.resultVersion,
    error: String(record.error || '').slice(0, 500),
    currentActivity: record.currentActivity ? summaryValue(record.currentActivity, -1) : null,
  }
}

export function agentCompletionPrompt(agent) {
  const output = String(agent.output || '').slice(0, 2_000)
  return `${AGENT_COMPLETION_MARKER}
Background agent "${agent.taskName}" (${agent.id}) has completed with status: ${agent.status}.

${output ? `Output:\n${output}` : 'No output.'}
${agent.error ? `\nError: ${agent.error}` : ''}
Use this result for your next actions. If other background agents are still running, briefly note the progress; if all background agents are done, summarize the combined results for the user.`
}

export function isAgentCompletionMessage(content) {
  return String(content || '').startsWith(AGENT_COMPLETION_MARKER)
}

export class MultiAgentService {
  constructor({
    path,
    agentDir,
    getModelRuntime,
    getSettingsManager,
    getCompactionThresholdPercent,
    createSession = createAgentSession,
    // 第二个参数是这个子代理对话记录的文件；没有时（未配置 transcriptDir）只留在内存里。
    createSessionManager = (cwd, file) =>
      file ? SessionManager.open(file, dirname(file), cwd) : SessionManager.inMemory(cwd),
    transcriptDir = '',
    createResourceLoader = createAgentResourceLoader,
    maxConcurrent = MAX_CONCURRENT_AGENTS,
    terminalSessionRetentionMs = TERMINAL_AGENT_SESSION_RETENTION_MS,
    setTimer = setTimeout,
    clearTimer = clearTimeout,
    now = () => Date.now(),
  } = {}) {
    this.path = path
    this.agentDir = agentDir
    this.getModelRuntime = getModelRuntime || (() => null)
    this.getSettingsManager = getSettingsManager || (() => null)
    this.getCompactionThresholdPercent = getCompactionThresholdPercent || (() => undefined)
    this.createSession = createSession
    this.createSessionManager = createSessionManager
    this.transcriptDir = transcriptDir ? String(transcriptDir) : ''
    this.createResourceLoader = createResourceLoader
    this.maxConcurrent = Math.max(1, Number(maxConcurrent) || MAX_CONCURRENT_AGENTS)
    this.terminalSessionRetentionMs = Math.max(0, Number(terminalSessionRetentionMs) || 0)
    this.setTimer = setTimer
    this.clearTimer = clearTimer
    this.now = now
    this.records = new Map()
    this.mailbox = new Map()
    this.mailboxWaiters = new Map()
    this.terminalSessionTimers = new Map()
    this.sequence = 0
    this.write = Promise.resolve()
    this.disposing = false
    // 新增：完成通知回调，用于向父会话注入消息
    this.completionNotifier = null
  }

  setCompletionNotifier(notifier) {
    // 完成通知回调：Agent 结束时通知父会话（注入隐藏消息或触发唤醒）。
    this.completionNotifier = notifier
    this.completionNotifier = typeof notifier === 'function' ? notifier : null
  }

  notifyCompletion(agent) {
    if (!this.completionNotifier) return
    try {
      this.completionNotifier(agent)
    } catch {
      // Completion notifications are best-effort and must not crash the agent service.
    }
  }

  async init() {
    // 加载注册表、清理遗留 active 状态（重启后视为 interrupted），并补偿队列。

    if (!this.path) return
    const state = await readJson(this.path, null)
    this.sequence = 0
    this.records.clear()
    this.mailbox.clear()
    if (!state) return
    if (state.version !== AGENT_REGISTRY_VERSION) {
      await this.save()
      void this.sweepTranscripts().catch(() => {})
      return
    }
    this.sequence = Math.max(0, Number(state.sequence) || 0)
    for (const value of Array.isArray(state.mailbox) ? state.mailbox : []) {
      const entry = restoredMailboxEntry(value)
      if (entry) this.mailbox.set(entry.mailboxId, entry)
    }
    let changed = false
    for (const value of Array.isArray(state?.records) ? state.records : []) {
      const record = restoredRecord(value)
      if (!record) continue
      if (ACTIVE_AGENT_STATUSES.has(record.status)) {
        const completedAt = new Date(this.now()).toISOString()
        record.status = 'interrupted'
        record.error = RESTART_INTERRUPTION_REASON
        record.tools = record.tools.map((tool) =>
          tool.status === 'running'
            ? { ...tool, status: 'error', message: RESTART_INTERRUPTION_REASON }
            : tool,
        )
        record.completedAt = completedAt
        record.lastActivityAt = completedAt
        record.durationMs = Math.max(0, this.now() - new Date(record.startedAt).getTime())
        record.resultVersion += 1
        this.enqueueMailbox(publicRecord(record))
        changed = true
      }
      this.records.set(record.id, record)
    }
    if (changed) await this.save()
    // 不等它：启动不依赖用户数据，清理失败下次启动再试。
    void this.sweepTranscripts().catch(() => {})
  }

  transcriptFile(record) {
    if (!this.transcriptDir || !record?.id || !record.parentSessionId) return ''
    return join(
      this.transcriptDir,
      transcriptSegment(record.parentSessionId),
      `${transcriptSegment(record.id)}.jsonl`,
    )
  }

  removeTranscript(record) {
    const file = this.transcriptFile(record)
    if (file) void unlink(file).catch(() => {})
  }

  // 登记表里已经没有的子代理（裁剪、父会话删除时进程没在跑、登记表换了版本）留下的记录文件与空目录。
  async sweepTranscripts() {
    if (!this.transcriptDir) return
    const known = new Set(
      [...this.records.values()].map((record) => this.transcriptFile(record)).filter(Boolean),
    )
    const parents = await readdir(this.transcriptDir, { withFileTypes: true }).catch(() => [])
    for (const parent of parents) {
      if (!parent.isDirectory()) continue
      const directory = join(this.transcriptDir, parent.name)
      const files = await readdir(directory).catch(() => [])
      let kept = 0
      for (const name of files) {
        const file = join(directory, name)
        if (known.has(file)) kept += 1
        else await unlink(file).catch(() => {})
      }
      if (!kept) await rm(directory, { recursive: true, force: true }).catch(() => {})
    }
  }

  // 只读查看一个子代理的对话：在内存里就用当前会话的消息（运行中也能看），否则读它的记录文件。
  // 返回 Pi 的原始消息，由运行时层投影成界面消息；不是这个父会话的子代理返回 null。
  transcriptSource(parentSessionId, agentId) {
    const record = this.records.get(String(agentId || ''))
    if (!record || record.parentSessionId !== parentSessionId) return null
    const agent = summaryRecord(record)
    const active = ACTIVE_AGENT_STATUSES.has(record.status)
    if (record.session?.messages) return { agent, active, messages: record.session.messages }
    const file = this.transcriptFile(record)
    if (!file || !existsSync(file)) return { agent, active, messages: null }
    try {
      return {
        agent,
        active,
        messages: SessionManager.open(file, dirname(file), record.cwd).buildSessionContext()
          .messages,
      }
    } catch {
      return { agent, active, messages: null }
    }
  }

  save() {
    if (!this.path) return Promise.resolve()
    const snapshot = {
      version: AGENT_REGISTRY_VERSION,
      sequence: this.sequence,
      records: [...this.records.values()].map(durableRecord),
      mailbox: [...this.mailbox.values()].map(durableMailboxEntry),
    }
    this.write = this.write.catch(() => {}).then(() => writeJsonAtomic(this.path, snapshot))
    return this.write
  }

  flush() {
    return this.write
  }

  // 邮箱入队：Agent 完成后把结果投递到父会话邮箱。
  enqueueMailbox(agent) {
    const mailboxId = `${agent.id}:${agent.resultVersion}`
    const existing = this.mailbox.get(mailboxId)
    if (existing) return existing
    const entry = {
      ...agent,
      fullOutput: String(agent.output || ''),
      mailboxId,
      queuedAt: new Date(this.now()).toISOString(),
    }
    this.mailbox.set(mailboxId, entry)
    return entry
  }

  peekMailbox(parentSessionId) {
    return [...this.mailbox.values()]
      .filter((entry) => entry.parentSessionId === parentSessionId)
      .sort((left, right) => left.queuedAt.localeCompare(right.queuedAt))
      .map((entry) => ({ ...entry }))
  }

  async acknowledge(parentSessionId, agents = []) {
    const deliveries = Array.isArray(agents) ? agents : []
    let changed = false
    for (const [mailboxId, entry] of this.mailbox) {
      if (entry.parentSessionId !== parentSessionId) continue
      const delivered = deliveries.some((agent) =>
        agent?.mailboxId
          ? agent.mailboxId === mailboxId
          : agent?.id === entry.id && Number(agent?.resultVersion) === entry.resultVersion,
      )
      if (!delivered) continue
      this.mailbox.delete(mailboxId)
      changed = true
    }
    if (changed) await this.save()
    return changed
  }

  list(parentSessionId) {
    return [...this.records.values()]
      .filter((record) => !parentSessionId || record.parentSessionId === parentSessionId)
      .sort((left, right) => left.startedAt.localeCompare(right.startedAt))
      .map(publicRecord)
  }

  summaries(parentSessionId) {
    return [...this.records.values()]
      .filter((record) => !parentSessionId || record.parentSessionId === parentSessionId)
      .sort((left, right) => left.startedAt.localeCompare(right.startedAt))
      .map(summaryRecord)
  }

  find(parentSessionId, target) {
    const value = String(target || '').trim()
    return [...this.records.values()]
      .reverse()
      .find(
        (record) =>
          record.parentSessionId === parentSessionId &&
          [record.id, record.taskName, record.canonicalName].includes(value),
      )
  }

  hasActive(parentSessionId) {
    return [...this.records.values()].some(
      (record) =>
        record.parentSessionId === parentSessionId && ACTIVE_AGENT_STATUSES.has(record.status),
    )
  }

  clearTerminalSessionTimer(record) {
    const timer = this.terminalSessionTimers.get(record.id)
    if (!timer) return
    this.clearTimer(timer)
    this.terminalSessionTimers.delete(record.id)
  }

  releaseTerminalSession(record) {
    this.clearTerminalSessionTimer(record)
    if (ACTIVE_AGENT_STATUSES.has(record.status) || record.slotActive) return false
    try {
      record.unsubscribe?.()
    } catch {}
    try {
      record.session?.dispose?.()
    } catch {}
    record.session = null
    record.unsubscribe = () => {}
    record.customTools = []
    record.createCustomTools = null
    try {
      void record.disposeCustomTools?.()
    } catch {}
    record.disposeCustomTools = null
    record.pendingMessages = []
    record.fullOutput = record.output
    record.onProgress = null
    record.onSession = null
    record.onCompleted = null
    record.onTerminal = null
    return true
  }

  scheduleTerminalSessionRelease(record) {
    this.clearTerminalSessionTimer(record)
    if (!TERMINAL_AGENT_STATUSES.has(record.status) || record.slotActive) return
    if (!this.terminalSessionRetentionMs) {
      this.releaseTerminalSession(record)
      return
    }
    const timer = this.setTimer(() => {
      this.terminalSessionTimers.delete(record.id)
      this.releaseTerminalSession(record)
    }, this.terminalSessionRetentionMs)
    timer?.unref?.()
    this.terminalSessionTimers.set(record.id, timer)
  }

  removeRecord(record) {
    this.releaseTerminalSession(record)
    this.records.delete(record.id)
    this.removeTranscript(record)
    for (const [mailboxId, entry] of this.mailbox) {
      if (entry.id === record.id) this.mailbox.delete(mailboxId)
    }
  }

  prune(parentSessionId) {
    const records = [...this.records.values()].filter(
      (record) => record.parentSessionId === parentSessionId,
    )
    const removable = records.filter((record) => !ACTIVE_AGENT_STATUSES.has(record.status))
    while (records.length >= MAX_AGENTS_PER_PARENT && removable.length) {
      const record = removable.shift()
      this.removeRecord(record)
      records.splice(records.indexOf(record), 1)
    }
    const globalRemovable = [...this.records.values()].filter(
      (record) => !ACTIVE_AGENT_STATUSES.has(record.status),
    )
    while (this.records.size >= MAX_AGENT_RECORDS && globalRemovable.length)
      this.removeRecord(globalRemovable.shift())
  }

  notifyMailbox(agent) {
    if (!TERMINAL_AGENT_STATUSES.has(agent?.status)) return
    const waiters = this.mailboxWaiters.get(agent.parentSessionId)
    if (!waiters?.size) return
    for (const waiter of [...waiters]) {
      if (waiter.matches(agent)) waiter.finish(agent, false)
    }
  }

  emit(record, onProgress) {
    record.lastActivityAt = new Date(this.now()).toISOString()
    try {
      onProgress?.(publicRecord(record))
    } catch {}
    void this.save().catch(() => {})
  }

  executingCount() {
    return [...this.records.values()].filter((record) => record.slotActive).length
  }

  async scheduleQueued() {
    if (this.disposing) return
    let slots = Math.max(0, this.maxConcurrent - this.executingCount())
    const queued = [...this.records.values()]
      .filter((record) => record.status === 'queued')
      .sort((left, right) => left.startedAt.localeCompare(right.startedAt))
    // 每个父会话自己的上限：满了的父会话这一轮跳过，后面的父会话照常取。
    const perParent = new Map()
    for (const record of this.records.values())
      if (record.slotActive)
        perParent.set(record.parentSessionId, (perParent.get(record.parentSessionId) || 0) + 1)
    for (const record of queued) {
      if (slots <= 0) break
      const running = perParent.get(record.parentSessionId) || 0
      if (record.parentLimit && running >= record.parentLimit) continue
      perParent.set(record.parentSessionId, running + 1)
      slots -= 1
      record.status = 'starting'
      record.slotActive = true
      record.currentActivity = {
        type: 'model',
        stage: 'starting',
        updatedAt: new Date(this.now()).toISOString(),
      }
      this.emit(record, record.onProgress)
      record.runningPromise = this.startRun(record, record.message).catch(() => {})
    }
    await this.save()
  }

  // 等待 Agent 结果（wait_agent）：从邮箱取结果或超时。
  async wait(parentSessionId, timeoutMs = 15_000, target = '') {
    const current = this.list(parentSessionId)
    const targetRecord = target ? this.find(parentSessionId, target) : null
    if (target && !targetRecord) throw new Error(`Unknown agent: ${target}`)
    const pendingMailbox = this.peekMailbox(parentSessionId)
    const pendingDelivery = targetRecord
      ? pendingMailbox.find((agent) => agent.id === targetRecord.id)
      : pendingMailbox[0]
    if (pendingDelivery) return { timedOut: false, agents: current, agent: pendingDelivery }
    if (targetRecord && TERMINAL_AGENT_STATUSES.has(targetRecord.status))
      return { timedOut: false, agents: current, agent: publicRecord(targetRecord) }
    const activeIds = new Set(
      current
        .filter((record) => ACTIVE_AGENT_STATUSES.has(record.status))
        .map((record) => record.id),
    )
    if (!activeIds.size) return { timedOut: false, agents: current, agent: null }
    const matches = targetRecord
      ? (agent) => agent.id === targetRecord.id
      : (agent) => activeIds.has(agent.id)
    return new Promise((resolve) => {
      let settled = false
      let timer = null
      let waiter = null
      const finish = (agent, timedOut) => {
        if (settled) return
        settled = true
        this.clearTimer(timer)
        const waiters = this.mailboxWaiters.get(parentSessionId)
        waiters?.delete(waiter)
        if (!waiters?.size) this.mailboxWaiters.delete(parentSessionId)
        resolve({ timedOut, agents: this.list(parentSessionId), agent: timedOut ? null : agent })
      }
      waiter = { matches, finish }
      const waiters = this.mailboxWaiters.get(parentSessionId) || new Set()
      waiters.add(waiter)
      this.mailboxWaiters.set(parentSessionId, waiters)
      timer = this.setTimer(
        () => finish(null, true),
        Math.max(250, Math.min(30_000, Number(timeoutMs) || 15_000)),
      )
      timer?.unref?.()
    })
  }

  // 派生子 Agent：校验并发与记录数量，排队执行，返回 Agent 记录；执行轮次由 Agent 自主结束或人工中断。
  async spawn({
    parentSessionId,
    cwd,
    model,
    thinkingLevel,
    agentProfile,
    profileInstructions,
    modelSource,
    modelTier,
    modelFallbackReasons,
    taskName,
    message,
    allowedTools,
    customTools,
    createCustomTools,
    onProgress,
    onSession,
    onCompleted,
    onTerminal,
    parentLimit,
    title,
    skillPaths,
    toolCallId,
  } = {}) {
    if (!parentSessionId) throw new Error('Agent requires a parent session.')
    if (!cwd) throw new Error('Agent requires a workspace directory.')
    if (!model) throw new Error('Agent requires an active parent model.')
    const normalizedMessage = String(message || '').trim()
    if (!normalizedMessage) throw new Error('Agent task cannot be empty.')
    if (normalizedMessage.length > MAX_AGENT_TASK_CHARS)
      throw new Error(`Agent task is limited to ${MAX_AGENT_TASK_CHARS} characters.`)
    this.prune(parentSessionId)
    const parentRecordCount = [...this.records.values()].filter(
      (record) => record.parentSessionId === parentSessionId,
    ).length
    if (parentRecordCount >= MAX_AGENTS_PER_PARENT)
      throw new Error(`Agent record limit reached for this session (${MAX_AGENTS_PER_PARENT}).`)
    if (this.records.size >= MAX_AGENT_RECORDS)
      throw new Error(`Global Agent record limit reached (${MAX_AGENT_RECORDS}).`)
    const id = randomUUID()
    const normalizedName = normalizeTaskName(taskName)
    const record = {
      id,
      taskName: normalizedName,
      // 给人看的标题（Bot 派活时写的中文标题等）；没有时界面退回 taskName。
      title: String(title || '')
        .trim()
        .slice(0, 80),
      toolCallId: toolCallIdValue(toolCallId),
      canonicalName: `/root/${normalizedName}_${++this.sequence}`,
      parentSessionId,
      cwd,
      model,
      thinkingLevel: thinkingLevel || 'medium',
      ...profileFields({
        agentProfile,
        profileInstructions,
        modelSource,
        modelTier,
        modelFallbackReasons,
      }),
      message: normalizedMessage,
      availableTools: childTools(allowedTools),
      customTools,
      createCustomTools: typeof createCustomTools === 'function' ? createCustomTools : null,
      disposeCustomTools: null,
      turnCount: 0,
      toolCallCount: 0,
      tools: [],
      output: '',
      fullOutput: '',
      outputTruncated: false,
      usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, totalTokens: 0 },
      runUsage: emptyUsage(),
      runNumber: 0,
      runGeneration: 0,
      resultVersion: 0,
      slotActive: false,
      error: '',
      startedAt: new Date(this.now()).toISOString(),
      lastActivityAt: null,
      completedAt: null,
      durationMs: null,
      status: 'queued',
      currentActivity: { type: 'queue', updatedAt: new Date(this.now()).toISOString() },
      session: null,
      unsubscribe: () => {},
      aborted: false,
      abortReason: '',
      runningPromise: Promise.resolve(),
      restartChain: Promise.resolve(),
      pendingMessages: [],
      onProgress,
      onSession,
      onCompleted,
      onTerminal,
      // 这个父会话同时最多跑几个（Bot 的对话按设定，默认不限，只受全局上限）。
      parentLimit: Number.isInteger(parentLimit) && parentLimit > 0 ? parentLimit : null,
      // Bot 派的活带上它自己写的技能目录，技能加载器把它们列进子 agent 的系统提示。
      skillPaths: skillPathList(skillPaths),
    }
    this.records.set(id, record)
    this.emit(record, onProgress)
    await this.save()
    await this.scheduleQueued()
    return publicRecord(record)
  }

  // 启动子 Agent 运行：创建隔离会话、注入工具，执行到结束并归账。
  async startRun(record, message) {
    this.clearTerminalSessionTimer(record)
    const generation = ++record.runGeneration
    const startedAt = this.now()
    const isCurrent = () => record.runGeneration === generation
    try {
      if (!isCurrent()) return
      if (record.aborted) throw new Error(record.abortReason || 'Agent was interrupted.')
      if (!record.session) {
        const settingsManager = createCompactionSettingsManager(
          this.getSettingsManager(),
          () => record.model?.contextWindow,
          this.getCompactionThresholdPercent,
        )
        const resourceLoader = await this.createResourceLoader({
          cwd: record.cwd,
          agentDir: this.agentDir || record.cwd,
          settingsManager,
          skillPaths: record.skillPaths || [],
          appendSystemPrompt: record.profileInstructions
            ? `${MULTI_AGENT_SYSTEM_PROMPT}\n\n## Agent definition: ${record.agentProfile}\n\nStanding instructions from this agent definition apply to every task you receive; follow them together with the delegated task.\n\n${record.profileInstructions}`
            : MULTI_AGENT_SYSTEM_PROMPT,
        })
        if (!isCurrent()) return
        if (record.aborted) throw new Error(record.abortReason || 'Agent was interrupted.')
        const sessionManager = this.createSessionManager(record.cwd, this.transcriptFile(record))
        let suppliedCustomTools = record.customTools
        if (record.createCustomTools) {
          const created = await record.createCustomTools({
            id: record.id,
            parentSessionId: record.parentSessionId,
            cwd: record.cwd,
            availableTools: [...record.availableTools],
          })
          suppliedCustomTools = Array.isArray(created) ? created : created?.tools
          record.disposeCustomTools = Array.isArray(created) ? null : created?.dispose || null
        }
        const childCustomTools = inheritedCustomTools(record.availableTools, suppliedCustomTools)
        const result = await this.createSession({
          cwd: record.cwd,
          agentDir: this.agentDir,
          model: record.model,
          thinkingLevel: record.thinkingLevel,
          modelRuntime: this.getModelRuntime(),
          settingsManager,
          resourceLoader,
          sessionManager,
          tools: record.availableTools,
          excludeTools: [...PARENT_ONLY_TOOL_NAMES],
          ...(childCustomTools.length ? { customTools: childCustomTools } : {}),
        })
        if (!isCurrent()) {
          try {
            result?.session?.dispose?.()
          } catch {}
          return
        }
        if (record.aborted) {
          try {
            result?.session?.dispose?.()
          } catch {}
          throw new Error(record.abortReason || 'Agent was interrupted.')
        }
        if (!result?.session) throw new Error('Agent session could not be created.')
        record.session = result.session
        applyPisperSystemPrompt(record.session, record.model)
        record.onSession?.(record.session)
        let lastProgressEmitAt = this.now()
        record.unsubscribe = record.session.subscribe((event) => {
          // 会话会跨 follow-up 复用；细粒度文本事件只做租约心跳，状态变化才立即外发。
          let shouldEmit = false
          if (event.type === 'turn_start') {
            shouldEmit = true
            record.turnCount += 1
            record.currentActivity = {
              type: 'model',
              stage: 'thinking',
              updatedAt: new Date(this.now()).toISOString(),
            }
          } else if (event.type === 'tool_execution_start') {
            shouldEmit = true
            record.toolCallCount += 1
            const tool = {
              type: 'tool',
              id: event.toolCallId,
              name: event.toolName,
              args: event.args,
              status: 'running',
              startedAt: new Date(this.now()).toISOString(),
            }
            record.tools.push(tool)
            record.currentActivity = tool
          } else if (event.type === 'tool_execution_end') {
            shouldEmit = true
            const finishedAt = new Date(this.now()).toISOString()
            record.tools = record.tools.map((tool) =>
              tool.id === event.toolCallId
                ? { ...tool, status: event.isError ? 'error' : 'done', finishedAt }
                : tool,
            )
            record.currentActivity = event.isError
              ? {
                  ...(record.currentActivity || {}),
                  type: 'tool',
                  id: event.toolCallId,
                  name: event.toolName,
                  status: 'error',
                  finishedAt,
                }
              : { type: 'model', stage: 'processing_result', updatedAt: finishedAt }
          }
          const observedAt = this.now()
          if (!shouldEmit && observedAt - lastProgressEmitAt >= AGENT_PROGRESS_HEARTBEAT_MS)
            shouldEmit = true
          if (shouldEmit) {
            lastProgressEmitAt = observedAt
            this.emit(record, record.onProgress)
          } else record.lastActivityAt = new Date(observedAt).toISOString()
        })
      }

      if (!isCurrent()) return
      const usageBeforeRun = usageTotal(record.session.messages)
      record.runNumber += 1
      record.message = message
      record.turnCount = 0
      record.toolCallCount = 0
      record.tools = []
      record.output = ''
      record.fullOutput = ''
      record.outputTruncated = false
      record.runUsage = emptyUsage()
      record.status = 'running'
      record.currentActivity = {
        type: 'model',
        stage: 'thinking',
        updatedAt: new Date(this.now()).toISOString(),
      }
      record.error = ''
      record.completedAt = null
      record.durationMs = null
      this.emit(record, record.onProgress)
      const promptPromise = record.session.prompt(message)
      try {
        const pendingMessages = record.pendingMessages.splice(0)
        for (const pending of pendingMessages) await record.session[pending.behavior](pending.text)
      } catch (error) {
        try {
          await record.session.abort?.()
        } catch {}
        await promptPromise.catch(() => {})
        throw error
      }
      await promptPromise
      if (!isCurrent()) return
      if (record.aborted) throw new Error(record.abortReason || 'Agent was interrupted.')
      const last = [...record.session.messages].reverse().find((item) => item?.role === 'assistant')
      if (last?.errorMessage) throw new Error(last.errorMessage)
      const output = contextLimitedText(textFromContent(last?.content), record.model)
      record.output = output.text || '(Agent returned no text output.)'
      record.fullOutput = record.output
      record.outputTruncated = output.truncated
      record.usage = usageTotal(record.session.messages)
      record.runUsage = usageDifference(record.usage, usageBeforeRun)
      record.status = 'completed'
      record.currentActivity = null
      record.completedAt = new Date(this.now()).toISOString()
      record.lastActivityAt = record.completedAt
      record.durationMs = this.now() - startedAt
      record.resultVersion += 1
      const terminal = publicRecord(record)
      const delivery = this.enqueueMailbox(terminal)
      this.emit(record, record.onProgress)
      await this.save()
      this.notifyMailbox(delivery)
      this.notifyCompletion(terminal)
      try {
        await record.onCompleted?.(terminal)
      } catch {}
      try {
        await record.onTerminal?.(terminal)
      } catch {}
    } catch (error) {
      if (!isCurrent()) return
      const wasTerminal = TERMINAL_AGENT_STATUSES.has(record.status) && Boolean(record.completedAt)
      record.error = error instanceof Error ? error.message : String(error)
      record.status = record.aborted ? 'interrupted' : 'failed'
      record.currentActivity = null
      record.tools = record.tools.map((tool) =>
        tool.status === 'running' ? { ...tool, status: 'error', message: record.error } : tool,
      )
      record.completedAt ||= new Date(this.now()).toISOString()
      record.lastActivityAt = record.completedAt
      record.durationMs ??= this.now() - startedAt
      if (!wasTerminal) record.resultVersion += 1
      record.pendingMessages = []
      const terminal = !wasTerminal ? publicRecord(record) : null
      const delivery = terminal ? this.enqueueMailbox(terminal) : null
      this.emit(record, record.onProgress)
      await this.save()
      if (terminal) {
        this.notifyMailbox(delivery)
        this.notifyCompletion(terminal)
        try {
          await record.onTerminal?.(terminal)
        } catch {}
      }
    } finally {
      if (record.runGeneration === generation) {
        record.slotActive = false
        this.scheduleTerminalSessionRelease(record)
      }
      try {
        await this.scheduleQueued()
      } catch {}
    }
  }

  async sendMessageFromAgent(parentSessionId, senderId, target, message) {
    const sender = this.find(parentSessionId, senderId)
    if (!sender) throw new Error(`Unknown sending agent: ${senderId}`)
    if (sender.id === this.find(parentSessionId, target)?.id)
      throw new Error('An Agent cannot send a message to itself.')
    if (!['starting', 'running'].includes(sender.status))
      throw new Error('Only a starting or running Agent can send team messages.')
    return this.sendMessage(parentSessionId, target, message)
  }

  async sendMessage(parentSessionId, target, message) {
    const record = this.find(parentSessionId, target)
    if (!record) throw new Error(`Unknown agent: ${target}`)
    const text = String(message || '').trim()
    if (!text) throw new Error('Agent message cannot be empty.')
    if (text.length > MAX_AGENT_TASK_CHARS)
      throw new Error(`Agent message is limited to ${MAX_AGENT_TASK_CHARS} characters.`)
    if (['queued', 'starting'].includes(record.status)) {
      record.pendingMessages.push({ behavior: 'steer', text })
      this.emit(record, record.onProgress)
      return publicRecord(record)
    }
    if (record.status !== 'running' || !record.session)
      throw new Error('Agent is not running. Use followup_task to start another run.')
    await record.session.steer(text)
    this.emit(record, record.onProgress)
    return publicRecord(record)
  }

  async followup(parentSessionId, target, message) {
    const record = this.find(parentSessionId, target)
    if (!record) throw new Error(`Unknown agent: ${target}`)
    const text = String(message || '').trim()
    if (!text) throw new Error('Follow-up task cannot be empty.')
    if (text.length > MAX_AGENT_TASK_CHARS)
      throw new Error(`Follow-up task is limited to ${MAX_AGENT_TASK_CHARS} characters.`)
    this.clearTerminalSessionTimer(record)

    if (['queued', 'starting'].includes(record.status)) {
      record.pendingMessages.push({ behavior: 'followUp', text })
      this.emit(record, record.onProgress)
      return publicRecord(record)
    }
    if (record.status === 'running') {
      if (!record.session) throw new Error('Agent session is not available.')
      await record.session.followUp(text)
      this.emit(record, record.onProgress)
      return publicRecord(record)
    }

    // Serialize restarts and wait for the previous run to fully settle before clearing abort state.
    const restart = async () => {
      await (record.runningPromise || Promise.resolve()).catch(() => {})
      if (['queued', 'starting'].includes(record.status)) {
        record.pendingMessages.push({ behavior: 'followUp', text })
        this.emit(record, record.onProgress)
        return publicRecord(record)
      }
      if (record.status === 'running') {
        if (!record.session) throw new Error('Agent session is not available.')
        await record.session.followUp(text)
        this.emit(record, record.onProgress)
        return publicRecord(record)
      }
      if (!record.session)
        // 上下文已随终态清理：给出可操作的恢复路径，避免调用方卡在死胡同。
        throw new Error(
          'Agent context expired from memory. Spawn a new Agent with the same taskName to continue this task.',
        )
      record.aborted = false
      record.abortReason = ''
      record.startedAt = new Date(this.now()).toISOString()
      record.message = text
      record.status = 'queued'
      record.currentActivity = { type: 'queue', updatedAt: record.startedAt }
      record.error = ''
      record.completedAt = null
      record.durationMs = null
      this.emit(record, record.onProgress)
      await this.scheduleQueued()
      return publicRecord(record)
    }

    record.restartChain = (record.restartChain || Promise.resolve()).then(restart, restart)
    return record.restartChain
  }

  // 中断 Agent：标记记录并尝试中止其会话。
  interrupt(
    parentSessionId,
    target,
    reason = 'Agent was interrupted.',
    schedule = true,
    { notifyParent = true } = {},
  ) {
    const record = this.find(parentSessionId, target)
    if (!record) throw new Error(`Unknown agent: ${target}`)
    if (!ACTIVE_AGENT_STATUSES.has(record.status)) return publicRecord(record)
    record.aborted = true
    record.abortReason = reason
    record.status = 'interrupted'
    record.currentActivity = null
    record.error = reason
    record.tools = record.tools.map((tool) =>
      tool.status === 'running' ? { ...tool, status: 'error', message: reason } : tool,
    )
    record.completedAt = new Date(this.now()).toISOString()
    record.lastActivityAt = record.completedAt
    record.durationMs = Math.max(0, this.now() - new Date(record.startedAt).getTime())
    record.resultVersion += 1
    record.pendingMessages = []
    try {
      const aborting = record.session?.abort?.()
      if (aborting?.catch) void aborting.catch(() => {})
    } catch {}
    const terminal = publicRecord(record)
    const delivery = this.enqueueMailbox(terminal)
    this.emit(record, record.onProgress)
    void this.flush()
      .then(() => {
        this.notifyMailbox(delivery)
        if (notifyParent) this.notifyCompletion(terminal)
        return record.onTerminal?.(terminal)
      })
      .catch(() => {})
    if (!record.slotActive) this.scheduleTerminalSessionRelease(record)
    if (schedule) void this.scheduleQueued().catch(() => {})
    return terminal
  }

  // 父会话中止/删除时终止其全部子 Agent。
  abortParent(parentSessionId, reason = 'Agent was cancelled because the parent session stopped.') {
    let count = 0
    for (const record of this.records.values()) {
      if (record.parentSessionId !== parentSessionId || !ACTIVE_AGENT_STATUSES.has(record.status))
        continue
      // 是用户停下的，不再把“子代理已结束、请继续下一步”排给父会话：
      // 否则它会跟在用户的下一条消息后面，模型去接着干原来的活，新消息像没发过一样。
      this.interrupt(parentSessionId, record.id, reason, false, { notifyParent: false })
      count += 1
    }
    void this.scheduleQueued().catch(() => {})
    return count
  }

  // 清理父会话的全部 Agent 记录（删除会话时调用）。
  async removeParent(parentSessionId) {
    this.abortParent(parentSessionId)
    for (const [id, record] of this.records) {
      if (record.parentSessionId !== parentSessionId) continue
      this.clearTerminalSessionTimer(record)
      try {
        record.unsubscribe?.()
      } catch {}
      try {
        record.session?.dispose?.()
      } catch {}
      try {
        await record.disposeCustomTools?.()
      } catch {}
      this.records.delete(id)
    }
    for (const [mailboxId, entry] of this.mailbox) {
      if (entry.parentSessionId === parentSessionId) this.mailbox.delete(mailboxId)
    }
    await this.save()
    if (this.transcriptDir)
      await rm(join(this.transcriptDir, transcriptSegment(parentSessionId)), {
        recursive: true,
        force: true,
      }).catch(() => {})
  }

  async dispose() {
    this.disposing = true
    for (const record of this.records.values()) {
      if (ACTIVE_AGENT_STATUSES.has(record.status))
        this.interrupt(record.parentSessionId, record.id, 'Agent service is shutting down.', false)
      this.clearTerminalSessionTimer(record)
      try {
        record.unsubscribe?.()
      } catch {}
      try {
        record.session?.dispose?.()
      } catch {}
      try {
        await record.disposeCustomTools?.()
      } catch {}
    }
    await this.flush()
    this.records.clear()
    this.mailbox.clear()
    for (const waiters of this.mailboxWaiters.values())
      for (const waiter of [...waiters]) waiter.finish(null, false)
    this.mailboxWaiters.clear()
  }
}
