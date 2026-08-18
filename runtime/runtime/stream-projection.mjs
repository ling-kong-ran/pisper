// 流投影：把会话的引擎内部状态（消息、工具、用量、压缩、生命周期）投影成前端可消费的
// 视图（transcript 消息、实时快照、上下文用量、历史消息页）。带多级缓存（投影缓存/历史
// 消息缓存/上下文用量缓存），并在源数据未变时直接命中缓存避免重复序列化。
import { createHash } from 'node:crypto'
import { open, stat } from 'node:fs/promises'
import { calculateContextTokens, estimateTokens } from './pi-coding-agent.mjs'
import {
  MULTI_AGENT_TOOL_NAMES,
  isAgentCompletionMessage,
} from '../services/multi-agent-service.mjs'
import { isGoalContinuationMessage } from '../services/goal-service.mjs'
import { PLAN_ALL_TOOL_NAMES } from '../tools/app/plan.mjs'
import { attachGeneratedAssets } from '../services/session-assets.mjs'
import { assetHasSource, generatedAssetsForSession } from '../services/asset-storage.mjs'
import { permissionModeForExecutionMode } from '../security/execution-mode.mjs'
import { effectiveCompactionSettings } from './compaction-policy.mjs'
import { isCompletedTurnBoundaryMessage } from './session-derivation.mjs'

// 活动流长度/思考文本上限：限制实时视图的内存与传输量。
export const MAX_LIVE_ACTIVITY_ITEMS = 6
export const MAX_LIVE_THINKING_CHARS = 6_000
const DEFAULT_MESSAGE_PAGE_SIZE = 40
const MAX_MESSAGE_PAGE_SIZE = 100
const LIVE_MESSAGE_PAGE_SIZE = 60
const SESSION_HISTORY_READ_CHUNK_BYTES = 1024 * 1024
const MAX_SESSION_HISTORY_CACHE_ENTRIES = 4
const MAX_SESSION_HISTORY_CACHE_SOURCE_BYTES = 8 * 1024 * 1024
const MAX_SESSION_HISTORY_CACHE_ESTIMATED_BYTES = 48 * 1024 * 1024
const SESSION_HISTORY_CACHE_MEMORY_MULTIPLIER = 4
const MAX_PROJECTION_CACHE_ENTRIES = 8
const MAX_PROJECTION_CACHE_BYTES = 24 * 1024 * 1024
// 冷会话上下文用量缓存上限：条目很小且纯派生，驱逐不参与会话存在性判断
// （findSessionInfo 已在查缓存前执行）。
const MAX_SESSION_CONTEXT_USAGE_CACHE_ENTRIES = 64
const ATTACHMENT_MARKER = '\n\n---\nAttachment context (injected by Pisper):\n'

// 内部注入消息（目标延续/Agent 完成）不展示给用户，也不进入排队列表。
export function isInternalParentMessage(content) {
  return isGoalContinuationMessage(content) || isAgentCompletionMessage(content)
}

// 从消息内容（字符串或内容块数组）中提取纯文本。
export function textFromContent(content) {
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content
    .filter((part) => part?.type === 'text')
    .map((part) => part.text || '')
    .join('')
}

// 思考文本只保留尾部（流式场景下旧内容对用户已无意义），降低传输与渲染成本。
export function liveThinkingTail(value) {
  return String(value || '').slice(-MAX_LIVE_THINKING_CHARS)
}

// 新文本块开始时清空 live.text（旧块内容已由 text_end 提交）。
export function beginTextBlock(activeBlocks, blockIndex, live, emit) {
  if (activeBlocks.has(blockIndex)) return
  activeBlocks.add(blockIndex)
  if (live.text) emit('text_patch', { start: 0, text: '', updatedAt: live.lastActivityAt })
  live.text = ''
}

// 活动去重键：同类型同对象的活动合并，避免重复条目堆积。
function liveActivityKey(activity) {
  if (!activity?.type) return ''
  if (activity.type === 'tool') return `tool:${activity.id || activity.name || ''}`
  if (activity.type === 'agent')
    return `agent:${activity.agent?.id || activity.agent?.canonicalName || ''}`
  if (activity.type === 'plan')
    return `plan:${activity.updatedAt || activity.plan?.updatedAt || ''}`
  if (activity.type === 'model') return `model:${activity.stage || ''}`
  if (activity.type === 'compaction')
    return `compaction:${activity.compaction?.status || activity.compaction?.active || ''}`
  return `${activity.type}:${activity.id || activity.updatedAt || ''}`
}

// 活动流追加：计划/Agent 事件会替换同名的工具条目（工具本身不再重复展示），
// 并按活动键去重、只保留最近 N 条。
export function pushLiveActivity(feed, activity) {
  const current = Array.isArray(feed) ? feed : []
  if (!['tool', 'plan', 'agent'].includes(activity?.type)) return current
  let next = [...current]
  if (activity.type === 'plan') {
    next = next.filter((item) => item?.type !== 'tool' || !PLAN_ALL_TOOL_NAMES.includes(item.name))
  }
  if (activity.type === 'agent') {
    next = next.filter(
      (item) => item?.type !== 'tool' || !MULTI_AGENT_TOOL_NAMES.includes(item.name),
    )
  }
  const key = liveActivityKey(activity)
  const existingIndex = next.findIndex((item) => liveActivityKey(item) === key)
  if (existingIndex >= 0) next[existingIndex] = { ...next[existingIndex], ...activity }
  else next.push(activity)
  return next.slice(-MAX_LIVE_ACTIVITY_ITEMS)
}

export function setLiveActivity(live, activity) {
  if (!live) return
  live.currentActivity = activity || null
  live.activityFeed = activity ? pushLiveActivity(live.activityFeed, activity) : []
}

