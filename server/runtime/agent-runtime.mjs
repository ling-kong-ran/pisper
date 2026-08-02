import { mkdir, open, readFile, stat, unlink, writeFile } from 'node:fs/promises'
import { createHash, randomUUID } from 'node:crypto'
import { basename, extname, join, resolve, sep } from 'node:path'
import {
  calculateContextTokens,
  createAgentSession,
  estimateTokens,
  ModelRuntime,
  SessionManager,
  SettingsManager,
} from './pi-coding-agent.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { cleanupRemovedLocalEmbeddingData } from '../data-dir-migration.mjs'
import { ChannelService } from '../services/channels/channel-service.mjs'
import { NotificationSettingsService } from '../services/notification-settings-service.mjs'
import { McpService } from '../services/mcp-service.mjs'
import { migrateKimiCodeProvider } from '../services/provider-migrations.mjs'
import { ProviderDiscoveryService } from '../services/provider-discovery.mjs'
import { ProviderModelCatalogService } from '../services/provider-model-catalog-service.mjs'
import { ModelMetadataService } from '../services/model-metadata-service.mjs'
import { ProviderModelDiscoveryService } from '../services/provider-model-discovery-service.mjs'
import { ScheduleService } from '../services/schedule-service.mjs'
import { WorkflowService } from '../services/workflow-service.mjs'
import { SkillsService } from '../services/skills-service.mjs'
import { PERMISSION_MODES, SessionPermissionService } from '../services/session-permission-service.mjs'
import { ToolPluginService } from '../services/tool-plugin-service.mjs'
import { WebSearchService } from '../services/web-search-service.mjs'
import { extractConversationMemories } from '../services/memory/conversation-memory.mjs'
import { LocalMemoryRuntime } from '../services/memory/local-memory-runtime.mjs'
import { createSemanticMemorySummarizer } from '../services/memory/semantic-memory.mjs'
import { inferModelKind, VisualGenerationService } from '../services/visual-generation/index.mjs'
import { MultiAgentService, MULTI_AGENT_TOOL_NAMES, agentCompletionPrompt, isAgentCompletionMessage } from '../services/multi-agent-service.mjs'
import { GoalService, goalBudgetPrompt, goalContinuationPrompt, isGoalContinuationMessage } from '../services/goal-service.mjs'
import { GitChangesService } from '../services/git-changes-service.mjs'
import { PlanService } from '../services/plan-service.mjs'
import { BrowserAutomationService } from '../services/browser-automation-service.mjs'
import {
  listWorkspaceDirectories,
  normalizeWorkspacePath,
  resolveWorkspaceDirectory,
  workspacePathKey,
} from './workspace-directories.mjs'
import { assetMessageAttachment, attachGeneratedAssets } from '../services/session-assets.mjs'
import { createAppTools, createMultiAgentTools, TOOL_PRESETS, toolsFromConfig } from '../tools/registry.mjs'
import { createGoalTools, GOAL_TOOL_NAMES } from '../tools/app/goal.mjs'
import { createPlanTools, PLAN_ALL_TOOL_NAMES, PLAN_COMPATIBILITY_TOOL_NAMES } from '../tools/app/plan.mjs'
import { createToolDiscoveryTool, TOOL_DISCOVERY_NAME } from '../tools/app/tool-discovery.mjs'
import { createPisperBashTool } from '../tools/host-bash.mjs'
import { hotToolNames, mergePromotedToolNames, schemaOnlyToolDefinitions, selectedToolNames } from '../tools/tool-activation.mjs'
import { DEFAULT_EXECUTION_MODE, EXECUTION_MODES, filterToolsForExecutionMode, migrateLegacyExecutionMode, normalizeExecutionMode, permissionModeForExecutionMode } from '../security/execution-mode.mjs'
import { redactSecretText } from '../security/secret-redaction.mjs'
import { applyPisperSystemPrompt, pisperPromptExtension } from '../prompts/pisper-system-prompt.mjs'
import {
  DEFAULT_COMPACTION_THRESHOLD_PERCENT,
  MAX_COMPACTION_THRESHOLD_PERCENT,
  MIN_COMPACTION_THRESHOLD_PERCENT,
  createCompactionSettingsManager,
  effectiveCompactionSettings,
  normalizeCompactionThresholdPercent,
  pisperCompactionExtension,
} from './compaction-policy.mjs'

const KNOWN_PROVIDERS = ['openai', 'anthropic', 'google', 'deepseek', 'xai', 'openrouter', 'kimi-coding', 'zai-coding-cn']
const PROVIDER_LABELS = {
  openai: 'OpenAI',
  anthropic: 'Anthropic',
  google: 'Google',
  deepseek: 'DeepSeek',
  xai: 'xAI',
  openrouter: 'OpenRouter',
  'kimi-coding': 'Kimi Code',
  'zai-coding-cn': 'GLM',
}
const PROVIDER_DEFAULT_BASE_URLS = {
  openai: 'https://api.openai.com/v1',
  anthropic: 'https://api.anthropic.com/v1',
  google: 'https://generativelanguage.googleapis.com/v1beta',
  deepseek: 'https://api.deepseek.com',
  xai: 'https://api.x.ai/v1',
  openrouter: 'https://openrouter.ai/api/v1',
  'kimi-coding': 'https://api.kimi.com/coding/',
  'zai-coding-cn': 'https://open.bigmodel.cn/api/paas/v4',
}
const ATTACHMENT_MARKER = '\n\n---\nAttachment context (injected by Pisper):\n'
const MAX_EXTRACTED_CHARS = 400_000
const MAX_ASSET_BYTES = 24 * 1024 * 1024
const MAX_CHAT_ASSET_BYTES = 10 * 1024 * 1024
const DEFAULT_SESSION_NAME = '新会话'
const MAX_SESSION_TITLE_CHARS = 20
const DEFAULT_MESSAGE_PAGE_SIZE = 40
const MAX_MESSAGE_PAGE_SIZE = 100
const LIVE_MESSAGE_PAGE_SIZE = 60
const MAX_RESIDENT_SESSION_RUNTIMES = 3
const SESSION_RUNTIME_IDLE_TTL_MS = 5 * 60 * 1000
const SESSION_RUNTIME_SWEEP_INTERVAL_MS = 60 * 1000
const SESSION_HISTORY_READ_CHUNK_BYTES = 1024 * 1024
const MAX_SESSION_HISTORY_CACHE_ENTRIES = 4
const MAX_SESSION_HISTORY_CACHE_SOURCE_BYTES = 8 * 1024 * 1024
const MAX_SESSION_HISTORY_CACHE_ESTIMATED_BYTES = 48 * 1024 * 1024
const SESSION_HISTORY_CACHE_MEMORY_MULTIPLIER = 4
const ISOLATED_CONTEXT_BLOCKED_TOOLS = ['memory_search', 'memory_remember']
const ASSET_TEXT_EXTENSIONS = new Set(['.txt', '.md', '.json', '.js', '.jsx', '.ts', '.tsx', '.css', '.html', '.xml', '.yaml', '.yml', '.csv', '.log', '.py', '.java', '.go', '.rs', '.sh', '.ps1', '.toml', '.sql'])
const ASSET_DOCUMENT_EXTENSIONS = new Set(['.pdf', '.docx', '.pptx', '.xlsx', '.odt', '.odp', '.ods', '.rtf', '.epub'])
const IMAGE_EXTENSIONS = new Set(['.png', '.jpg', '.jpeg', '.gif', '.webp', '.bmp', '.svg'])
const TRANSIENT_STREAM_READ_ERROR_PATTERN = /\bstream[_\s-]?read[_\s-]?error\b/i
const PISPER_STREAM_RETRY_PATCH = Symbol('pisper.stream-retry-patch')

export function installTransientStreamRetry(session) {
  if (!session || session[PISPER_STREAM_RETRY_PATCH] || typeof session._isRetryableError !== 'function') return session
  const isRetryableError = session._isRetryableError.bind(session)
  session._isRetryableError = (message) => {
    if (message?.stopReason === 'error' && TRANSIENT_STREAM_READ_ERROR_PATTERN.test(String(message.errorMessage || ''))) return true
    return isRetryableError(message)
  }
  session[PISPER_STREAM_RETRY_PATCH] = true
  return session
}

function isInternalParentMessage(content) {
  return isGoalContinuationMessage(content) || isAgentCompletionMessage(content)
}

function textFromContent(content) {
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content
    .filter((part) => part?.type === 'text')
    .map((part) => part.text || '')
    .join('')
}

export function storedSessionModelId(sessionManager) {
  let modelId = ''
  for (const entry of sessionManager?.getBranch?.() || []) {
    if (entry?.type === 'model_change') modelId = entry.modelId || modelId
    else if (entry?.type === 'message' && entry.message?.role === 'assistant') modelId = entry.message.model || modelId
  }
  return modelId
}

const MAX_LIVE_ACTIVITY_ITEMS = 6
const MAX_LIVE_THINKING_CHARS = 6_000

function liveThinkingTail(value) {
  return String(value || '').slice(-MAX_LIVE_THINKING_CHARS)
}

export function multiAgentResultAgent(toolName, details) {
  if (!MULTI_AGENT_TOOL_NAMES.includes(toolName) || !details) return null
  if (toolName === 'wait_agent') return details.agent?.id ? details.agent : null
  if (['spawn_agent', 'send_message', 'followup_task', 'interrupt_agent'].includes(toolName)) return details.id ? details : null
  return null
}

export async function waitForAgentMailbox(multiAgents, sessionId, timeoutMs, target) {
  const result = await multiAgents.wait(sessionId, timeoutMs, target)
  if (!result.timedOut && result.agent) await multiAgents.acknowledge(sessionId, [result.agent])
  return result
}

function liveActivityKey(activity) {
  if (!activity?.type) return ''
  if (activity.type === 'tool') return `tool:${activity.id || activity.name || ''}`
  if (activity.type === 'agent') return `agent:${activity.agent?.id || activity.agent?.canonicalName || ''}`
  if (activity.type === 'plan') return `plan:${activity.updatedAt || activity.plan?.updatedAt || ''}`
  if (activity.type === 'model') return `model:${activity.stage || ''}`
  if (activity.type === 'compaction') return `compaction:${activity.compaction?.status || activity.compaction?.active || ''}`
  return `${activity.type}:${activity.id || activity.updatedAt || ''}`
}

function pushLiveActivity(feed, activity) {
  const current = Array.isArray(feed) ? feed : []
  if (!['tool', 'plan', 'agent'].includes(activity?.type)) return current
  let next = [...current]
  if (activity.type === 'plan') next = next.filter((item) => item?.type !== 'tool' || !PLAN_ALL_TOOL_NAMES.includes(item.name))
  if (activity.type === 'agent') next = next.filter((item) => item?.type !== 'tool' || !MULTI_AGENT_TOOL_NAMES.includes(item.name))
  const key = liveActivityKey(activity)
  const existingIndex = next.findIndex((item) => liveActivityKey(item) === key)
  if (existingIndex >= 0) next[existingIndex] = { ...next[existingIndex], ...activity }
  else next.push(activity)
  return next.slice(-MAX_LIVE_ACTIVITY_ITEMS)
}

function setLiveActivity(live, activity) {
  if (!live) return
  live.currentActivity = activity || null
  live.activityFeed = activity ? pushLiveActivity(live.activityFeed, activity) : []
}

function livePlanChanges(previous, next) {
  const previousItems = new Map((previous?.items || []).map((item) => [item.id, item]))
  const nextItems = new Map((next?.items || []).map((item) => [item.id, item]))
  const changes = []
  for (const item of nextItems.values()) {
    const before = previousItems.get(item.id)
    if (!before) changes.push({ id: item.id, title: item.title, status: item.status, kind: 'added' })
    else if (
      before.status !== item.status ||
      before.title !== item.title ||
      before.note !== item.note ||
      before.assignee !== item.assignee ||
      JSON.stringify(before.dependsOn || []) !== JSON.stringify(item.dependsOn || [])
    ) {
      changes.push({ id: item.id, title: item.title, status: item.status, previousStatus: before.status, kind: 'updated' })
    }
  }
  for (const item of previousItems.values()) {
    if (!nextItems.has(item.id)) changes.push({ id: item.id, title: item.title, status: item.status, kind: 'removed' })
  }
  return changes
}

function queuedSessionInputs(session) {
  const steering = typeof session?.getSteeringMessages === 'function' ? session.getSteeringMessages() : []
  const followUp = typeof session?.getFollowUpMessages === 'function' ? session.getFollowUpMessages() : []
  return [
    ...steering.filter((text) => !isInternalParentMessage(text)).map((text) => ({ behavior: 'steer', text })),
    ...followUp.filter((text) => !isInternalParentMessage(text)).map((text) => ({ behavior: 'followUp', text })),
  ]
}

