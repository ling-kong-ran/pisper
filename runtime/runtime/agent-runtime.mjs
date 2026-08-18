// AgentRuntimeService：Pisper 的核心服务类，串起 Pi 引擎会话、工具、服务与外部 API。
// 职责包括：会话创建/生命周期管理、消息流式执行与实时事件桥接、上下文压缩、
// 多 Agent 协作、目标(Goal)/计划(Plan)、记忆、资产、权限审批、Provider/模型偏好等。
// 对外暴露的能力被 HTTP API（runtime/http/）与 agent-runtime-facade 包装层消费。
import { mkdir, open, readFile, stat, unlink } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { basename, extname, isAbsolute, join, resolve, sep } from 'node:path'
import {
  createAgentSession,
  createCompressedReadTool,
  SessionManager,
  SettingsManager,
} from './pi-coding-agent.mjs'
import { ensureSessionFilePersisted } from './session-file-persist.mjs'
import { upsertStoredSessionCache } from './stored-session-cache.mjs'
import {
  capturePromptCacheShape,
  comparePromptCacheShapes,
  promptCacheRuntime,
} from './prompt-cache-diagnostics.mjs'
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
import { WorkspaceTrustService } from '../services/workspace-trust-service.mjs'
import { SessionPermissionService } from '../services/session-permission-service.mjs'
import { ToolPluginService } from '../services/tool-plugin-service.mjs'
import { WebSearchService } from '../services/web-search-service.mjs'
import { captureConversationMemory, localDayKey } from './conversation-memory-capture.mjs'
import { LocalMemoryRuntime } from '../services/memory/local-memory-runtime.mjs'
import { createSemanticMemorySummarizer } from '../services/memory/semantic-memory.mjs'
import { VisualGenerationService } from '../services/visual-generation/index.mjs'
import {
  MultiAgentService,
  MULTI_AGENT_TOOL_NAMES,
  agentCompletionPrompt,
  isAgentCompletionMessage,
} from '../services/multi-agent-service.mjs'
import {
  GoalService,
  goalBudgetPrompt,
  goalContinuationPrompt,
  isGoalContinuationMessage,
} from '../services/goal-service.mjs'
import { GitChangesService } from '../services/git-changes-service.mjs'
import { VcsChangesService } from '../services/vcs-changes-service.mjs'
import { PlanService } from '../services/plan-service.mjs'
import { BrowserAutomationService } from '../services/browser-automation-service.mjs'
import {
  normalizeWorkspacePath,
  resolveWorkspaceDirectory,
  workspacePathKey,
} from './workspace-directories.mjs'
import { assetMessageAttachment } from '../services/session-assets.mjs'
import * as assetStorage from '../services/asset-storage.mjs'
import { createAppTools, createMultiAgentTools } from '../tools/registry.mjs'
import { createGoalTools, GOAL_TOOL_NAMES } from '../tools/app/goal.mjs'
import {
  createPlanTools,
  PLAN_ALL_TOOL_NAMES,
  PLAN_COMPATIBILITY_TOOL_NAMES,
} from '../tools/app/plan.mjs'
import { createToolDiscoveryTool, TOOL_DISCOVERY_NAME } from '../tools/app/tool-discovery.mjs'
import { TOOL_GATEWAY_NAME } from '../tools/app/tool-gateway.mjs'
import { createPisperBashTool } from '../tools/host-bash.mjs'
import {
  hotToolNames,
  mergePromotedToolNames,
  schemaOnlyToolDefinitions,
} from '../tools/tool-activation.mjs'
import {
  DEFAULT_EXECUTION_MODE,
  EXECUTION_MODES,
  filterToolsForExecutionMode,
  migrateLegacyExecutionMode,
  normalizeExecutionMode,
  permissionModeForExecutionMode,
} from '../security/execution-mode.mjs'
import { applyPisperSystemPrompt, pisperPromptExtension } from '../prompts/pisper-system-prompt.mjs'
import { createRuntimeToolGateway } from './tool-gateway-runtime.mjs'
import { agentSessionMethods } from './agent-session-methods.mjs'
import {
  DEFAULT_COMPACTION_THRESHOLD_PERCENT,
  createCompactionSettingsManager,
  installTurnBoundaryCompaction,
  normalizeCompactionThresholdPercent,
  pisperCompactionExtension,
} from './compaction-policy.mjs'
import {
  AgentRuntimeFacade,
  DEFAULT_MEMORY_AUTO_APPROVE_CONFIDENCE,
  ISOLATED_CONTEXT_BLOCKED_TOOLS,
  createScheduleWorkflowAdapter,
  normalizeMemoryAutoApproveConfidence,
} from './agent-runtime-facade.mjs'
import { ToolActivation } from './tool-activation.mjs'
import {
  bridgeAgentSessionEvent,
  createLiveRunState,
  finishAgentLifecycle,
} from './agent-event-bridge.mjs'
import { SessionLifecycle } from './session-lifecycle.mjs'
import { ProviderPreferences } from './provider-preferences.mjs'
import {
  MAX_LIVE_ACTIVITY_ITEMS,
  StreamProjection,
  addSessionUsage,
  beginTextBlock,
  finishedCompaction,
  isInternalParentMessage,
  livePlanChanges,
  liveThinkingTail,
  pushLiveActivity,
  queuedSessionInputs,
  setLiveActivity,
  startedCompaction,
  textFromContent,
} from './stream-projection.mjs'
// 注入到消息末尾的附件上下文标记，供模型区分“用户原文”与“系统注入的上下文”。
const ATTACHMENT_MARKER = '\n\n---\nAttachment context (injected by Pisper):\n'
// 附件文本/文档提取后的最大字符数，防止超大文件撑爆上下文。
const MAX_EXTRACTED_CHARS = 400_000
// 资产（上传文件）存储上限；聊天中直接注入的资产另有更严格的限制。
const MAX_ASSET_BYTES = 24 * 1024 * 1024
const MAX_CHAT_ASSET_BYTES = 10 * 1024 * 1024
const DEFAULT_SESSION_NAME = '新会话'
const MAX_SESSION_TITLE_CHARS = 20
// 常驻会话运行时（resident runtime）的上限：会话关闭后运行时不会立即销毁，
// 而是保留最近使用的一批以加速重新打开；超出的空闲运行时被周期回收。
const MAX_RESIDENT_SESSION_RUNTIMES = 3
const SESSION_RUNTIME_IDLE_TTL_MS = 5 * 60 * 1000
const SESSION_RUNTIME_SWEEP_INTERVAL_MS = 60 * 1000
// 强制中断会话的宽限期：正常停止信号超时后仍不退出时，直接 dispose 会话。
const ABORT_FORCE_TIMEOUT_MS = 10_000
// 中止护栏：模型未在 abortForceTimeoutMs 内响应停止时强制销毁会话。
// 每 250ms 轮询一次，避免长任务卡在“已请求停止却仍运行”的状态。
async function runPromptWithAbortGuard(value, run) {
  const timeoutMs = Number(value?.abortForceTimeoutMs) || ABORT_FORCE_TIMEOUT_MS
  const runPromise = Promise.resolve().then(run)
  while (true) {
    const outcome = await Promise.race([
      runPromise.then(
        (result) => ({ done: true, result }),
        (error) => ({ done: true, error }),
      ),
      new Promise((resolve) => setTimeout(() => resolve({ done: false }), 250)),
    ])
    if (outcome.done) {
      if (outcome.error) throw outcome.error
      return outcome.result
    }
    const abortedAt = value?.abortedAt
    if (abortedAt && Date.now() - abortedAt >= timeoutMs) {
      const session = value?.session
      if (session && typeof session.dispose === 'function') {
        try {
          session.dispose()
        } catch {}
        value.forceDisposed = true
      }
      throw new Error('Agent 未能在超时时间内响应停止，本次运行已被强制中断。')
    }
  }
}
export { runPromptWithAbortGuard }
const SESSION_HISTORY_READ_CHUNK_BYTES = 1024 * 1024
// 历史消息缓存：限制条目数与源文件大小，避免大量会话文件导致内存膨胀。
const MAX_SESSION_HISTORY_CACHE_ENTRIES = 4
const MAX_SESSION_HISTORY_CACHE_SOURCE_BYTES = 8 * 1024 * 1024
const MAX_SESSION_HISTORY_CACHE_ESTIMATED_BYTES = 48 * 1024 * 1024
// 可按文本提取的附件扩展名集合（区别于需 officeparser 解析的文档类型）。
const ASSET_TEXT_EXTENSIONS = new Set([
  '.txt',
  '.md',
  '.json',
  '.js',
  '.jsx',
  '.ts',
  '.tsx',
  '.css',
  '.html',
  '.xml',
  '.yaml',
  '.yml',
  '.csv',
  '.log',
  '.py',
  '.java',
  '.go',
  '.rs',
  '.sh',
  '.ps1',
  '.toml',
  '.sql',
])
// 需要 officeparser 解析的办公文档扩展名集合。
const ASSET_DOCUMENT_EXTENSIONS = new Set([
  '.pdf',
  '.docx',
  '.pptx',
  '.xlsx',
  '.odt',
  '.odp',
  '.ods',
  '.rtf',
  '.epub',
])
const IMAGE_EXTENSIONS = new Set(['.png', '.jpg', '.jpeg', '.gif', '.webp', '.bmp', '.svg'])
// 部分上游（模型网关）在流式中断时会报 “stream read error” 之类的瞬时错误，
// 该错误可安全重试，用 Symbol 标记避免重复打补丁。
const TRANSIENT_STREAM_READ_ERROR_PATTERN = /\bstream[_\s-]?read[_\s-]?error\b/i
const PISPER_STREAM_RETRY_PATCH = Symbol('pisper.stream-retry-patch')

// 给会话打上瞬时流错误重试补丁：命中“stream read error”时视为可重试错误，
// 交给 Pi 引擎的内置重试机制处理；同一会话只打一次补丁。
export function installTransientStreamRetry(session) {
  if (
    !session ||
    session[PISPER_STREAM_RETRY_PATCH] ||
    typeof session._isRetryableError !== 'function'
  )
    return session
  const isRetryableError = session._isRetryableError.bind(session)
  session._isRetryableError = (message) => {
    if (
      message?.stopReason === 'error' &&
      TRANSIENT_STREAM_READ_ERROR_PATTERN.test(String(message.errorMessage || ''))
    )
      return true
    return isRetryableError(message)
  }
  session[PISPER_STREAM_RETRY_PATCH] = true
  return session
}