// 计算计划项变化摘要（新增/更新/移除），供前端展示计划变更。
export function livePlanChanges(previous, next) {
  const previousItems = new Map((previous?.items || []).map((item) => [item.id, item]))
  const nextItems = new Map((next?.items || []).map((item) => [item.id, item]))
  const changes = []
  for (const item of nextItems.values()) {
    const before = previousItems.get(item.id)
    if (!before)
      changes.push({ id: item.id, title: item.title, status: item.status, kind: 'added' })
    else if (
      before.status !== item.status ||
      before.title !== item.title ||
      before.note !== item.note ||
      before.assignee !== item.assignee ||
      JSON.stringify(before.dependsOn || []) !== JSON.stringify(item.dependsOn || [])
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
    if (!nextItems.has(item.id)) {
      changes.push({ id: item.id, title: item.title, status: item.status, kind: 'removed' })
    }
  }
  return changes
}

// 排队中的会话输入（steer/followUp），过滤内部消息并去掉附件注入标记。
export function queuedSessionInputs(session) {
  const steering =
    typeof session?.getSteeringMessages === 'function' ? session.getSteeringMessages() : []
  const followUp =
    typeof session?.getFollowUpMessages === 'function' ? session.getFollowUpMessages() : []
  return [
    ...steering
      .filter((text) => !isInternalParentMessage(text))
      .map((text) => ({ behavior: 'steer', text: text.split(ATTACHMENT_MARKER)[0] })),
    ...followUp
      .filter((text) => !isInternalParentMessage(text))
      .map((text) => ({ behavior: 'followUp', text: text.split(ATTACHMENT_MARKER)[0] })),
  ]
}

// 单条消息 → 前端消息结构；用户消息剥离注入的附件上下文，助手消息带错误信息。
function serializeMessage(message, index, resolveImageUrl = null) {
  if (!message || !['user', 'assistant'].includes(message.role)) return null
  const rawText = textFromContent(message.content)
  if (message.role === 'user' && isInternalParentMessage(rawText)) return null
  const text = message.role === 'user' ? rawText.split(ATTACHMENT_MARKER)[0] : rawText
  if (!text) return null
  const attachments = Array.isArray(message.content)
    ? message.content
        .filter((part) => part?.type === 'image')
        .map((part, attachmentIndex) => {
          const attachment = {
            id: `image-${index}-${attachmentIndex}`,
            kind: 'image',
            name: `图片附件 ${attachmentIndex + 1}`,
            mimeType: part.mimeType,
          }
          const url = resolveImageUrl?.(part)
          if (url) attachment.url = url
          else attachment.data = part.data
          return attachment
        })
    : []
  return {
    id: `${message.role}-${message.timestamp || index}-${index}`,
    role: message.role === 'assistant' ? 'agent' : 'user',
    text,
    timestamp: message.timestamp || null,
    error: message.role === 'assistant' ? message.errorMessage || null : null,
    attachments,
  }
}

function serializedTimestamp(value) {
  if (value == null || value === '') return null
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? null : date.toISOString()
}

// 整个会话消息流 → 前端 transcript：把工具调用/思考/文本块归并为带 runActivity 的消息，
// 附带回合边界（turnBoundaryEntryId）供会话树定位。
export function serializeTranscriptMessages(messages, resolveImageUrl = null, entryIds = []) {
  const result = []
  let thinkingParts = []
  let tools = new Map()
  let startedAt = null
  let lastActivityAt = null
  let lastActivityMessage = null
  let runResultIndex = null
  let pendingRunItem = null

  const hasActivity = () => thinkingParts.length > 0 || tools.size > 0
  const resetRun = () => {
    thinkingParts = []
    tools = new Map()
    startedAt = null
    lastActivityAt = null
    lastActivityMessage = null
    runResultIndex = null
    pendingRunItem = null
  }
  const finishRun = ({ terminal = false } = {}) => {
    if (!hasActivity()) return
    let item = runResultIndex == null ? null : result[runResultIndex]
    if (!item && pendingRunItem) {
      item = pendingRunItem
      result.push(item)
      runResultIndex = result.length - 1
    }
    if (!item) {
      const { message, index } = lastActivityMessage || { message: {}, index: result.length }
      item = {
        id: `assistant-${message.timestamp || index}-${index}`,
        role: 'agent',
        text: '',
        timestamp: message.timestamp || null,
        error: message.errorMessage || null,
        attachments: [],
      }
      result.push(item)
    }
    const unresolvedMessage = String(
      item.error || (!terminal ? '工具调用未记录完成结果。' : ''),
    ).trim()
    const activityTools = [...tools.values()].slice(-MAX_LIVE_ACTIVITY_ITEMS).map((tool) =>
      tool.status === 'running'
        ? {
            ...tool,
            status: unresolvedMessage ? 'error' : 'done',
            message: unresolvedMessage || tool.message || '',
            updatedAt: lastActivityAt || tool.updatedAt,
            finishedAt: lastActivityAt || tool.updatedAt || tool.startedAt,
          }
        : tool,
    )
    item.runActivity = {
      thinkingText: thinkingParts.join('\n\n').slice(-MAX_LIVE_THINKING_CHARS),
      tools: activityTools,
      activityFeed: activityTools,
      startedAt,
      lastActivityAt,
      finishedAt: lastActivityAt,
    }
  }

  for (const [index, message] of (messages || []).entries()) {
    if (message?.role === 'toolResult') {
      const tool = tools.get(message.toolCallId)
      if (!tool) continue
      const finishedAt = serializedTimestamp(message.timestamp)
      const output = textFromContent(message.content).slice(-MAX_LIVE_THINKING_CHARS)
      Object.assign(tool, {
        status: message.isError ? 'error' : 'done',
        message: message.isError ? output.replace(/\s+/g, ' ').trim().slice(0, 180) : '',
        updatedAt: finishedAt || tool.updatedAt,
        finishedAt,
        ...(tool.name === 'bash' ? { output } : {}),
      })
      lastActivityAt = finishedAt || lastActivityAt
      continue
    }

    if (message?.role === 'assistant') {
      const timestamp = serializedTimestamp(message.timestamp)
      const content = Array.isArray(message.content) ? message.content : []
      const thinking = content
        .filter((part) => part?.type === 'thinking')
        .map((part) => String(part.thinking || ''))
        .filter(Boolean)
      if (thinking.length) thinkingParts.push(...thinking)
      const toolCalls = content.filter((part) => part?.type === 'toolCall')
      for (const call of toolCalls) {
        const activity = {
          type: 'tool',
          id: call.id,
          name: call.name,
          args: call.arguments || {},
          status: 'running',
          startedAt: timestamp,
          updatedAt: timestamp,
          ...(call.name === 'bash' ? { output: '' } : {}),
        }
        tools.set(call.id, activity)
      }
      if (thinking.length || toolCalls.length) {
        if (!startedAt) startedAt = timestamp
        lastActivityAt = timestamp || lastActivityAt
        lastActivityMessage = { message, index }
      }

      const serialized = serializeMessage(message, index, resolveImageUrl)
      const terminal = message.stopReason
        ? message.stopReason !== 'toolUse'
        : toolCalls.length === 0
      // Pi 每个模型回合持久化一条 assistant 消息，工具回合会包含多段评论文本块，
      // 而实时 UI 有意用同一响应体替换每个块；因此只保留最新非终态块作为候选，
      // 在回合到达终态响应时输出一条 ChatMessage。
      if (!terminal && serialized && hasActivity()) {
        pendingRunItem = serialized
      } else if (serialized || (terminal && hasActivity())) {
        const item = serialized || {
          id: `${message.role}-${message.timestamp || index}-${index}`,
          role: 'agent',
          text: pendingRunItem?.text || '',
          timestamp: message.timestamp || null,
          error: message.errorMessage || null,
          attachments: pendingRunItem?.attachments || [],
        }
        const boundaryEntryId = String(entryIds[index] || '')
        if (boundaryEntryId && isCompletedTurnBoundaryMessage(message)) {
          item.turnBoundaryEntryId = boundaryEntryId
        }
        result.push(item)
        runResultIndex = result.length - 1
      }
      if (terminal) {
        finishRun({ terminal: true })
        resetRun()
      }
      continue
    }

    const serialized = serializeMessage(message, index, resolveImageUrl)
    if (serialized) result.push(serialized)
  }

  finishRun()
  return result
}

export function optionalTokenCount(value) {
  const tokens = Number(value)
  return Number.isFinite(tokens) ? Math.max(0, Math.round(tokens)) : null
}

function usageTokenCount(value) {
  const tokens = Number(value)
  return Number.isFinite(tokens) ? Math.max(0, tokens) : 0
}

export function emptySessionUsage() {
  return {
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    reasoning: 0,
    totalTokens: 0,
    processedTokens: 0,
    requests: 0,
    promptTokens: 0,
    cacheHitRate: null,
  }
}

export function addSessionUsage(target, usage) {
  if (!usage) return target
  const input = usageTokenCount(usage.input)
  const output = usageTokenCount(usage.output)
  const cacheRead = usageTokenCount(usage.cacheRead)
  const cacheWrite = usageTokenCount(usage.cacheWrite)
  const reasoning = usageTokenCount(usage.reasoning)
  const reportedTotal = usageTokenCount(usage.totalTokens ?? usage.total)
  target.input += input
  target.output += output
  target.cacheRead += cacheRead
  target.cacheWrite += cacheWrite
  target.reasoning += reasoning
  // 推理 token 属于输出的一部分，若再加一次会重复计算生成量。
  target.totalTokens += reportedTotal || input + output + cacheRead + cacheWrite
  target.processedTokens += input + output + cacheWrite
  target.requests += 1
  target.promptTokens = target.input + target.cacheRead + target.cacheWrite
  target.cacheHitRate = target.promptTokens ? (target.cacheRead / target.promptTokens) * 100 : null
  return target
}

// 汇总一组消息的用量（只统计带 usage 的 assistant 消息）。
export function summarizeSessionUsage(messages = []) {
  const total = emptySessionUsage()
  for (const message of messages) {
    if (message?.role === 'assistant' && message.usage) addSessionUsage(total, message.usage)
  }
  return total
}

// 压缩开始状态构造。
export function startedCompaction(reason, startedAt) {
  return {
    active: true,
    status: 'running',
    reason: ['manual', 'threshold', 'overflow'].includes(reason) ? reason : 'threshold',
    startedAt,
    finishedAt: null,
    tokensBefore: null,
    estimatedTokensAfter: null,
    tokensSaved: null,
    aborted: false,
    willRetry: false,
    error: '',
  }
}

// 有效助手用量：跳过中止/出错消息。
function validAssistantUsage(message) {
  if (
    message?.role !== 'assistant' ||
    message.stopReason === 'aborted' ||
    message.stopReason === 'error' ||
    !message.usage
  )
    return null
  return calculateContextTokens(message.usage) > 0 ? message.usage : null
}

// 估算消息上下文 token：以最近的 valid usage 为锚点（锚点前用 calculateContextTokens），
// 锚点后逐条 estimateTokens，兼顾精度与成本。
function estimateMessageContextTokens(messages = []) {
  let usageIndex = -1
  let tokens = 0
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const usage = validAssistantUsage(messages[index])
    if (!usage) continue
    usageIndex = index
    tokens = calculateContextTokens(usage)
    break
  }
  const start = usageIndex >= 0 ? usageIndex + 1 : 0
  for (let index = start; index < messages.length; index += 1) {
    tokens += estimateTokens(messages[index])
  }
  return Math.max(0, Math.round(tokens))
}