function serializeMessage(message, index, resolveImageUrl = null) {
  if (!message || !['user', 'assistant'].includes(message.role)) return null
  const rawText = textFromContent(message.content)
  if (message.role === 'user' && isInternalParentMessage(rawText)) return null
  const text = message.role === 'user' ? rawText.split(ATTACHMENT_MARKER)[0] : rawText
  if (!text) return null
  const attachments = Array.isArray(message.content)
    ? message.content.filter((part) => part?.type === 'image').map((part, attachmentIndex) => {
        const attachment = {
          id: `image-${index}-${attachmentIndex}`,
          kind: 'image',
          name: `图片附件 ${attachmentIndex + 1}`,
          mimeType: part.mimeType,
        }
        // Avoid re-sending base64 payloads on every poll: archived user images
        // are served through the asset pipeline and cached by the browser.
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

function serializeTranscriptMessages(messages, resolveImageUrl = null) {
  const result = []
  let thinkingParts = []
  let tools = new Map()
  let startedAt = null
  let lastActivityAt = null
  let lastActivityMessage = null
  let runResultIndex = null

  const hasActivity = () => thinkingParts.length > 0 || tools.size > 0
  const resetRun = () => {
    thinkingParts = []
    tools = new Map()
    startedAt = null
    lastActivityAt = null
    lastActivityMessage = null
    runResultIndex = null
  }
  const finishRun = ({ terminal = false } = {}) => {
    if (!hasActivity()) return
    let item = runResultIndex == null ? null : result[runResultIndex]
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
    const activityTools = [...tools.values()]
      .slice(-MAX_LIVE_ACTIVITY_ITEMS)
      .map((tool) =>
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
      if (serialized || (terminal && hasActivity())) {
        const item = serialized || {
          id: `${message.role}-${message.timestamp || index}-${index}`,
          role: 'agent',
          text: '',
          timestamp: message.timestamp || null,
          error: message.errorMessage || null,
          attachments: [],
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

function safeAttachmentName(name) {
  return String(name || '附件').replace(/[\r\n<>]/g, '_').slice(0, 180)
}

function mimeFromName(name) {
  const extension = extname(String(name || '')).toLowerCase()
  return ({
    '.png': 'image/png', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.gif': 'image/gif', '.webp': 'image/webp', '.svg': 'image/svg+xml',
    '.txt': 'text/plain', '.md': 'text/markdown', '.json': 'application/json', '.js': 'text/javascript', '.ts': 'text/typescript', '.css': 'text/css', '.html': 'text/html',
    '.pdf': 'application/pdf', '.docx': 'application/vnd.openxmlformats-officedocument.wordprocessingml.document', '.pptx': 'application/vnd.openxmlformats-officedocument.presentationml.presentation', '.xlsx': 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
    '.mp4': 'video/mp4', '.webm': 'video/webm', '.mov': 'video/quicktime',
  })[extension] || 'application/octet-stream'
}

function truncateTitle(value) {
  const characters = Array.from(String(value || '').trim())
  return characters.length > MAX_SESSION_TITLE_CHARS
    ? `${characters.slice(0, MAX_SESSION_TITLE_CHARS).join('')}…`
    : characters.join('')
}

function cleanSessionTitle(value) {
  const title = String(value || '')
    .split(/\r?\n/)[0]
    .replace(/^\s*(?:[-*#>]+\s*)?/, '')
    .replace(/^\s*(?:会话)?标题\s*[:：]\s*/i, '')
    .replace(/^[“”"'`]+|[“”"'`。.!！?？]+$/g, '')
    .replace(/\s+/g, ' ')
    .trim()
  return truncateTitle(title)
}

function localDayKey(value = new Date()) {
  const date = value instanceof Date ? value : new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  const pad = (part) => String(part).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

function normalizedUsage(usage) {
  const number = (value) => Number.isFinite(Number(value)) ? Math.max(0, Number(value)) : 0
  return {
    input: number(usage?.input),
    output: number(usage?.output),
    cacheRead: number(usage?.cacheRead),
    cacheWrite: number(usage?.cacheWrite),
    reasoning: number(usage?.reasoning),
    totalTokens: number(usage?.totalTokens ?? usage?.total),
  }
}

function addUsage(target, usage) {
  const value = normalizedUsage(usage)
  for (const key of Object.keys(value)) target[key] = (target[key] || 0) + value[key]
  return target
}

function optionalTokenCount(value) {
  const tokens = Number(value)
  return Number.isFinite(tokens) ? Math.max(0, Math.round(tokens)) : null
}

function startedCompaction(reason, startedAt) {
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

function validAssistantUsage(message) {
  if (message?.role !== 'assistant' || message.stopReason === 'aborted' || message.stopReason === 'error' || !message.usage) return null
  return calculateContextTokens(message.usage) > 0 ? message.usage : null
}

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
  for (let index = start; index < messages.length; index += 1) tokens += estimateTokens(messages[index])
  return Math.max(0, Math.round(tokens))
}

function persistedContextUsage(manager, contextWindow) {
  if (!manager || !contextWindow) return undefined
  const branch = manager.getBranch()
  const latestCompactionIndex = branch.findLastIndex((entry) => entry?.type === 'compaction')
  if (latestCompactionIndex >= 0) {
    const hasPostCompactionUsage = branch.slice(latestCompactionIndex + 1).some((entry) => entry?.type === 'message' && validAssistantUsage(entry.message))
    if (!hasPostCompactionUsage) return { tokens: null, contextWindow, percent: null }
  }
  const tokens = estimateMessageContextTokens(manager.buildSessionContext().messages)
  return { tokens, contextWindow, percent: (tokens / contextWindow) * 100 }
}

function finishedCompaction(previous, event, finishedAt) {
  const tokensBefore = optionalTokenCount(event.result?.tokensBefore)
  const estimatedTokensAfter = optionalTokenCount(event.result?.estimatedTokensAfter)
  return {
    ...(previous || startedCompaction(event.reason, finishedAt)),
    active: false,
    status: event.errorMessage ? 'failed' : event.aborted ? 'aborted' : event.result ? 'completed' : 'failed',
    reason: ['manual', 'threshold', 'overflow'].includes(event.reason) ? event.reason : previous?.reason || 'threshold',
    finishedAt,
    tokensBefore,
    estimatedTokensAfter,
    tokensSaved: tokensBefore != null && estimatedTokensAfter != null ? Math.max(0, tokensBefore - estimatedTokensAfter) : null,
    aborted: Boolean(event.aborted),
    willRetry: Boolean(event.willRetry),
    error: String(event.errorMessage || ''),
  }
}

async function resolveDirectory(input, fallback) {
  return resolveWorkspaceDirectory(input, fallback)
}

function sessionThinkingState(session) {
  const availableLevels = session.getAvailableThinkingLevels()
  const supported = availableLevels.length > 0
  return {
    thinkingLevel: session.thinkingLevel,
    availableLevels,
    status: supported ? 'supported' : 'unsupported',
    message: supported ? '' : 'The current model does not expose configurable thinking levels.',
    model: session.model ? `${session.model.provider}/${session.model.id}` : '',
  }
}

function temporarySessionTitle(message, attachments = []) {
  const attachmentNames = attachments
    .map((attachment) => safeAttachmentName(attachment?.name))
    .filter(Boolean)
  let title = String(message || '')
    .replace(/```[\s\S]*?```/g, '代码内容')
    .replace(/https?:\/\/\S+/g, '链接')
    .replace(/^[\s，,。.!！?？]*(?:请|麻烦)?(?:你)?(?:帮我|帮忙|协助|请问|能否|可以)?[\s，,。.!！?？]*/i, '')
    .replace(/^(?:分析|查看|检查)(?:一下)?(?:这些|这个)?附件[\s，,。.!！?？]*/i, '')
    .replace(/\s+/g, ' ')
    .trim()
  if ((!title || /^(?:分析|查看|检查)?(?:这些|这个)?附件$/i.test(title)) && attachmentNames.length) {
    title = `分析 ${attachmentNames[0]}`
  }
  return cleanSessionTitle(title) || DEFAULT_SESSION_NAME
}

async function extractDocumentText(attachment) {
  const buffer = Buffer.from(String(attachment.data || ''), 'base64')
  if (!buffer.length) throw new Error(`${safeAttachmentName(attachment.name)} 内容为空`)
  // Lazily loaded: officeparser is only needed when a document attachment arrives.
  const { OfficeParser } = await import('officeparser')
  const ast = await OfficeParser.parseOffice(buffer, {
    fileType: String(attachment.extension || '').toLowerCase() || undefined,
    ocr: false,
  })
  const extracted = typeof ast.toText === 'function' ? await ast.toText() : await ast.to('text')
  const text = typeof extracted === 'string' ? extracted : extracted?.value
  if (!text?.trim()) throw new Error(`${safeAttachmentName(attachment.name)} 未提取到可分析文本`)
  return text.slice(0, MAX_EXTRACTED_CHARS)
}

function modelRank(provider, model) {
  const id = model.id.toLowerCase()
  if ((provider === 'openai' || provider === 'openai-codex') && id.startsWith('gpt-5')) return 100
  if (provider === 'anthropic' && /claude-(opus|sonnet)-4/.test(id)) return 100
  if (provider === 'google' && /gemini-(3|2\.5)/.test(id)) return 100
  if (provider === 'deepseek' && /reasoner|chat/.test(id)) return 90
  if (provider === 'kimi-coding') {
    if (id === 'k3') return 120
    if (id === 'kimi-for-coding-highspeed') return 115
    if (id === 'kimi-for-coding' || id === 'k2p7') return 110
    if (id.includes('k2-thinking')) return 100
  }
  if (provider === 'zai-coding-cn') {
    if (id === 'glm-5.2') return 120
    if (id === 'glm-5.1') return 110
    if (id.includes('glm-5-turbo')) return 105
    if (id === 'glm-4.7') return 100
    if (id.includes('glm-4.7-flash')) return 90
  }
  return model.reasoning ? 50 : 10
}

function providerProfileId(value) {
  return String(value || '')
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 60)
}

function credentialSecret(credential) {
  if (typeof credential === 'string') return credential.trim()
  if (!credential || typeof credential !== 'object') return ''
  return String(credential.key || credential.apiKey || credential.token || credential.accessToken || '').trim()
}

function configuredProviderSecret(credential, providerConfig) {
  const stored = credentialSecret(credential)
  if (stored) return stored
  const reference = String(providerConfig?.apiKey || '').trim()
  if (reference.startsWith('$')) return String(process.env[reference.slice(1)] || '').trim()
  return reference
}

function normalizedProviderBaseUrl(value) {
  return String(value || '').trim().replace(/\/+$/, '').toLowerCase()
}

function sameBaseUrl(left, right) {
  return normalizedProviderBaseUrl(left) === normalizedProviderBaseUrl(right)
}

function hasHeader(headers, expectedName) {
  const expected = String(expectedName || '').toLowerCase()
  return Object.keys(headers || {}).some((name) => name.toLowerCase() === expected)
}

function usesCustomProviderEndpoint(providerId, providerConfig) {
  const baseUrl = String(providerConfig?.baseUrl || '').trim()
  if (!baseUrl) return false
  const officialBaseUrl = PROVIDER_DEFAULT_BASE_URLS[providerId]
  return !officialBaseUrl || !sameBaseUrl(baseUrl, officialBaseUrl)
}

function providerHeaders(providerId, providerConfig, userAgent, modelHeaders = {}) {
  const headers = { ...(providerConfig?.headers || {}), ...(modelHeaders || {}) }
  if (usesCustomProviderEndpoint(providerId, providerConfig) && !hasHeader(headers, 'user-agent')) {
    headers['User-Agent'] = userAgent
  }
  return headers
}

function inferredProviderType(providerConfig) {
  const models = Array.isArray(providerConfig?.models) ? providerConfig.models : []
  if (!models.length) return 'chat'
  return models.every((model) => inferModelKind(model.id, model.kind) !== 'chat') ? 'visual' : 'chat'
}

function visualModelClaimKey(baseUrl, modelId, kind) {
  return [normalizedProviderBaseUrl(baseUrl), String(modelId || '').toLowerCase(), kind].join('\0')
}

function dedicatedVisualModelClaims(modelsJson, appConfig) {
  const claims = new Map()
  const disabled = new Set(appConfig.disabledProviders || [])
  for (const [providerId, provider] of Object.entries(modelsJson.providers || {})) {
    if (disabled.has(providerId)) continue
    const type = appConfig.providerTypes?.[providerId] || inferredProviderType(provider)
    if (type !== 'visual') continue
    for (const model of provider.models || []) {
      const kind = inferModelKind(model.id, model.kind)
      if (kind === 'chat') continue
      const baseUrl = model.baseUrl || provider.baseUrl || PROVIDER_DEFAULT_BASE_URLS[providerId] || ''
      if (!baseUrl) continue
      const key = visualModelClaimKey(baseUrl, model.id, kind)
      const providerIds = claims.get(key) || new Set()
      providerIds.add(providerId)
      claims.set(key, providerIds)
    }
  }
  return claims
}

function claimedByOtherVisualProvider(claims, providerId, baseUrl, modelId, kind) {
  const providerIds = claims.get(visualModelClaimKey(baseUrl, modelId, kind))
  return Boolean(providerIds && [...providerIds].some((id) => id !== providerId))
}

function claimedByOtherVisualProviderAnyKind(claims, providerId, baseUrl, modelId) {
  const prefix = [normalizedProviderBaseUrl(baseUrl), String(modelId || '').toLowerCase(), ''].join('\0')
  for (const [key, providerIds] of claims) {
    if (!key.startsWith(prefix)) continue
    if ([...providerIds].some((id) => id !== providerId)) return true
  }
  return false
}

export class AgentRuntimeService {
  constructor({ cwd, dataDir, appVersion, providerDiscovery, providerModelDiscovery, browserAutomationDriver, eventObserver, legacyDefaultCwds = [] } = {}) {
    this.cwd = normalizeWorkspacePath(cwd)
    cwd = this.cwd
    const currentWorkspaceKey = workspacePathKey(cwd)
    this.legacyDefaultWorkspaceKeys = new Set(
      legacyDefaultCwds
        .map((path) => workspacePathKey(path))
        .filter((path) => path && path !== currentWorkspaceKey),
    )
    this.eventObserver = typeof eventObserver === 'function' ? eventObserver : null
    this.dataDir = dataDir
    this.providerUserAgent = String(appVersion || '').trim() ? `Pisper/${String(appVersion).trim()}` : 'Pisper'
    this.providerDiscovery = providerDiscovery || new ProviderDiscoveryService({ cwd })
    this.providerModelDiscovery = providerModelDiscovery || new ProviderModelDiscoveryService()
    this.sessionDir = join(dataDir, 'sessions')
    this.authPath = join(dataDir, 'auth.json')
    this.modelsPath = join(dataDir, 'models.json')
    this.providerModelCatalogPath = join(dataDir, 'pisper-provider-models.json')
    this.modelMetadataPath = join(dataDir, 'pisper-model-metadata.json')
    this.modelMetadata = new ModelMetadataService({ path: this.modelMetadataPath })
    this.providerModelCatalog = new ProviderModelCatalogService({ path: this.providerModelCatalogPath, metadata: this.modelMetadata })
    this.settingsPath = join(dataDir, 'settings.json')
    this.appConfigPath = join(dataDir, 'pisper.json')
    this.toolPlugins = new ToolPluginService(this.appConfigPath)
    this.webSearch = new WebSearchService({ configPath: this.appConfigPath })
    this.visualGeneration = new VisualGenerationService({
      modelsPath: this.modelsPath,
      authPath: this.authPath,
      appConfigPath: this.appConfigPath,
      getModelRuntime: () => this.modelRuntime,
    })
    this.sessionMetaPath = join(dataDir, 'pisper-sessions.json')
    this.usagePath = join(dataDir, 'pisper-usage.json')
    this.assetsDir = join(dataDir, 'pisper-assets')
    this.assetIndexPath = join(dataDir, 'pisper-assets.json')
    this.memory = new LocalMemoryRuntime({ path: join(dataDir, 'pisper-memory.sqlite'), cwd })
    this.memorySummarizer = createSemanticMemorySummarizer({
      getModelRuntime: () => this.modelRuntime,
      getDefaultModel: () => this.resolveDefaultModel(),
    })
    this.goals = new GoalService({ path: join(dataDir, 'pisper-goals.json') })
    this.gitChanges = new GitChangesService()
    this.plans = new PlanService({
      path: join(dataDir, 'pisper-plans.json'),
      legacyPath: join(dataDir, 'pisper-task-lists.json'),
    })
    this.browserAutomation = new BrowserAutomationService({ driver: browserAutomationDriver })
    this.goalEmitters = new Map()
    this.agentEmitters = new Map()
    this.mcp = new McpService({ path: join(dataDir, 'pisper-mcp.json'), cwd })
    this.skills = new SkillsService({
      path: join(dataDir, 'pisper-skills.json'),
      agentDir: dataDir,
      cwd,
      getSettingsManager: (skillsCwd = this.cwd) => {
        if (!this.settingsManager || workspacePathKey(skillsCwd) === workspacePathKey(this.cwd)) return this.settingsManager
        return SettingsManager.create(skillsCwd, this.dataDir)
      },
      extensionFactories: [pisperPromptExtension, pisperCompactionExtension],
    })
    this.channels = new ChannelService({
      path: join(dataDir, 'pisper-channels.json'),
      cwd,
      agent: {
        prompt: (input) => this.promptFromChannel(input),
        abort: (sessionId) => this.abortSession(sessionId),
        validateDirectory: (input) => resolveDirectory(input, this.cwd),
      },
    })
    this.notificationSettings = new NotificationSettingsService({ path: this.appConfigPath, browserEventsPath: join(dataDir, 'pisper-browser-notifications.json'), channels: this.channels })
    this.schedules = new ScheduleService({
      path: join(dataDir, 'pisper-schedules.json'),
      cwd,
      agent: {
        prompt: (input) => this.promptFromChannel({ sessionId: '', ...input }),
        validateDirectory: (input) => resolveDirectory(input, this.cwd),
      },
      notifications: this.notificationSettings,
    })
    this.workflows = new WorkflowService({
      path: join(dataDir, 'pisper-workflows.json'),
      cwd,
      agent: {
        prompt: (input) => this.promptFromChannel({ sessionId: '', ...input }),
        abort: (sessionId) => this.abortSession(sessionId),
        validateDirectory: (input) => resolveDirectory(input, this.cwd),
      },
      notifications: this.notificationSettings,
    })
    this.sessions = new Map()
    this.maxResidentSessionRuntimes = MAX_RESIDENT_SESSION_RUNTIMES
    this.sessionRuntimeIdleTtlMs = SESSION_RUNTIME_IDLE_TTL_MS
    this.sessionRuntimeSweepIntervalMs = SESSION_RUNTIME_SWEEP_INTERVAL_MS
    this.sessionRuntimeSweepTimer = null
    this.pendingSessions = new Map()
    this.storedSessionsCache = null
    this.storedSessionsPromise = null
    this.sessionRuntimeVersion = 0
    this.liveSessions = new Map()
    this.planEmitters = new Map()
    this.sessionHistoryCache = new Map()
    this.sessionHistoryReadChunkBytes = SESSION_HISTORY_READ_CHUNK_BYTES
    this.maxSessionHistoryCacheEntries = MAX_SESSION_HISTORY_CACHE_ENTRIES
    this.maxSessionHistoryCacheSourceBytes = MAX_SESSION_HISTORY_CACHE_SOURCE_BYTES
    this.maxSessionHistoryCacheEstimatedBytes = MAX_SESSION_HISTORY_CACHE_ESTIMATED_BYTES
    this.sessionHistoryPaths = new Map()
    this.sessionContextUsageCache = new Map()
    this.modelRuntime = null
    this.settingsManager = null
    this.compactionThresholdPercent = DEFAULT_COMPACTION_THRESHOLD_PERCENT
    this.sessionMeta = {}
    this.permissions = new SessionPermissionService({
      getMode: (sessionId) => this.sessionMeta[sessionId]?.permissionMode || permissionModeForExecutionMode(this.getSessionExecutionMode(sessionId)),
      getExecutionMode: (sessionId) => this.getSessionExecutionMode(sessionId),
      getToolRisk: (toolName) => this.mcp.getToolRisk(toolName),
    })
    this.multiAgents = new MultiAgentService({
      path: join(dataDir, 'pisper-agents.json'),
      agentDir: this.dataDir,
      getModelRuntime: () => this.modelRuntime,
      getSettingsManager: () => this.settingsManager,
      getCompactionThresholdPercent: () => this.compactionThresholdPercent,
      createResourceLoader: ({ cwd: childCwd, appendSystemPrompt }) => this.skills.createResourceLoader(childCwd, { appendSystemPrompt }),
    })
    // 设置 Agent 完成通知器：向父会话注入隐藏消息
    this.multiAgents.setCompletionNotifier((agent) => this.injectAgentCompletion(agent))
    this.sessionMetaWrite = Promise.resolve()
    this.usageLedger = { days: {}, sessionScans: {} }
    this.usageWrite = Promise.resolve()
    this.assetIndex = { assets: [] }
    this.assetWrite = Promise.resolve()
    this.providerModelRefreshPromise = null
    this.agentWakeupTimers = new Map()
  }

  async init() {
    await mkdir(this.sessionDir, { recursive: true })
    await mkdir(this.assetsDir, { recursive: true })
    await cleanupRemovedLocalEmbeddingData(this.dataDir)
    this.sessionMeta = await readJson(this.sessionMetaPath, {})
    await this.migrateLegacyDefaultWorkspaces()
    await this.migrateSessionExecutionModes()
    this.usageLedger = await readJson(this.usagePath, { days: {}, sessionScans: {} })
    this.usageLedger.days ||= {}
    this.usageLedger.sessionScans ||= {}
    this.assetIndex = await readJson(this.assetIndexPath, { assets: [] })
    this.assetIndex.assets = Array.isArray(this.assetIndex.assets) ? this.assetIndex.assets : []
    const appConfig = await readJson(this.appConfigPath, {})
    this.compactionThresholdPercent = normalizeCompactionThresholdPercent(appConfig.compactionThresholdPercent)
    await migrateKimiCodeProvider({
      authPath: this.authPath,
      modelsPath: this.modelsPath,
      settingsPath: this.settingsPath,
      appConfigPath: this.appConfigPath,
    })
    await Promise.all([this.providerModelCatalog.init(), this.modelMetadata.init()])
    this.settingsManager = SettingsManager.create(this.cwd, this.dataDir)
    await this.skills.init()
    await this.mcp.init()
    await this.toolPlugins.ensureDefaultTools(['memory_search', 'memory_remember'], 'memoryToolsV1')
    await this.toolPlugins.ensureDefaultTools(['mcp_list', 'mcp_manage'], 'mcpManagementToolsV1')
    await this.toolPlugins.ensureDefaultTools(['web_search'], 'webSearchToolV1')
    await this.toolPlugins.ensureDefaultTools(['browser_automation'], 'browserAutomationToolV1')
    await this.reloadModelRuntime()
    await this.memory.init()
    this.memory.setSemanticSummarizer(this.memorySummarizer)
    await this.goals.init({ pauseActive: true })
    await this.plans.init()
    await this.multiAgents.init()
    await this.channels.init()
    await this.schedules.init()
    await this.workflows.init()
    this.startSessionRuntimeSweeper()
    void this.refreshProviderModels().catch(() => {})
  }

  async reloadModelRuntime() {
    const modelsJson = await readJson(this.modelsPath, { providers: {} })
    this.modelRuntime = await ModelRuntime.create({
      authPath: this.authPath,
      modelsPath: this.modelsPath,
      allowModelNetwork: false,
    })
    const configuredBaseUrls = {}
    const configuredHeaders = {}
    const configuredContextWindows = {}
    const configuredInputs = {}
    for (const provider of this.modelRuntime.getProviders()) {
      const overlay = modelsJson.providers?.[provider.id] || {}
      configuredBaseUrls[provider.id] = overlay.baseUrl || PROVIDER_DEFAULT_BASE_URLS[provider.id] || this.modelRuntime.getModels(provider.id)[0]?.baseUrl || ''
      if (usesCustomProviderEndpoint(provider.id, overlay)) {
        configuredHeaders[provider.id] = providerHeaders(provider.id, overlay, this.providerUserAgent)
      }
      for (const model of overlay.models || []) {
        if (Number(model?.contextWindow) > 0) configuredContextWindows[`${provider.id}:${model.id}`] = Number(model.contextWindow)
        if (Array.isArray(model?.input)) configuredInputs[`${provider.id}:${model.id}`] = model.input
      }
    }
    this.providerModelCatalog.decorateRuntime(this.modelRuntime, configuredBaseUrls, configuredHeaders, configuredContextWindows, configuredInputs)
  }

  emitGoalUpdate(sessionId, goal, send = this.goalEmitters.get(sessionId)) {
    const live = this.liveSessions.get(sessionId)
    if (live) live.goal = goal || null
    try { send?.('goal_update', { sessionId, goal: goal || null }) } catch {}
  }

  emitPlanUpdate(sessionId, plan, send = this.planEmitters.get(sessionId)) {
    const live = this.liveSessions.get(sessionId)
    const nextPlan = plan || this.plans.get(sessionId)
    const updatedAt = nextPlan?.updatedAt || new Date().toISOString()
    const currentActivity = { type: 'plan', plan: nextPlan, changes: livePlanChanges(live?.plan, nextPlan), updatedAt }
    if (live) {
      live.plan = nextPlan
      setLiveActivity(live, currentActivity)
    }
    try { send?.('plan_update', { sessionId, plan: nextPlan, currentActivity }) } catch {}
  }

  emitAgentUpdate(sessionId, agent, send = this.agentEmitters.get(sessionId)) {
    const allAgents = this.multiAgents.summaries(sessionId)
    const updatedAgent = allAgents.find((item) => item.id === agent?.id) || null
    const agents = allAgents.filter((item) => ['queued', 'starting', 'running'].includes(item.status))
    const live = this.liveSessions.get(sessionId)
    const currentActivity = updatedAgent
      ? { type: 'agent', agent: updatedAgent, updatedAt: updatedAgent.lastActivityAt || new Date().toISOString() }
      : live?.currentActivity || null
    if (live) {
      live.agents = agents
      if (updatedAgent) {
        live.activityFeed = pushLiveActivity(live.activityFeed, currentActivity)
        if (live.currentActivity?.type !== 'tool') live.currentActivity = currentActivity
      }
    }
    try { send?.('agent_update', { sessionId, agent: updatedAgent, agents, currentActivity }) } catch {}
  }

  getSessionExecutionMode(sessionId) {
    return normalizeExecutionMode(this.sessionMeta[sessionId]?.executionMode, DEFAULT_EXECUTION_MODE)
  }

  listStoredSessions({ refresh = false } = {}) {
    if (!refresh && this.storedSessionsCache) return Promise.resolve(this.storedSessionsCache)
    if (this.storedSessionsPromise) return this.storedSessionsPromise
    this.storedSessionsPromise = SessionManager.listAll(this.sessionDir)
      .then((sessions) => {
        this.storedSessionsCache = sessions
        return sessions
      })
      .finally(() => {
        this.storedSessionsPromise = null
      })
    return this.storedSessionsPromise
  }

  openStoredSession(path) {
    return SessionManager.open(path, this.sessionDir)
  }

  async migrateLegacyDefaultWorkspaces() {
    if (!this.legacyDefaultWorkspaceKeys.size) return
    const sessions = await this.listStoredSessions()
    let changed = false
    for (const session of sessions) {
      const current = this.sessionMeta[session.id] || {}
      const cwd = current.cwd || session.cwd
      if (!this.legacyDefaultWorkspaceKeys.has(workspacePathKey(cwd))) continue
      this.sessionMeta[session.id] = { ...current, cwd: this.cwd }
      changed = true
    }
    if (changed) await this.saveSessionMeta()
  }

  async migrateSessionExecutionModes() {
    const sessions = await this.listStoredSessions()
    let changed = false
    for (const session of sessions) {
      const current = this.sessionMeta[session.id] || {}
      if (EXECUTION_MODES.has(current.executionMode)) continue
      const executionMode = migrateLegacyExecutionMode(current)
      this.sessionMeta[session.id] = {
        ...current,
        executionMode,
        permissionMode: permissionModeForExecutionMode(executionMode),
      }
      changed = true
    }
    if (changed) await this.saveSessionMeta()
  }

  optionalToolNames(value) {
    const blockedToolNames = new Set(value?.blockedToolNames || [])
    return [
      ...(value?.baseToolNames || []),
      ...PLAN_COMPATIBILITY_TOOL_NAMES,
      ...MULTI_AGENT_TOOL_NAMES,
    ].filter((name) => !blockedToolNames.has(name))
  }

  syncGoalTools(value, goal) {
    if (!value?.session) return
    const mode = this.getSessionExecutionMode(value.session.sessionId)
    const blockedToolNames = new Set(value.blockedToolNames || [])
    const availableToolNames = [
      ...(value.baseToolNames || []),
      TOOL_DISCOVERY_NAME,
      ...PLAN_ALL_TOOL_NAMES,
      ...MULTI_AGENT_TOOL_NAMES,
      ...GOAL_TOOL_NAMES,
    ].filter((name) => !blockedToolNames.has(name))
    const names = selectedToolNames({
      availableToolNames,
      promotedToolNames: value.promotedToolNames || [],
      requestedToolNames: value.requestedToolNames || [],
      goalToolNames: GOAL_TOOL_NAMES,
      goalActive: goal?.status === 'active',
    })
    value.session.setActiveToolsByName(filterToolsForExecutionMode(
      names,
      mode,
      (toolName) => this.mcp.getToolRisk(toolName),
    ))
  }

  async promoteSessionTools(value, toolNames = []) {
    if (!value?.session) return { activatedToolNames: [], promotedToolNames: [] }
    const availableToolNames = this.optionalToolNames(value)
    const permittedToolNames = filterToolsForExecutionMode(
      toolNames.filter((name) => availableToolNames.includes(name)),
      this.getSessionExecutionMode(value.session.sessionId),
      (toolName) => this.mcp.getToolRisk(toolName),
    )
    const previousPromotedToolNames = value.promotedToolNames || []
    const promotedToolNames = mergePromotedToolNames({
      availableToolNames: availableToolNames.filter((name) => !PLAN_COMPATIBILITY_TOOL_NAMES.includes(name)),
      promotedToolNames: previousPromotedToolNames,
      requestedToolNames: permittedToolNames,
    })
    let promotionWrite = null
    if (promotedToolNames.join('\0') !== previousPromotedToolNames.join('\0')) {
      value.promotedToolNames = promotedToolNames
      const sessionId = value.session.sessionId
      this.sessionMeta[sessionId] = { ...(this.sessionMeta[sessionId] || {}), promotedToolNames }
      promotionWrite = this.saveSessionMeta()
    }
    this.syncGoalTools(value, this.goals.get(value.session.sessionId))
    applyPisperSystemPrompt(value.session, value.session.model)
    if (promotionWrite) await promotionWrite
    const active = new Set(value.session.getActiveToolNames())
    return {
      activatedToolNames: permittedToolNames.filter((name) => active.has(name)),
      promotedToolNames,
    }
  }

  async selectToolsForMessage(value, _message, { requestedToolNames = [], preserveRequested = false } = {}) {
    if (!value?.session) return []
    const requested = [...new Set((Array.isArray(requestedToolNames) ? requestedToolNames : [])
      .map((name) => String(name || '').trim())
      .filter(Boolean))]
    value.requestedToolNames = preserveRequested
      ? [...new Set([...(value.requestedToolNames || []), ...requested])]
      : requested
    await this.promoteSessionTools(value, requested)
    return value.session.getActiveToolNames()
  }

  async pauseSessionGoal(id) {
    const goal = await this.goals.pause(id)
    const value = this.sessions.get(id)
    if (value) this.syncGoalTools(value, goal)
    this.emitGoalUpdate(id, goal)
    return goal
  }

  async setSessionGoalBudget(id, tokenBudget) {
    const goal = await this.goals.setBudget(id, tokenBudget)
    const value = this.sessions.get(id)
    if (value) this.syncGoalTools(value, goal)
    this.emitGoalUpdate(id, goal)
    return goal
  }

  getSessionGoal(id) {
    return this.goals.get(id)
  }

  touchSessionRuntime(value) {
    if (value) value.lastAccessedAt = Date.now()
    return value
  }

  sessionRuntimeIsProtected(id, value) {
    return Boolean(
      value?.session?.isStreaming ||
      this.goals.get(id)?.status === 'active' ||
      this.agentWakeupTimers.has(id) ||
      value?.pendingAgentNotifications?.length ||
      this.multiAgents.hasActive?.(id),
    )
  }

  disposeSessionRuntime(id, value) {
    if (!value || this.sessions.get(id) !== value) return false
    this.permissions.resolveSession(id, false, '会话运行时已从内存释放，请重新发送消息。')
    try {
      value.session.dispose()
    } finally {
      this.sessions.delete(id)
      this.sessionContextUsageCache.delete(id)
      const sessionPath = value.session.sessionFile
      if (sessionPath) this.sessionHistoryCache.delete(sessionPath)
    }
    return true
  }

  evictIdleSessionRuntimes(exceptId = '', now = Date.now()) {
    const maximum = Math.max(1, Number(this.maxResidentSessionRuntimes) || MAX_RESIDENT_SESSION_RUNTIMES)
    const idleTtlMs = Math.max(0, Number(this.sessionRuntimeIdleTtlMs) || SESSION_RUNTIME_IDLE_TTL_MS)
    const candidates = () => [...this.sessions.entries()]
      .filter(([id, value]) => id !== exceptId && !this.sessionRuntimeIsProtected(id, value))
      .sort((left, right) => (left[1].lastAccessedAt || 0) - (right[1].lastAccessedAt || 0))
    let evicted = 0
    if (idleTtlMs > 0) {
      for (const [id, value] of candidates()) {
        if (now - (value.lastAccessedAt || 0) < idleTtlMs) continue
        if (this.disposeSessionRuntime(id, value)) evicted += 1
      }
    }
    const overflow = candidates()
    while (this.sessions.size > maximum && overflow.length) {
      const [id, value] = overflow.shift()
      if (this.disposeSessionRuntime(id, value)) evicted += 1
    }
    return evicted
  }

  startSessionRuntimeSweeper() {
    if (this.sessionRuntimeSweepTimer) return
    const intervalMs = Math.max(1_000, Number(this.sessionRuntimeSweepIntervalMs) || SESSION_RUNTIME_SWEEP_INTERVAL_MS)
    this.sessionRuntimeSweepTimer = setInterval(() => {
      try { this.evictIdleSessionRuntimes() } catch {}
    }, intervalMs)
    this.sessionRuntimeSweepTimer.unref?.()
  }

  getRuntimeDiagnostics() {
    const now = Date.now()
    const memory = process.memoryUsage()
    const resident = [...this.sessions.entries()]
    const protectedSessions = resident.filter(([id, value]) => this.sessionRuntimeIsProtected(id, value)).length
    const idleAges = resident
      .filter(([id, value]) => !this.sessionRuntimeIsProtected(id, value))
      .map(([, value]) => Math.max(0, now - (value.lastAccessedAt || now)))
    const historySourceBytes = [...this.sessionHistoryCache.values()].reduce((total, entry) => total + entry.size, 0)
    return {
      timestamp: new Date(now).toISOString(),
      uptimeSeconds: Math.round(process.uptime()),
      workspaceCwd: this.cwd,
      memory: {
        rss: memory.rss,
        heapTotal: memory.heapTotal,
        heapUsed: memory.heapUsed,
        external: memory.external,
        arrayBuffers: memory.arrayBuffers,
      },
      sessions: {
        resident: resident.length,
        protected: protectedSessions,
        idle: resident.length - protectedSessions,
        pending: this.pendingSessions.size,
        live: this.liveSessions.size,
        oldestIdleMs: idleAges.length ? Math.max(...idleAges) : 0,
        maxResident: this.maxResidentSessionRuntimes,
        idleTtlMs: this.sessionRuntimeIdleTtlMs,
      },
      historyCache: {
        entries: this.sessionHistoryCache.size,
        sourceBytes: historySourceBytes,
        estimatedBytes: historySourceBytes * SESSION_HISTORY_CACHE_MEMORY_MULTIPLIER,
        maxEntries: this.maxSessionHistoryCacheEntries,
        maxSourceBytes: this.maxSessionHistoryCacheSourceBytes,
        maxEstimatedBytes: this.maxSessionHistoryCacheEstimatedBytes,
      },
    }
  }

  async sessionWorkspaceCwd(id) {
    if (!id) return this.cwd
    const activeCwd = this.sessions.get(id)?.cwd
    if (activeCwd) return activeCwd
    const pendingCwd = this.pendingSessions.get(id)?.cwd
    if (pendingCwd) return pendingCwd
    const metaCwd = this.sessionMeta[id]?.cwd
    if (metaCwd) return metaCwd
    const stored = await this.findSessionInfo(id)
    return stored?.cwd || this.cwd
  }

  async sessionGitCwd(id) {
    return this.sessionWorkspaceCwd(id)
  }

  async getSessionGitChanges(id) {
    return this.gitChanges.getChanges(await this.sessionGitCwd(id))
  }

  async commitSessionGitChanges(id, message) {
    if (this.sessions.get(id)?.session.isStreaming) throw new Error('当前会话正在运行，请完成或停止后再提交改动。')
    return this.gitChanges.commit(await this.sessionGitCwd(id), message)
  }

  async pushSessionGitChanges(id) {
    return this.gitChanges.push(await this.sessionGitCwd(id))
  }

  async revertSessionGitChanges(id) {
    if (this.sessions.get(id)?.session.isStreaming) throw new Error('当前会话正在运行，请完成或停止后再撤销改动。')
    return this.gitChanges.revert(await this.sessionGitCwd(id))
  }

  async disposeSessions() {
    for (const [id, value] of this.sessions) {
      await this.pauseSessionGoal(id)
      this.multiAgents.abortParent(id)
      this.permissions.resolveSession(id, false, 'Agent Runtime 正在重新加载，工具未执行。')
      value.session.dispose()
    }
    this.sessions.clear()
  }

  invalidateSessionRuntimes() {
    this.sessionRuntimeVersion += 1
    for (const [id, value] of this.sessions) {
      if (value.session.isStreaming) continue
      this.multiAgents.abortParent(id)
      this.permissions.resolveSession(id, false, 'Agent Runtime resources changed before the tool could run.')
      value.session.dispose()
      this.sessions.delete(id)
    }
  }

  saveSessionMeta() {
    const snapshot = JSON.parse(JSON.stringify(this.sessionMeta))
    this.sessionMetaWrite = this.sessionMetaWrite
      .catch(() => {})
      .then(() => writeJsonAtomic(this.sessionMetaPath, snapshot))
    return this.sessionMetaWrite
  }

  async markSessionTitle(id, name, manual) {
    this.sessionMeta[id] = { ...(this.sessionMeta[id] || {}), name, manual: Boolean(manual) }
    await this.saveSessionMeta()
  }

  saveUsageLedger() {
    const snapshot = JSON.parse(JSON.stringify(this.usageLedger))
    this.usageWrite = this.usageWrite
      .catch(() => {})
      .then(() => writeJsonAtomic(this.usagePath, snapshot))
    return this.usageWrite
  }

  async recordUsage(day, key, usage) {
    if (!day || !key) return false
    const normalized = normalizedUsage(usage)
    if (!normalized.totalTokens && !normalized.input && !normalized.output) return false
    const days = this.usageLedger.days
    days[day] ||= { records: {} }
    days[day].records ||= {}
    if (days[day].records[key]) return false
    days[day].records[key] = normalized
    const retainedDays = Object.keys(days).sort().slice(-45)
    for (const existingDay of Object.keys(days)) {
      if (!retainedDays.includes(existingDay)) delete days[existingDay]
    }
    await this.saveUsageLedger()
    return true
  }

  async scanSessionUsage(info, day) {
    const file = await stat(info.path)
    const scans = this.usageLedger.sessionScans
    const previous = scans[info.id]
    const records = this.usageLedger.days[day]?.records || {}
    const hasRecordedSessionUsage = Object.keys(records).some((key) => key.startsWith(`session:${info.id}:`))
    if (!previous && hasRecordedSessionUsage) {
      scans[info.id] = { path: info.path, size: file.size }
      return true
    }

    let offset = previous?.path === info.path && file.size >= Number(previous.size || 0)
      ? Number(previous.size || 0)
      : 0
    if (offset >= file.size) return false

    const handle = await open(info.path, 'r')
    let changed = false
    let position = offset
    let scannedUntil = offset
    let remainder = Buffer.alloc(0)
    try {
      while (position < file.size) {
        const chunk = Buffer.allocUnsafe(Math.min(256 * 1024, file.size - position))
        const { bytesRead } = await handle.read(chunk, 0, chunk.length, position)
        if (!bytesRead) break
        position += bytesRead
        const combined = remainder.length ? Buffer.concat([remainder, chunk.subarray(0, bytesRead)]) : chunk.subarray(0, bytesRead)
        const newline = combined.lastIndexOf(0x0a)
        if (newline < 0) {
          remainder = combined
          continue
        }
        const complete = combined.subarray(0, newline).toString('utf8')
        remainder = combined.subarray(newline + 1)
        scannedUntil = position - remainder.length
        for (const line of complete.split('\n')) {
          if (!line.trim()) continue
          try {
            const entry = JSON.parse(line.trimEnd())
            if (entry.type !== 'message' || entry.message?.role !== 'assistant' || !entry.message.usage) continue
            const timestamp = entry.message.timestamp || entry.timestamp
            if (localDayKey(timestamp) !== day) continue
            const key = `session:${info.id}:${entry.id}`
            this.usageLedger.days[day] ||= { records: {} }
            this.usageLedger.days[day].records ||= {}
            if (this.usageLedger.days[day].records[key]) continue
            this.usageLedger.days[day].records[key] = normalizedUsage(entry.message.usage)
            changed = true
          } catch {
            // Ignore malformed or partially written history lines.
          }
        }
      }
    } finally {
      await handle.close()
    }
    if (scannedUntil !== Number(previous?.size || 0) || previous?.path !== info.path) {
      scans[info.id] = { path: info.path, size: scannedUntil }
      changed = true
    }
    return changed
  }

  async getTodayUsage() {
    const day = localDayKey()
    const sessions = await this.listStoredSessions()
    let changed = false
    for (const info of sessions) {
      if (localDayKey(info.modified) !== day) continue
      if (await this.scanSessionUsage(info, day)) changed = true
    }
    if (changed) await this.saveUsageLedger()
    const totals = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, totalTokens: 0 }
    for (const usage of Object.values(this.usageLedger.days[day]?.records || {})) addUsage(totals, usage)
    return { day, ...totals }
  }

  saveAssetIndex() {
    const snapshot = structuredClone(this.assetIndex)
    this.assetWrite = this.assetWrite
      .catch(() => {})
      .then(() => writeJsonAtomic(this.assetIndexPath, snapshot))
    return this.assetWrite
  }

  publicAsset(asset) {
    if (!asset) return null
    const publicValue = { ...asset }
    delete publicValue.storagePath
    return publicValue
  }

  async createAsset(input) {
    const now = new Date().toISOString()
    const source = String(input.source || 'upload')
    if (input.kind === 'link' || input.url) {
      const url = new URL(String(input.url || ''))
      if (!['http:', 'https:'].includes(url.protocol)) throw new Error('链接只支持 http 或 https。')
      const existing = this.assetIndex.assets.find((asset) => asset.kind === 'link' && asset.url === url.href)
      if (existing) return this.publicAsset(existing)
      const asset = {
        id: randomUUID(), kind: 'link', name: safeAttachmentName(input.name || url.hostname), url: url.href,
        mimeType: 'text/uri-list', size: 0, source, sessionId: input.sessionId || '', sessionName: input.sessionName || '', created: now, modified: now,
      }
      this.assetIndex.assets.unshift(asset)
      await this.saveAssetIndex()
      return this.publicAsset(asset)
    }

    const name = safeAttachmentName(input.name)
    const buffer = input.text !== undefined
      ? Buffer.from(String(input.text), 'utf8')
      : Buffer.from(String(input.data || ''), 'base64')
    if (!buffer.length) throw new Error(`${name} 内容为空。`)
    if (buffer.length > MAX_ASSET_BYTES) throw new Error(`${name} 超过 24 MB 资产限制。`)
    const hash = createHash('sha256').update(buffer).digest('hex')
    const duplicate = this.assetIndex.assets.find((asset) => asset.hash === hash && asset.name === name)
    if (duplicate) {
      duplicate.modified = now
      if (input.sessionId && !duplicate.sessionId) duplicate.sessionId = input.sessionId
      if (input.sessionName && !duplicate.sessionName) duplicate.sessionName = input.sessionName
      await this.saveAssetIndex()
      return this.publicAsset(duplicate)
    }
    const id = randomUUID()
    const extension = extname(name).slice(0, 12)
    const storagePath = join(this.assetsDir, `${id}${extension}`)
    await writeFile(storagePath, buffer)
    const mimeType = String(input.mimeType || mimeFromName(name))
    const asset = {
      id, kind: mimeType.startsWith('image/') || IMAGE_EXTENSIONS.has(extname(name).toLowerCase()) ? 'image' : 'file',
      name, mimeType, size: buffer.length, hash, storagePath, source,
      sessionId: input.sessionId || '', sessionName: input.sessionName || '', created: now, modified: now,
    }
    this.assetIndex.assets.unshift(asset)
    await this.saveAssetIndex()
    return this.publicAsset(asset)
  }

  async archiveAttachments(sessionId, sessionName, attachments = []) {
    const archived = []
    for (const attachment of attachments) {
      try {
        const created = await this.createAsset({
          name: attachment.name,
          mimeType: attachment.mimeType,
          data: attachment.data,
          text: attachment.kind === 'text' ? attachment.text : undefined,
          source: 'attachment',
          sessionId,
          sessionName,
        })
        const stored = this.assetIndex.assets.find((asset) => asset.id === created.id)
        archived.push(stored ? { id: stored.id, path: stored.storagePath || stored.filePath || '' } : null)
      } catch {
        // Asset archival must not block the chat request.
        archived.push(null)
      }
    }
    return archived
  }

  async recordGeneratedFile(sessionId, value, filePath) {
    const fileInfo = await stat(filePath).catch(() => null)
    if (!fileInfo?.isFile()) return
    const now = new Date().toISOString()
    const existing = this.assetIndex.assets.find((asset) => asset.filePath === filePath)
    if (existing) {
      existing.size = fileInfo.size
      existing.modified = now
      existing.sessionId = sessionId
      existing.sessionName = value.name
      await this.saveAssetIndex()
      return
    }
    this.assetIndex.assets.unshift({
      id: randomUUID(),
      kind: IMAGE_EXTENSIONS.has(extname(filePath).toLowerCase()) ? 'image' : 'file',
      name: basename(filePath),
      mimeType: mimeFromName(filePath),
      size: fileInfo.size,
      filePath,
      source: 'agent',
      sessionId,
      sessionName: value.name,
      created: now,
      modified: now,
    })
    await this.saveAssetIndex()
  }

  async listAssets({ query = '', kind = '', sessionId = '' } = {}) {
    const needle = String(query || '').trim().toLowerCase()
    const assets = this.assetIndex.assets.filter((asset) => {
      if (kind && asset.kind !== kind) return false
      if (sessionId && asset.sessionId !== sessionId) return false
      return !needle || `${asset.name} ${asset.sessionName} ${asset.url || ''}`.toLowerCase().includes(needle)
    })
    return assets.map((asset) => this.publicAsset(asset))
  }

  findAsset(id) {
    return this.assetIndex.assets.find((asset) => asset.id === id)
  }

  async getAssetContent(id) {
    const asset = this.findAsset(id)
    if (!asset) return null
    if (asset.kind === 'link') {
      return { id: asset.id, kind: 'text', name: `${asset.name}.url.txt`, mimeType: 'text/plain', size: asset.url.length, text: `链接：${asset.url}` }
    }
    const path = asset.storagePath || asset.filePath
    const buffer = await readFile(path)
    if (buffer.length > MAX_CHAT_ASSET_BYTES) throw new Error('资产超过 10 MB，无法直接加入对话；仍可下载或在工作目录中读取。')
    const extension = extname(asset.name).toLowerCase()
    if (asset.kind === 'image') return { id: asset.id, kind: 'image', name: asset.name, mimeType: asset.mimeType, size: buffer.length, data: buffer.toString('base64') }
    if (ASSET_TEXT_EXTENSIONS.has(extension) || asset.mimeType.startsWith('text/')) {
      const text = buffer.toString('utf8')
      return { id: asset.id, kind: 'text', name: asset.name, mimeType: asset.mimeType, size: buffer.length, text: text.slice(0, MAX_EXTRACTED_CHARS), truncated: text.length > MAX_EXTRACTED_CHARS }
    }
    if (ASSET_DOCUMENT_EXTENSIONS.has(extension)) return { id: asset.id, kind: 'document', name: asset.name, mimeType: asset.mimeType, extension: extension.slice(1), size: buffer.length, data: buffer.toString('base64') }
    return { id: asset.id, kind: 'text', name: `${asset.name}.path.txt`, mimeType: 'text/plain', size: path.length, text: asset.filePath ? `本地文件路径：${asset.filePath}` : `资产 ${asset.name} 是二进制文件，请结合文件名称和元数据分析。` }
  }

  async getAssetDownload(id) {
    const asset = this.findAsset(id)
    if (!asset || asset.kind === 'link') return null
    return { asset: this.publicAsset(asset), buffer: await readFile(asset.storagePath || asset.filePath) }
  }

  async deleteAsset(id) {
    const index = this.assetIndex.assets.findIndex((asset) => asset.id === id)
    if (index < 0) return false
    const [asset] = this.assetIndex.assets.splice(index, 1)
    if (asset.storagePath) {
      const root = resolve(this.assetsDir)
      const target = resolve(asset.storagePath)
      if (target !== root && target.startsWith(`${root}${sep}`)) await unlink(target).catch(() => {})
    }
    await this.saveAssetIndex()
    return true
  }

  async listSessions() {
    const sessions = await this.listStoredSessions()
    const settings = this.settingsManager.getGlobalSettings()
    const defaultModel = settings.defaultProvider && settings.defaultModel
      ? `${settings.defaultProvider}/${settings.defaultModel}`
      : ''
    const defaultThinkingLevel = settings.defaultThinkingLevel || 'medium'
    const result = sessions.map((session) => {
      const active = this.sessions.get(session.id)
      const contextModel = active?.session.model
        ? `${active.session.model.provider}/${active.session.model.id}`
        : this.sessionMeta[session.id]?.model || defaultModel
      return {
        id: session.id,
        name: active?.name || session.name || session.firstMessage || DEFAULT_SESSION_NAME,
        firstMessage: session.firstMessage || '',
        messageCount: active
          ? active.session.messages.filter((message) => ['user', 'assistant'].includes(message.role)).length
          : session.messageCount,
        model: contextModel,
        thinkingLevel: active?.session.thinkingLevel || defaultThinkingLevel,
        cwd: active?.cwd || this.sessionMeta[session.id]?.cwd || session.cwd || this.cwd,
        created: session.created.toISOString(),
        modified: active?.modified || session.modified.toISOString(),
        streaming: Boolean(active?.session.isStreaming),
        permissionMode: this.sessionMeta[session.id]?.permissionMode || permissionModeForExecutionMode(this.getSessionExecutionMode(session.id)),
        executionMode: this.getSessionExecutionMode(session.id),
        goal: this.goals.get(session.id),
        plan: this.plans.get(session.id),
        agents: this.multiAgents.summaries(session.id).filter((agent) => ['queued', 'starting', 'running'].includes(agent.status)),
      }
    })
    const persistedIds = new Set(result.map((session) => session.id))
    for (const [id, value] of this.sessions) {
      if (persistedIds.has(id)) continue
      result.unshift({
        id,
        name: value.name || DEFAULT_SESSION_NAME,
        firstMessage: '',
        messageCount: value.session.messages.filter((message) => ['user', 'assistant'].includes(message.role)).length,
        model: value.session.model ? `${value.session.model.provider}/${value.session.model.id}` : defaultModel,
        thinkingLevel: value.session.thinkingLevel || defaultThinkingLevel,
        cwd: value.cwd || this.cwd,
        created: value.created,
        modified: value.modified,
        streaming: Boolean(value.session.isStreaming),
        permissionMode: this.sessionMeta[id]?.permissionMode || permissionModeForExecutionMode(this.getSessionExecutionMode(id)),
        executionMode: this.getSessionExecutionMode(id),
        goal: this.goals.get(id),
        plan: this.plans.get(id),
        agents: this.multiAgents.summaries(id).filter((agent) => ['queued', 'starting', 'running'].includes(agent.status)),
      })
    }
    for (const [id, value] of this.pendingSessions) {
      if (persistedIds.has(id) || this.sessions.has(id)) continue
      result.unshift({
        id,
        name: value.name,
        firstMessage: '',
        messageCount: 0,
        model: defaultModel,
        thinkingLevel: defaultThinkingLevel,
        cwd: value.cwd,
        created: value.created,
        modified: value.modified,
        streaming: false,
        permissionMode: this.sessionMeta[id]?.permissionMode || permissionModeForExecutionMode(this.getSessionExecutionMode(id)),
        executionMode: this.getSessionExecutionMode(id),
        goal: this.goals.get(id),
        plan: this.plans.get(id),
        agents: [],
      })
    }
    result.sort((left, right) => new Date(right.modified).getTime() - new Date(left.modified).getTime())
    return result
  }

  async createSession(name, cwd) {
    const resolvedName = cleanSessionTitle(name) || DEFAULT_SESSION_NAME
    const effectiveCwd = await resolveDirectory(cwd, this.cwd)
    const manager = SessionManager.create(effectiveCwd, this.sessionDir)
    const id = manager.getSessionId()
    const now = new Date().toISOString()
    manager.appendSessionInfo(resolvedName)
    this.pendingSessions.set(id, {
      manager,
      name: resolvedName,
      cwd: effectiveCwd,
      created: now,
      modified: now,
    })
    this.sessionMeta[id] = {
      ...(this.sessionMeta[id] || {}),
      name: resolvedName,
      manual: resolvedName !== DEFAULT_SESSION_NAME,
      executionMode: DEFAULT_EXECUTION_MODE,
      permissionMode: permissionModeForExecutionMode(DEFAULT_EXECUTION_MODE),
    }
    await this.saveSessionMeta()
    const settings = this.settingsManager.getGlobalSettings()
    const model = settings.defaultProvider && settings.defaultModel
      ? `${settings.defaultProvider}/${settings.defaultModel}`
      : ''
    return {
      id,
      name: resolvedName,
      messageCount: 0,
      model,
      thinkingLevel: settings.defaultThinkingLevel || 'medium',
      cwd: effectiveCwd,
      created: now,
      modified: now,
      permissionMode: this.sessionMeta[id].permissionMode,
      executionMode: this.getSessionExecutionMode(id),
      goal: null,
      plan: this.plans.get(id),
      agents: [],
      contextUsage: null,
    }
  }

  async findSessionInfo(id) {
    const sessions = await this.listStoredSessions()
    return sessions.find((session) => session.id === id)
  }

  async getSessionMessages(id) {
    const active = this.sessions.get(id)
    let messages
    if (active) {
      this.touchSessionRuntime(active)
      messages = serializeTranscriptMessages(active.session.messages)
    } else {
      const info = await this.findSessionInfo(id)
      if (!info) return []
      const manager = this.openStoredSession(info.path)
      messages = serializeTranscriptMessages(manager.buildSessionContext().messages)
    }
    const assets = this.assetIndex.assets
      .filter((asset) => asset.sessionId === id && asset.source === 'agent' && /^(?:image|video)\//.test(asset.mimeType || ''))
      .sort((left, right) => new Date(left.created).getTime() - new Date(right.created).getTime())
    return attachGeneratedAssets(messages, assets)
  }

  trimSessionHistoryCache(protectedPath = '') {
    const maximumEntries = Math.max(1, Number(this.maxSessionHistoryCacheEntries) || MAX_SESSION_HISTORY_CACHE_ENTRIES)
    const maximumBytes = Math.max(1, Number(this.maxSessionHistoryCacheEstimatedBytes) || MAX_SESSION_HISTORY_CACHE_ESTIMATED_BYTES)
    const estimatedBytes = () => [...this.sessionHistoryCache.values()]
      .reduce((total, entry) => total + entry.size * SESSION_HISTORY_CACHE_MEMORY_MULTIPLIER, 0)
    const candidates = () => [...this.sessionHistoryCache.entries()]
      .filter(([path]) => path !== protectedPath)
      .sort((left, right) => left[1].touchedAt - right[1].touchedAt)
    while (this.sessionHistoryCache.size > maximumEntries || estimatedBytes() > maximumBytes) {
      const oldest = candidates()[0]?.[0]
      if (!oldest) break
      this.sessionHistoryCache.delete(oldest)
    }
  }

  async readSessionHistoryEntries(path) {
    const file = await stat(path)
    let cached = this.sessionHistoryCache.get(path)
    if (!cached || file.size < cached.size || (file.size === cached.size && file.mtimeMs !== cached.mtimeMs)) {
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
        const maximumChunkBytes = Math.max(1, Number(this.sessionHistoryReadChunkBytes) || SESSION_HISTORY_READ_CHUNK_BYTES)
        const readBuffer = Buffer.allocUnsafe(Math.min(maximumChunkBytes, file.size - position))
        while (position < file.size) {
          const length = Math.min(readBuffer.length, file.size - position)
          const { bytesRead } = await handle.read(readBuffer, 0, length, position)
          if (!bytesRead) break
          position += bytesRead
          const chunk = readBuffer.subarray(0, bytesRead)
          const combined = cached.remainder.length ? Buffer.concat([cached.remainder, chunk]) : chunk
          const newline = combined.lastIndexOf(0x0a)
          const complete = newline >= 0 ? combined.subarray(0, newline).toString('utf8') : ''
          cached.remainder = newline >= 0 ? Buffer.from(combined.subarray(newline + 1)) : Buffer.from(combined)
          for (const line of complete.split('\n')) {
            if (!line.trim()) continue
            try {
              const entry = JSON.parse(line.trimEnd())
              cached.entries.push(entry)
              if (entry?.id) cached.byId.set(entry.id, entry)
            } catch {
              // Ignore a malformed history line without making the rest of the session unreadable.
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
    const maximumSourceBytes = Math.max(1, Number(this.maxSessionHistoryCacheSourceBytes) || MAX_SESSION_HISTORY_CACHE_SOURCE_BYTES)
    if (file.size <= maximumSourceBytes) {
      this.sessionHistoryCache.set(path, cached)
      this.trimSessionHistoryCache(path)
    } else {
      this.sessionHistoryCache.delete(path)
    }
    return cached
  }

  async getSessionHistoryMessages(id) {
    const active = this.sessions.get(id)
    const activePath = active?.session.sessionFile
    let path = activePath || this.sessionHistoryPaths.get(id)
    if (!path) {
      path = (await this.findSessionInfo(id))?.path
      if (path) this.sessionHistoryPaths.set(id, path)
    }
    if (!path) return this.getSessionMessages(id)
    let history
    try {
      history = await this.readSessionHistoryEntries(path)
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
      this.sessionHistoryPaths.delete(id)
      this.sessionHistoryCache.delete(path)
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
        for (const asset of this.assetIndex.assets) {
          if (asset.source === 'attachment' && asset.hash) {
            assetUrlByHash.set(asset.hash, `/api/assets/${encodeURIComponent(asset.id)}/download?inline=1`)
          }
        }
      }
      try {
        const hash = createHash('sha256').update(Buffer.from(String(part.data), 'base64')).digest('hex')
        return assetUrlByHash.get(hash) || null
      } catch {
        return null
      }
    }
    let messages = history.serializedSize === history.size ? history.serializedMessages : null
    if (!messages) {
      messages = serializeTranscriptMessages(
        branch.filter((entry) => entry?.type === 'message').map((entry) => entry.message),
        resolveImageUrl,
      )
      history.serializedSize = history.size
      history.serializedMessages = messages
    }
    const assets = this.assetIndex.assets
      .filter((asset) => asset.sessionId === id && asset.source === 'agent' && /^(?:image|video)\//.test(asset.mimeType || ''))
      .sort((left, right) => new Date(left.created).getTime() - new Date(right.created).getTime())
    return attachGeneratedAssets(messages, assets)
  }

  compactionAwareContextUsage(session, compaction = null) {
    if (!session?.model) return undefined
    const contextWindow = optionalTokenCount(session.model.contextWindow)
    const raw = typeof session.getContextUsage === 'function'
      ? session.getContextUsage()
      : contextWindow
        ? (() => {
            const tokens = estimateMessageContextTokens(session.messages)
            return { tokens, contextWindow, percent: (tokens / contextWindow) * 100 }
          })()
        : undefined
    return this.decorateContextUsage(raw, compaction)
  }

  decorateContextUsage(raw, compaction = null) {
    const contextWindow = optionalTokenCount(raw?.contextWindow)
    if (!contextWindow) return undefined
    let tokens = raw?.tokens == null ? null : optionalTokenCount(raw.tokens)
    let estimated = false
    if (tokens == null && compaction?.status === 'completed' && compaction.estimatedTokensAfter != null) {
      tokens = optionalTokenCount(compaction.estimatedTokensAfter)
      estimated = tokens != null
    }
    const settings = effectiveCompactionSettings(
      this.settingsManager?.getCompactionSettings?.() || { enabled: true, reserveTokens: 16_384, keepRecentTokens: 20_000 },
      contextWindow,
      this.compactionThresholdPercent,
    )
    const compactAtTokens = settings.enabled ? Math.max(0, contextWindow - Math.max(0, Number(settings.reserveTokens) || 0)) : null
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

  async getSessionContextUsage(id, compaction = null) {
    const active = this.sessions.get(id)
    if (active) {
      this.touchSessionRuntime(active)
      return this.compactionAwareContextUsage(active.session, compaction || this.liveSessions.get(id)?.compaction)
    }
    const info = await this.findSessionInfo(id)
    if (!info) return undefined
    const fileStat = await stat(info.path).catch(() => null)
    const cached = this.sessionContextUsageCache.get(id)
    if (fileStat && cached?.path === info.path && cached.size === fileStat.size && cached.mtimeMs === fileStat.mtimeMs) {
      return cached.value
    }
    const manager = this.openStoredSession(info.path)
    const context = manager.buildSessionContext()
    const globalSettings = this.settingsManager?.getGlobalSettings?.() || {}
    const provider = context.model?.provider || globalSettings.defaultProvider
    const modelId = context.model?.modelId || globalSettings.defaultModel
    const model = provider && modelId ? this.modelRuntime?.getModel?.(provider, modelId) : null
    const value = this.decorateContextUsage(persistedContextUsage(manager, model?.contextWindow || 0), compaction)
    this.sessionContextUsageCache.set(id, { path: info.path, size: fileStat?.size || 0, mtimeMs: fileStat?.mtimeMs || 0, value })
    return value
  }

  async getSessionMessagePage(id, { before, limit = DEFAULT_MESSAGE_PAGE_SIZE } = {}) {
    const messages = await this.getSessionHistoryMessages(id)
    const pageSize = Math.min(MAX_MESSAGE_PAGE_SIZE, Math.max(1, Number.parseInt(limit, 10) || DEFAULT_MESSAGE_PAGE_SIZE))
    const requestedEnd = before == null || before === '' ? messages.length : Number.parseInt(before, 10)
    const end = Number.isFinite(requestedEnd) ? Math.min(messages.length, Math.max(0, requestedEnd)) : messages.length
    const start = Math.max(0, end - pageSize)
    return {
      messages: messages.slice(start, end),
      contextUsage: await this.getSessionContextUsage(id),
      pageInfo: {
        start,
        end,
        total: messages.length,
        hasMore: start > 0,
        nextCursor: start > 0 ? String(start) : null,
      },
    }
  }

  async getSessionLive(id) {
    const active = this.sessions.get(id)
    if (active) this.touchSessionRuntime(active)
    const persisted = active ? null : await this.findSessionInfo(id)
    const live = this.liveSessions.get(id)
    const page = await this.getSessionMessagePage(id, { limit: LIVE_MESSAGE_PAGE_SIZE })
    const messages = page.messages
    const streaming = Boolean(active?.session.isStreaming || live?.streaming)
    if (streaming && live) {
      const lastUserIndex = messages.findLastIndex((message) => message.role === 'user')
      const assistantIndex = messages.findIndex((message, index) => index > lastUserIndex && message.role === 'agent')
      const liveMessage = {
        id: `live-${id}`,
        role: 'agent',
        text: live.text,
        streaming: true,
        attachments: live.assets,
      }
      if (assistantIndex >= 0) messages[assistantIndex] = { ...messages[assistantIndex], ...liveMessage, text: liveMessage.text || messages[assistantIndex].text, attachments: live.assets.length ? live.assets : messages[assistantIndex].attachments }
      else messages.push(liveMessage)
    }
    return {
      id,
      streaming,
      messages,
      tools: live?.tools || [],
      error: live?.error || '',
      startedAt: live?.startedAt || null,
      lastActivityAt: live?.lastActivityAt || null,
      finishedAt: live?.finishedAt || null,
      model: active?.session.model ? `${active.session.model.provider}/${active.session.model.id}` : '',
      cwd: active?.cwd || this.sessionMeta[id]?.cwd || persisted?.cwd || this.cwd,
      permissionMode: this.sessionMeta[id]?.permissionMode || permissionModeForExecutionMode(this.getSessionExecutionMode(id)),
      executionMode: this.getSessionExecutionMode(id),
      goal: live?.goal ?? this.goals.get(id),
      plan: live?.plan ?? this.plans.get(id),
      agents: live?.agents ?? this.multiAgents.summaries(id).filter((agent) => ['queued', 'starting', 'running'].includes(agent.status)),
      currentActivity: live?.currentActivity || null,
      activityFeed: live?.activityFeed || [],
      thinkingText: live?.thinkingText || '',
      queuedInputs: live?.queuedInputs ?? queuedSessionInputs(active?.session),
      contextUsage: this.compactionAwareContextUsage(active?.session, live?.compaction) || page.contextUsage,
      compaction: live?.compaction || null,
      approvals: this.permissions.getPending(id),
      pageInfo: page.pageInfo,
    }
  }

  async renameSession(id, name, { manual = true } = {}) {
    const title = cleanSessionTitle(name)
    if (!title) throw new Error('会话标题不能为空。')
    const active = this.sessions.get(id)
    const pending = this.pendingSessions.get(id)
    if (active) {
      active.session.setSessionName(title)
      active.name = title
      active.modified = new Date().toISOString()
    } else if (pending) {
      pending.manager.appendSessionInfo(title)
      pending.name = title
      pending.modified = new Date().toISOString()
    } else {
      const info = await this.findSessionInfo(id)
      if (!info) return null
      const manager = this.openStoredSession(info.path)
      manager.appendSessionInfo(title)
    }
    await this.markSessionTitle(id, title, manual)
    return { id, name: title, manual: Boolean(manual) }
  }

  async setSessionModel(id, provider, modelId) {
    const appConfig = await readJson(this.appConfigPath, { toolMode: 'full', disabledProviders: [] })
    if ((appConfig.disabledProviders || []).includes(String(provider || ''))) throw new Error('该 Provider 当前未启用。')
    await this.modelMetadata.ensure(modelId)
    const model = this.modelRuntime.getModel(String(provider || ''), String(modelId || ''))
    if (!model) throw new Error('指定的模型不存在。')
    const value = await this.getOrCreateSession(id)
    if (value.session.isStreaming) throw new Error('当前会话正在运行，请完成或停止后再切换模型。')
    const settings = this.settingsManager.getGlobalSettings()
    const defaultProvider = settings.defaultProvider
    const defaultModel = settings.defaultModel
    const defaultThinkingLevel = settings.defaultThinkingLevel
    try {
      await value.session.setModel(model)
      applyPisperSystemPrompt(value.session, model)
    } finally {
      if (defaultProvider && defaultModel) {
        this.settingsManager.setDefaultModelAndProvider(defaultProvider, defaultModel)
      }
      if (defaultThinkingLevel) this.settingsManager.setDefaultThinkingLevel(defaultThinkingLevel)
    }
    value.modified = new Date().toISOString()
    const thinking = sessionThinkingState(value.session)
    return {
      id: value.session.sessionId,
      model: `${model.provider}/${model.id}`,
      provider: model.provider,
      modelId: model.id,
      thinkingLevel: thinking.thinkingLevel,
      availableThinkingLevels: thinking.availableLevels,
      thinkingStatus: thinking.status,
      thinkingMessage: thinking.message,
      contextUsage: this.compactionAwareContextUsage(value.session),
    }
  }

  async getSessionThinkingState(id) {
    const value = await this.getOrCreateSession(id)
    return { id: value.session.sessionId, ...sessionThinkingState(value.session) }
  }

  async setSessionThinkingLevel(id, level) {
    const value = await this.getOrCreateSession(id)
    if (value.session.isStreaming) throw new Error('当前会话正在运行，请完成或停止后再切换思考等级。')
    const requested = String(level || '')
    const availableLevels = value.session.getAvailableThinkingLevels()
    if (!availableLevels.includes(requested)) throw new Error('当前模型不支持该思考等级。')
    const defaultThinkingLevel = this.settingsManager.getGlobalSettings().defaultThinkingLevel || 'medium'
    try {
      value.session.setThinkingLevel(requested)
    } finally {
      this.settingsManager.setDefaultThinkingLevel(defaultThinkingLevel)
    }
    value.modified = new Date().toISOString()
    return { id: value.session.sessionId, ...sessionThinkingState(value.session) }
  }

  async setSessionPermission(id, mode) {
    const permissionMode = String(mode || '')
    if (!PERMISSION_MODES.has(permissionMode)) throw new Error('权限模式无效。')
    if (!this.sessions.has(id) && !this.pendingSessions.has(id) && !(await this.findSessionInfo(id))) return null
    this.sessionMeta[id] = { ...(this.sessionMeta[id] || {}), permissionMode }
    await this.saveSessionMeta()
    if (permissionMode !== 'ask') this.permissions.resolveSession(id, true, `权限模式已切换为${permissionMode === 'ignore' ? '忽略' : '自动'}。`)
    return { id, permissionMode, executionMode: this.getSessionExecutionMode(id) }
  }

  async setSessionExecutionMode(id, mode) {
    const executionMode = normalizeExecutionMode(mode, '')
    if (!executionMode) throw new Error('执行模式无效。')
    if (!this.sessions.has(id) && !this.pendingSessions.has(id) && !(await this.findSessionInfo(id))) return null
    const permissionMode = permissionModeForExecutionMode(executionMode)
    this.sessionMeta[id] = { ...(this.sessionMeta[id] || {}), executionMode, permissionMode }
    await this.saveSessionMeta()
    const active = this.sessions.get(id)
    if (active) this.syncGoalTools(active, this.goals.get(id))
    this.permissions.resolveSession(id, executionMode === 'full-access', executionMode === 'full-access' ? '已切换为完全访问。' : '执行模式已切换，请重新发起工具调用。')
    return { id, executionMode, permissionMode }
  }

  resolveToolApproval(sessionId, approvalId, approved) {
    return this.permissions.resolve(sessionId, approvalId, approved)
  }

  async setSessionCwd(id, input) {
    const cwd = await resolveDirectory(input, this.cwd)
    const active = this.sessions.get(id)
    const pending = this.pendingSessions.get(id)
    if (pending) {
      pending.cwd = cwd
      pending.modified = new Date().toISOString()
      this.sessionMeta[id] = { ...(this.sessionMeta[id] || {}), cwd }
      await this.saveSessionMeta()
      return { id, cwd }
    }
    if (active?.session.isStreaming) throw new Error('当前会话正在运行，请完成或停止后再切换工作目录。')
    const activeSessionFile = active?.session.sessionFile
    const activeSessionFileInfo = activeSessionFile ? await stat(activeSessionFile).catch(() => null) : null
    const info = activeSessionFileInfo?.isFile()
      ? { path: activeSessionFile, name: active.name }
      : await this.findSessionInfo(id)
    if (!active && !info) return null

    const name = active?.name || info?.name || this.sessionMeta[id]?.name || DEFAULT_SESSION_NAME
    const previousModel = active?.session.model
    if (active) {
      active.session.dispose()
      this.sessions.delete(id)
    }
    this.sessionMeta[id] = { ...(this.sessionMeta[id] || {}), cwd }
    await this.saveSessionMeta()

    const manager = info?.path
      ? this.openStoredSession(info.path)
      : SessionManager.create(this.cwd, this.sessionDir, { id })
    const next = await this.createSessionRuntime(manager, name)
    if (!info?.path) next.session.setSessionName(name)
    if (previousModel && (!next.session.model || previousModel.provider !== next.session.model.provider || previousModel.id !== next.session.model.id)) {
      await this.setSessionModel(id, previousModel.provider, previousModel.id)
    }
    return { id, cwd: next.cwd }
  }

  listDirectories(input) {
    return listWorkspaceDirectories(input, this.cwd)
  }

  async getOrCreateSession(id) {
    if (id && this.sessions.has(id)) {
      const current = this.sessions.get(id)
      if (current.session.isStreaming || (current.runtimeVersion ?? this.sessionRuntimeVersion) === this.sessionRuntimeVersion) return this.touchSessionRuntime(current)
      current.session.dispose()
      this.sessions.delete(id)
    }
    if (id && this.pendingSessions.has(id)) {
      const pending = this.pendingSessions.get(id)
      this.pendingSessions.delete(id)
      try {
        const value = await this.createSessionRuntime(pending.manager, pending.name)
        value.created = pending.created
        value.modified = pending.modified
        return value
      } catch (error) {
        this.pendingSessions.set(id, pending)
        throw error
      }
    }
    if (id) {
      const info = await this.findSessionInfo(id)
      if (info) return this.createSessionRuntime(this.openStoredSession(info.path))
    }
    return this.createSessionRuntime(SessionManager.create(this.cwd, this.sessionDir))
  }

  async createSessionRuntime(sessionManager, name) {
    const settings = this.settingsManager.getGlobalSettings()
    const storedModelId = storedSessionModelId(sessionManager)
    await this.modelMetadata.ensure(storedModelId || settings.defaultModel)
    const appConfig = await readJson(this.appConfigPath, { toolMode: 'full' })
    const effectiveCwd = await resolveDirectory(this.sessionMeta[sessionManager.getSessionId()]?.cwd, sessionManager.getCwd() || this.cwd)
    const enabledTools = toolsFromConfig(appConfig)
    const runtimeSessionId = sessionManager.getSessionId()
    const [resourceLoader, mcpTools] = await Promise.all([
      this.skills.createResourceLoader(effectiveCwd),
      this.mcp.createToolDefinitions(),
    ])
    const baseToolNames = [...new Set([...enabledTools, ...mcpTools.map((tool) => tool.name)])]
    const promotedToolNames = mergePromotedToolNames({
      availableToolNames: [...baseToolNames, ...MULTI_AGENT_TOOL_NAMES],
      promotedToolNames: this.sessionMeta[runtimeSessionId]?.promotedToolNames || [],
    })
    let runtimeValue = null
    let runtimeSession = null
    const goalTools = schemaOnlyToolDefinitions(createGoalTools({
      getGoal: () => this.goals.get(runtimeSessionId),
      completeGoal: async () => {
        const goal = await this.goals.complete(runtimeSessionId)
        if (runtimeValue) this.syncGoalTools(runtimeValue, goal)
        this.emitGoalUpdate(runtimeSessionId, goal)
        return goal
      },
    }))
    const planTools = createPlanTools({
      getPlan: () => this.plans.get(runtimeSessionId),
      updatePlan: async (items) => {
        const plan = await this.plans.replace(runtimeSessionId, items)
        this.emitPlanUpdate(runtimeSessionId, plan)
        return plan
      },
    })
    const planReader = planTools.find((tool) => tool.name === 'get_plan')
    const installSubagentPermissions = (subagentSession) => this.permissions.install(subagentSession, {
      sessionId: runtimeSession.sessionId,
      cwd: effectiveCwd,
    })
    const accountSubagentUsage = async ({ id, runNumber, runUsage, completedAt }) => {
      await this.recordUsage(localDayKey(completedAt), `agent:${runtimeSession.sessionId}:${id}:${runNumber}`, runUsage)
      const goal = this.goals.get(runtimeSession.sessionId)
      if (goal?.status !== 'active') return
      const accounting = this.goals.account(runtimeSession.sessionId, { goalId: goal.id, usage: runUsage })
      const updatedGoal = this.goals.get(runtimeSession.sessionId)
      if (runtimeValue) this.syncGoalTools(runtimeValue, updatedGoal)
      this.emitGoalUpdate(runtimeSession.sessionId, updatedGoal)
      await accounting
    }
    const parentActiveToolNames = () => {
      const active = new Set(runtimeSession?.getActiveToolNames?.() || [])
      return filterToolsForExecutionMode(
        baseToolNames.filter((name) => active.has(name)),
        this.getSessionExecutionMode(runtimeSessionId),
        (toolName) => this.mcp.getToolRisk(toolName),
      )
    }
    const multiAgentRuntime = {
      spawn: (input) => {
        if (!runtimeSession?.model) throw new Error('当前会话没有可用模型，无法启动 Agent。')
        return this.multiAgents.spawn({
          ...input,
          parentSessionId: runtimeSession.sessionId,
          cwd: effectiveCwd,
          model: runtimeSession.model,
          thinkingLevel: runtimeSession.thinkingLevel,
          allowedTools: [...parentActiveToolNames(), ...(planReader ? [planReader.name] : [])],
          customTools: [...createInheritedCustomTools(), ...(planReader ? [planReader] : [])],
          onProgress: (agent) => this.emitAgentUpdate(runtimeSession.sessionId, agent),
          onSession: installSubagentPermissions,
          onCompleted: accountSubagentUsage,
        })
      },
      list: () => this.multiAgents.list(runtimeSession.sessionId),
      sendMessage: (target, message) => this.multiAgents.sendMessage(runtimeSession.sessionId, target, message),
      followup: (target, message) => this.multiAgents.followup(runtimeSession.sessionId, target, message),
      wait: (timeoutMs, target) => waitForAgentMailbox(this.multiAgents, runtimeSession.sessionId, timeoutMs, target),
      interrupt: (target) => this.multiAgents.interrupt(runtimeSession.sessionId, target),
    }
    const multiAgentTools = schemaOnlyToolDefinitions(createMultiAgentTools({ multiAgentRuntime }))
    const toolDiscovery = createToolDiscoveryTool({
      listTools: () => {
        if (!runtimeValue || !runtimeSession) return []
        const optionalToolNames = filterToolsForExecutionMode(
          this.optionalToolNames(runtimeValue),
          this.getSessionExecutionMode(runtimeSession.sessionId),
          (toolName) => this.mcp.getToolRisk(toolName),
        )
        const staticHotToolNames = new Set(hotToolNames(optionalToolNames))
        const activeToolNames = new Set(runtimeSession.getActiveToolNames())
        return optionalToolNames
          .filter((name) => !staticHotToolNames.has(name) && !PLAN_COMPATIBILITY_TOOL_NAMES.includes(name))
          .map((name) => {
            const definition = runtimeSession.getToolDefinition(name)
            if (!definition) return null
            const description = String(definition.description || '')
              .split('\n')
              .map((line) => line.trim())
              .find(Boolean) || ''
            return {
              name,
              label: definition.label || name,
              description: description.slice(0, 320),
              active: activeToolNames.has(name),
            }
          })
          .filter(Boolean)
      },
      activateTools: (toolNames) => this.promoteSessionTools(runtimeValue, toolNames),
    })
    const bashTool = enabledTools.includes('bash')
      ? await createPisperBashTool(effectiveCwd)
      : null
    const createInheritedCustomTools = () => [
      ...schemaOnlyToolDefinitions(createAppTools({
        cwd: effectiveCwd,
        enabledTools,
        memoryRuntime: this.memory,
        getUserMessage: () => runtimeValue?.pendingUserMessage || '',
        webSearchService: this.webSearch,
        browserAutomationService: this.browserAutomation,
        browserSessionId: runtimeSessionId,
        visualGenerationService: this.visualGeneration,
        onGeneratedFile: ({ path }) => runtimeValue && runtimeSession
          ? this.recordGeneratedFile(runtimeSession.sessionId, runtimeValue, path)
          : undefined,
        mcpRuntime: {
          list: (options) => this.getMcpDashboard(options),
          add: (input) => this.createMcpServer(input),
          update: (id, input) => this.updateMcpServer(id, input),
          remove: (id) => this.deleteMcpServer(id),
          test: (id, options) => this.mcp.test(id, options),
          setToolEnabled: (id, toolName, nextEnabled) => this.setMcpToolEnabled(id, toolName, nextEnabled),
        },
      })),
      ...schemaOnlyToolDefinitions(mcpTools),
      ...(bashTool ? [bashTool] : []),
    ]
    const sessionSettingsManager = createCompactionSettingsManager(
      this.settingsManager,
      () => runtimeSession?.model?.contextWindow,
      () => this.compactionThresholdPercent,
    )
    const { session, modelFallbackMessage } = await createAgentSession({
      cwd: effectiveCwd,
      agentDir: this.dataDir,
      modelRuntime: this.modelRuntime,
      settingsManager: sessionSettingsManager,
      resourceLoader,
      sessionManager,
      tools: [...baseToolNames, TOOL_DISCOVERY_NAME, ...GOAL_TOOL_NAMES, ...PLAN_ALL_TOOL_NAMES, ...MULTI_AGENT_TOOL_NAMES],
      customTools: [...createInheritedCustomTools(), toolDiscovery, ...goalTools, ...planTools, ...multiAgentTools],
    })
    installTransientStreamRetry(session)
    const now = new Date().toISOString()
    const value = {
      session,
      modelFallbackMessage,
      name: name || sessionManager.getSessionName() || DEFAULT_SESSION_NAME,
      created: now,
      modified: now,
      cwd: effectiveCwd,
      baseToolNames,
      enabledTools,
      mcpTools: mcpTools.map((tool) => ({ name: tool.name, label: tool.label || '' })),
      promotedToolNames,
      requestedToolNames: [],
      runtimeVersion: this.sessionRuntimeVersion,
      lastAccessedAt: Date.now(),
    }
    runtimeValue = value
    runtimeSession = session
    if (session.model) {
      const model = `${session.model.provider}/${session.model.id}`
      if (this.sessionMeta[session.sessionId]?.model !== model) {
        this.sessionMeta[session.sessionId] = { ...(this.sessionMeta[session.sessionId] || {}), model }
        void this.saveSessionMeta()
      }
    }
    this.syncGoalTools(value, this.goals.get(session.sessionId))
    this.permissions.install(session, { sessionId: session.sessionId, cwd: effectiveCwd })
    applyPisperSystemPrompt(session, session.model)
    this.sessions.set(session.sessionId, value)
    this.evictIdleSessionRuntimes(session.sessionId)
    return value
  }

  async queueSessionMessage(id, { message, behavior = 'steer' } = {}) {
    const value = this.sessions.get(id)
    if (!value) throw new Error('会话不存在或尚未加载。')
    const text = String(message || '').trim()
    if (!text) throw new Error('消息不能为空。')
    if (text.length > 12_000) throw new Error('运行中追加消息不能超过 12000 个字符。')
    if (!value.session.isStreaming) throw new Error('当前会话已经结束运行，请作为新消息发送。')
    const streamingBehavior = behavior === 'followUp' ? 'followUp' : 'steer'
    await this.selectToolsForMessage(value, text, { preserveRequested: true })
    value.pendingUserMessage = text
    await value.session.prompt(text, { streamingBehavior, source: 'interactive' })
    value.modified = new Date().toISOString()
    return {
      queued: true,
      behavior: streamingBehavior,
      pendingMessageCount: value.session.pendingMessageCount || 0,
      queuedInputs: queuedSessionInputs(value.session),
    }
  }

  /**
   * 向父会话注入 Agent 完成通知（对用户隐藏）
   * 如果父会话正在运行，直接 steer；否则作为 pending 消息，下次用户消息时注入
   */
  injectAgentCompletion(agent) {
    const sessionId = agent.parentSessionId
    const value = this.sessions.get(sessionId)
    if (!value?.session) return

    const prompt = agentCompletionPrompt(agent)
    const session = value.session

    // 如果会话正在运行，直接 steer 注入
    if (session.isStreaming) {
      void session.steer(prompt).catch(() => {})
      return
    }

    // 会话未运行：存入 pending，并调度自动唤醒（防抖合并短时间内的多个完成）
    value.pendingAgentNotifications = value.pendingAgentNotifications || []
    value.pendingAgentNotifications.push(prompt)
    this.scheduleAgentWakeup(sessionId)
  }

  scheduleAgentWakeup(sessionId) {
    if (this.agentWakeupTimers.has(sessionId)) return
    const timer = setTimeout(() => {
      this.agentWakeupTimers.delete(sessionId)
      void this.runAgentWakeup(sessionId).catch(() => {})
    }, 500)
    timer.unref?.()
    this.agentWakeupTimers.set(sessionId, timer)
  }

  async runAgentWakeup(sessionId) {
    const value = this.sessions.get(sessionId)
    if (!value?.session) return
    const pending = (value.pendingAgentNotifications || []).splice(0)
    if (!pending.length) return
    const message = pending.join('\n\n')
    // 防抖窗口内会话恰好开始运行：改为 steer 注入，避免另起一轮
    if (value.session.isStreaming) {
      void value.session.steer(message).catch(() => {})
      return
    }
    // 服务端自动唤醒：以内部消息发起一轮，让大模型立即感知完成结果。
    // 消息以 AGENT_COMPLETION_MARKER 开头，对用户隐藏；产生的助手回复正常显示。
    await this.streamPrompt({ sessionId, message, send: () => {} })
  }

  async streamPrompt({ sessionId, message, attachments = [], requestedToolNames = [], goalMode = false, goalTokenBudget = null, isolatedContext = false, send }) {
    const emit = (event, data) => {
      send(event, data)
      try {
        this.eventObserver?.({ event, data, sessionId: data?.sessionId || sessionId || '' })
      } catch {
        // Desktop observers are best-effort and must never interrupt an Agent stream.
      }
    }
    const value = await this.getOrCreateSession(sessionId)
    if (isolatedContext) {
      value.isolatedContext = true
      value.blockedToolNames = ISOLATED_CONTEXT_BLOCKED_TOOLS
    }
    const { session } = value
    const appConfig = await readJson(this.appConfigPath, { toolMode: 'full', disabledProviders: [] })
    if ((appConfig.disabledProviders || []).includes(session.model?.provider)) {
      throw new Error('当前会话使用的 Provider 已停用，请先启用或切换模型。')
    }
    if (!session.model || session.model.provider === 'unknown' || session.model.id === 'unknown') {
      throw new Error('没有可用模型，请先在配置页设置 Provider、模型和 API Key。')
    }
    if (session.isStreaming) throw new Error('当前会话仍在运行，请等待完成或先停止。')
    let goal = this.goals.get(session.sessionId)
    if (goalMode) {
      if (goal?.status === 'paused') {
        if (goalTokenBudget != null) await this.goals.setBudget(session.sessionId, goalTokenBudget)
        goal = await this.goals.resume(session.sessionId)
      } else {
        goal = await this.goals.start(session.sessionId, { objective: message, tokenBudget: goalTokenBudget ?? undefined })
      }
    }
    await this.selectToolsForMessage(value, message, { requestedToolNames })
    value.pendingUserMessage = String(message || '')
    // Drop stale plans from previous turns unless a Goal is actively driving multi-turn work or this is an internal wakeup turn.
    const keepPlan = goal?.status === 'active' || isGoalContinuationMessage(message) || isAgentCompletionMessage(message)
    if (!keepPlan) await this.plans.replace(session.sessionId, [])
    
    // 注入待处理的 Agent 完成通知（对用户隐藏）
    const pendingAgentNotes = value.pendingAgentNotifications || []
    value.pendingAgentNotifications = []
    if (pendingAgentNotes.length) {
      for (const note of pendingAgentNotes) {
        try {
          await session.steer(note)
        } catch {
          // 注入失败时忽略，不影响主消息流
        }
      }
    }
    
    const startedAt = new Date().toISOString()
    value.modified = startedAt
    const initialActivity = { type: 'model', stage: 'thinking', updatedAt: startedAt }
    const live = { streaming: true, text: '', thinkingText: '', tools: [], assets: [], error: '', goal, plan: this.plans.get(session.sessionId), agents: this.multiAgents.summaries(session.sessionId).filter((agent) => ['queued', 'starting', 'running'].includes(agent.status)), currentActivity: initialActivity, activityFeed: [], queuedInputs: queuedSessionInputs(session), contextUsage: this.compactionAwareContextUsage(session), compaction: null, startedAt, lastActivityAt: startedAt }
    this.liveSessions.set(session.sessionId, live)
    this.goalEmitters.set(session.sessionId, emit)
    this.planEmitters.set(session.sessionId, emit)
    this.agentEmitters.set(session.sessionId, emit)

    const firstTurn = !session.messages.some((item) => item.role === 'user')
    const sessionMeta = this.sessionMeta[session.sessionId]
    const mayAutoTitle = firstTurn && !sessionMeta?.manual
    const temporaryTitle = mayAutoTitle ? temporarySessionTitle(message, attachments) : ''
    if (temporaryTitle && temporaryTitle !== value.name) {
      session.setSessionName(temporaryTitle)
      value.name = temporaryTitle
      await this.markSessionTitle(session.sessionId, temporaryTitle, false)
      emit('session_title', { sessionId: session.sessionId, name: temporaryTitle, source: 'temporary' })
    }

    emit('meta', {
      sessionId: session.sessionId,
      model: `${session.model.provider}/${session.model.id}`,
      thinkingLevel: session.thinkingLevel,
      cwd: value.cwd,
      permissionMode: this.sessionMeta[session.sessionId]?.permissionMode || permissionModeForExecutionMode(this.getSessionExecutionMode(session.sessionId)),
      executionMode: this.getSessionExecutionMode(session.sessionId),
      goal,
      plan: live.plan,
      agents: live.agents,
      currentActivity: live.currentActivity,
      activityFeed: live.activityFeed,
      thinkingText: live.thinkingText,
      queuedInputs: live.queuedInputs,
      contextUsage: live.contextUsage,
      startedAt: live.startedAt,
      lastActivityAt: live.lastActivityAt,
    })

    let goalTurnId = ''
    let goalTurnStartedAt = 0
    let continuationQueued = false
    let budgetSummaryQueued = false
    let thinkingPrefix = ''
    let thinkingTurnText = ''
    const activeTextBlocks = new Set()
    const activeThinkingBlocks = new Set()
    const streamBlockIndex = (update) => Number.isInteger(update?.contentIndex) ? update.contentIndex : 0
    const appendThinking = (delta) => {
      thinkingTurnText += String(delta || '')
      const next = liveThinkingTail([thinkingPrefix, thinkingTurnText].filter(Boolean).join('\n\n'))
      let start = 0
      const limit = Math.min(live.thinkingText.length, next.length)
      while (start < limit && live.thinkingText.charCodeAt(start) === next.charCodeAt(start)) start += 1
      live.thinkingText = next
      emit('thinking_patch', { start, text: next.slice(start) })
    }
    const finishLiveRun = (error = '') => {
      const finishedAt = new Date().toISOString()
      live.streaming = false
      live.finishedAt = finishedAt
      live.lastActivityAt = finishedAt
      live.tools = live.tools.map((tool) => tool.status === 'running'
        ? { ...tool, status: error ? 'error' : 'done', message: error || tool.message || '', updatedAt: finishedAt, finishedAt }
        : tool)
      const backgroundAgents = this.multiAgents.summaries(session.sessionId)
        .filter((agent) => ['queued', 'starting', 'running'].includes(agent.status))
      const backgroundActivities = backgroundAgents.map((agent) => ({
        type: 'agent',
        agent,
        updatedAt: agent.lastActivityAt || finishedAt,
      }))
      live.agents = backgroundAgents
      live.currentActivity = backgroundActivities.at(-1) || null
      live.activityFeed = backgroundActivities.slice(-MAX_LIVE_ACTIVITY_ITEMS)
      return finishedAt
    }
    const unsubscribe = session.subscribe((event) => {
      live.lastActivityAt = new Date().toISOString()
      if (event.type === 'message_update') {
        const update = event.assistantMessageEvent
        const blockIndex = streamBlockIndex(update)
        if (update.type === 'text_start') activeTextBlocks.add(blockIndex)
        if (update.type === 'text_delta') {
          activeTextBlocks.add(blockIndex)
          const delta = String(update.delta || '')
          live.text += delta
          setLiveActivity(live, { type: 'model', stage: 'responding', updatedAt: live.lastActivityAt })
          if (delta) emit('text_delta', { delta })
        }
        if (update.type === 'text_end') {
          activeTextBlocks.delete(blockIndex)
          if (!activeTextBlocks.size) {
            emit('text_end', { text: live.text, updatedAt: live.lastActivityAt })
          }
        }
        if (update.type === 'thinking_start') activeThinkingBlocks.add(blockIndex)
        if (update.type === 'thinking_delta') {
          activeThinkingBlocks.add(blockIndex)
          appendThinking(update.delta)
          setLiveActivity(live, { type: 'model', stage: 'thinking', updatedAt: live.lastActivityAt })
        }
        if (update.type === 'thinking_end') {
          activeThinkingBlocks.delete(blockIndex)
        }
      } else if (event.type === 'compaction_start') {
        live.compaction = startedCompaction(event.reason, live.lastActivityAt)
        setLiveActivity(live, { type: 'compaction', compaction: live.compaction, updatedAt: live.lastActivityAt })
        emit('compaction_start', live.compaction)
      } else if (event.type === 'compaction_end') {
        live.compaction = finishedCompaction(live.compaction, event, live.lastActivityAt)
        live.contextUsage = this.compactionAwareContextUsage(session, live.compaction)
        emit('compaction_end', live.compaction)
        emit('context_usage', live.contextUsage)
      } else if (event.type === 'message_end') {
        if (event.message?.role === 'assistant') {
          activeTextBlocks.clear()
          activeThinkingBlocks.clear()
          const finalMessage = ['stop', 'length'].includes(event.message.stopReason)
          if (finalMessage) {
            const completedText = textFromContent(event.message.content)
            if (completedText) live.text = completedText
          }
          emit('text_end', {
            text: live.text,
            final: finalMessage,
            updatedAt: live.lastActivityAt,
          })
        }
        live.contextUsage = this.compactionAwareContextUsage(session, live.compaction)
        emit('context_usage', live.contextUsage)
      } else if (event.type === 'queue_update') {
        live.queuedInputs = [
          ...(event.steering || []).filter((text) => !isInternalParentMessage(text)).map((text) => ({ behavior: 'steer', text })),
          ...(event.followUp || []).filter((text) => !isInternalParentMessage(text)).map((text) => ({ behavior: 'followUp', text })),
        ]
        emit('queue_update', { queuedInputs: live.queuedInputs })
      } else if (event.type === 'tool_execution_start') {
        activeTextBlocks.clear()
        activeThinkingBlocks.clear()
        const toolStartedAt = live.lastActivityAt
        const tool = { type: 'tool', id: event.toolCallId, name: event.toolName, args: event.args, status: 'running', startedAt: toolStartedAt, updatedAt: toolStartedAt, ...(event.toolName === 'bash' ? { output: '' } : {}) }
        live.tools.push(tool)
        setLiveActivity(live, tool)
        emit('tool_start', { id: event.toolCallId, name: event.toolName, args: event.args, startedAt: toolStartedAt, ...(event.toolName === 'bash' ? { output: '' } : {}) })
      } else if (event.type === 'tool_execution_update') {
        const rawOutput = textFromContent(event.partialResult?.content)
        const message = rawOutput.replace(/\s+/g, ' ').trim().slice(0, 180)
        const outputPatch = event.toolName === 'bash' ? { output: rawOutput } : {}
        const agent = multiAgentResultAgent(event.toolName, event.partialResult?.details)
        live.tools = live.tools.map((item) => item.id === event.toolCallId
          ? { ...item, ...outputPatch, message: message || item.message || '', updatedAt: live.lastActivityAt, ...(agent ? { agent } : {}) }
          : item)
        if (live.currentActivity?.id === event.toolCallId) {
          setLiveActivity(live, { ...live.currentActivity, ...outputPatch, message: message || live.currentActivity.message || '', updatedAt: live.lastActivityAt, ...(agent ? { agent } : {}) })
        }
        emit('tool_update', { id: event.toolCallId, name: event.toolName, message, ...outputPatch, updatedAt: live.lastActivityAt, ...(agent ? { agent } : {}) })
      } else if (event.type === 'tool_execution_end') {
        if (!event.isError && ['generate_visual', 'browser_automation'].includes(event.toolName) && event.result?.details?.path) {
          const generatedPath = resolve(event.result.details.path)
          const asset = this.assetIndex.assets.find((item) => item.filePath && resolve(item.filePath) === generatedPath)
          if (asset) {
            const attachment = assetMessageAttachment(asset)
            live.assets = [...live.assets.filter((item) => item.id !== attachment.id), attachment]
            emit('generated_asset', attachment)
          }
        }
        const resultOutput = event.toolName === 'bash' ? textFromContent(event.result?.content) : ''
        const resultMessage = event.isError ? resultOutput || textFromContent(event.result?.content) || '工具执行失败。' : ''
        const completedTool = live.tools.find((item) => item.id === event.toolCallId)
        const outputPatch = event.toolName === 'bash' ? { output: resultOutput || completedTool?.output || '' } : {}
        const resultDetails = event.result?.details
        const resultAgent = multiAgentResultAgent(event.toolName, resultDetails)
        const toolFinishedAt = live.lastActivityAt
        live.tools = live.tools.map((item) => item.id === event.toolCallId ? { ...item, ...outputPatch, status: event.isError ? 'error' : 'done', message: resultMessage || item.message || '', updatedAt: toolFinishedAt, finishedAt: toolFinishedAt } : item)
        const finishedActivity = event.isError
          ? { ...(completedTool || {}), ...outputPatch, type: 'tool', status: 'error', message: resultMessage || completedTool?.message || '', updatedAt: toolFinishedAt, finishedAt: toolFinishedAt }
          : resultAgent
            ? { type: 'agent', agent: resultAgent, updatedAt: resultAgent.lastActivityAt || toolFinishedAt }
            : { ...(completedTool || {}), ...outputPatch, type: 'tool', status: 'done', message: completedTool?.message || '', updatedAt: toolFinishedAt, finishedAt: toolFinishedAt }
        const preserveEvent = PLAN_ALL_TOOL_NAMES.includes(event.toolName) && live.currentActivity?.type === 'plan'
        if (event.isError || !preserveEvent) live.activityFeed = pushLiveActivity(live.activityFeed, finishedActivity)
        if (live.currentActivity?.id === event.toolCallId) live.currentActivity = finishedActivity
        emit('tool_end', {
          id: event.toolCallId,
          name: event.toolName,
          error: event.isError,
          message: resultMessage || completedTool?.message || '',
          ...outputPatch,
          finishedAt: toolFinishedAt,
          ...(resultAgent ? { agent: resultAgent } : {}),
        })
      } else if (event.type === 'turn_start') {
        activeThinkingBlocks.clear()
        thinkingPrefix = live.thinkingText
        thinkingTurnText = ''
        live.thinkingText = thinkingPrefix
        setLiveActivity(live, { type: 'model', stage: 'thinking', updatedAt: live.lastActivityAt })
        emit('thinking_reset', { thinkingText: thinkingPrefix, updatedAt: live.lastActivityAt })
        const activeGoal = this.goals.get(session.sessionId)
        goalTurnId = activeGoal?.status === 'active' ? activeGoal.id : ''
        goalTurnStartedAt = Date.now()
      } else if (event.type === 'turn_end') {
        if (!goalTurnId) return
        const accounting = this.goals.account(session.sessionId, {
          goalId: goalTurnId,
          usage: event.message?.usage,
          elapsedSeconds: goalTurnStartedAt ? (Date.now() - goalTurnStartedAt) / 1000 : 0,
        })
        goalTurnId = ''
        goalTurnStartedAt = 0
        const updatedGoal = this.goals.get(session.sessionId)
        this.syncGoalTools(value, updatedGoal)
        this.emitGoalUpdate(session.sessionId, updatedGoal)
        if (updatedGoal?.status === 'budget_limited' && !budgetSummaryQueued) {
          budgetSummaryQueued = true
          void session.followUp(goalBudgetPrompt(updatedGoal)).catch(() => {})
        }
        void accounting.catch(() => {})
      } else if (event.type === 'agent_end') {
        if (event.willRetry) return
        const finalAssistant = [...(event.messages || [])].reverse().find((item) => item?.role === 'assistant')
        if (finalAssistant?.stopReason === 'error' || finalAssistant?.stopReason === 'aborted' || finalAssistant?.errorMessage) return
        const activeGoal = this.goals.get(session.sessionId)
        if (activeGoal?.status !== 'active' || continuationQueued) return
        continuationQueued = true
        void session.followUp(goalContinuationPrompt(activeGoal)).catch(() => {}).finally(() => { continuationQueued = false })
      } else if (event.type === 'auto_retry_start') {
        emit('retry', { attempt: event.attempt, maxAttempts: event.maxAttempts, message: event.errorMessage })
      }
    })

    this.permissions.attachEmitter(session.sessionId, emit)
    try {
      const safeAttachments = Array.isArray(attachments) ? attachments.slice(0, 8) : []
      const archivedAttachments = await this.archiveAttachments(session.sessionId, value.name, safeAttachments)
      const images = []
      const contexts = []
      const sharedContextEnabled = !value.isolatedContext
      const memoryContext = sharedContextEnabled && value.enabledTools?.includes('memory_search')
        ? await this.memory.relevantContext(message, value.cwd)
        : { text: '', memories: [] }
      if (memoryContext.text) contexts.push(memoryContext.text)
      const activeGoal = sharedContextEnabled ? this.goals.get(session.sessionId) : null
      if (activeGoal?.status === 'active') contexts.push(goalContinuationPrompt(activeGoal))
      for (const [attachmentIndex, attachment] of safeAttachments.entries()) {
        const name = safeAttachmentName(attachment.name)
        if (attachment.kind === 'image') {
          const data = String(attachment.data || '')
          const mimeType = String(attachment.mimeType || '')
          if (!mimeType.startsWith('image/') || !data) throw new Error(`${name} 不是有效图片`)
          if (data.length > 15_000_000) throw new Error(`${name} 图片数据过大`)
          images.push({ type: 'image', data, mimeType })
          const localPath = archivedAttachments[attachmentIndex]?.path
          contexts.push(`[Image attachment] ${name}${localPath ? `\nLocal path: ${localPath}\nTo edit this image, pass this path in generate_visual sourceImages.` : ''}`)
        } else if (attachment.kind === 'text') {
          const text = String(attachment.text || '').slice(0, MAX_EXTRACTED_CHARS)
          contexts.push(`[Text attachment: ${name}]\n${text}${attachment.truncated ? '\n(Content truncated)' : ''}`)
        } else if (attachment.kind === 'document') {
          const text = await extractDocumentText(attachment)
          contexts.push(`[Document attachment: ${name}]\n${text}`)
        }
      }
      const prompt = contexts.length ? `${message}${ATTACHMENT_MARKER}${contexts.join('\n\n')}` : message
      const titlePromise = mayAutoTitle
        ? this.generateSessionTitle(session.model, message, safeAttachments, temporaryTitle, session.sessionId).catch(() => '')
        : null
      applyPisperSystemPrompt(session, session.model)
      await session.prompt(prompt, { images })
      const last = [...session.messages].reverse().find((item) => item.role === 'assistant')
      if (last?.errorMessage) throw new Error(last.errorMessage)
      const assistantText = textFromContent(last?.content)
      live.text = assistantText || live.text
      // Resolve auto-title before done so clients that stop the SSE stream on `done` still receive it.
      if (titlePromise) {
        try {
          const generatedTitle = await titlePromise
          if (generatedTitle && !this.sessionMeta[session.sessionId]?.manual && generatedTitle !== value.name) {
            session.setSessionName(generatedTitle)
            value.name = generatedTitle
            await this.markSessionTitle(session.sessionId, generatedTitle, false)
            emit('session_title', { sessionId: session.sessionId, name: generatedTitle, source: 'generated' })
          }
        } catch {}
      }
      const finishedAt = finishLiveRun()
      live.contextUsage = this.compactionAwareContextUsage(session, live.compaction)
      emit('done', {
        sessionId: session.sessionId,
        text: live.text,
        tools: live.tools,
        assets: live.assets,
        approvals: [],
        goal: this.goals.get(session.sessionId),
        plan: this.plans.get(session.sessionId),
        agents: live.agents,
        currentActivity: live.currentActivity,
        activityFeed: live.activityFeed,
        queuedInputs: live.queuedInputs,
        contextUsage: live.contextUsage,
        compaction: live.compaction,
        startedAt: live.startedAt,
        finishedAt,
      })
      if (!value.isolatedContext && value.enabledTools?.includes('memory_remember')) {
        void this.captureConversationMemory({
          sessionId: session.sessionId,
          cwd: value.cwd,
          model: session.model,
          user: message,
          assistant: assistantText,
          sourceTimestamp: live.startedAt,
        }).catch(() => {})
      }
    } catch (error) {
      session.clearQueue?.()
      live.queuedInputs = []
      live.error = error instanceof Error ? error.message : String(error)
      const finishedAt = live.streaming ? finishLiveRun(live.error) : live.finishedAt
      live.contextUsage = this.compactionAwareContextUsage(session, live.compaction)
      if (this.goals.get(session.sessionId)?.status === 'active') await this.pauseSessionGoal(session.sessionId)
      emit('error', {
        sessionId: session.sessionId,
        message: live.error,
        text: live.text,
        tools: live.tools,
        assets: live.assets,
        approvals: [],
        goal: this.goals.get(session.sessionId),
        plan: this.plans.get(session.sessionId),
        agents: live.agents,
        currentActivity: live.currentActivity,
        activityFeed: live.activityFeed,
        queuedInputs: live.queuedInputs,
        contextUsage: live.contextUsage,
        compaction: live.compaction,
        startedAt: live.startedAt,
        finishedAt,
      })
      // Terminal snapshot already delivered over SSE; avoid a second bare error event from the HTTP handler.
      return
    } finally {
      unsubscribe()
      this.permissions.detachEmitter(session.sessionId, emit)
      if (this.goalEmitters.get(session.sessionId) === emit) this.goalEmitters.delete(session.sessionId)
      if (this.planEmitters.get(session.sessionId) === emit) this.planEmitters.delete(session.sessionId)
      if (this.agentEmitters.get(session.sessionId) === emit) this.agentEmitters.delete(session.sessionId)
      if (live.streaming) finishLiveRun(live.error)
      this.touchSessionRuntime(value)
      this.evictIdleSessionRuntimes(session.sessionId)
      const timer = setTimeout(() => { if (this.liveSessions.get(session.sessionId) === live) this.liveSessions.delete(session.sessionId) }, 60_000)
      timer.unref?.()
    }
  }

  async generateSessionTitle(model, message, attachments, fallback, sessionId) {
    const attachmentText = attachments.length
      ? `\n附件：${attachments.map((item) => safeAttachmentName(item.name)).join('、')}`
      : ''
    const result = await this.modelRuntime.completeSimple(model, {
      systemPrompt: 'You generate clear, specific session titles from the user task. Output only a Simplified Chinese title with no quotes, punctuation, explanation, or title prefix. Use at most 20 Chinese characters while preserving necessary filenames, technical terms, and error names.',
      messages: [{
        role: 'user',
        content: `${String(message || '').slice(0, 1200)}${attachmentText}`,
        timestamp: Date.now(),
      }],
    }, {
      ...(model.reasoning ? { reasoning: 'low' } : { temperature: 0.2 }),
      maxTokens: 128,
    })
    if (sessionId) await this.recordUsage(localDayKey(result.timestamp || Date.now()), `title:${sessionId}`, result.usage)
    if (result.errorMessage) return fallback
    return cleanSessionTitle(textFromContent(result.content)) || fallback
  }

  async captureConversationMemory({ sessionId, cwd, model, user, assistant, sourceTimestamp = '' }) {
    const result = await extractConversationMemories({ modelRuntime: this.modelRuntime, model, user, assistant })
    if (result.usage) await this.recordUsage(localDayKey(result.timestamp || Date.now()), `memory:${sessionId}:${result.timestamp || Date.now()}`, result.usage)
    if (!result.memories.length) return []
    const projectSpaceId = await this.memory.ensureWorkspaceSpace(cwd)
    return result.memories.map((item, index) => this.memory.propose({
      ...item,
      spaceId: item.scope === 'global' ? 'global' : projectSpaceId,
      cwd,
      sessionId,
      sourceId: `${sessionId}:${sourceTimestamp || result.timestamp || Date.now()}:${index}`,
      sourceTimestamp: sourceTimestamp || new Date(result.timestamp || Date.now()).toISOString(),
      sourceType: 'conversation',
    }))
  }

  resolveDefaultModel() {
    const settings = this.settingsManager?.getGlobalSettings?.() || {}
    const provider = settings.defaultProvider
    const modelId = settings.defaultModel
    if (!provider || !modelId || !this.modelRuntime?.getModel) return null
    return this.modelRuntime.getModel(String(provider), String(modelId)) || null
  }

  getMemoryDashboard(input) {
    return this.memory.getDashboard(input)
  }

  getMemoryCandidateInbox(input) {
    return this.memory.candidateInbox(input)
  }

  createMemorySpace(input) {
    return this.memory.createSpace(input)
  }

  updateMemorySpace(id, input) {
    return this.memory.updateSpace(id, input)
  }

  deleteMemorySpace(id) {
    return this.memory.deleteSpace(id)
  }

  createMemory(input) {
    return this.memory.remember({ ...input, sourceType: 'manual' })
  }

  updateMemory(id, input) {
    return this.memory.updateMemory(id, input)
  }

  deleteMemory(id) {
    return this.memory.forget(id)
  }

  acceptMemoryCandidate(id) {
    return this.memory.acceptCandidate(id)
  }

  rejectMemoryCandidate(id) {
    return this.memory.rejectCandidate(id)
  }

  rejectAllMemoryCandidates() {
    return this.memory.rejectAllCandidates()
  }

  async abortSession(id) {
    const wakeupTimer = this.agentWakeupTimers.get(id)
    if (wakeupTimer) {
      clearTimeout(wakeupTimer)
      this.agentWakeupTimers.delete(id)
    }
    const value = this.sessions.get(id)
    if (!value) return false
    await this.pauseSessionGoal(id)
    this.multiAgents.abortParent(id)
    this.permissions.resolveSession(id, false, '会话已停止，工具未执行。')
    value.session.clearQueue?.()
    await value.session.abort()
    return true
  }

  async deleteSession(id) {
    await this.goals.remove(id)
    await this.plans.remove(id)
    await this.browserAutomation.closeSession(id)
    await this.multiAgents.removeParent(id)
    this.permissions.resolveSession(id, false, '会话已删除，工具未执行。')
    const active = this.sessions.get(id)
    const pending = this.pendingSessions.get(id)
    this.pendingSessions.delete(id)
    let sessionFile = active?.session.sessionFile
    if (active) {
      if (active.session.isStreaming) await active.session.abort()
      active.session.dispose()
      this.sessions.delete(id)
    }
    if (!sessionFile) sessionFile = (await this.findSessionInfo(id))?.path
    this.sessionHistoryPaths.delete(id)
    this.sessionContextUsageCache.delete(id)
    if (sessionFile) this.sessionHistoryCache.delete(sessionFile)
    if (!sessionFile) {
      if (this.sessionMeta[id]) {
        delete this.sessionMeta[id]
        await this.saveSessionMeta()
      }
      if (this.usageLedger.sessionScans?.[id]) {
        delete this.usageLedger.sessionScans[id]
        await this.saveUsageLedger()
      }
      if (this.storedSessionsCache) this.storedSessionsCache = this.storedSessionsCache.filter((session) => session.id !== id)
      return Boolean(active || pending)
    }
    const root = resolve(this.sessionDir)
    const target = resolve(sessionFile)
    if (target !== root && !target.startsWith(`${root}${sep}`)) throw new Error('拒绝删除会话目录之外的文件。')
    try {
      await unlink(target)
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
    if (this.sessionMeta[id]) {
      delete this.sessionMeta[id]
      await this.saveSessionMeta()
    }
    if (this.usageLedger.sessionScans?.[id]) {
      delete this.usageLedger.sessionScans[id]
      await this.saveUsageLedger()
    }
    if (this.storedSessionsCache) this.storedSessionsCache = this.storedSessionsCache.filter((session) => session.id !== id)
    return true
  }

  async getPlugins() {
    return this.toolPlugins.getState()
  }

  testWebSearch(input) {
    return this.webSearch.test(input)
  }

  async promptFromChannel({ sessionId, message, attachments = [], cwd, title, model, executionMode, isolatedContext = false, onSession }) {
    let id = String(sessionId || '')
    if (id && !this.sessions.has(id) && !this.pendingSessions.has(id) && !(await this.findSessionInfo(id))) id = ''
    if (!id) {
      const created = await this.createSession(title || '飞书会话')
      id = created.id
      if (cwd) await this.setSessionCwd(id, cwd)
    }
    if (isolatedContext) {
      const active = await this.getOrCreateSession(id)
      active.isolatedContext = true
      active.blockedToolNames = ISOLATED_CONTEXT_BLOCKED_TOOLS
    }
    if (executionMode) await this.setSessionExecutionMode(id, executionMode)
    if (model?.provider && model?.model) {
      const active = await this.getOrCreateSession(id)
      if (active.session.model?.provider !== model.provider || active.session.model?.id !== model.model) await this.setSessionModel(id, model.provider, model.model)
    }
    onSession?.(id)
    let actualId = id
    let text = ''
    const assetIds = new Set()
    await this.streamPrompt({
      sessionId: id,
      message,
      attachments,
      isolatedContext,
      send: (event, data) => {
        if ((event === 'meta' || event === 'done') && data?.sessionId) actualId = data.sessionId
        if (event === 'text_delta') text += data?.delta || ''
        if (event === 'generated_asset' && data?.id) assetIds.add(data.id)
      },
    })
    if (!text.trim()) {
      const messages = await this.getSessionMessages(actualId)
      text = [...messages].reverse().find((item) => item.role === 'agent')?.text || ''
    }
    const runtime = this.sessions.get(actualId)
    const assets = [...assetIds].map((assetId) => this.assetIndex.assets.find((asset) => asset.id === assetId)).filter(Boolean).map((asset) => ({
      id: asset.id,
      name: asset.name,
      path: asset.filePath,
      mimeType: asset.mimeType,
    })).filter((asset) => asset.path)
    return { sessionId: actualId, text: text.trim(), cwd: runtime?.cwd || this.sessionMeta[actualId]?.cwd || this.cwd, model: runtime?.session.model ? `${runtime.session.model.provider}/${runtime.session.model.id}` : '', assets }
  }

  async getChannels() {
    const state = this.channels.getState()
    const config = await this.getConfig()
    return {
      providers: state.providers,
      connections: state.connections,
      scopes: state.scopes,
      models: config.providers.filter((provider) => provider.type !== 'visual' && provider.enabled && provider.configured).flatMap((provider) => provider.models.filter((model) => model.kind === 'chat').map((model) => ({ provider: provider.id, model: model.id, label: `${provider.name} / ${model.name}` }))),
    }
  }

  startChannelOnboarding(platform) {
    return this.channels.startOnboarding(platform)
  }

  getChannelOnboarding(platform, id) {
    return this.channels.getOnboarding(platform, id)
  }

  cancelChannelOnboarding(platform, id) {
    return this.channels.cancelOnboarding(platform, id)
  }

  verifyChannelOnboarding(platform, id, code) {
    return this.channels.verifyOnboarding(platform, id, code)
  }

  async updateChannel(platform, input) {
    await this.channels.update(platform, input)
    return this.getChannels()
  }

  async reconnectChannel(platform) {
    await this.channels.connect(platform)
    return this.getChannels()
  }

  deleteChannel(platform) {
    return this.channels.remove(platform)
  }

  resetChannelScope(key) {
    return this.channels.resetScope(key)
  }

  getCompactionPreference() {
    return {
      thresholdPercent: this.compactionThresholdPercent,
      minPercent: MIN_COMPACTION_THRESHOLD_PERCENT,
      maxPercent: MAX_COMPACTION_THRESHOLD_PERCENT,
    }
  }

  async updateCompactionPreference(input) {
    const requested = Number(input?.thresholdPercent)
    if (!Number.isFinite(requested)
      || requested < MIN_COMPACTION_THRESHOLD_PERCENT
      || requested > MAX_COMPACTION_THRESHOLD_PERCENT) {
      throw new Error(`自动压缩阈值必须在 ${MIN_COMPACTION_THRESHOLD_PERCENT}% 到 ${MAX_COMPACTION_THRESHOLD_PERCENT}% 之间。`)
    }
    const thresholdPercent = normalizeCompactionThresholdPercent(requested)
    const appConfig = await readJson(this.appConfigPath, {})
    await writeJsonAtomic(this.appConfigPath, {
      ...appConfig,
      compactionThresholdPercent: thresholdPercent,
    })
    this.compactionThresholdPercent = thresholdPercent
    this.sessionContextUsageCache.clear()
    return this.getCompactionPreference()
  }

  getNotificationSettings() {
    return this.notificationSettings.getState()
  }

  updateBrowserNotifications(input) {
    return this.notificationSettings.updateBrowser(input)
  }

  saveNotificationTemplate(event, platform, input) {
    return this.notificationSettings.updateTemplate(event, platform, input)
  }

  testNotificationTemplate(event, platform) {
    return this.notificationSettings.testTemplate(event, platform)
  }

  getBrowserNotificationEvents(after) {
    return this.notificationSettings.getBrowserEvents(after)
  }

  async getSchedules() {
    const config = await this.getConfig()
    const notificationSettings = await this.notificationSettings.getState()
    return {
      ...this.schedules.getState(),
      defaultCwd: this.cwd,
      models: config.providers.filter((provider) => provider.type !== 'visual' && provider.enabled && provider.configured).flatMap((provider) => provider.models.filter((model) => model.kind === 'chat').map((model) => ({ provider: provider.id, model: model.id, label: `${provider.name} / ${model.name}` }))),
      notificationTargets: {
        browser: { enabled: notificationSettings.browser.enabled },
        feishu: { enabled: Boolean(notificationSettings.connections.feishu?.enabled) },
        weixin: { enabled: Boolean(notificationSettings.connections.weixin?.enabled) },
      },
    }
  }

  async createSchedule(input) {
    const task = await this.schedules.create(input)
    return { task, state: await this.getSchedules() }
  }

  async updateSchedule(id, input) {
    const task = await this.schedules.update(id, input)
    return task ? { task, state: await this.getSchedules() } : null
  }

  deleteSchedule(id) {
    return this.schedules.remove(id)
  }

  async runSchedule(id) {
    const task = await this.schedules.runNow(id)
    return task ? { started: true, task } : null
  }

  async getWorkflows() {
    const config = await this.getConfig()
    const notificationSettings = await this.notificationSettings.getState()
    return {
      ...this.workflows.getState(),
      cwd: this.cwd,
      models: config.providers.filter((provider) => provider.type !== 'visual' && provider.enabled && provider.configured).flatMap((provider) => provider.models.filter((model) => model.kind === 'chat').map((model) => ({ provider: provider.id, model: model.id, label: `${provider.name} / ${model.name}` }))),
      notificationTargets: {
        browser: { enabled: notificationSettings.browser.enabled },
        feishu: { enabled: Boolean(notificationSettings.connections.feishu?.enabled) },
        weixin: { enabled: Boolean(notificationSettings.connections.weixin?.enabled) },
      },
    }
  }

  async createWorkflow(input) {
    const workflow = await this.workflows.create(input)
    return { workflow, state: await this.getWorkflows() }
  }

  async updateWorkflow(id, input) {
    const workflow = await this.workflows.update(id, input)
    return workflow ? { workflow, state: await this.getWorkflows() } : null
  }

  deleteWorkflow(id) {
    return this.workflows.remove(id)
  }

  async runWorkflow(id) {
    const run = await this.workflows.runNow(id)
    return run ? { started: true, run } : null
  }

  async stopWorkflowRun(id) {
    const run = await this.workflows.stop(id)
    return run ? { stopping: true, run } : null
  }

  notifyChannels(event, data) {
    return this.notificationSettings.notify(event, data)
  }

  async dispose() {
    if (this.sessionRuntimeSweepTimer) clearInterval(this.sessionRuntimeSweepTimer)
    this.sessionRuntimeSweepTimer = null
    for (const timer of this.agentWakeupTimers.values()) clearTimeout(timer)
    this.agentWakeupTimers.clear()
    this.providerModelDiscovery.abort?.()
    await this.providerModelRefreshPromise?.catch(() => {})
    await this.workflows.dispose()
    await this.schedules.dispose()
    await this.channels.dispose()
    await this.goals.pauseAllActive()
    await this.multiAgents.dispose()
    await this.browserAutomation.dispose()
    this.permissions.dispose()
    await this.disposeSessions()
    this.pendingSessions.clear()
    await this.mcp.dispose()
    this.memory.dispose()
    await Promise.allSettled([this.sessionMetaWrite, this.usageWrite, this.assetWrite])
  }

  async savePlugins(input) {
    const result = await this.toolPlugins.saveState(input)
    this.invalidateSessionRuntimes()
    return result
  }

  getMcpDashboard({ refresh = true } = {}) {
    return this.mcp.getDashboard({ refresh })
  }

  async createMcpServer(input) {
    const result = await this.mcp.add(input)
    this.invalidateSessionRuntimes()
    return result
  }

  async updateMcpServer(id, input) {
    const result = await this.mcp.update(id, input)
    if (result) this.invalidateSessionRuntimes()
    return result
  }

  async deleteMcpServer(id) {
    const deleted = await this.mcp.remove(id)
    if (deleted) this.invalidateSessionRuntimes()
    return deleted
  }

  async testMcpServer(id) {
    return this.mcp.test(id)
  }

  async setMcpToolEnabled(id, toolName, enabled) {
    const result = await this.mcp.setToolEnabled(id, toolName, enabled)
    if (result) this.invalidateSessionRuntimes()
    return result
  }

  async getSkillsDashboard(sessionId = '') {
    return this.skills.dashboard({ cwd: await this.sessionWorkspaceCwd(sessionId) })
  }

  async installSkill(input, sessionId = '') {
    const result = await this.skills.install(input, { cwd: await this.sessionWorkspaceCwd(sessionId) })
    this.invalidateSessionRuntimes()
    return result
  }

  async updateSkill(id, input, sessionId = '') {
    const result = await this.skills.update(id, input, { cwd: await this.sessionWorkspaceCwd(sessionId) })
    if (result) this.invalidateSessionRuntimes()
    return result
  }

  async deleteSkill(id, sessionId = '') {
    const deleted = await this.skills.remove(id, { cwd: await this.sessionWorkspaceCwd(sessionId) })
    if (deleted) this.invalidateSessionRuntimes()
    return deleted
  }

  async reloadSkills(sessionId = '') {
    this.invalidateSessionRuntimes()
    this.skills.invalidateDashboardCache()
    return this.skills.dashboard({ cwd: await this.sessionWorkspaceCwd(sessionId), force: true })
  }

  async getProviderDiscovery() {
    const [discovery, credentials, modelsJson, appConfig] = await Promise.all([
      this.providerDiscovery.discover(),
      readJson(this.authPath, {}),
      readJson(this.modelsPath, { providers: {} }),
      readJson(this.appConfigPath, { providerImports: {} }),
    ])
    return {
      ...discovery,
      providers: discovery.providers.map((provider) => {
        const imported = Boolean(modelsJson.providers?.[provider.providerId]) && appConfig.providerImports?.[provider.id]?.fingerprint === provider.fingerprint
        return {
          ...provider,
          configured: Boolean(credentials[provider.providerId]) || this.modelRuntime.hasConfiguredAuth(provider.providerId),
          imported,
          conflict: Boolean(modelsJson.providers?.[provider.providerId]) && !imported,
        }
      }),
    }
  }

  async importDiscoveredProvider(discoveryId) {
    const loaded = await this.providerDiscovery.loadConfiguration(String(discoveryId || '').trim())
    const [credentials, modelsJson, appConfig] = await Promise.all([
      readJson(this.authPath, {}),
      readJson(this.modelsPath, { providers: {} }),
      readJson(this.appConfigPath, { toolMode: 'full', disabledProviders: [], providerImports: {} }),
    ])
    modelsJson.providers ||= {}
    const existingProvider = modelsJson.providers[loaded.providerId]
    if (existingProvider && JSON.stringify(existingProvider) !== JSON.stringify(loaded.providerConfig)) {
      throw new Error('Pisper 已存在该 Provider 的模型配置，不会自动覆盖。')
    }
    if (loaded.credential && credentials[loaded.providerId] && JSON.stringify(credentials[loaded.providerId]) !== JSON.stringify(loaded.credential)) {
      throw new Error('Pisper 已存在该 Provider 的认证，不会自动覆盖。')
    }

    modelsJson.providers[loaded.providerId] = loaded.providerConfig
    await writeJsonAtomic(this.modelsPath, modelsJson)
    if (loaded.credential && !credentials[loaded.providerId]) {
      credentials[loaded.providerId] = loaded.credential
      await writeJsonAtomic(this.authPath, credentials)
    }

    const disabledProviders = new Set(appConfig.disabledProviders || [])
    disabledProviders.delete(loaded.providerId)
    const providerImports = { ...(appConfig.providerImports || {}) }
    providerImports[String(discoveryId)] = { providerId: loaded.providerId, fingerprint: loaded.fingerprint, source: loaded.source }
    await writeJsonAtomic(this.appConfigPath, { ...appConfig, disabledProviders: [...disabledProviders], providerImports })

    await this.disposeSessions()
    await this.reloadModelRuntime()
    return {
      providerId: loaded.providerId,
      selectedModel: loaded.selectedModel,
      config: await this.getConfig(),
      discovery: await this.getProviderDiscovery(),
    }
  }

  async getConfig() {
    const settings = this.settingsManager.getGlobalSettings()
    const appConfig = await readJson(this.appConfigPath, { toolMode: 'full' })
    const modelsJson = await readJson(this.modelsPath, { providers: {} })
    const credentials = await readJson(this.authPath, {})
    const runtimeProviders = this.modelRuntime.getProviders()
    const providerIds = [...new Set([...KNOWN_PROVIDERS, ...Object.keys(modelsJson.providers || {})])]
    const disabledProviders = new Set(appConfig.disabledProviders || [])
    const visualClaims = dedicatedVisualModelClaims(modelsJson, appConfig)
    const providers = providerIds.map((id) => {
      const runtimeProvider = runtimeProviders.find((item) => item.id === id)
      const overlay = modelsJson.providers?.[id] || {}
      const overlayModels = Array.isArray(overlay.models) ? overlay.models : []
      const type = appConfig.providerTypes?.[id] || inferredProviderType(overlay)
      const models = this.modelRuntime.getModels(id).map((model) => {
        const definition = overlayModels.find((item) => item.id === model.id)
        return {
          id: model.id,
          name: model.name || model.id,
          kind: inferModelKind(model.id, definition?.kind || model.pisperKind),
          reasoning: Boolean(model.reasoning),
          contextWindow: model.contextWindow || null,
          baseUrl: model.baseUrl || '',
          baseUrlOverride: definition?.baseUrl || '',
        }
      }).filter((model) => {
        if (type === 'visual') return model.kind !== 'chat'
        if (model.kind === 'chat') return true
        const baseUrl = model.baseUrl || overlay.baseUrl || PROVIDER_DEFAULT_BASE_URLS[id] || ''
        return !claimedByOtherVisualProvider(visualClaims, id, baseUrl, model.id, model.kind)
      }).sort((a, b) => modelRank(id, b) - modelRank(id, a) || a.name.localeCompare(b.name))
      const chatModels = models.filter((model) => model.kind === 'chat')
      const preferredModel = appConfig.providerDefaultModels?.[id]
        || (settings.defaultProvider === id ? settings.defaultModel : '')
      const defaultModel = chatModels.some((model) => model.id === preferredModel)
        ? preferredModel
        : (chatModels[0]?.id || '')
      return {
        id,
        name: PROVIDER_LABELS[id] || overlay.name || runtimeProvider?.name || id,
        type,
        configured: Boolean(credentials[id]) || this.modelRuntime.hasConfiguredAuth(id),
        enabled: !disabledProviders.has(id),
        custom: !KNOWN_PROVIDERS.includes(id),
        api: overlay.api || this.modelRuntime.getModels(id)[0]?.api || 'openai-responses',
        baseUrl: overlay.baseUrl || PROVIDER_DEFAULT_BASE_URLS[id] || '',
        organization: overlay.headers?.['OpenAI-Organization'] || '',
        defaultModel,
        models,
      }
    }).filter((provider) => provider.models.length > 0 || KNOWN_PROVIDERS.includes(provider.id))

    const hasChatModel = (provider) => provider.type !== 'visual' && provider.models.some((model) => model.kind === 'chat')
    const selectedProviderEntry = providers.find((item) => item.id === settings.defaultProvider && item.enabled && item.configured && hasChatModel(item))
      || providers.find((item) => item.enabled && item.configured && hasChatModel(item))
      || providers.find((item) => item.enabled && hasChatModel(item))
      || providers[0]
    const selectedProvider = selectedProviderEntry?.id || 'openai'
    const selectedModel = selectedProviderEntry?.defaultModel || ''
    return {
      provider: selectedProvider,
      model: selectedModel,
      thinkingLevel: settings.defaultThinkingLevel || 'medium',
      toolMode: appConfig.toolMode || 'full',
      providers,
      apiKeyConfigured: Boolean(credentials[selectedProvider]),
    }
  }

  async saveConfig(input) {
    const provider = String(input.provider || '').trim()
    const model = String(input.model || '').trim()
    if (!provider) throw new Error('Provider 不能为空。')
    const currentAppConfig = await readJson(this.appConfigPath, { toolMode: 'full', disabledProviders: [] })
    const existingOverlay = await readJson(this.modelsPath, { providers: {} })
    const providerType = input.providerType === 'visual' || input.providerType === 'chat'
      ? input.providerType
      : currentAppConfig.providerTypes?.[provider] || inferredProviderType(existingOverlay.providers?.[provider] || {})
    if ((currentAppConfig.disabledProviders || []).includes(provider)) throw new Error('请先启用该 Provider，再将其设为默认配置。')

    const credentials = await readJson(this.authPath, {})
    let apiKeyUpdated = false
    if (input.clearApiKey) {
      delete credentials[provider]
      apiKeyUpdated = true
    }
    if (typeof input.apiKey === 'string' && input.apiKey.trim()) {
      credentials[provider] = { type: 'api_key', key: input.apiKey.trim() }
      apiKeyUpdated = true
    }
    if (apiKeyUpdated) await writeJsonAtomic(this.authPath, credentials)

    const modelsJson = existingOverlay
    modelsJson.providers ||= {}
    const providerOverlay = { ...(modelsJson.providers[provider] || {}) }
    const baseUrl = String(input.baseUrl || '').trim()
    const modelBaseUrl = String(input.modelBaseUrl || '').trim()
    const organization = String(input.organization || '').trim()
    const requestedApi = ['openai-responses', 'openai-completions', 'anthropic-messages', 'google-generative-ai'].includes(input.api)
      ? input.api
      : ''
    if (requestedApi) {
      providerOverlay.api = requestedApi
      if (Array.isArray(providerOverlay.models)) {
        providerOverlay.models = providerOverlay.models.map((item) => ({ ...item, api: requestedApi }))
      }
    }
    const runtimeModel = model ? this.modelRuntime.getModel(provider, model) : null
    if (model && !runtimeModel) {
      providerOverlay.name ||= String(input.providerName || provider)
      providerOverlay.api ||= String(input.api || 'openai-responses')
      providerOverlay.models = Array.isArray(providerOverlay.models) ? [...providerOverlay.models] : []
      if (!providerOverlay.models.some((item) => item.id === model)) {
        providerOverlay.models.push({
          id: model,
          name: String(input.modelName || model),
          api: String(input.api || 'openai-responses'),
          kind: inferModelKind(model, input.modelKind),
          reasoning: input.reasoning !== false,
          input: ['text', 'image'],
          contextWindow: Number(input.contextWindow) || 200_000,
          maxTokens: Number(input.maxTokens) || 128_000,
        })
      }
    }
    const modelDefinitions = Array.isArray(providerOverlay.models) ? [...providerOverlay.models] : []
    const definitionIndex = model ? modelDefinitions.findIndex((item) => item.id === model) : -1
    if (model && (modelBaseUrl || definitionIndex >= 0)) {
      const definition = definitionIndex >= 0 ? { ...modelDefinitions[definitionIndex] } : {
        id: model,
        name: runtimeModel?.name || String(input.modelName || model),
        api: requestedApi || runtimeModel?.api || String(input.api || providerOverlay.api || 'openai-responses'),
        kind: inferModelKind(model, input.modelKind),
        reasoning: runtimeModel?.reasoning ?? input.reasoning !== false,
        input: runtimeModel?.input || ['text', 'image'],
        contextWindow: runtimeModel?.contextWindow || Number(input.contextWindow) || 200_000,
        maxTokens: runtimeModel?.maxTokens || Number(input.maxTokens) || 128_000,
      }
      if (modelBaseUrl) definition.baseUrl = modelBaseUrl
      else delete definition.baseUrl
      definition.kind = inferModelKind(model, input.modelKind || definition.kind)
      if (definitionIndex >= 0) modelDefinitions[definitionIndex] = definition
      else modelDefinitions.push(definition)
      providerOverlay.models = modelDefinitions
    }
    if (baseUrl) providerOverlay.baseUrl = baseUrl
    else delete providerOverlay.baseUrl
    if (organization) providerOverlay.headers = { ...(providerOverlay.headers || {}), 'OpenAI-Organization': organization }
    else if (providerOverlay.headers) {
      delete providerOverlay.headers['OpenAI-Organization']
      if (Object.keys(providerOverlay.headers).length === 0) delete providerOverlay.headers
    }
    if (Object.keys(providerOverlay).length) modelsJson.providers[provider] = providerOverlay
    else delete modelsJson.providers[provider]
    await writeJsonAtomic(this.modelsPath, modelsJson)

    if (providerType !== 'visual' && model) this.settingsManager.setDefaultModelAndProvider(provider, model)
    this.settingsManager.setDefaultThinkingLevel(input.thinkingLevel || 'medium')
    await this.settingsManager.flush()
    const errors = this.settingsManager.drainErrors()
    if (errors.length) throw errors[0].error

    const requestedToolMode = ['read-only', 'workspace', 'full', 'custom'].includes(input.toolMode) ? input.toolMode : 'full'
    await writeJsonAtomic(this.appConfigPath, {
      ...currentAppConfig,
      toolMode: requestedToolMode,
      enabledTools: requestedToolMode === 'custom' ? toolsFromConfig(currentAppConfig) : TOOL_PRESETS[requestedToolMode],
      disabledProviders: [...new Set(currentAppConfig.disabledProviders || [])],
      providerTypes: { ...(currentAppConfig.providerTypes || {}), [provider]: providerType },
      providerDefaultModels: providerType === 'visual' || !model
        ? { ...(currentAppConfig.providerDefaultModels || {}) }
        : { ...(currentAppConfig.providerDefaultModels || {}), [provider]: model },
    })
    await this.disposeSessions()
    await this.reloadModelRuntime()
    return { ...(await this.getConfig()), apiKeyUpdated }
  }

  async setProviderEnabled(id, enabled) {
    const provider = String(id || '').trim()
    if (!this.modelRuntime.getProviders().some((item) => item.id === provider) && !KNOWN_PROVIDERS.includes(provider)) {
      throw new Error('Provider 不存在。')
    }
    const appConfig = await readJson(this.appConfigPath, { toolMode: 'full', disabledProviders: [] })
    const disabled = new Set(appConfig.disabledProviders || [])
    if (enabled) disabled.delete(provider)
    else disabled.add(provider)

    const settings = this.settingsManager.getGlobalSettings()
    if (!enabled && settings.defaultProvider === provider) {
      const credentials = await readJson(this.authPath, {})
      const providerTypes = appConfig.providerTypes || {}
      const modelsJson = await readJson(this.modelsPath, { providers: {} })
      const alternative = this.modelRuntime.getProviders().find((item) => {
        const type = providerTypes[item.id] || inferredProviderType(modelsJson.providers?.[item.id] || {})
        return item.id !== provider
          && type !== 'visual'
          && !disabled.has(item.id)
          && credentials[item.id]
          && this.modelRuntime.getModels(item.id).some((model) => inferModelKind(model.id, model.pisperKind) === 'chat')
      })
      if (!alternative) throw new Error('至少需要保留一个已配置并启用的 Provider。')
      const alternativeModel = this.modelRuntime.getModels(alternative.id).find((model) => inferModelKind(model.id, model.pisperKind) === 'chat')
      this.settingsManager.setDefaultModelAndProvider(alternative.id, alternativeModel.id)
      await this.settingsManager.flush()
    }
    await writeJsonAtomic(this.appConfigPath, {
      ...appConfig,
      disabledProviders: [...disabled],
    })
    return this.getConfig()
  }

  async createProvider(input) {
    const id = providerProfileId(input.id || input.name)
    const name = String(input.name || '').trim()
    const api = String(input.api || 'openai-responses').trim()
    const baseUrl = String(input.baseUrl || '').trim()
    const modelId = String(input.model || '').trim()
    const providerType = input.providerType === 'visual' || inferModelKind(modelId, input.modelKind) !== 'chat' ? 'visual' : 'chat'
    if (!id || !name || !baseUrl || !modelId) throw new Error('名称、Provider ID、Base URL 和初始模型不能为空。')
    if (providerType === 'visual' && inferModelKind(modelId, input.modelKind) === 'chat') throw new Error('视觉 Provider 的初始模型必须是图像或视频模型。')
    if (this.modelRuntime.getProviders().some((item) => item.id === id) || KNOWN_PROVIDERS.includes(id)) throw new Error('Provider ID 已存在，请使用不同的连接标识。')

    const modelsJson = await readJson(this.modelsPath, { providers: {} })
    modelsJson.providers ||= {}
    modelsJson.providers[id] = {
      name,
      api,
      baseUrl,
      models: [{
        id: modelId,
        name: String(input.modelName || modelId).trim() || modelId,
        api,
        kind: inferModelKind(modelId, input.modelKind),
        reasoning: input.reasoning !== false,
        input: ['text', 'image'],
        contextWindow: Number(input.contextWindow) || 200_000,
        maxTokens: Number(input.maxTokens) || 128_000,
      }],
    }
    await writeJsonAtomic(this.modelsPath, modelsJson)

    const apiKey = String(input.apiKey || '').trim()
    if (apiKey) {
      const credentials = await readJson(this.authPath, {})
      credentials[id] = { type: 'api_key', key: apiKey }
      await writeJsonAtomic(this.authPath, credentials)
    }
    const appConfig = await readJson(this.appConfigPath, { toolMode: 'full', disabledProviders: [] })
    const disabled = new Set(appConfig.disabledProviders || [])
    if (input.enabled === false) disabled.add(id)
    else disabled.delete(id)
    await writeJsonAtomic(this.appConfigPath, {
      ...appConfig,
      disabledProviders: [...disabled],
      providerTypes: { ...(appConfig.providerTypes || {}), [id]: providerType },
      providerDefaultModels: providerType === 'visual'
        ? { ...(appConfig.providerDefaultModels || {}) }
        : { ...(appConfig.providerDefaultModels || {}), [id]: modelId },
    })
    await this.disposeSessions()
    await this.reloadModelRuntime()
    return { ...(await this.getConfig()), createdProviderId: id }
  }

  async addProviderModel(providerId, input) {
    return this.addProviderModels(providerId, [input], { skipExisting: false })
  }

  async reconcileDefaultModel() {
    const settings = this.settingsManager.getGlobalSettings()
    const config = await this.getConfig()
    if (config.provider && config.model && (settings.defaultProvider !== config.provider || settings.defaultModel !== config.model)) {
      this.settingsManager.setDefaultModelAndProvider(config.provider, config.model)
      await this.settingsManager.flush()
    }
    return config
  }

  async refreshProviderModels() {
    if (this.providerModelRefreshPromise) return this.providerModelRefreshPromise
    const refresh = async () => {
      const [modelsJson, credentials, appConfig] = await Promise.all([
        readJson(this.modelsPath, { providers: {} }),
        readJson(this.authPath, {}),
        readJson(this.appConfigPath, { disabledProviders: [] }),
      ])
      const disabled = new Set(appConfig.disabledProviders || [])
      const providerIds = new Set([...KNOWN_PROVIDERS, ...Object.keys(modelsJson.providers || {})])
      const jobs = []
      for (const provider of providerIds) {
        if (disabled.has(provider) || provider === 'openai-codex') continue
        const overlay = modelsJson.providers?.[provider] || {}
        const baseUrl = String(overlay.baseUrl || PROVIDER_DEFAULT_BASE_URLS[provider] || '').trim()
        if (!baseUrl) continue
        const hasAuthentication = Boolean(configuredProviderSecret(credentials[provider], overlay)) || this.modelRuntime.hasConfiguredAuth(provider)
        const isExplicitConnection = Boolean(overlay.baseUrl)
        if (!hasAuthentication && !isExplicitConnection) continue
        jobs.push((async () => {
          try {
            const result = await this.discoverProviderModels(provider, { reconcile: false, includeConfig: false })
            return { provider, ok: true, count: result.count, added: result.addedModelIds.length, removed: result.removedModelIds.length }
          } catch (error) {
            return { provider, ok: false, error: redactSecretText(error instanceof Error ? error.message : String(error)) }
          }
        })())
      }
      const results = await Promise.all(jobs)
      return { results, config: await this.reconcileDefaultModel() }
    }
    const pending = refresh().finally(() => {
      if (this.providerModelRefreshPromise === pending) this.providerModelRefreshPromise = null
    })
    this.providerModelRefreshPromise = pending
    return pending
  }

  async discoverProviderModels(providerId, input = {}) {
    const provider = String(providerId || '').trim()
    if (!this.modelRuntime.getProviders().some((item) => item.id === provider) && !KNOWN_PROVIDERS.includes(provider)) throw new Error('Provider 不存在。')
    const [modelsJson, credentials, appConfig] = await Promise.all([
      readJson(this.modelsPath, { providers: {} }),
      readJson(this.authPath, {}),
      readJson(this.appConfigPath, { providerTypes: {} }),
    ])
    const overlay = modelsJson.providers?.[provider] || {}
    const runtimeModel = this.modelRuntime.getModels(provider)[0]
    const api = String(input.api || overlay.api || runtimeModel?.api || 'openai-responses').trim()
    const configuredBaseUrl = String(overlay.baseUrl || PROVIDER_DEFAULT_BASE_URLS[provider] || '').trim()
    const baseUrl = String(input.baseUrl || configuredBaseUrl || '').trim()
    if (!baseUrl) throw new Error('请先配置 Provider Base URL。')
    const apiKey = String(input.apiKey || '').trim() || configuredProviderSecret(credentials[provider], overlay)
    const discovered = await this.providerModelDiscovery.discover({
      api,
      baseUrl,
      apiKey,
      organization: String(input.organization || overlay.headers?.['OpenAI-Organization'] || '').trim(),
      headers: providerHeaders(provider, overlay, this.providerUserAgent),
    })
    const scope = input.providerType === 'visual' || input.providerType === 'chat'
      ? input.providerType
      : appConfig.providerTypes?.[provider] || inferredProviderType(overlay)
    const visualClaims = dedicatedVisualModelClaims(modelsJson, appConfig)
    // 发现的模型不再按 ID 推断用途（统一为 chat），所以这里不按 kind 过滤：
    // 视觉 Provider 也列出全部发现结果，由用户添加时显式选择图像/视频类型。
    const models = scope === 'visual'
      ? discovered.models
      : discovered.models.filter((model) => !claimedByOtherVisualProviderAnyKind(visualClaims, provider, baseUrl, model.id))
    if (!models.length) throw new Error('Provider 没有返回可用的模型。')
    const result = { ...discovered, count: models.length, models, scope }
    const previousModelIds = new Set(this.modelRuntime.getModels(provider).map((model) => model.id))
    let sync = { addedModelIds: [], removedModelIds: [] }
    const synchronized = sameBaseUrl(baseUrl, configuredBaseUrl)
    if (synchronized) {
      await this.providerModelCatalog.sync(provider, { baseUrl, api, models: result.models })
      const nextModelIds = new Set(result.models.map((model) => model.id))
      sync = {
        addedModelIds: [...nextModelIds].filter((id) => !previousModelIds.has(id)),
        removedModelIds: [...previousModelIds].filter((id) => !nextModelIds.has(id)),
      }
      if (input.reconcile !== false) await this.reconcileDefaultModel()
    }
    const existing = new Set(this.modelRuntime.getModels(provider).map((model) => model.id))
    return {
      ...result,
      models: result.models.map((model) => ({ ...model, added: existing.has(model.id) })),
      synchronized,
      addedModelIds: sync.addedModelIds,
      removedModelIds: sync.removedModelIds,
      config: synchronized && input.includeConfig !== false ? await this.getConfig() : null,
    }
  }

  async addProviderModels(providerId, inputs, { skipExisting = true } = {}) {
    const provider = String(providerId || '').trim()
    if (!this.modelRuntime.getProviders().some((item) => item.id === provider) && !KNOWN_PROVIDERS.includes(provider)) throw new Error('Provider 不存在。')
    const models = Array.isArray(inputs) ? inputs : []
    if (!models.length) throw new Error('请至少选择一个模型。')
    if (models.length > 250) throw new Error('单次最多添加 250 个模型。')
    const [modelsJson, appConfig] = await Promise.all([
      readJson(this.modelsPath, { providers: {} }),
      readJson(this.appConfigPath, { providerTypes: {} }),
    ])
    modelsJson.providers ||= {}
    const overlay = { ...(modelsJson.providers[provider] || {}) }
    const providerType = appConfig.providerTypes?.[provider] || inferredProviderType(overlay)
    overlay.models = Array.isArray(overlay.models) ? [...overlay.models] : []
    const existing = new Set([...overlay.models.map((item) => item.id), ...this.modelRuntime.getModels(provider).map((item) => item.id)])
    const addedModelIds = []
    for (const input of models) {
      const modelId = String(input?.id || '').trim()
      if (!modelId) throw new Error('模型 ID 不能为空。')
      if (modelId.length > 240) throw new Error('模型 ID 过长。')
      const modelKind = inferModelKind(modelId, input.kind)
      if (providerType === 'visual' && modelKind === 'chat') throw new Error('视觉 Provider 只能添加图像或视频模型。')
      if (existing.has(modelId)) {
        if (!skipExisting) throw new Error('该模型已经存在。')
        continue
      }
      overlay.models.push({
        id: modelId,
        name: String(input.name || modelId).trim() || modelId,
        api: String(input.api || overlay.api || 'openai-responses'),
        kind: modelKind,
        ...(String(input.baseUrl || '').trim() ? { baseUrl: String(input.baseUrl).trim() } : {}),
        reasoning: input.reasoning !== false,
        input: ['text', 'image'],
        contextWindow: Number(input.contextWindow) || 200_000,
        maxTokens: Number(input.maxTokens) || 128_000,
      })
      existing.add(modelId)
      addedModelIds.push(modelId)
    }
    if (!addedModelIds.length) throw new Error('所选模型均已添加。')
    modelsJson.providers[provider] = overlay
    await writeJsonAtomic(this.modelsPath, modelsJson)
    const catalog = this.providerModelCatalog.get(provider)
    const providerBaseUrl = overlay.baseUrl || PROVIDER_DEFAULT_BASE_URLS[provider] || ''
    if (catalog && sameBaseUrl(catalog.baseUrl, providerBaseUrl)) {
      const added = models.filter((model) => addedModelIds.includes(String(model.id)))
      await this.providerModelCatalog.sync(provider, { baseUrl: catalog.baseUrl, api: catalog.api, models: [...catalog.models, ...added] })
    }
    await this.disposeSessions()
    await this.reloadModelRuntime()
    return { ...(await this.getConfig()), addedModelIds }
  }

  async deleteProvider(id) {
    const provider = String(id || '').trim()
    if (KNOWN_PROVIDERS.includes(provider)) throw new Error('内置 Provider 不能删除，可以将其停用。')
    const modelsJson = await readJson(this.modelsPath, { providers: {} })
    if (!modelsJson.providers?.[provider]) return null
    delete modelsJson.providers[provider]
    await writeJsonAtomic(this.modelsPath, modelsJson)
    const credentials = await readJson(this.authPath, {})
    delete credentials[provider]
    await writeJsonAtomic(this.authPath, credentials)
    const appConfig = await readJson(this.appConfigPath, { toolMode: 'full', disabledProviders: [] })
    appConfig.disabledProviders = (appConfig.disabledProviders || []).filter((item) => item !== provider)
    if (appConfig.providerTypes) delete appConfig.providerTypes[provider]
    if (appConfig.providerDefaultModels) delete appConfig.providerDefaultModels[provider]
    await writeJsonAtomic(this.appConfigPath, appConfig)
    await this.providerModelCatalog.remove(provider)
    const settings = this.settingsManager.getGlobalSettings()
    if (settings.defaultProvider === provider) {
      const providerTypes = appConfig.providerTypes || {}
      const alternative = this.modelRuntime.getProviders().find((item) => {
        const type = providerTypes[item.id] || inferredProviderType(modelsJson.providers?.[item.id] || {})
        return item.id !== provider
          && type !== 'visual'
          && credentials[item.id]
          && this.modelRuntime.getModels(item.id).some((model) => inferModelKind(model.id, model.pisperKind) === 'chat')
      })
      if (alternative) {
        const alternativeModel = this.modelRuntime.getModels(alternative.id).find((model) => inferModelKind(model.id, model.pisperKind) === 'chat')
        this.settingsManager.setDefaultModelAndProvider(alternative.id, alternativeModel.id)
        await this.settingsManager.flush()
      }
    }
    await this.disposeSessions()
    await this.reloadModelRuntime()
    return this.getConfig()
  }
}