// 从会话分支记录中推导最后一次使用的模型（model_change 或 assistant 消息携带）。
// 用于恢复会话时还原其模型选择。
export function storedSessionModel(sessionManager) {
  let model = null
  for (const entry of sessionManager?.getBranch?.() || []) {
    if (entry?.type === 'model_change' && entry.provider && entry.modelId) {
      model = { provider: entry.provider, modelId: entry.modelId }
      continue
    }
    if (
      entry?.type === 'message' &&
      entry.message?.role === 'assistant' &&
      entry.message?.provider &&
      entry.message?.model
    ) {
      model = { provider: entry.message.provider, modelId: entry.message.model }
    }
  }
  return model
}

export function storedSessionModelId(sessionManager) {
  return storedSessionModel(sessionManager)?.modelId || ''
}

// 解析 “provider/modelId” 形式的模型引用；格式非法时返回 null。
function parseSessionModelRef(value) {
  const raw = String(value || '').trim()
  if (!raw) return null
  const slash = raw.indexOf('/')
  if (slash <= 0 || slash >= raw.length - 1) return null
  return {
    provider: raw.slice(0, slash),
    modelId: raw.slice(slash + 1),
  }
}

// 会话模型解析优先级：会话分支记录 > 会话元数据的 model 字段。
function resolveSessionModelRef(sessionManager, sessionMeta = {}) {
  return storedSessionModel(sessionManager) || parseSessionModelRef(sessionMeta.model) || null
}

// 从多 Agent 工具的执行结果中提取 Agent 摘要信息，供实时活动流展示。
export function multiAgentResultAgent(toolName, details) {
  if (!MULTI_AGENT_TOOL_NAMES.includes(toolName) || !details) return null
  if (toolName === 'wait_agent') return details.agent?.id ? details.agent : null
  if (['spawn_agent', 'send_message', 'followup_task', 'interrupt_agent'].includes(toolName))
    return details.id ? details : null
  return null
}

// 等待 Agent 完成并确认（acknowledge）结果：确认后 Agent 不再进入“完成”提示流。
export async function waitForAgentMailbox(multiAgents, sessionId, timeoutMs, target) {
  const result = await multiAgents.wait(sessionId, timeoutMs, target)
  if (!result.timedOut && result.agent) await multiAgents.acknowledge(sessionId, [result.agent])
  return result
}

// 附件名清洗：去掉换行与尖括号等可能破坏 Markdown/文件名的字符，并限制长度。
function safeAttachmentName(name) {
  return String(name || '附件')
    .replace(/[\r\n<>]/g, '_')
    .slice(0, 180)
}

// 依据扩展名推断 MIME 类型；未知类型回退到二进制流。
function mimeFromName(name) {
  const extension = extname(String(name || '')).toLowerCase()
  return (
    {
      '.png': 'image/png',
      '.jpg': 'image/jpeg',
      '.jpeg': 'image/jpeg',
      '.gif': 'image/gif',
      '.webp': 'image/webp',
      '.svg': 'image/svg+xml',
      '.txt': 'text/plain',
      '.md': 'text/markdown',
      '.json': 'application/json',
      '.js': 'text/javascript',
      '.ts': 'text/typescript',
      '.css': 'text/css',
      '.html': 'text/html',
      '.pdf': 'application/pdf',
      '.docx': 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
      '.pptx': 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
      '.xlsx': 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
      '.mp4': 'video/mp4',
      '.webm': 'video/webm',
      '.mov': 'video/quicktime',
    }[extension] || 'application/octet-stream'
  )
}

// 标题截断：按 Unicode 码点截断（避免截断代理对产生乱码），超出部分以省略号结尾。
function truncateTitle(value) {
  const characters = Array.from(String(value || '').trim())
  return characters.length > MAX_SESSION_TITLE_CHARS
    ? `${characters.slice(0, MAX_SESSION_TITLE_CHARS).join('')}…`
    : characters.join('')
}

// 从首条消息清洗出会话标题：取第一句、去掉 Markdown 列表前缀与“标题：”等引导词、
// 去除首尾标点，供自动命名使用。
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
function promptCacheTools(session) {
  const names = session?.getActiveToolNames?.() || []
  if (typeof session?.getToolDefinition === 'function')
    return names.map((name) => session.getToolDefinition(name)).filter(Boolean)
  return session?.agent?.state?.tools || []
}