function persistedContextUsage(manager, contextWindow) {
  if (!manager || !contextWindow) return undefined
  const branch = manager.getBranch()
  const latestCompactionIndex = branch.findLastIndex((entry) => entry?.type === 'compaction')
  if (latestCompactionIndex >= 0) {
    const hasPostCompactionUsage = branch
      .slice(latestCompactionIndex + 1)
      .some((entry) => entry?.type === 'message' && validAssistantUsage(entry.message))
    if (!hasPostCompactionUsage) return { tokens: null, contextWindow, percent: null }
  }
  const tokens = estimateMessageContextTokens(manager.buildSessionContext().messages)
  return { tokens, contextWindow, percent: (tokens / contextWindow) * 100 }
}

// 压缩完成状态：合并结果字段并计算节省的 token。
export function finishedCompaction(previous, event, finishedAt) {
  const tokensBefore = optionalTokenCount(event.result?.tokensBefore)
  const estimatedTokensAfter = optionalTokenCount(event.result?.estimatedTokensAfter)
  return {
    ...(previous || startedCompaction(event.reason, finishedAt)),
    active: false,
    status: event.errorMessage
      ? 'failed'
      : event.aborted
        ? 'aborted'
        : event.result
          ? 'completed'
          : 'failed',
    reason: ['manual', 'threshold', 'overflow'].includes(event.reason)
      ? event.reason
      : previous?.reason || 'threshold',
    finishedAt,
    tokensBefore,
    estimatedTokensAfter,
    tokensSaved:
      tokensBefore != null && estimatedTokensAfter != null
        ? Math.max(0, tokensBefore - estimatedTokensAfter)
        : null,
    aborted: Boolean(event.aborted),
    willRetry: Boolean(event.willRetry),
    error: String(event.errorMessage || ''),
  }
}

function sameToken(left, right) {
  return left?.length === right?.length && left.every((value, index) => value === right[index])
}

// 消息指纹：用于判断消息序列是否变化（变化则缓存失效）。
function messageToken(messages) {
  const last = messages?.at?.(-1)
  return [
    messages,
    messages?.length || 0,
    last,
    last?.content,
    last?.usage,
    last?.stopReason,
    last?.errorMessage,
  ]
}

function estimateProjectionBytes(value, seen = new Set()) {
  if (value == null) return 8
  if (typeof value === 'string') return value.length * 2
  if (typeof value !== 'object') return 8
  if (ArrayBuffer.isView(value)) return value.byteLength
  if (value instanceof ArrayBuffer) return value.byteLength
  if (seen.has(value)) return 0
  seen.add(value)
  if (Array.isArray(value))
    return 24 + value.reduce((total, item) => total + estimateProjectionBytes(item, seen), 0)
  return (
    32 +
    Object.entries(value).reduce(
      (total, [key, item]) => total + key.length * 2 + estimateProjectionBytes(item, seen),
      0,
    )
  )
}

// 投影缓存：按 key + 内容指纹缓存 transcript/用量/实时快照，
// 带条目数与估算字节上限，LRU 驱逐。
export class ProjectionCache {
  constructor({
    maxEntries = MAX_PROJECTION_CACHE_ENTRIES,
    maxBytes = MAX_PROJECTION_CACHE_BYTES,
  } = {}) {
    this.maxEntries = Math.max(1, Number(maxEntries) || MAX_PROJECTION_CACHE_ENTRIES)
    this.maxBytes = Math.max(1, Number(maxBytes) || MAX_PROJECTION_CACHE_BYTES)
    this.transcripts = new Map()
    this.assetTranscripts = new Map()
    this.contextUsages = new Map()
    this.liveSnapshots = new Map()
  }

  get(map, key, token) {
    // 指纹不一致视为缓存失效；命中时刷新 touch 时间供 LRU 使用。
    const cached = map.get(key)
    if (!cached || !sameToken(cached.token, token)) return { hit: false, value: null }
    cached.touchedAt = Date.now()
    return { hit: true, value: cached.value }
  }

  set(map, key, token, value) {
    map.delete(key)
    const entry = { token, value, bytes: estimateProjectionBytes(value), touchedAt: Date.now() }
    if (entry.bytes > this.maxBytes) return value
    map.set(key, entry)
    this.trim(map)
    return value
  }

  trim(map) {
    const entries = () =>
      [...map.entries()].sort((left, right) => left[1].touchedAt - right[1].touchedAt)
    const totalBytes = () => [...map.values()].reduce((total, entry) => total + entry.bytes, 0)
    while (map.size > this.maxEntries || totalBytes() > this.maxBytes) {
      const oldest = entries()[0]?.[0]
      if (oldest === undefined) break
      map.delete(oldest)
    }
  }

  stats() {
    const describe = (map) => ({
      entries: map.size,
      estimatedBytes: [...map.values()].reduce((total, entry) => total + entry.bytes, 0),
    })
    return {
      maxEntries: this.maxEntries,
      maxEstimatedBytes: this.maxBytes,
      transcripts: describe(this.transcripts),
      assetTranscripts: describe(this.assetTranscripts),
      contextUsages: describe(this.contextUsages),
      liveSnapshots: describe(this.liveSnapshots),
    }
  }