// 把各种来源的用量数据规整为统一的非负数值结构，缺省字段补 0。
function normalizedUsage(usage) {
  const number = (value) => (Number.isFinite(Number(value)) ? Math.max(0, Number(value)) : 0)
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

async function resolveDirectory(input, fallback) {
  return resolveWorkspaceDirectory(input, fallback)
}

// 由首条消息生成会话标题：去掉代码块、取第一句，失败时回退到附件名或默认名。
export function sessionTitleFromFirstMessage(message, attachments = []) {
  const content = String(message || '')
    .replace(/```[\s\S]*?```/g, '代码内容')
    .trim()
  const firstSentence = content.match(/^.*?(?:[。！？!?]+|[.]+(?=\s|$)|\r?\n|$)/u)?.[0] || ''
  const title = firstSentence.replace(/\s+/g, ' ').trim()
  const attachmentName = attachments
    .map((attachment) => safeAttachmentName(attachment?.name))
    .find(Boolean)
  return cleanSessionTitle(title || attachmentName) || DEFAULT_SESSION_NAME
}

// 解析办公文档附件：base64 解码后交给 officeparser 提取文本。
// 模块是懒加载的，只有真正收到文档附件时才引入依赖。
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

export class AgentRuntimeService extends AgentRuntimeFacade {
  // 构造器只做依赖装配：把所有服务/路径/配置对象挂到 this 上，不做 IO。
  // 真正的文件读写与初始化在 init() 中按阶段进行。
  constructor({
    cwd,
    dataDir,
    appVersion,
    providerDiscovery,
    providerModelDiscovery,
    browserAutomationDriver,
    eventObserver,
    legacyDefaultCwds = [],
  } = {}) {
    super()
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
    this.providerUserAgent = String(appVersion || '').trim()
      ? `Pisper/${String(appVersion).trim()}`
      : 'Pisper'
    this.providerDiscovery = providerDiscovery || new ProviderDiscoveryService({ cwd })
    this.providerModelDiscovery = providerModelDiscovery || new ProviderModelDiscoveryService()
    this.sessionDir = join(dataDir, 'sessions')
    this.authPath = join(dataDir, 'auth.json')
    this.modelsPath = join(dataDir, 'models.json')
    this.providerModelCatalogPath = join(dataDir, 'pisper-provider-models.json')
    this.modelMetadataPath = join(dataDir, 'pisper-model-metadata.json')
    this.modelMetadata = new ModelMetadataService({ path: this.modelMetadataPath })
    this.providerModelCatalog = new ProviderModelCatalogService({
      path: this.providerModelCatalogPath,
      metadata: this.modelMetadata,
    })
    this.settingsPath = join(dataDir, 'settings.json')
    this.appConfigPath = join(dataDir, 'pisper.json')
    this.toolPlugins = new ToolPluginService(this.appConfigPath, { dataDir })
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
    this.memory = new LocalMemoryRuntime({
      path: join(dataDir, 'pisper-memory.sqlite'),
      cwd,
      getAutoApproveConfidence: () => this.memoryAutoApproveConfidence / 100,
    })
    this.memorySummarizer = createSemanticMemorySummarizer({
      getModelRuntime: () => this.modelRuntime,
      getDefaultModel: () => this.resolveDefaultModel(),
    })
    this.goals = new GoalService({ path: join(dataDir, 'pisper-goals.json') })
    this.gitChanges = new GitChangesService()
    this.vcsChanges = new VcsChangesService({ git: this.gitChanges })
    this.plans = new PlanService({
      path: join(dataDir, 'pisper-plans.json'),
      legacyPath: join(dataDir, 'pisper-task-lists.json'),
    })
    this.browserAutomation = new BrowserAutomationService({ driver: browserAutomationDriver })
    this.workspaceTrust = new WorkspaceTrustService({ agentDir: dataDir })
    this.goalEmitters = new Map()
    this.agentEmitters = new Map()
    this.mcp = new McpService({ path: join(dataDir, 'pisper-mcp.json'), cwd })
    this.skills = new SkillsService({
      path: join(dataDir, 'pisper-skills.json'),
      agentDir: dataDir,
      cwd,
      getSettingsManager: (skillsCwd = this.cwd) => {
        if (!this.settingsManager || workspacePathKey(skillsCwd) === workspacePathKey(this.cwd))
          return this.settingsManager
        return SettingsManager.create(skillsCwd, this.dataDir, {
          projectTrusted: this.workspaceTrust.isTrusted(skillsCwd),
        })
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
    this.notificationSettings = new NotificationSettingsService({
      path: this.appConfigPath,
      browserEventsPath: join(dataDir, 'pisper-browser-notifications.json'),
      channels: this.channels,
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
    this.schedules = new ScheduleService({
      path: join(dataDir, 'pisper-schedules.json'),
      cwd,
      agent: {
        prompt: (input) => this.promptFromChannel({ sessionId: '', ...input }),
        validateDirectory: (input) => resolveDirectory(input, this.cwd),
      },
      workflows: createScheduleWorkflowAdapter(this.workflows),
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
    this.memoryAutoApproveConfidence = DEFAULT_MEMORY_AUTO_APPROVE_CONFIDENCE
    this.sessionMeta = {}
    this.permissions = new SessionPermissionService({
      approvalPath: join(dataDir, 'pisper-approvals.json'),
      getMode: (sessionId) =>
        this.sessionMeta[sessionId]?.permissionMode ||
        permissionModeForExecutionMode(this.getSessionExecutionMode(sessionId)),
      getExecutionMode: (sessionId) => this.getSessionExecutionMode(sessionId),
      getToolRisk: (toolName) => this.getToolRisk(toolName),
    })
    this.multiAgents = new MultiAgentService({
      path: join(dataDir, 'pisper-agents.json'),
      agentDir: this.dataDir,
      getModelRuntime: () => this.modelRuntime,
      getSettingsManager: () => this.settingsManager,
      getCompactionThresholdPercent: () => this.compactionThresholdPercent,
      createResourceLoader: ({ cwd: childCwd, appendSystemPrompt }) =>
        this.skills.createResourceLoader(childCwd, { appendSystemPrompt }),
    })
    this.multiAgents.setCompletionNotifier((agent) => this.injectAgentCompletion(agent))
    this.sessionMetaWrite = Promise.resolve()
    this.usageLedger = { days: {}, sessionScans: {} }
    this.usageWrite = Promise.resolve()
    this.assetIndex = { assets: [] }
    this.assetWrite = Promise.resolve()
    this.assetReconcile = Promise.resolve(false)
    this.providerState = { refreshPromise: null }
    Object.defineProperty(this, 'providerModelRefreshPromise', {
      configurable: true,
      get: () => this.providerState.refreshPromise,
      set: (value) => {
        this.providerState.refreshPromise = value
      },
    })
    this.agentWakeupTimers = new Map()
    this.assetProjectionRevision = 0
    this.streamProjection = new StreamProjection({
      cwd: this.cwd,
      sessions: () => this.sessions,
      liveSessions: () => this.liveSessions,
      sessionMeta: () => this.sessionMeta,
      assetIndex: () => this.assetIndex,
      getAssetRevision: () => this.assetProjectionRevision,
      getExecutionMode: (id) => this.getSessionExecutionMode(id),
      goals: { get: (id) => this.goals.get(id) },
      plans: { get: (id) => this.plans.get(id) },
      multiAgents: { summaries: (id) => this.multiAgents.summaries(id) },
      permissions: { getPending: (id) => this.permissions.getPending(id) },
      settingsManager: () => this.settingsManager,
      modelRuntime: () => this.modelRuntime,
      compactionThresholdPercent: () => this.compactionThresholdPercent,
      findSessionInfo: (id) => this.findSessionInfo(id),
      openStoredSession: (path) => this.openStoredSession(path),
      touchSessionRuntime: (value) => this.touchSessionRuntime(value),
      saveSessionMeta: () => this.saveSessionMeta(),
      history: () => ({
        cache: this.sessionHistoryCache,
        paths: this.sessionHistoryPaths,
        contextUsageCache: this.sessionContextUsageCache,
        readChunkBytes: this.sessionHistoryReadChunkBytes,
        maxEntries: this.maxSessionHistoryCacheEntries,
        maxSourceBytes: this.maxSessionHistoryCacheSourceBytes,
        maxEstimatedBytes: this.maxSessionHistoryCacheEstimatedBytes,
      }),
    })
    this.toolActivation = new ToolActivation({
      getExecutionMode: (id) => this.getSessionExecutionMode(id),
      getToolRisk: (name) => this.getToolRisk(name),
      getSessionMeta: () => this.sessionMeta,
      saveSessionMeta: () => this.saveSessionMeta(),
      getGoal: (id) => this.goals.get(id),
      promoteTools: (value, names) => this.promoteSessionTools(value, names),
    })
    this.lifecycleState = {}
    for (const property of [
      'maxResidentSessionRuntimes',
      'sessionRuntimeIdleTtlMs',
      'maxSessionHistoryCacheEntries',
      'maxSessionHistoryCacheSourceBytes',
      'maxSessionHistoryCacheEstimatedBytes',
      'sessionRuntimeVersion',
      'storedSessionsCache',
    ]) {
      Object.defineProperty(this.lifecycleState, property, {
        enumerable: true,
        get: () => this[property],
        set: (value) => {
          this[property] = value
        },
      })
    }
    this.sessionLifecycle = new SessionLifecycle({
      cwd: this.cwd,
      sessionDir: this.sessionDir,
      sessions: this.sessions,
      pendingSessions: this.pendingSessions,
      liveSessions: this.liveSessions,
      sessionHistoryCache: this.sessionHistoryCache,
      sessionHistoryPaths: this.sessionHistoryPaths,
      sessionContextUsageCache: this.sessionContextUsageCache,
      agentWakeupTimers: this.agentWakeupTimers,
      getSessionMeta: () => this.sessionMeta,
      getSettingsManager: () => this.settingsManager,
      getGoals: () => this.goals,
      getPlans: () => this.plans,
      getMultiAgents: () => this.multiAgents,
      getPermissions: () => this.permissions,
      getBrowserAutomation: () => this.browserAutomation,
      getExecutionMode: (id) => this.getSessionExecutionMode(id),
      resolveDirectory,
      cleanSessionTitle,
      listStoredSessions: (options) => this.listStoredSessions(options),
      upsertStoredSession: (info) => this.upsertStoredSession(info),
      openStoredSession: (path) => this.openStoredSession(path),
      saveSessionMeta: () => this.saveSessionMeta(),
      saveUsageLedger: () => this.saveUsageLedger(),
      getUsageLedger: () => this.usageLedger,
      createSessionRuntime: (manager, name) => this.createSessionRuntime(manager, name),
      setSessionModel: (id, provider, model) => this.setSessionModel(id, provider, model),
      syncGoalTools: (value, goal) => this.syncGoalTools(value, goal),
      pauseSessionGoal: (id) => this.pauseSessionGoal(id),
      invalidateProjection: (id, scopes) => this.streamProjection.invalidate(id, scopes),
      getRuntimeState: () => this.lifecycleState,
      setRuntimeVersion: (version) => {
        this.sessionRuntimeVersion = version
      },
    })
    this.providerPreferences = new ProviderPreferences({
      authPath: this.authPath,
      modelsPath: this.modelsPath,
      appConfigPath: this.appConfigPath,
      providerUserAgent: this.providerUserAgent,
      providerDiscovery: this.providerDiscovery,
      providerModelDiscovery: this.providerModelDiscovery,
      providerModelCatalog: this.providerModelCatalog,
      modelMetadata: this.modelMetadata,
      getModelRuntime: () => this.modelRuntime,
      setModelRuntime: (runtime) => {
        this.modelRuntime = runtime
      },
      getSettingsManager: () => this.settingsManager,
      getSession: (id) => this.getOrCreateSession(id),
      isSessionRunActive: (id, value) => this.sessionRunIsActive(id, value),
      contextUsage: (session, compaction) => this.compactionAwareContextUsage(session, compaction),
      invalidateProjection: (id, scopes) => {
        if (scopes?.allUsage) this.streamProjection.invalidateAllUsage()
        else this.streamProjection.invalidate(id, scopes)
      },
      invalidateSessionRuntimes: () => this.invalidateSessionRuntimes(),
      reloadModelRuntime: () => this.reloadModelRuntime(),
      getConfig: () => this.getConfig(),
      getProviderDiscovery: () => this.getProviderDiscovery(),
      discoverProviderModels: (id, input) => this.discoverProviderModels(id, input),
      reconcileDefaultModel: () => this.reconcileDefaultModel(),
      providerState: this.providerState,
    })
  }

  async init({ startupObserver = null } = {}) {
    // 初始化按阶段推进：阶段回调仅用于诊断打点，绝不能改变初始化行为。
    const stage = (name) => {
      try {
        startupObserver?.(name)
      } catch {
        // Diagnostics are best-effort and cannot change initialization behavior.
      }
    }

    stage('filesystem')
    // 先建目录与清理旧数据，保证后续文件读写路径都存在。
    await mkdir(this.sessionDir, { recursive: true })
    await mkdir(this.assetsDir, { recursive: true })
    await cleanupRemovedLocalEmbeddingData(this.dataDir)

    stage('session-state')
    // 从磁盘加载会话元数据/用量/资产索引，并执行老版本数据迁移。
    this.sessionMeta = await readJson(this.sessionMetaPath, {})
    await this.migrateLegacyDefaultWorkspaces()
    await this.migrateSessionExecutionModes()
    this.usageLedger = await readJson(this.usagePath, { days: {}, sessionScans: {} })
    this.usageLedger.days ||= {}
    this.usageLedger.sessionScans ||= {}
    this.assetIndex = await readJson(this.assetIndexPath, { assets: [] })
    this.assetIndex.assets = Array.isArray(this.assetIndex.assets) ? this.assetIndex.assets : []
    this.startAssetReconciliation()
    const appConfig = await readJson(this.appConfigPath, {})
    this.compactionThresholdPercent = normalizeCompactionThresholdPercent(
      appConfig.compactionThresholdPercent,
    )
    this.memoryAutoApproveConfidence = normalizeMemoryAutoApproveConfidence(
      appConfig.memoryAutoApproveConfidence,
    )

    stage('providers')
    // 兼容历史 KimiCode Provider 配置，再加载模型目录/元数据。
    await migrateKimiCodeProvider({
      authPath: this.authPath,
      modelsPath: this.modelsPath,
      settingsPath: this.settingsPath,
      appConfigPath: this.appConfigPath,
    })
    await Promise.all([this.providerModelCatalog.init(), this.modelMetadata.init()])
    this.settingsManager = SettingsManager.create(this.cwd, this.dataDir, {
      projectTrusted: this.workspaceTrust.isTrusted(this.cwd),
    })

    stage('skills')
    await this.skills.init()
    stage('mcp')
    await this.mcp.init()
    stage('default-tools')
    await this.initializeToolPlugins()

    stage('model-runtime')
    // 模型运行时就绪后对齐默认模型，再初始化记忆（记忆的语义摘要依赖模型）。
    await this.reloadModelRuntime()
    await this.reconcileDefaultModel()
    stage('memory')
    await this.memory.init()
    this.memory.setSemanticSummarizer(this.memorySummarizer)

    stage('automation-services')
    // 自动化服务（目标/计划/多 Agent/渠道/工作流/定时任务）逐个就绪；
    // 目标以暂停态初始化，避免启动即恢复历史未完成任务。
    await this.goals.init({ pauseActive: true })
    await this.plans.init()
    await this.multiAgents.init()
    await this.channels.init()
    await this.workflows.init()
    await this.schedules.init()
    this.startSessionRuntimeSweeper()
    void this.refreshProviderModels().catch(() => {})
    stage('complete')
  }

  async reloadModelRuntime() {
    // 委托给 ProviderPreferences：统一处理 Provider 鉴权与模型运行时装配。
    return this.providerPreferences.reload()
  }

  // 目标状态变化时：更新实时状态、失效投影缓存，并通知前端（goal_update 事件）。
  emitGoalUpdate(sessionId, goal, send = this.goalEmitters.get(sessionId)) {
    const live = this.liveSessions.get(sessionId)
    if (live) live.goal = goal || null
    this.streamProjection.invalidate(sessionId, { transcript: false, activity: true, usage: false })
    try {
      send?.('goal_update', { sessionId, goal: goal || null })
    } catch {}
  }

  // 计划更新：对比新旧计划生成变更摘要（livePlanChanges）供前端增量展示。
  emitPlanUpdate(sessionId, plan, send = this.planEmitters.get(sessionId)) {
    const live = this.liveSessions.get(sessionId)
    const nextPlan = plan || this.plans.get(sessionId)
    const updatedAt = nextPlan?.updatedAt || new Date().toISOString()
    const currentActivity = {
      type: 'plan',
      plan: nextPlan,
      changes: livePlanChanges(live?.plan, nextPlan),
      updatedAt,
    }
    if (live) {
      live.plan = nextPlan
      setLiveActivity(live, currentActivity)
    }
    this.streamProjection.invalidate(sessionId, { transcript: false, activity: true, usage: false })
    try {
      send?.('plan_update', { sessionId, plan: nextPlan, currentActivity })
    } catch {}
  }

  // Agent 状态更新：把多 Agent 摘要同步进实时状态，并推送给前端活动流。
  emitAgentUpdate(sessionId, agent, send = this.agentEmitters.get(sessionId)) {
    const allAgents = this.multiAgents.summaries(sessionId)
    const updatedAgent = allAgents.find((item) => item.id === agent?.id) || null
    const agents = allAgents.filter((item) =>
      ['queued', 'starting', 'running'].includes(item.status),
    )
    const live = this.liveSessions.get(sessionId)
    const currentActivity = updatedAgent
      ? {
          type: 'agent',
          agent: updatedAgent,
          updatedAt: updatedAgent.lastActivityAt || new Date().toISOString(),
        }
      : live?.currentActivity || null
    if (live) {
      live.agents = agents
      if (updatedAgent) {
        live.activityFeed = pushLiveActivity(live.activityFeed, currentActivity)
        if (live.currentActivity?.type !== 'tool') live.currentActivity = currentActivity
      }
    }
    this.streamProjection.invalidate(sessionId, { transcript: false, activity: true, usage: false })
    try {
      send?.('agent_update', { sessionId, agent: updatedAgent, agents, currentActivity })
    } catch {}
  }

  getSessionExecutionMode(sessionId) {
    return normalizeExecutionMode(
      this.sessionMeta[sessionId]?.executionMode,
      DEFAULT_EXECUTION_MODE,
    )
  }

  // 列出磁盘上的全部会话：带缓存与并发去重（同一次扫描只跑一遍）。
  listStoredSessions({ refresh = false } = {}) {
    if (refresh) {
      this.storedSessionsCache = null
      this.storedSessionsPromise = null
    }
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

  upsertStoredSession(info) {
    // 只做增量插入，避免新建/物化单个会话时全量重扫所有会话文件。
    upsertStoredSessionCache(this.storedSessionsCache, info)
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

  // 会话执行模式/权限模式的历史迁移：老版本可能缺失或使用已废弃的执行模式。
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
    return this.toolActivation.optionalToolNames(value)
  }

  syncGoalTools(value, goal) {
    return this.toolActivation.syncGoalTools(value, goal)
  }

  async promoteSessionTools(value, toolNames = []) {
    return this.toolActivation.promoteSessionTools(value, toolNames)
  }

  async selectToolsForMessage(value, message, options = {}) {
    if (this.toolActivation) {
      return this.toolActivation.selectToolsForMessage(value, message, options)
    }
    const requested = [
      ...new Set(
        (Array.isArray(options.requestedToolNames) ? options.requestedToolNames : [])
          .map((name) => String(name || '').trim())
          .filter(Boolean),
      ),
    ]
    value.requestedToolNames = options.preserveRequested
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
    return this.sessionLifecycle.touchSessionRuntime(value)
  }

  sessionRunIsActive(id, value = this.sessions.get(id)) {
    return this.sessionLifecycle.sessionRunIsActive(id, value)
  }

  sessionRuntimeIsProtected(id, value) {
    return this.sessionLifecycle.sessionRuntimeIsProtected(id, value)
  }

  disposeSessionRuntime(id, value, options) {
    return this.sessionLifecycle.disposeSessionRuntime(id, value, options)
  }

  evictIdleSessionRuntimes(exceptId = '', now = Date.now()) {
    return this.sessionLifecycle.evictIdleSessionRuntimes(exceptId, now)
  }

  startSessionRuntimeSweeper() {
    if (this.sessionRuntimeSweepTimer) return
    const intervalMs = Math.max(
      1_000,
      Number(this.sessionRuntimeSweepIntervalMs) || SESSION_RUNTIME_SWEEP_INTERVAL_MS,
    )
    this.sessionRuntimeSweepTimer = setInterval(() => {
      try {
        this.evictIdleSessionRuntimes()
      } catch {}
    }, intervalMs)
    this.sessionRuntimeSweepTimer.unref?.()
  }

  getRuntimeDiagnostics() {
    return {
      ...this.sessionLifecycle.getRuntimeDiagnostics(),
      projectionCache: this.streamProjection.cache.stats(),
    }
  }

  async sessionWorkspaceCwd(id) {
    return this.sessionLifecycle.sessionWorkspaceCwd(id)
  }

  async disposeSessions() {
    return this.sessionLifecycle.disposeSessions()
  }

  invalidateSessionRuntimes() {
    return this.sessionLifecycle.invalidateSessionRuntimes()
  }

  // 会话元数据持久化：写盘串行化（链式 Promise），防止并发写覆盖。
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

  // 记录单条用量：同一 key 只记一次（幂等）；历史超过 45 天的按天清理。
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

  // 增量扫描会话历史文件中的用量：从上次扫描位置继续读，按换行切分逐条解析，
  // 只在会话文件追加场景下工作，避免每次都全量重读。
  async scanSessionUsage(info, day) {
    const file = await stat(info.path)
    const scans = this.usageLedger.sessionScans
    const previous = scans[info.id]
    const records = this.usageLedger.days[day]?.records || {}
    const hasRecordedSessionUsage = Object.keys(records).some((key) =>
      key.startsWith(`session:${info.id}:`),
    )
    if (!previous && hasRecordedSessionUsage) {
      scans[info.id] = { path: info.path, size: file.size }
      return true
    }

    let offset =
      previous?.path === info.path && file.size >= Number(previous.size || 0)
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
        const combined = remainder.length
          ? Buffer.concat([remainder, chunk.subarray(0, bytesRead)])
          : chunk.subarray(0, bytesRead)
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
            if (
              entry.type !== 'message' ||
              entry.message?.role !== 'assistant' ||
              !entry.message.usage
            )
              continue
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

  // 汇总今日用量：先增量扫描今日有改动的会话，再聚合各记录。
  async getTodayUsage() {
    const day = localDayKey()
    const sessions = await this.listStoredSessions()
    let changed = false
    for (const info of sessions) {
      if (localDayKey(info.modified) !== day) continue
      if (await this.scanSessionUsage(info, day)) changed = true
    }
    if (changed) await this.saveUsageLedger()
    const totals = {
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      reasoning: 0,
      totalTokens: 0,
    }
    for (const usage of Object.values(this.usageLedger.days[day]?.records || {}))
      addUsage(totals, usage)
    return { day, ...totals }
  }

  // 资产索引保存：版本号递增并失效资产投影缓存，写盘同样串行化。
  saveAssetIndex() {
    this.assetProjectionRevision += 1
    this.streamProjection.invalidateAssets()
    const snapshot = structuredClone(this.assetIndex)
    this.assetWrite = this.assetWrite
      .catch(() => {})
      .then(() => writeJsonAtomic(this.assetIndexPath, snapshot))
    return this.assetWrite
  }

  // 启动时对账资产索引：删除磁盘上已不存在的孤儿资产记录。
  startAssetReconciliation() {
    this.assetReconcile = Promise.resolve()
      .then(() => {
        const assets = structuredClone(this.assetIndex.assets)
        return assetStorage.reconcileAssetIndex({
          assets,
          assetsDir: this.assetsDir,
          save: async () => {
            this.assetIndex.assets = assets
            await this.saveAssetIndex()
          },
        })
      })
      .catch(() => false)
    return this.assetReconcile
  }

  publicAsset(asset) {
    return assetStorage.publicAsset(asset)
  }

  // 创建资产（上传文件或链接）：链接去重后直接入库；文件走 base64 存储。
  async createAsset(input) {
    await this.assetReconcile
    const now = new Date().toISOString()
    const source = String(input.source || 'upload')
    if (input.kind === 'link' || input.url) {
      const url = new URL(String(input.url || ''))
      if (!['http:', 'https:'].includes(url.protocol)) throw new Error('链接只支持 http 或 https。')
      const existing = this.assetIndex.assets.find(
        (asset) => asset.kind === 'link' && asset.url === url.href,
      )
      if (existing) return this.publicAsset(existing)
      const asset = {
        id: randomUUID(),
        kind: 'link',
        name: safeAttachmentName(input.name || url.hostname),
        url: url.href,
        mimeType: 'text/uri-list',
        size: 0,
        source,
        sessionId: input.sessionId || '',
        sessionName: input.sessionName || '',
        created: now,
        modified: now,
      }
      this.assetIndex.assets.unshift(asset)
      await this.saveAssetIndex()
      return this.publicAsset(asset)
    }

    const name = safeAttachmentName(input.name)
    const buffer =
      input.text !== undefined
        ? Buffer.from(String(input.text), 'utf8')
        : Buffer.from(String(input.data || ''), 'base64')
    if (!buffer.length) throw new Error(`${name} 内容为空。`)
    if (buffer.length > MAX_ASSET_BYTES) throw new Error(`${name} 超过 24 MB 资产限制。`)
    const mimeType = String(input.mimeType || mimeFromName(name))
    const kind =
      mimeType.startsWith('image/') || IMAGE_EXTENSIONS.has(extname(name).toLowerCase())
        ? 'image'
        : 'file'
    const asset = await assetStorage.storeAssetBuffer({
      assets: this.assetIndex.assets,
      assetsDir: this.assetsDir,
      buffer,
      name,
      kind,
      mimeType,
      source,
      sessionId: input.sessionId,
      sessionName: input.sessionName,
      created: now,
    })
    await this.saveAssetIndex()
    return this.publicAsset(asset)
  }

  async recordGeneratedFile(sessionId, value, filePath) {
    await this.assetReconcile
    const name = basename(filePath)
    const asset = await assetStorage.archiveGeneratedAsset({
      assets: this.assetIndex.assets,
      assetsDir: this.assetsDir,
      filePath,
      kind: IMAGE_EXTENSIONS.has(extname(name).toLowerCase()) ? 'image' : 'file',
      mimeType: mimeFromName(filePath),
      sessionId,
      sessionName: value.name,
    })
    if (!asset) return null
    await this.saveAssetIndex()
    return this.publicAsset(asset)
  }

  async listAssets({ query = '', kind = '', sessionId = '' } = {}) {
    await this.assetReconcile
    const needle = String(query || '')
      .trim()
      .toLowerCase()
    const assets = this.assetIndex.assets
      .map((asset) => (sessionId ? assetStorage.assetForSession(asset, sessionId) : asset))
      .filter(Boolean)
      .filter((asset) => {
        if (kind && asset.kind !== kind) return false
        return (
          !needle ||
          `${asset.name} ${asset.sessionName} ${asset.url || ''}`.toLowerCase().includes(needle)
        )
      })
    return assets.map((asset) => this.publicAsset(asset))
  }

  findAsset(id) {
    return this.assetIndex.assets.find((asset) => asset.id === id)
  }

  async getAssetContent(id) {
    await this.assetReconcile
    const asset = this.findAsset(id)
    if (!asset) return null
    if (asset.kind === 'link') {
      return {
        id: asset.id,
        kind: 'text',
        name: `${asset.name}.url.txt`,
        mimeType: 'text/plain',
        size: asset.url.length,
        text: `链接：${asset.url}`,
      }
    }
    const path = asset.storagePath || asset.filePath
    const buffer = await readFile(path)
    if (buffer.length > MAX_CHAT_ASSET_BYTES)
      throw new Error('资产超过 10 MB，无法直接加入对话；仍可下载或在工作目录中读取。')
    const extension = extname(asset.name).toLowerCase()
    if (asset.kind === 'image')
      return {
        id: asset.id,
        kind: 'image',
        name: asset.name,
        mimeType: asset.mimeType,
        size: buffer.length,
        data: buffer.toString('base64'),
      }
    if (ASSET_TEXT_EXTENSIONS.has(extension) || asset.mimeType.startsWith('text/')) {
      const text = buffer.toString('utf8')
      return {
        id: asset.id,
        kind: 'text',
        name: asset.name,
        mimeType: asset.mimeType,
        size: buffer.length,
        text: text.slice(0, MAX_EXTRACTED_CHARS),
        truncated: text.length > MAX_EXTRACTED_CHARS,
      }
    }
    if (ASSET_DOCUMENT_EXTENSIONS.has(extension))
      return {
        id: asset.id,
        kind: 'document',
        name: asset.name,
        mimeType: asset.mimeType,
        extension: extension.slice(1),
        size: buffer.length,
        data: buffer.toString('base64'),
      }
    return {
      id: asset.id,
      kind: 'text',
      name: `${asset.name}.path.txt`,
      mimeType: 'text/plain',
      size: path.length,
      text: asset.filePath
        ? `本地文件路径：${asset.filePath}`
        : `资产 ${asset.name} 是二进制文件，请结合文件名称和元数据分析。`,
    }
  }

  async getAssetDownload(id) {
    await this.assetReconcile
    const asset = this.findAsset(id)
    if (!asset || asset.kind === 'link') return null
    return {
      asset: this.publicAsset(asset),
      buffer: await readFile(asset.storagePath || asset.filePath),
    }
  }

  async deleteAsset(id) {
    await this.assetReconcile
    const index = this.assetIndex.assets.findIndex((asset) => asset.id === id)
    if (index < 0) return false
    const [asset] = this.assetIndex.assets.splice(index, 1)
    if (asset.storagePath) {
      const root = resolve(this.assetsDir)
      const target = resolve(asset.storagePath)
      if (target !== root && target.startsWith(`${root}${sep}`))
        await unlink(target).catch(() => {})
    }
    await this.saveAssetIndex()
    return true
  }

  async getOrCreateSession(id) {
    return this.sessionLifecycle.getOrCreateSession(id)
  }

  // 创建会话运行时（resident runtime）：为会话装配模型、工具、网关、权限与目标/计划/多 Agent 工具，
  // 并返回可运行的会话对象。这是“打开一个会话”的核心路径。
  async createSessionRuntime(sessionManager, name) {
    // 首选模型解析：会话历史 > 元数据；无鉴权/不可用的模型会回退到默认模型。
    const settings = this.settingsManager.getGlobalSettings()
    const runtimeSessionId = sessionManager.getSessionId()
    const preferredModelRef = resolveSessionModelRef(
      sessionManager,
      this.sessionMeta[runtimeSessionId],
    )
    await this.modelMetadata.ensure(preferredModelRef?.modelId || settings.defaultModel)
    const preferredModel =
      preferredModelRef &&
      this.modelRuntime?.getModel?.(preferredModelRef.provider, preferredModelRef.modelId)
    const preferredModelHasAuth =
      !preferredModel ||
      !this.modelRuntime?.hasConfiguredAuth ||
      this.modelRuntime.hasConfiguredAuth(preferredModel.provider)
    const sessionModel = preferredModel && preferredModelHasAuth ? preferredModel : undefined
    const appConfig = await readJson(this.appConfigPath, { toolMode: 'full' })
    const effectiveCwd = await resolveDirectory(
      this.sessionMeta[runtimeSessionId]?.cwd,
      sessionManager.getCwd() || this.cwd,
    )
    const executionMode = this.getSessionExecutionMode(runtimeSessionId)
    // 并行准备资源加载器与 MCP 工具定义；MCP 工具按名称排序保证工具列表稳定（利于缓存命中）。
    const enabledTools = this.toolPlugins.enabledTools(appConfig, executionMode)
    const [resourceLoader, mcpTools] = await Promise.all([
      this.skills.createResourceLoader(effectiveCwd),
      this.mcp.createToolDefinitions(),
    ])
    const stableMcpTools = [...mcpTools].sort((left, right) =>
      String(left.name || '').localeCompare(String(right.name || '')),
    )
    const pluginTools = this.toolPlugins.createToolDefinitions({
      cwd: effectiveCwd,
      sessionId: runtimeSessionId,
      enabledTools,
    })
    const baseToolNames = [
      ...new Set([
        ...enabledTools,
        ...stableMcpTools.map((tool) => tool.name),
        ...pluginTools.map((tool) => tool.name),
      ]),
    ]
    const promotedToolNames = mergePromotedToolNames({
      availableToolNames: [...baseToolNames, ...MULTI_AGENT_TOOL_NAMES],
      promotedToolNames: this.sessionMeta[runtimeSessionId]?.promotedToolNames || [],
    })
    let runtimeValue = null
    let runtimeSession = null
    const goalTools = schemaOnlyToolDefinitions(
      createGoalTools({
        getGoal: () => this.goals.get(runtimeSessionId),
        completeGoal: async () => {
          const goal = await this.goals.complete(runtimeSessionId)
          if (runtimeValue) this.syncGoalTools(runtimeValue, goal)
          this.emitGoalUpdate(runtimeSessionId, goal)
          return goal
        },
      }),
    )
    const planTools = createPlanTools({
      getPlan: () => this.plans.get(runtimeSessionId),
      updatePlan: async (items) => {
        const plan = await this.plans.replace(runtimeSessionId, items)
        this.emitPlanUpdate(runtimeSessionId, plan)
        return plan
      },
    })
    const planReader = planTools.find((tool) => tool.name === 'get_plan')
    // Pi 引擎惰性持久化会话文件（首条助手消息时才写盘）。若对话在模型回复前被打断，
    // 磁盘上将没有文件；这里强制先落盘，保证会话在常驻运行时被释放后仍可寻址/恢复。
    await ensureSessionFilePersisted(sessionManager, name, effectiveCwd)
    const installSubagentPermissions = (subagentSession) =>
      this.permissions.install(subagentSession, {
        sessionId: runtimeSession.sessionId,
        cwd: effectiveCwd,
      })
    // 子 Agent 用量归账：写入用量账本，并同步到目标预算。
    const accountSubagentUsage = async ({ id, runNumber, runUsage, completedAt }) => {
      await this.recordUsage(
        localDayKey(completedAt),
        `agent:${runtimeSession.sessionId}:${id}:${runNumber}`,
        runUsage,
      )
      const goal = this.goals.get(runtimeSession.sessionId)
      if (goal?.status !== 'active') return
      const accounting = this.goals.account(runtimeSession.sessionId, {
        goalId: goal.id,
        usage: runUsage,
      })
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
        (toolName) => this.getToolRisk(toolName),
      )
    }
    // 多 Agent 运行时适配：把 MultiAgentService 包装成 Pi 会话可调用的工具接口。
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
          createCustomTools: async () => {
            const childBashTool =
              enabledTools.includes('bash') &&
              ['approval-required', 'workspace-write', 'full-access'].includes(executionMode)
                ? await createPisperBashTool(effectiveCwd)
                : null
            return {
              tools: [
                ...createInheritedCustomTools(childBashTool),
                ...(planReader ? [planReader] : []),
              ],
            }
          },
          onProgress: (agent) => this.emitAgentUpdate(runtimeSession.sessionId, agent),
          onSession: installSubagentPermissions,
          onCompleted: accountSubagentUsage,
        })
      },
      list: () => this.multiAgents.list(runtimeSession.sessionId),
      sendMessage: (target, message) =>
        this.multiAgents.sendMessage(runtimeSession.sessionId, target, message),
      followup: (target, message) =>
        this.multiAgents.followup(runtimeSession.sessionId, target, message),
      wait: (timeoutMs, target) =>
        waitForAgentMailbox(this.multiAgents, runtimeSession.sessionId, timeoutMs, target),
      interrupt: (target) => this.multiAgents.interrupt(runtimeSession.sessionId, target),
    }
    const multiAgentTools = schemaOnlyToolDefinitions(createMultiAgentTools({ multiAgentRuntime }))
    const toolDiscovery = createToolDiscoveryTool({
      listTools: () => {
        if (!runtimeValue || !runtimeSession) return []
        const optionalToolNames = filterToolsForExecutionMode(
          this.optionalToolNames(runtimeValue),
          this.getSessionExecutionMode(runtimeSession.sessionId),
          (toolName) => this.getToolRisk(toolName),
        )
        const staticHotToolNames = new Set(hotToolNames(optionalToolNames))
        const activeToolNames = new Set(runtimeSession.getActiveToolNames())
        return optionalToolNames
          .filter(
            (name) =>
              !staticHotToolNames.has(name) && !PLAN_COMPATIBILITY_TOOL_NAMES.includes(name),
          )
          .map((name) => {
            const definition = runtimeSession.getToolDefinition(name)
            if (!definition) return null
            const description =
              String(definition.description || '')
                .split('\n')
                .map((line) => line.trim())
                .find(Boolean) || ''
            return {
              name,
              label: definition.label || name,
              description: description.slice(0, 320),
              active: activeToolNames.has(name),
              required: Array.isArray(definition.parameters?.required)
                ? definition.parameters.required
                : [],
            }
          })
          .filter(Boolean)
      },
    })
    const bashTool =
      enabledTools.includes('bash') &&
      ['approval-required', 'workspace-write', 'full-access'].includes(executionMode)
        ? await createPisperBashTool(effectiveCwd)
        : null
    const createInheritedCustomTools = (inheritedBashTool = bashTool) => [
      ...schemaOnlyToolDefinitions(
        createAppTools({
          cwd: effectiveCwd,
          enabledTools,
          memoryRuntime: this.memory,
          getUserMessage: () => runtimeValue?.pendingUserMessage || '',
          webSearchService: this.webSearch,
          browserAutomationService: this.browserAutomation,
          browserSessionId: runtimeSessionId,
          visualGenerationService: this.visualGeneration,
          skillsRuntime: this.skills,
          onSkillsChanged: () => this.invalidateSessionRuntimes(),
          pluginRuntime: this.toolPlugins,
          executionMode,
          onPluginsChanged: () => this.invalidateSessionRuntimes(),
          onGeneratedFile: ({ path }) =>
            runtimeValue && runtimeSession
              ? this.recordGeneratedFile(runtimeSession.sessionId, runtimeValue, path)
              : undefined,
          mcpRuntime: {
            list: (options) => this.getMcpDashboard(options),
            add: (input) => this.createMcpServer(input),
            update: (id, input) => this.updateMcpServer(id, input),
            remove: (id) => this.deleteMcpServer(id),
            test: (id, options) => this.mcp.test(id, options),
            setToolEnabled: (id, toolName, nextEnabled) =>
              this.setMcpToolEnabled(id, toolName, nextEnabled),
          },
        }),
      ),
      ...schemaOnlyToolDefinitions(stableMcpTools),
      ...schemaOnlyToolDefinitions(pluginTools),
      ...(inheritedBashTool ? [inheritedBashTool] : []),
    ]
    const inheritedCustomTools = createInheritedCustomTools()
    const callableTools = new Map(inheritedCustomTools.map((tool) => [tool.name, tool]))
    // 工具网关：集中管控工具调用（按执行模式过滤 + 权限审批），
    // 让 Pi 会话通过单一 tool_gateway 工具间接调用应用工具。
    const toolGateway = createRuntimeToolGateway({
      tools: callableTools,
      getExecutionMode: (sessionId) => this.getSessionExecutionMode(sessionId),
      getToolRisk: (name) => this.getToolRisk(name),
      authorize: (input) => this.permissions.authorize({ cwd: effectiveCwd, ...input }),
      sessionId: runtimeSessionId,
    })
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
      ...(sessionModel ? { model: sessionModel } : {}),
      tools: [
        ...baseToolNames,
        TOOL_DISCOVERY_NAME,
        TOOL_GATEWAY_NAME,
        ...GOAL_TOOL_NAMES,
        ...PLAN_ALL_TOOL_NAMES,
        ...MULTI_AGENT_TOOL_NAMES,
      ],
      customTools: [
        await createCompressedReadTool(effectiveCwd),
        ...inheritedCustomTools,
        toolDiscovery,
        toolGateway,
        ...goalTools,
        ...planTools,
        ...multiAgentTools,
      ],
    })
    installTransientStreamRetry(session)
    installTurnBoundaryCompaction(session)
    const now = new Date().toISOString()
    // 会话运行时值：缓存会话快照，供后续消息/查询直接使用，避免重复装配。
    const value = {
      session,
      modelFallbackMessage,
      name: name || sessionManager.getSessionName() || DEFAULT_SESSION_NAME,
      created: now,
      modified: now,
      cwd: effectiveCwd,
      baseToolNames,
      enabledTools,
      mcpTools: stableMcpTools.map((tool) => ({ name: tool.name, label: tool.label || '' })),
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
        this.sessionMeta[session.sessionId] = {
          ...(this.sessionMeta[session.sessionId] || {}),
          model,
        }
        void this.saveSessionMeta()
      }
    }
    this.syncGoalTools(value, this.goals.get(session.sessionId))
    this.permissions.install(session, { sessionId: session.sessionId, cwd: effectiveCwd })
    applyPisperSystemPrompt(session, session.model)
    // 捕获提示词缓存形态：对比形状变化以诊断 prompt cache 失效原因。
    value.promptCache = capturePromptCacheShape({
      systemPrompt: session.agent.state.systemPrompt,
      tools: promptCacheTools(session),
      runtime: promptCacheRuntime(session),
    })
    this.sessions.set(session.sessionId, value)
    this.streamProjection.invalidate(session.sessionId)
    this.evictIdleSessionRuntimes(session.sessionId)
    return value
  }

  // 整理消息附件：归档附件并转成模型可理解的上下文片段（文本/图片/文档/本地路径）。
  async preparePromptAttachments(value, attachments = []) {
    const safeAttachments = Array.isArray(attachments) ? attachments.slice(0, 8) : []
    const archivedAttachments = await this.archiveAttachments(
      value.session.sessionId,
      value.name,
      safeAttachments,
    )
    const images = []
    const contexts = []
    for (const [attachmentIndex, attachment] of safeAttachments.entries()) {
      const name = safeAttachmentName(attachment.name)
      if (attachment.kind === 'path') {
        const path = normalizeWorkspacePath(attachment.path)
        if (!path || !isAbsolute(path)) throw new Error(`${name} 不是有效的本地绝对路径`)
        contexts.push(
          `[Local path attachment] ${name}\nPath: ${path}\nThis attachment is a path reference only. Read it with available workspace tools when needed.`,
        )
      } else if (attachment.kind === 'image') {
        const data = String(attachment.data || '')
        const mimeType = String(attachment.mimeType || '')
        if (!mimeType.startsWith('image/') || !data) throw new Error(`${name} 不是有效图片`)
        if (data.length > 15_000_000) throw new Error(`${name} 图片数据过大`)
        images.push({ type: 'image', data, mimeType })
        const localPath = archivedAttachments[attachmentIndex]?.path
        contexts.push(
          `[Image attachment] ${name}${localPath ? `\nLocal path: ${localPath}\nTo edit this image, pass this path in generate_visual sourceImages.` : ''}`,
        )
      } else if (attachment.kind === 'text') {
        const text = String(attachment.text || '').slice(0, MAX_EXTRACTED_CHARS)
        contexts.push(
          `[Text attachment: ${name}]\n${text}${attachment.truncated ? '\n(Content truncated)' : ''}`,
        )
      } else if (attachment.kind === 'document') {
        const text = await extractDocumentText(attachment)
        contexts.push(`[Document attachment: ${name}]\n${text}`)
      }
    }
    return { images, contexts }
  }

  // 向运行中的会话追加消息（steer/followUp）：会话必须正在流式运行，
  // 否则走新的 runSessionPrompt 路径。
  async queueSessionMessage(id, { message, attachments = [], behavior = 'steer' } = {}) {
    const value = this.sessions.get(id)
    if (!value) throw new Error('会话不存在或尚未加载。')
    const text = String(message || '').trim()
    if (!text && !attachments.length) throw new Error('消息不能为空。')
    if (text.length > 12_000) throw new Error('运行中追加消息不能超过 12000 个字符。')
    if (!value.session.isStreaming) throw new Error('当前会话已经结束运行，请作为新消息发送。')
    const streamingBehavior = behavior === 'followUp' ? 'followUp' : 'steer'
    const displayText = text || '请分析这些附件。'
    const prepared = await this.preparePromptAttachments(value, attachments)
    const prompt = prepared.contexts.length
      ? `${displayText}${ATTACHMENT_MARKER}${prepared.contexts.join('\n\n')}`
      : displayText
    await this.selectToolsForMessage(value, displayText, { preserveRequested: true })
    value.pendingUserMessage = displayText
    await value.session.prompt(prompt, {
      images: prepared.images,
      streamingBehavior,
      source: 'interactive',
    })
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
    if (this.sessionRunIsActive(sessionId, value)) {
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
    if (this.sessionRunIsActive(sessionId, value)) {
      void value.session.steer(message).catch(() => {})
      return
    }
    // 服务端自动唤醒：以内部消息发起一轮，让大模型立即感知完成结果。
    // 消息以 AGENT_COMPLETION_MARKER 开头，对用户隐藏；产生的助手回复正常显示。
    await this.streamPrompt({ sessionId, message, send: () => {} })
  }

  // 手动发起一轮会话运行：构造实时状态（live），订阅会话事件并桥接为
  // 前端 SSE 事件（文本增量、工具执行、压缩、用量、目标/计划/Agent 等）。
  async runSessionPrompt(
    value,
    {
      sessionId,
      message,
      attachments = [],
      requestedToolNames = [],
      goalMode = false,
      goalTokenBudget = null,
      isolatedContext = false,
      send,
    },
  ) {
    const emit = (event, data) => {
      this.streamProjection.invalidate(data?.sessionId || sessionId || '')
      send(event, data)
      try {
        this.eventObserver?.({ event, data, sessionId: data?.sessionId || sessionId || '' })
      } catch {
        // Desktop observers are best-effort and must never interrupt an Agent stream.
      }
    }
    if (isolatedContext) {
      value.isolatedContext = true
      value.blockedToolNames = ISOLATED_CONTEXT_BLOCKED_TOOLS
    }
    const { session } = value
    const appConfig = await readJson(this.appConfigPath, {
      toolMode: 'full',
      disabledProviders: [],
    })
    // 前置校验：停用的 Provider、不可用模型都直接拒绝，避免发起注定失败的调用。
    if ((appConfig.disabledProviders || []).includes(session.model?.provider)) {
      throw new Error('当前会话使用的 Provider 已停用，请先启用或切换模型。')
    }
    if (!session.model || session.model.provider === 'unknown' || session.model.id === 'unknown') {
      throw new Error('没有可用模型，请先在配置页设置 Provider、模型和 API Key。')
    }
    let goal = this.goals.get(session.sessionId)
    // 目标模式：暂停态恢复或新开目标；预算可在恢复时更新。
    if (goalMode) {
      if (goal?.status === 'paused') {
        if (goalTokenBudget != null) await this.goals.setBudget(session.sessionId, goalTokenBudget)
        goal = await this.goals.resume(session.sessionId)
      } else {
        goal = await this.goals.start(session.sessionId, {
          objective: message,
          tokenBudget: goalTokenBudget ?? undefined,
        })
      }
    }
    await this.selectToolsForMessage(value, message, { requestedToolNames })
    value.promptCache = comparePromptCacheShapes(
      value.promptCache,
      capturePromptCacheShape({
        systemPrompt: session.agent.state.systemPrompt,
        tools: promptCacheTools(session),
        runtime: promptCacheRuntime(session),
      }),
    )
    value.pendingUserMessage = String(message || '')
    // 上一轮的陈旧计划在无目标驱动时清空，避免残留计划误导本轮行为。
    // 内部唤醒轮（Agent 完成/目标延续）保留计划。
    const keepPlan =
      goal?.status === 'active' ||
      isGoalContinuationMessage(message) ||
      isAgentCompletionMessage(message)
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
    const live = createLiveRunState({
      startedAt,
      goal,
      plan: this.plans.get(session.sessionId),
      agents: this.multiAgents
        .summaries(session.sessionId)
        .filter((agent) => ['queued', 'starting', 'running'].includes(agent.status)),
      queuedInputs: queuedSessionInputs(session),
      contextUsage: this.compactionAwareContextUsage(session),
      sessionUsage: await this.streamProjection.getSessionTokenUsage(session.sessionId),
      promptCache: value.promptCache,
    })
    this.liveSessions.set(session.sessionId, live)
    this.streamProjection.invalidate(session.sessionId)
    this.goalEmitters.set(session.sessionId, emit)
    this.planEmitters.set(session.sessionId, emit)
    this.agentEmitters.set(session.sessionId, emit)
    // 首条用户消息自动命名（用户手动改过标题则跳过）。
    const firstTurn = !session.messages.some((item) => item.role === 'user')
    const sessionMeta = this.sessionMeta[session.sessionId]
    const mayAutoTitle = firstTurn && !sessionMeta?.manual
    const firstMessageTitle = mayAutoTitle ? sessionTitleFromFirstMessage(message, attachments) : ''
    if (firstMessageTitle && firstMessageTitle !== value.name) {
      session.setSessionName(firstMessageTitle)
      value.name = firstMessageTitle
      await this.markSessionTitle(session.sessionId, firstMessageTitle, false)
      emit('session_title', {
        sessionId: session.sessionId,
        name: firstMessageTitle,
        source: 'first_message',
      })
    }

    emit('meta', {
      sessionId: session.sessionId,
      model: `${session.model.provider}/${session.model.id}`,
      thinkingLevel: session.thinkingLevel,
      cwd: value.cwd,
      permissionMode:
        this.sessionMeta[session.sessionId]?.permissionMode ||
        permissionModeForExecutionMode(this.getSessionExecutionMode(session.sessionId)),
      executionMode: this.getSessionExecutionMode(session.sessionId),
      goal,
      plan: live.plan,
      agents: live.agents,
      currentActivity: live.currentActivity,
      activityFeed: live.activityFeed,
      lifecycle: live.lifecycle,
      sessionTreeRevision: live.sessionTreeRevision || 0,
      thinkingText: live.thinkingText,
      queuedInputs: live.queuedInputs,
      contextUsage: live.contextUsage,
      sessionUsage: live.sessionUsage,
      startedAt: live.startedAt,
      lastActivityAt: live.lastActivityAt,
    })
    let goalTurnId = '',
      goalTurnStartedAt = 0,
      continuationQueued = false,
      budgetSummaryQueued = false
    let thinkingPrefix = '',
      thinkingTurnText = ''
    // 流式文本/思考分块记账：多个并发内容块（如并行工具调用后的多段文本）需要独立跟踪。
    const activeTextBlocks = new Set(),
      activeThinkingBlocks = new Set()
    // 思考文本以“增量补丁 + 行尾裁剪”的方式同步，减少前端渲染压力。
    const streamBlockIndex = (update) =>
      Number.isInteger(update?.contentIndex) ? update.contentIndex : 0
    const appendThinking = (delta) => {
      thinkingTurnText = liveThinkingTail(thinkingTurnText + String(delta || ''))
      const next = liveThinkingTail([thinkingPrefix, thinkingTurnText].filter(Boolean).join('\n\n'))
      let start = 0
      const limit = Math.min(live.thinkingText.length, next.length)
      while (start < limit && live.thinkingText.charCodeAt(start) === next.charCodeAt(start))
        start += 1
      live.thinkingText = next
      emit('thinking_patch', { start, text: next.slice(start) })
    }
    // 收尾：标记本轮结束、修正仍在 running 的工具状态，并把后台 Agent 活动带出来。
    const finishLiveRun = (error = '') => {
      const finishedAt = new Date().toISOString()
      live.streaming = false
      live.finishedAt = finishedAt
      live.lastActivityAt = finishedAt
      live.lifecycle = finishAgentLifecycle(live.lifecycle, error, finishedAt)
      live.tools = live.tools.map((tool) =>
        tool.status === 'running'
          ? {
              ...tool,
              status: error ? 'error' : 'done',
              message: error || tool.message || '',
              updatedAt: finishedAt,
              finishedAt,
            }
          : tool,
      )
      const backgroundAgents = this.multiAgents
        .summaries(session.sessionId)
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
      bridgeAgentSessionEvent(event, live, emit)
      // 文本/思考流事件：维护分块状态并把增量转发给前端。
      if (event.type === 'message_update') {
        const update = event.assistantMessageEvent
        const blockIndex = streamBlockIndex(update)
        if (update.type === 'text_start') beginTextBlock(activeTextBlocks, blockIndex, live, emit)
        if (update.type === 'text_delta') {
          beginTextBlock(activeTextBlocks, blockIndex, live, emit)
          const delta = String(update.delta || '')
          live.text += delta
          setLiveActivity(live, {
            type: 'model',
            stage: 'responding',
            updatedAt: live.lastActivityAt,
          })
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
          setLiveActivity(live, {
            type: 'model',
            stage: 'thinking',
            updatedAt: live.lastActivityAt,
          })
        }
        if (update.type === 'thinking_end') {
          activeThinkingBlocks.delete(blockIndex)
        }
      } else if (event.type === 'compaction_start') {
        // 上下文压缩开始/结束：同步 live 状态并广播。
        live.compaction = startedCompaction(event.reason, live.lastActivityAt)
        setLiveActivity(live, {
          type: 'compaction',
          compaction: live.compaction,
          updatedAt: live.lastActivityAt,
        })
        emit('compaction_start', live.compaction)
      } else if (event.type === 'compaction_end') {
        live.compaction = finishedCompaction(live.compaction, event, live.lastActivityAt)
        live.contextUsage = this.compactionAwareContextUsage(session, live.compaction)
        emit('compaction_end', live.compaction)
        emit('context_usage', live.contextUsage)
      } else if (event.type === 'message_end') {
        if (event.message?.role === 'assistant') {
          if (event.message.usage) {
            addSessionUsage(live.sessionUsage, event.message.usage)
            emit('session_usage', live.sessionUsage)
          }
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
          ...(event.steering || [])
            .filter((text) => !isInternalParentMessage(text))
            .map((text) => ({ behavior: 'steer', text })),
          ...(event.followUp || [])
            .filter((text) => !isInternalParentMessage(text))
            .map((text) => ({ behavior: 'followUp', text })),
        ]
        emit('queue_update', { queuedInputs: live.queuedInputs })
      } else if (event.type === 'tool_execution_start') {
        // 工具执行开始：记录工具状态与活动项；bash 工具附带输出缓冲。
        activeTextBlocks.clear()
        activeThinkingBlocks.clear()
        const toolStartedAt = live.lastActivityAt
        const tool = {
          type: 'tool',
          id: event.toolCallId,
          name: event.toolName,
          args: event.args,
          status: 'running',
          startedAt: toolStartedAt,
          updatedAt: toolStartedAt,
          ...(event.toolName === 'bash' ? { output: '' } : {}),
        }
        live.tools.push(tool)
        if (live.tools.length > MAX_LIVE_ACTIVITY_ITEMS) {
          live.tools = live.tools.slice(-MAX_LIVE_ACTIVITY_ITEMS)
        }
        setLiveActivity(live, tool)
        emit('tool_start', {
          id: event.toolCallId,
          name: event.toolName,
          args: event.args,
          startedAt: toolStartedAt,
          ...(event.toolName === 'bash' ? { output: '' } : {}),
        })
      } else if (event.type === 'tool_execution_update') {
        // 工具执行中：更新输出与摘要消息；多 Agent 工具的 partialResult 带 Agent 信息。
        const rawOutput = liveThinkingTail(textFromContent(event.partialResult?.content))
        const message = rawOutput.replace(/\s+/g, ' ').trim().slice(0, 180)
        const outputPatch = event.toolName === 'bash' ? { output: rawOutput } : {}
        const agent = multiAgentResultAgent(event.toolName, event.partialResult?.details)
        live.tools = live.tools.map((item) =>
          item.id === event.toolCallId
            ? {
                ...item,
                ...outputPatch,
                message: message || item.message || '',
                updatedAt: live.lastActivityAt,
                ...(agent ? { agent } : {}),
              }
            : item,
        )
        if (live.currentActivity?.id === event.toolCallId) {
          setLiveActivity(live, {
            ...live.currentActivity,
            ...outputPatch,
            message: message || live.currentActivity.message || '',
            updatedAt: live.lastActivityAt,
            ...(agent ? { agent } : {}),
          })
        }
        emit('tool_update', {
          id: event.toolCallId,
          name: event.toolName,
          message,
          ...outputPatch,
          updatedAt: live.lastActivityAt,
          ...(agent ? { agent } : {}),
        })
      } else if (event.type === 'tool_execution_end') {
        // 工具执行结束：生成的图片/可视化结果登记为资产，并推送 generated_asset。
        if (
          !event.isError &&
          ['generate_visual', 'browser_automation'].includes(event.toolName) &&
          event.result?.details?.path
        ) {
          const generatedPath = resolve(event.result.details.path)
          const asset = assetStorage.findAssetByFilePath(this.assetIndex.assets, generatedPath)
          if (asset) {
            const attachment = assetMessageAttachment(asset)
            live.assets = [...live.assets.filter((item) => item.id !== attachment.id), attachment]
            emit('generated_asset', attachment)
          }
        }
        const resultOutput =
          event.toolName === 'bash' ? liveThinkingTail(textFromContent(event.result?.content)) : ''
        const resultMessage = event.isError
          ? resultOutput || textFromContent(event.result?.content) || '工具执行失败。'
          : ''
        const completedTool = live.tools.find((item) => item.id === event.toolCallId)
        const outputPatch =
          event.toolName === 'bash' ? { output: resultOutput || completedTool?.output || '' } : {}
        const resultDetails = event.result?.details
        const resultAgent = multiAgentResultAgent(event.toolName, resultDetails)
        const toolFinishedAt = live.lastActivityAt
        live.tools = live.tools.map((item) =>
          item.id === event.toolCallId
            ? {
                ...item,
                ...outputPatch,
                status: event.isError ? 'error' : 'done',
                message: resultMessage || item.message || '',
                updatedAt: toolFinishedAt,
                finishedAt: toolFinishedAt,
              }
            : item,
        )
        const finishedActivity = event.isError
          ? {
              ...(completedTool || {}),
              ...outputPatch,
              type: 'tool',
              status: 'error',
              message: resultMessage || completedTool?.message || '',
              updatedAt: toolFinishedAt,
              finishedAt: toolFinishedAt,
            }
          : resultAgent
            ? {
                type: 'agent',
                agent: resultAgent,
                updatedAt: resultAgent.lastActivityAt || toolFinishedAt,
              }
            : {
                ...(completedTool || {}),
                ...outputPatch,
                type: 'tool',
                status: 'done',
                message: completedTool?.message || '',
                updatedAt: toolFinishedAt,
                finishedAt: toolFinishedAt,
              }
        const preserveEvent =
          PLAN_ALL_TOOL_NAMES.includes(event.toolName) && live.currentActivity?.type === 'plan'
        if (event.isError || !preserveEvent)
          live.activityFeed = pushLiveActivity(live.activityFeed, finishedActivity)
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
        // 新一轮开始：重置思考文本（保留前缀），并为目标用量归账准备计时。
        activeThinkingBlocks.clear()
        thinkingPrefix = live.thinkingText
        thinkingTurnText = ''
        live.thinkingText = thinkingPrefix
        setLiveActivity(live, {
          type: 'model',
          stage: 'thinking',
          updatedAt: live.lastActivityAt,
        })
        emit('thinking_reset', { thinkingText: thinkingPrefix, updatedAt: live.lastActivityAt })
        const activeGoal = this.goals.get(session.sessionId)
        goalTurnId = activeGoal?.status === 'active' ? activeGoal.id : ''
        goalTurnStartedAt = Date.now()
      } else if (event.type === 'turn_end') {
        // 一轮结束：把本轮的 token 用量与耗时计入目标预算。
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
        // 目标模式下的多轮延续：正常结束时用延续提示再驱动一轮，直到目标完成/预算耗尽。
        if (event.willRetry) return
        const finalAssistant = [...(event.messages || [])]
          .reverse()
          .find((item) => item?.role === 'assistant')
        if (
          finalAssistant?.stopReason === 'error' ||
          finalAssistant?.stopReason === 'aborted' ||
          finalAssistant?.errorMessage
        )
          return
        const activeGoal = this.goals.get(session.sessionId)
        if (activeGoal?.status !== 'active' || continuationQueued) return
        continuationQueued = true
        void session
          .followUp(goalContinuationPrompt(activeGoal))
          .catch(() => {})
          .finally(() => {
            continuationQueued = false
          })
      } else if (event.type === 'auto_retry_start') {
        emit('retry', {
          attempt: event.attempt,
          maxAttempts: event.maxAttempts,
          message: event.errorMessage,
        })
      }
    })
    this.permissions.attachEmitter(session.sessionId, emit)
    try {
      const preparedAttachments = await this.preparePromptAttachments(value, attachments)
      const images = preparedAttachments.images
      // 组装注入上下文：请求的可选工具、记忆检索结果、活动目标延续提示、附件上下文。
      const contexts = []
      if (value.requestedToolNames?.length) {
        contexts.push(
          `[Requested optional tool]\nCall ${value.requestedToolNames.join(', ')} through call_tool for this request.`,
        )
      }
      const sharedContextEnabled = !value.isolatedContext
      const memoryContext =
        sharedContextEnabled && value.enabledTools?.includes('memory_search')
          ? await this.memory.relevantContext(message, value.cwd)
          : { text: '', memories: [] }
      if (memoryContext.text) contexts.push(memoryContext.text)
      const activeGoal = sharedContextEnabled ? this.goals.get(session.sessionId) : null
      if (activeGoal?.status === 'active') contexts.push(goalContinuationPrompt(activeGoal))
      contexts.push(...preparedAttachments.contexts)
      const prompt = contexts.length
        ? `${message}${ATTACHMENT_MARKER}${contexts.join('\n\n')}`
        : message
      applyPisperSystemPrompt(session, session.model)
      // 真正的模型调用：带中止护栏，超时未停止则强制销毁会话。
      await runPromptWithAbortGuard(value, () => session.prompt(prompt, { images }))
      const last = [...session.messages].reverse().find((item) => item.role === 'assistant')
      // 模型返回的最后一轮若带错误信息，视为本轮失败。
      if (last?.errorMessage) throw new Error(last.errorMessage)
      const assistantText = textFromContent(last?.content)
      live.text = assistantText || live.text
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
        lifecycle: live.lifecycle,
        sessionTreeRevision: live.sessionTreeRevision || 0,
        queuedInputs: live.queuedInputs,
        contextUsage: live.contextUsage,
        sessionUsage: live.sessionUsage,
        compaction: live.compaction,
        startedAt: live.startedAt,
        finishedAt,
      })
      if (!value.isolatedContext && value.enabledTools?.includes('memory_remember')) {
        // 非隔离上下文时，把本轮对话摘要写入长期记忆。
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
      // 出错路径：清空排队输入、记录错误、暂停活动目标并广播 error 事件。
      session.clearQueue?.()
      live.queuedInputs = []
      live.error = error instanceof Error ? error.message : String(error)
      const finishedAt = live.streaming ? finishLiveRun(live.error) : live.finishedAt
      live.contextUsage = this.compactionAwareContextUsage(session, live.compaction)
      if (this.goals.get(session.sessionId)?.status === 'active')
        await this.pauseSessionGoal(session.sessionId)
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
        lifecycle: live.lifecycle,
        sessionTreeRevision: live.sessionTreeRevision || 0,
        queuedInputs: live.queuedInputs,
        contextUsage: live.contextUsage,
        sessionUsage: live.sessionUsage,
        compaction: live.compaction,
        startedAt: live.startedAt,
        finishedAt,
      })
      return
    } finally {
      // 无论成败：退订事件、清理发射器，并在 60 秒后移除 live 状态（延迟是为了
      // 让前端有足够时间消费完成事件后再从实时视图回落到历史视图）。
      unsubscribe()
      this.permissions.detachEmitter(session.sessionId, emit)
      if (this.goalEmitters.get(session.sessionId) === emit)
        this.goalEmitters.delete(session.sessionId)
      if (this.planEmitters.get(session.sessionId) === emit)
        this.planEmitters.delete(session.sessionId)
      if (this.agentEmitters.get(session.sessionId) === emit)
        this.agentEmitters.delete(session.sessionId)
      if (live.streaming) finishLiveRun(live.error)
      const timer = setTimeout(() => {
        if (this.liveSessions.get(session.sessionId) === live) {
          this.liveSessions.delete(session.sessionId)
          this.streamProjection.invalidate(session.sessionId, {
            transcript: false,
            activity: true,
            usage: false,
          })
        }
      }, 60_000)
      timer.unref?.()
    }
  }
  async captureConversationMemory(input) {
    return captureConversationMemory(this, input)
  }
}

Object.assign(AgentRuntimeService.prototype, agentSessionMethods)