  transcript(key, messages, build) {
    const token = messageToken(messages)
    const cached = this.get(this.transcripts, key, token)
    if (cached.hit) return cached.value
    return this.set(this.transcripts, key, token, build())
  }

  transcriptWithAssets(key, transcript, assetRevision, build) {
    const token = [transcript, assetRevision]
    const cached = this.get(this.assetTranscripts, key, token)
    if (cached.hit) return cached.value
    return this.set(this.assetTranscripts, key, token, build())
  }

  contextUsage(key, token, build) {
    const cached = this.get(this.contextUsages, key, token)
    if (cached.hit) return cached.value
    return this.set(this.contextUsages, key, token, build())
  }

  liveSnapshot(key, token) {
    return this.get(this.liveSnapshots, key, token)
  }

  storeLiveSnapshot(key, token, value) {
    return this.set(this.liveSnapshots, key, token, value)
  }

  // 失效：可按范围（transcript/activity/usage）选择性清除。
  invalidate(key, { transcript = true, activity = true, usage = true } = {}) {
    if (transcript) {
      this.transcripts.delete(key)
      this.assetTranscripts.delete(key)
    }
    if (usage) this.contextUsages.delete(key)
    if (transcript || activity || usage) this.liveSnapshots.delete(key)
  }

  invalidateAssets() {
    this.assetTranscripts.clear()
    this.liveSnapshots.clear()
  }

  invalidateAllUsage() {
    this.contextUsages.clear()
    this.liveSnapshots.clear()
  }

  delete(key) {
    this.transcripts.delete(key)
    this.assetTranscripts.delete(key)
    this.contextUsages.delete(key)
    this.liveSnapshots.delete(key)
  }
}

// StreamProjection：会话视图投影服务——把引擎会话状态投影为前端可读的结构，
// 并统一处理历史消息读取（增量缓存）、上下文用量、实时快照与消息分页。
export class StreamProjection {
  constructor({
    cwd,
    sessions,
    liveSessions,
    sessionMeta,
    assetIndex,
    getAssetRevision,
    getExecutionMode,
    goals,
    plans,
    multiAgents,
    permissions,
    settingsManager,
    modelRuntime,
    compactionThresholdPercent,
    findSessionInfo,
    openStoredSession,
    touchSessionRuntime,
    saveSessionMeta,
    history,
  }) {
    this.cwd = cwd
    this.sessions = sessions
    this.liveSessions = liveSessions
    this.sessionMeta = sessionMeta
    this.assetIndex = assetIndex
    this.getAssetRevision = getAssetRevision
    this.getExecutionMode = getExecutionMode
    this.goals = goals
    this.plans = plans
    this.multiAgents = multiAgents
    this.permissions = permissions
    this.settingsManager = settingsManager
    this.modelRuntime = modelRuntime
    this.compactionThresholdPercent = compactionThresholdPercent
    this.findSessionInfo = findSessionInfo
    this.openStoredSession = openStoredSession
    this.touchSessionRuntime = touchSessionRuntime
    this.saveSessionMeta = saveSessionMeta
    this.history = history
    this.cache = new ProjectionCache()
  }

  // 记住会话模型：写入内存元数据；仅冷历史恢复时持久化（热路径避免与流结束写竞争）。
  rememberSessionModel(id, model, { persist = false } = {}) {
    const next = String(model || '').trim()
    if (!id || !next || /(^|\/)unknown$/i.test(next)) return next
    const meta = this.sessionMeta()
    if (meta[id]?.model === next) return next
    meta[id] = { ...(meta[id] || {}), model: next }
    // Only persist when recovering model from cold history. Hot live paths keep this
    // in-memory to avoid racing session metadata writes during stream teardown.
    if (persist) void this.saveSessionMeta?.()?.catch?.(() => {})
    return next
  }

  // 解析会话模型：活动运行时 > 元数据 > 磁盘会话上下文（并持久化结果）。
  async resolveSessionModel(id) {
    const active = this.sessions().get(id)
    if (active?.session?.model?.provider && active.session.model.id) {
      return this.rememberSessionModel(
        id,
        `${active.session.model.provider}/${active.session.model.id}`,
      )
    }
    const cached = String(this.sessionMeta()[id]?.model || '').trim()
    if (cached && !/(^|\/)unknown$/i.test(cached)) return cached
    const info = await this.findSessionInfo(id)
    if (!info?.path) return cached
    try {
      const context = this.openStoredSession(info.path).buildSessionContext()
      if (context?.model?.provider && context.model.modelId) {
        return this.rememberSessionModel(id, `${context.model.provider}/${context.model.modelId}`, {
          persist: true,
        })
      }
    } catch {
      // Historical files can be missing or mid-write; fall back to cached metadata.
    }
    return cached
  }

  invalidate(sessionId, scopes) {
    this.cache.invalidate(sessionId, scopes)
  }

  invalidateAssets() {
    this.cache.invalidateAssets()
  }

  invalidateAllUsage() {
    this.cache.invalidateAllUsage()
  }

  delete(sessionId) {
    this.cache.delete(sessionId)
  }

  generatedAssets(sessionId) {
    return generatedAssetsForSession(this.assetIndex().assets, sessionId)
  }

  withGeneratedAssets(sessionId, messages) {
    const revision = this.getAssetRevision()
    return this.cache.transcriptWithAssets(sessionId, messages, revision, () =>
      attachGeneratedAssets(messages, this.generatedAssets(sessionId)),
    )
  }

  // 会话消息（transcript）：活动运行时直接序列化，否则从磁盘打开会话重建。
  async getSessionMessages(id) {
    const active = this.sessions().get(id)
    let messages
    if (active) {
      this.touchSessionRuntime(active)
      const source = active.session.messages
      messages = this.cache.transcript(id, source, () => {
        const entryIdByMessage = new Map(
          (active.session.sessionManager?.getBranch?.() || [])
            .filter((entry) => entry?.type === 'message')
            .map((entry) => [entry.message, entry.id]),
        )
        return serializeTranscriptMessages(
          source,
          null,
          source.map((message) => entryIdByMessage.get(message) || ''),
        )
      })
    } else {
      const info = await this.findSessionInfo(id)
      if (!info) return []
      const manager = this.openStoredSession(info.path)
      const source = manager.buildSessionContext().messages
      const entryIdByMessage = new Map(
        manager
          .getBranch()
          .filter((entry) => entry?.type === 'message')
          .map((entry) => [entry.message, entry.id]),
      )
      messages = this.cache.transcript(id, source, () =>
        serializeTranscriptMessages(
          source,
          null,
          source.map((message) => entryIdByMessage.get(message) || ''),
        ),
      )
    }
    return this.withGeneratedAssets(id, messages)
  }

  // 历史消息缓存修剪（按条目数与估算字节）。
  trimSessionHistoryCache(protectedPath = '') {
    const history = this.history()
    const maximumEntries = Math.max(
      1,
      Number(history.maxEntries) || MAX_SESSION_HISTORY_CACHE_ENTRIES,
    )
    const maximumBytes = Math.max(
      1,
      Number(history.maxEstimatedBytes) || MAX_SESSION_HISTORY_CACHE_ESTIMATED_BYTES,
    )
    const estimatedBytes = () =>
      [...history.cache.values()].reduce(
        (total, entry) => total + entry.size * SESSION_HISTORY_CACHE_MEMORY_MULTIPLIER,
        0,
      )
    const candidates = () =>
      [...history.cache.entries()]
        .filter(([path]) => path !== protectedPath)
        .sort((left, right) => left[1].touchedAt - right[1].touchedAt)
    while (history.cache.size > maximumEntries || estimatedBytes() > maximumBytes) {
      const oldest = candidates()[0]?.[0]
      if (!oldest) break
      history.cache.delete(oldest)
    }
  }

  trimSessionContextUsageCache(protectedId = '') {
    const history = this.history()
    const maximum = MAX_SESSION_CONTEXT_USAGE_CACHE_ENTRIES
    const cache = history.contextUsageCache
    const candidates = () =>
      [...cache.entries()]
        .filter(([id]) => id !== protectedId)
        .sort((left, right) => (left[1].touchedAt || 0) - (right[1].touchedAt || 0))
    while (cache.size > maximum) {
      const oldest = candidates()[0]?.[0]
      if (oldest === undefined) break
      cache.delete(oldest)
    }
  }

  // 增量读取会话历史文件：从上次读到的位置继续，按换行对齐切分逐条解析；
  // 文件变小/被重写（mtime 变化）时从头重建。
  async readSessionHistoryEntries(path) {
    const file = await stat(path)
    const history = this.history()
    let cached = history.cache.get(path)
    if (
      !cached ||
      file.size < cached.size ||
      (file.size === cached.size && file.mtimeMs !== cached.mtimeMs)
    ) {
      cached = {
        size: 0,
        mtimeMs: 0,
        remainder: Buffer.alloc(0),
        entries: [],
        byId: new Map(),
        serializedSize: -1,
        serializedMessages: null,
        touchedAt: Date.now(),
      }
    }
    if (file.size > cached.size) {
      const handle = await open(path, 'r')
      try {
        let position = cached.size
        const maximumChunkBytes = Math.max(
          1,
          Number(history.readChunkBytes) || SESSION_HISTORY_READ_CHUNK_BYTES,
        )
        const readBuffer = Buffer.allocUnsafe(Math.min(maximumChunkBytes, file.size - position))
        while (position < file.size) {
          const length = Math.min(readBuffer.length, file.size - position)
          const { bytesRead } = await handle.read(readBuffer, 0, length, position)
          if (!bytesRead) break
          position += bytesRead
          const chunk = readBuffer.subarray(0, bytesRead)
          const combined = cached.remainder.length
            ? Buffer.concat([cached.remainder, chunk])
            : chunk
          const newline = combined.lastIndexOf(0x0a)
          const complete = newline >= 0 ? combined.subarray(0, newline).toString('utf8') : ''
          cached.remainder =
            newline >= 0 ? Buffer.from(combined.subarray(newline + 1)) : Buffer.from(combined)
          for (const line of complete.split('\n')) {
            if (!line.trim()) continue
            try {
              const entry = JSON.parse(line.trimEnd())
              cached.entries.push(entry)
              if (entry?.id) cached.byId.set(entry.id, entry)
            } catch {
              // Ignore one malformed history line without hiding the remaining session.
            }
          }
        }
      } finally {
        await handle.close()
      }
    }
    cached.size = file.size
    cached.mtimeMs = file.mtimeMs
    cached.touchedAt = Date.now()
    const maximumSourceBytes = Math.max(
      1,
      Number(history.maxSourceBytes) || MAX_SESSION_HISTORY_CACHE_SOURCE_BYTES,
    )
    if (file.size <= maximumSourceBytes) {
      history.cache.set(path, cached)
      this.trimSessionHistoryCache(path)
    } else {
      history.cache.delete(path)
    }
    return cached
  }

  // 会话累计 token 用量：优先读历史文件（增量），缺失时回退到内存消息。
  async getSessionTokenUsage(id) {
    const active = this.sessions().get(id)
    const activePath = active?.session.sessionFile
    const historyState = this.history()
    let path = activePath || historyState.paths.get(id)
    if (!path) {
      path = (await this.findSessionInfo(id))?.path
      if (path) historyState.paths.set(id, path)
    }
    if (path) {
      try {
        const history = await this.readSessionHistoryEntries(path)
        return summarizeSessionUsage(
          history.entries
            .filter((entry) => entry?.type === 'message')
            .map((entry) => entry.message),
        )
      } catch (error) {
        if (error?.code !== 'ENOENT') throw error
      }
    }
    return summarizeSessionUsage(active?.session?.messages || [])
  }

  // 历史消息：从会话文件重建当前分支（沿 parentId 回溯），图片附件按哈希映射到资产 URL。
  async getSessionHistoryMessages(id) {
    const active = this.sessions().get(id)
    const activePath = active?.session.sessionFile
    const historyState = this.history()
    let path = activePath || historyState.paths.get(id)
    if (!path) {
      path = (await this.findSessionInfo(id))?.path
      if (path) historyState.paths.set(id, path)
    }
    if (!path) return this.getSessionMessages(id)
    let history
    try {
      history = await this.readSessionHistoryEntries(path)
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
      historyState.paths.delete(id)
      historyState.cache.delete(path)
      return active ? this.getSessionMessages(id) : []
    }
    let cursor = history.entries.findLast((entry) => entry?.id)
    const branch = []
    const visited = new Set()
    while (cursor?.id && !visited.has(cursor.id)) {
      visited.add(cursor.id)
      branch.push(cursor)
      cursor = cursor.parentId ? history.byId.get(cursor.parentId) : null
    }
    branch.reverse()
    let assetUrlByHash = null
    const resolveImageUrl = (part) => {
      if (!part?.data) return null
      if (!assetUrlByHash) {
        assetUrlByHash = new Map()
        for (const asset of this.assetIndex().assets) {
          if (assetHasSource(asset, 'attachment') && asset.hash) {
            assetUrlByHash.set(
              asset.hash,
              `/api/assets/${encodeURIComponent(asset.id)}/download?inline=1`,
            )
          }
        }
      }
      try {
        const hash = createHash('sha256')
          .update(Buffer.from(String(part.data), 'base64'))
          .digest('hex')
        return assetUrlByHash.get(hash) || null
      } catch {
        return null
      }
    }
    let messages = history.serializedSize === history.size ? history.serializedMessages : null
    if (!messages) {
      const messageEntries = branch.filter((entry) => entry?.type === 'message')
      messages = serializeTranscriptMessages(
        messageEntries.map((entry) => entry.message),
        resolveImageUrl,
        messageEntries.map((entry) => entry.id),
      )
      history.serializedSize = history.size
      history.serializedMessages = messages
    }
    return this.withGeneratedAssets(id, messages)
  }

  // 上下文用量装饰：补充压缩阈值、压缩触发点等前端展示字段。
  decorateContextUsage(raw, compaction = null) {
    const contextWindow = optionalTokenCount(raw?.contextWindow)
    if (!contextWindow) return undefined
    let tokens = raw?.tokens == null ? null : optionalTokenCount(raw.tokens)
    let estimated = false
    if (
      tokens == null &&
      compaction?.status === 'completed' &&
      compaction.estimatedTokensAfter != null
    ) {
      tokens = optionalTokenCount(compaction.estimatedTokensAfter)
      estimated = tokens != null
    }
    const settings = effectiveCompactionSettings(
      this.settingsManager()?.getCompactionSettings?.() || {
        enabled: true,
        reserveTokens: 16_384,
        keepRecentTokens: 20_000,
      },
      contextWindow,
      this.compactionThresholdPercent(),
    )
    const compactAtTokens = settings.enabled
      ? Math.max(0, contextWindow - Math.max(0, Number(settings.reserveTokens) || 0))
      : null
    return {
      tokens,
      contextWindow,
      percent: tokens == null ? null : (tokens / contextWindow) * 100,
      estimated,
      autoCompactEnabled: Boolean(settings.enabled),
      compactAtTokens,
      compactAtPercent: compactAtTokens == null ? null : (compactAtTokens / contextWindow) * 100,
    }
  }

  compactionAwareContextUsage(session, compaction = null) {
    if (!session?.model) return undefined
    const messages = session.messages || []
    const key = session.sessionId || session
    const token = [
      session,
      session.model,
      compaction,
      this.compactionThresholdPercent(),
      ...messageToken(messages),
    ]
    return this.cache.contextUsage(key, token, () => {
      const contextWindow = optionalTokenCount(session.model.contextWindow)
      const raw =
        typeof session.getContextUsage === 'function'
          ? session.getContextUsage()
          : contextWindow
            ? (() => {
                const tokens = estimateMessageContextTokens(messages)
                return { tokens, contextWindow, percent: (tokens / contextWindow) * 100 }
              })()
            : undefined
      return this.decorateContextUsage(raw, compaction)
    })
  }

  // 冷会话上下文用量：带 (path, size, mtime) 缓存，未变化不重读。
  async getSessionContextUsage(id, compaction = null) {
    const active = this.sessions().get(id)
    if (active) {
      this.touchSessionRuntime(active)
      return this.compactionAwareContextUsage(
        active.session,
        compaction || this.liveSessions().get(id)?.compaction,
      )
    }
    const info = await this.findSessionInfo(id)
    if (!info) return undefined
    const fileStat = await stat(info.path).catch(() => null)
    const historyState = this.history()
    const cached = historyState.contextUsageCache.get(id)
    if (
      fileStat &&
      cached?.path === info.path &&
      cached.size === fileStat.size &&
      cached.mtimeMs === fileStat.mtimeMs
    ) {
      cached.touchedAt = Date.now()
      return cached.value
    }
    const manager = this.openStoredSession(info.path)
    const context = manager.buildSessionContext()
    const globalSettings = this.settingsManager()?.getGlobalSettings?.() || {}
    const provider = context.model?.provider || globalSettings.defaultProvider
    const modelId = context.model?.modelId || globalSettings.defaultModel
    const model = provider && modelId ? this.modelRuntime()?.getModel?.(provider, modelId) : null
    const value = this.decorateContextUsage(
      persistedContextUsage(manager, model?.contextWindow || 0),
      compaction,
    )
    historyState.contextUsageCache.set(id, {
      path: info.path,
      size: fileStat?.size || 0,
      mtimeMs: fileStat?.mtimeMs || 0,
      value,
      touchedAt: Date.now(),
    })
    this.trimSessionContextUsageCache(id)
    return value
  }

  // 消息分页（向后翻页）：before 为游标（已加载条数）。
  async getSessionMessagePage(id, { before, limit = DEFAULT_MESSAGE_PAGE_SIZE } = {}) {
    const messages = await this.getSessionHistoryMessages(id)
    const pageSize = Math.min(
      MAX_MESSAGE_PAGE_SIZE,
      Math.max(1, Number.parseInt(limit, 10) || DEFAULT_MESSAGE_PAGE_SIZE),
    )
    const requestedEnd =
      before == null || before === '' ? messages.length : Number.parseInt(before, 10)
    const end = Number.isFinite(requestedEnd)
      ? Math.min(messages.length, Math.max(0, requestedEnd))
      : messages.length
    const start = Math.max(0, end - pageSize)
    return {
      messages: messages.slice(start, end),
      model: await this.resolveSessionModel(id),
      contextUsage: await this.getSessionContextUsage(id),
      sessionUsage: await this.getSessionTokenUsage(id),
      pageInfo: {
        start,
        end,
        total: messages.length,
        hasMore: start > 0,
        nextCursor: start > 0 ? String(start) : null,
      },
    }
  }

  liveToken(id, active, live, approvals) {
    const messages = active?.session?.messages || []
    return [
      active,
      active?.session?.model,
      ...messageToken(messages),
      live,
      this.sessionMeta()[id],
      approvals,
      approvals.length,
      approvals.at(-1),
    ]
  }

  // 实时快照：活动运行中把 live 状态合入消息流（流式中的最后一条 assistant 消息），
  // 供前端轮询/SSE 后全量刷新。
  async getSessionLive(id) {
    const active = this.sessions().get(id)
    if (active) this.touchSessionRuntime(active)
    const live = this.liveSessions().get(id)
    const approvals = this.permissions.getPending(id)
    const token = this.liveToken(id, active, live, approvals)
    if (active || live) {
      const cached = this.cache.liveSnapshot(id, token)
      if (cached.hit) return cached.value
    }
    const persisted = active ? null : await this.findSessionInfo(id)
    const page = await this.getSessionMessagePage(id, { limit: LIVE_MESSAGE_PAGE_SIZE })
    const messages = page.messages
    const streaming = Boolean(active?.session.isStreaming || live?.streaming)
    if (streaming && live) {
      const lastUserIndex = messages.findLastIndex((message) => message.role === 'user')
      const assistantIndex = messages.findIndex(
        (message, index) => index > lastUserIndex && message.role === 'agent',
      )
      const liveMessage = {
        id: `live-${id}`,
        role: 'agent',
        text: live.text,
        streaming: true,
        attachments: live.assets,
      }
      if (assistantIndex >= 0) {
        messages[assistantIndex] = {
          ...messages[assistantIndex],
          ...liveMessage,
          text: liveMessage.text || messages[assistantIndex].text,
          attachments: live.assets.length ? live.assets : messages[assistantIndex].attachments,
        }
      } else messages.push(liveMessage)
    }
    const meta = this.sessionMeta()
    const executionMode = this.getExecutionMode(id)
    const value = {
      id,
      streaming,
      messages,
      tools: live?.tools || [],
      error: live?.error || '',
      startedAt: live?.startedAt || null,
      lastActivityAt: live?.lastActivityAt || null,
      finishedAt: live?.finishedAt || null,
      model: active?.session.model
        ? this.rememberSessionModel(
            id,
            `${active.session.model.provider}/${active.session.model.id}`,
          )
        : page.model || meta[id]?.model || '',
      cwd: active?.cwd || meta[id]?.cwd || persisted?.cwd || this.cwd,
      permissionMode: meta[id]?.permissionMode || permissionModeForExecutionMode(executionMode),
      executionMode,
      goal: live?.goal ?? this.goals.get(id),
      plan: live?.plan ?? this.plans.get(id),
      agents:
        live?.agents ??
        this.multiAgents
          .summaries(id)
          .filter((agent) => ['queued', 'starting', 'running'].includes(agent.status)),
      currentActivity: live?.currentActivity || null,
      activityFeed: live?.activityFeed || [],
      lifecycle: live?.lifecycle || null,
      sessionTreeRevision: Number(live?.sessionTreeRevision || 0),
      thinkingText: live?.thinkingText || '',
      queuedInputs: live?.queuedInputs ?? queuedSessionInputs(active?.session),
      contextUsage:
        this.compactionAwareContextUsage(active?.session, live?.compaction) || page.contextUsage,
      sessionUsage: live?.sessionUsage || page.sessionUsage,
      promptCache: live?.promptCache || active?.promptCache || null,
      compaction: live?.compaction || null,
      approvals,
      pageInfo: page.pageInfo,
    }
    return active || live ? this.cache.storeLiveSnapshot(id, token, value) : value
  }
}
