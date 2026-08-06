import { mkdir, open, readFile, stat, unlink, writeFile } from 'node:fs/promises'
import { createHash, randomUUID } from 'node:crypto'
import { basename, extname, join, resolve, sep } from 'node:path'
import { createAgentSession, SessionManager, SettingsManager } from './pi-coding-agent.mjs'
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
import {
  PERMISSION_MODES,
  SessionPermissionService,
} from '../services/session-permission-service.mjs'
import { ToolPluginService } from '../services/tool-plugin-service.mjs'
import { WebSearchService } from '../services/web-search-service.mjs'
import { extractConversationMemories } from '../services/memory/conversation-memory.mjs'
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
import { PlanService } from '../services/plan-service.mjs'
import { BrowserAutomationService } from '../services/browser-automation-service.mjs'
import {
  listWorkspaceDirectories,
  normalizeWorkspacePath,
  resolveWorkspaceDirectory,
  workspacePathKey,
} from './workspace-directories.mjs'
import { assetMessageAttachment } from '../services/session-assets.mjs'
import { createAppTools, createMultiAgentTools, toolsFromConfig } from '../tools/registry.mjs'
import { createGoalTools, GOAL_TOOL_NAMES } from '../tools/app/goal.mjs'
import {
  createPlanTools,
  PLAN_ALL_TOOL_NAMES,
  PLAN_COMPATIBILITY_TOOL_NAMES,
} from '../tools/app/plan.mjs'
import { createToolDiscoveryTool, TOOL_DISCOVERY_NAME } from '../tools/app/tool-discovery.mjs'
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
import {
  DEFAULT_COMPACTION_THRESHOLD_PERCENT,
  createCompactionSettingsManager,
  installTurnBoundaryCompaction,
  normalizeCompactionThresholdPercent,
  pisperCompactionExtension,
} from './compaction-policy.mjs'
import { AgentRuntimeFacade, ISOLATED_CONTEXT_BLOCKED_TOOLS } from './agent-runtime-facade.mjs'
import { ToolActivation } from './tool-activation.mjs'
import { SessionLifecycle } from './session-lifecycle.mjs'
import { ProviderPreferences } from './provider-preferences.mjs'
import {
  MAX_LIVE_ACTIVITY_ITEMS,
  StreamProjection,
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
const ATTACHMENT_MARKER = '\n\n---\nAttachment context (injected by Pisper):\n'
const MAX_EXTRACTED_CHARS = 400_000
const MAX_ASSET_BYTES = 24 * 1024 * 1024
const MAX_CHAT_ASSET_BYTES = 10 * 1024 * 1024
const DEFAULT_SESSION_NAME = '新会话'
const MAX_SESSION_TITLE_CHARS = 20
const MAX_RESIDENT_SESSION_RUNTIMES = 3
const SESSION_RUNTIME_IDLE_TTL_MS = 5 * 60 * 1000
const SESSION_RUNTIME_SWEEP_INTERVAL_MS = 60 * 1000
const SESSION_HISTORY_READ_CHUNK_BYTES = 1024 * 1024
const MAX_SESSION_HISTORY_CACHE_ENTRIES = 4
const MAX_SESSION_HISTORY_CACHE_SOURCE_BYTES = 8 * 1024 * 1024
const MAX_SESSION_HISTORY_CACHE_ESTIMATED_BYTES = 48 * 1024 * 1024
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
const TRANSIENT_STREAM_READ_ERROR_PATTERN = /\bstream[_\s-]?read[_\s-]?error\b/i
const PISPER_STREAM_RETRY_PATCH = Symbol('pisper.stream-retry-patch')

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

function resolveSessionModelRef(sessionManager, sessionMeta = {}) {
  return storedSessionModel(sessionManager) || parseSessionModelRef(sessionMeta.model) || null
}

export function multiAgentResultAgent(toolName, details) {
  if (!MULTI_AGENT_TOOL_NAMES.includes(toolName) || !details) return null
  if (toolName === 'wait_agent') return details.agent?.id ? details.agent : null
  if (['spawn_agent', 'send_message', 'followup_task', 'interrupt_agent'].includes(toolName))
    return details.id ? details : null
  return null
}

export async function waitForAgentMailbox(multiAgents, sessionId, timeoutMs, target) {
  const result = await multiAgents.wait(sessionId, timeoutMs, target)
  if (!result.timedOut && result.agent) await multiAgents.acknowledge(sessionId, [result.agent])
  return result
}

function safeAttachmentName(name) {
  return String(name || '附件')
    .replace(/[\r\n<>]/g, '_')
    .slice(0, 180)
}

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

function temporarySessionTitle(message, attachments = []) {
  const attachmentNames = attachments
    .map((attachment) => safeAttachmentName(attachment?.name))
    .filter(Boolean)
  let title = String(message || '')
    .replace(/```[\s\S]*?```/g, '代码内容')
    .replace(/https?:\/\/\S+/g, '链接')
    .replace(
      /^[\s，,。.!！?？]*(?:请|麻烦)?(?:你)?(?:帮我|帮忙|协助|请问|能否|可以)?[\s，,。.!！?？]*/i,
      '',
    )
    .replace(/^(?:分析|查看|检查)(?:一下)?(?:这些|这个)?附件[\s，,。.!！?？]*/i, '')
    .replace(/\s+/g, ' ')
    .trim()
  if (
    (!title || /^(?:分析|查看|检查)?(?:这些|这个)?附件$/i.test(title)) &&
    attachmentNames.length
  ) {
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

export class AgentRuntimeService extends AgentRuntimeFacade {
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
        if (!this.settingsManager || workspacePathKey(skillsCwd) === workspacePathKey(this.cwd))
          return this.settingsManager
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
    this.notificationSettings = new NotificationSettingsService({
      path: this.appConfigPath,
      browserEventsPath: join(dataDir, 'pisper-browser-notifications.json'),
      channels: this.channels,
    })
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
      getMode: (sessionId) =>
        this.sessionMeta[sessionId]?.permissionMode ||
        permissionModeForExecutionMode(this.getSessionExecutionMode(sessionId)),
      getExecutionMode: (sessionId) => this.getSessionExecutionMode(sessionId),
      getToolRisk: (toolName) => this.mcp.getToolRisk(toolName),
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
      getToolRisk: (name) => this.mcp.getToolRisk(name),
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
      contextUsage: (session, compaction) => this.compactionAwareContextUsage(session, compaction),
      invalidateProjection: (id, scopes) => {
        if (scopes?.allUsage) this.streamProjection.invalidateAllUsage()
        else this.streamProjection.invalidate(id, scopes)
      },
      disposeSessions: () => this.disposeSessions(),
      reloadModelRuntime: () => this.reloadModelRuntime(),
      getConfig: () => this.getConfig(),
      getProviderDiscovery: () => this.getProviderDiscovery(),
      discoverProviderModels: (id, input) => this.discoverProviderModels(id, input),
      reconcileDefaultModel: () => this.reconcileDefaultModel(),
      providerState: this.providerState,
    })
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
    this.compactionThresholdPercent = normalizeCompactionThresholdPercent(
      appConfig.compactionThresholdPercent,
    )
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
    return this.providerPreferences.reload()
  }

  emitGoalUpdate(sessionId, goal, send = this.goalEmitters.get(sessionId)) {
    const live = this.liveSessions.get(sessionId)
    if (live) live.goal = goal || null
    this.streamProjection.invalidate(sessionId, { transcript: false, activity: true, usage: false })
    try {
      send?.('goal_update', { sessionId, goal: goal || null })
    } catch {}
  }

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

  sessionRuntimeIsProtected(id, value) {
    return this.sessionLifecycle.sessionRuntimeIsProtected(id, value)
  }

  disposeSessionRuntime(id, value) {
    return this.sessionLifecycle.disposeSessionRuntime(id, value)
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

  async sessionGitCwd(id) {
    return this.sessionWorkspaceCwd(id)
  }

  async getSessionGitChanges(id) {
    return this.gitChanges.getChanges(await this.sessionGitCwd(id))
  }

  async commitSessionGitChanges(id, message) {
    if (this.sessions.get(id)?.session.isStreaming)
      throw new Error('当前会话正在运行，请完成或停止后再提交改动。')
    return this.gitChanges.commit(await this.sessionGitCwd(id), message)
  }

  async pushSessionGitChanges(id) {
    return this.gitChanges.push(await this.sessionGitCwd(id))
  }

  async revertSessionGitChanges(id) {
    if (this.sessions.get(id)?.session.isStreaming)
      throw new Error('当前会话正在运行，请完成或停止后再撤销改动。')
    return this.gitChanges.revert(await this.sessionGitCwd(id))
  }

  async disposeSessions() {
    return this.sessionLifecycle.disposeSessions()
  }

  invalidateSessionRuntimes() {
    return this.sessionLifecycle.invalidateSessionRuntimes()
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

  saveAssetIndex() {
    this.assetProjectionRevision += 1
    this.streamProjection.invalidateAssets()
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
    const hash = createHash('sha256').update(buffer).digest('hex')
    const duplicate = this.assetIndex.assets.find(
      (asset) => asset.hash === hash && asset.name === name,
    )
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
      id,
      kind:
        mimeType.startsWith('image/') || IMAGE_EXTENSIONS.has(extname(name).toLowerCase())
          ? 'image'
          : 'file',
      name,
      mimeType,
      size: buffer.length,
      hash,
      storagePath,
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
        archived.push(
          stored ? { id: stored.id, path: stored.storagePath || stored.filePath || '' } : null,
        )
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
    const needle = String(query || '')
      .trim()
      .toLowerCase()
    const assets = this.assetIndex.assets.filter((asset) => {
      if (kind && asset.kind !== kind) return false
      if (sessionId && asset.sessionId !== sessionId) return false
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
    const asset = this.findAsset(id)
    if (!asset || asset.kind === 'link') return null
    return {
      asset: this.publicAsset(asset),
      buffer: await readFile(asset.storagePath || asset.filePath),
    }
  }

  async deleteAsset(id) {
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

  async listSessions() {
    return this.sessionLifecycle.listSessions()
  }

  async createSession(name, cwd) {
    return this.sessionLifecycle.createSession(name, cwd)
  }

  async findSessionInfo(id) {
    return this.sessionLifecycle.findSessionInfo(id)
  }

  async getSessionMessages(id) {
    return this.streamProjection.getSessionMessages(id)
  }

  trimSessionHistoryCache(protectedPath = '') {
    return this.streamProjection.trimSessionHistoryCache(protectedPath)
  }

  async readSessionHistoryEntries(path) {
    return this.streamProjection.readSessionHistoryEntries(path)
  }

  async getSessionHistoryMessages(id) {
    return this.streamProjection.getSessionHistoryMessages(id)
  }

  compactionAwareContextUsage(session, compaction = null) {
    return this.streamProjection.compactionAwareContextUsage(session, compaction)
  }

  decorateContextUsage(raw, compaction = null) {
    return this.streamProjection.decorateContextUsage(raw, compaction)
  }

  async getSessionContextUsage(id, compaction = null) {
    return this.streamProjection.getSessionContextUsage(id, compaction)
  }

  async getSessionMessagePage(id, options = {}) {
    return this.streamProjection.getSessionMessagePage(id, options)
  }

  async getSessionLive(id) {
    return this.streamProjection.getSessionLive(id)
  }

  async compactSession(id) {
    const value = await this.getOrCreateSession(id)
    const { session } = value
    const existingLive = this.liveSessions.get(session.sessionId)
    if (session.isStreaming) throw new Error('当前会话仍在运行，请等待完成后再压缩上下文。')
    if (existingLive?.compaction?.active) throw new Error('当前会话正在压缩上下文。')

    const startedAt = new Date().toISOString()
    const live = existingLive || {
      streaming: false,
      text: '',
      thinkingText: '',
      tools: [],
      assets: [],
      error: '',
      goal: this.goals.get(session.sessionId),
      plan: this.plans.get(session.sessionId),
      agents: [],
      currentActivity: null,
      activityFeed: [],
      queuedInputs: queuedSessionInputs(session),
      contextUsage: this.compactionAwareContextUsage(session),
      compaction: null,
      startedAt: null,
      finishedAt: null,
      lastActivityAt: startedAt,
    }
    live.compaction = startedCompaction('manual', startedAt)
    live.lastActivityAt = startedAt
    this.liveSessions.set(session.sessionId, live)
    this.streamProjection.invalidate(session.sessionId)

    try {
      const result = await session.compact()
      const finishedAt = new Date().toISOString()
      live.compaction = finishedCompaction(
        live.compaction,
        { reason: 'manual', result, aborted: false, willRetry: false },
        finishedAt,
      )
      live.contextUsage = this.compactionAwareContextUsage(session, live.compaction)
      live.lastActivityAt = finishedAt
      value.modified = finishedAt
      this.streamProjection.invalidate(session.sessionId, { allUsage: true })
      return { compaction: live.compaction, contextUsage: live.contextUsage }
    } catch (error) {
      const finishedAt = new Date().toISOString()
      const message = error instanceof Error ? error.message : String(error)
      live.compaction = finishedCompaction(
        live.compaction,
        {
          reason: 'manual',
          result: undefined,
          aborted: false,
          willRetry: false,
          errorMessage: `Compaction failed: ${message}`,
        },
        finishedAt,
      )
      live.lastActivityAt = finishedAt
      this.streamProjection.invalidate(session.sessionId)
      throw error
    }
  }

  async renameSession(id, name, options = {}) {
    return this.sessionLifecycle.renameSession(id, name, options)
  }

  async setSessionModel(id, provider, modelId) {
    const result = await this.providerPreferences.setSessionModel(id, provider, modelId)
    if (result?.model) {
      this.sessionMeta[id] = {
        ...(this.sessionMeta[id] || {}),
        model: result.model,
      }
      await this.saveSessionMeta()
    }
    return result
  }

  async getSessionThinkingState(id) {
    return this.providerPreferences.getSessionThinkingState(id)
  }

  async setSessionThinkingLevel(id, level) {
    return this.providerPreferences.setSessionThinkingLevel(id, level)
  }

  async setSessionPermission(id, mode) {
    return this.sessionLifecycle.setSessionPermission(id, mode, PERMISSION_MODES)
  }

  async setSessionExecutionMode(id, mode) {
    return this.sessionLifecycle.setSessionExecutionMode(id, normalizeExecutionMode(mode, ''))
  }

  resolveToolApproval(sessionId, approvalId, approved) {
    return this.permissions.resolve(sessionId, approvalId, approved)
  }

  async setSessionCwd(id, input) {
    return this.sessionLifecycle.setSessionCwd(id, input)
  }

  listDirectories(input) {
    return listWorkspaceDirectories(input, this.cwd)
  }

  async getOrCreateSession(id) {
    return this.sessionLifecycle.getOrCreateSession(id)
  }

  async createSessionRuntime(sessionManager, name) {
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
    const enabledTools = toolsFromConfig(appConfig)
    const executionMode = this.getSessionExecutionMode(runtimeSessionId)
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
    const installSubagentPermissions = (subagentSession) =>
      this.permissions.install(subagentSession, {
        sessionId: runtimeSession.sessionId,
        cwd: effectiveCwd,
      })
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
          createCustomTools: async () => {
            const childBashTool =
              enabledTools.includes('bash') && executionMode !== 'read-only'
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
          (toolName) => this.mcp.getToolRisk(toolName),
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
            }
          })
          .filter(Boolean)
      },
      activateTools: (toolNames) => this.promoteSessionTools(runtimeValue, toolNames),
    })
    const bashTool =
      enabledTools.includes('bash') && executionMode !== 'read-only'
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
      ...schemaOnlyToolDefinitions(mcpTools),
      ...(inheritedBashTool ? [inheritedBashTool] : []),
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
      ...(sessionModel ? { model: sessionModel } : {}),
      tools: [
        ...baseToolNames,
        TOOL_DISCOVERY_NAME,
        ...GOAL_TOOL_NAMES,
        ...PLAN_ALL_TOOL_NAMES,
        ...MULTI_AGENT_TOOL_NAMES,
      ],
      customTools: [
        ...createInheritedCustomTools(),
        toolDiscovery,
        ...goalTools,
        ...planTools,
        ...multiAgentTools,
      ],
    })
    installTransientStreamRetry(session)
    installTurnBoundaryCompaction(session)
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
    this.sessions.set(session.sessionId, value)
    this.streamProjection.invalidate(session.sessionId)
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

  async streamPrompt({
    sessionId,
    message,
    attachments = [],
    requestedToolNames = [],
    goalMode = false,
    goalTokenBudget = null,
    isolatedContext = false,
    send,
  }) {
    const emit = (event, data) => {
      this.streamProjection.invalidate(data?.sessionId || sessionId || '')
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
    const appConfig = await readJson(this.appConfigPath, {
      toolMode: 'full',
      disabledProviders: [],
    })
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
        goal = await this.goals.start(session.sessionId, {
          objective: message,
          tokenBudget: goalTokenBudget ?? undefined,
        })
      }
    }
    await this.selectToolsForMessage(value, message, { requestedToolNames })
    value.pendingUserMessage = String(message || '')
    // Drop stale plans from previous turns unless a Goal is actively driving multi-turn work or this is an internal wakeup turn.
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
    const initialActivity = { type: 'model', stage: 'thinking', updatedAt: startedAt }
    const live = {
      streaming: true,
      text: '',
      thinkingText: '',
      tools: [],
      assets: [],
      error: '',
      goal,
      plan: this.plans.get(session.sessionId),
      agents: this.multiAgents
        .summaries(session.sessionId)
        .filter((agent) => ['queued', 'starting', 'running'].includes(agent.status)),
      currentActivity: initialActivity,
      activityFeed: [],
      queuedInputs: queuedSessionInputs(session),
      contextUsage: this.compactionAwareContextUsage(session),
      compaction: null,
      startedAt,
      lastActivityAt: startedAt,
    }
    this.liveSessions.set(session.sessionId, live)
    this.streamProjection.invalidate(session.sessionId)
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
      emit('session_title', {
        sessionId: session.sessionId,
        name: temporaryTitle,
        source: 'temporary',
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
    const streamBlockIndex = (update) =>
      Number.isInteger(update?.contentIndex) ? update.contentIndex : 0
    const appendThinking = (delta) => {
      thinkingTurnText += String(delta || '')
      const next = liveThinkingTail([thinkingPrefix, thinkingTurnText].filter(Boolean).join('\n\n'))
      let start = 0
      const limit = Math.min(live.thinkingText.length, next.length)
      while (start < limit && live.thinkingText.charCodeAt(start) === next.charCodeAt(start))
        start += 1
      live.thinkingText = next
      emit('thinking_patch', { start, text: next.slice(start) })
    }
    const finishLiveRun = (error = '') => {
      const finishedAt = new Date().toISOString()
      live.streaming = false
      live.finishedAt = finishedAt
      live.lastActivityAt = finishedAt
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
      if (event.type === 'message_update') {
        const update = event.assistantMessageEvent
        const blockIndex = streamBlockIndex(update)
        if (update.type === 'text_start') activeTextBlocks.add(blockIndex)
        if (update.type === 'text_delta') {
          activeTextBlocks.add(blockIndex)
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
        setLiveActivity(live, tool)
        emit('tool_start', {
          id: event.toolCallId,
          name: event.toolName,
          args: event.args,
          startedAt: toolStartedAt,
          ...(event.toolName === 'bash' ? { output: '' } : {}),
        })
      } else if (event.type === 'tool_execution_update') {
        const rawOutput = textFromContent(event.partialResult?.content)
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
        if (
          !event.isError &&
          ['generate_visual', 'browser_automation'].includes(event.toolName) &&
          event.result?.details?.path
        ) {
          const generatedPath = resolve(event.result.details.path)
          const asset = this.assetIndex.assets.find(
            (item) => item.filePath && resolve(item.filePath) === generatedPath,
          )
          if (asset) {
            const attachment = assetMessageAttachment(asset)
            live.assets = [...live.assets.filter((item) => item.id !== attachment.id), attachment]
            emit('generated_asset', attachment)
          }
        }
        const resultOutput = event.toolName === 'bash' ? textFromContent(event.result?.content) : ''
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
      const safeAttachments = Array.isArray(attachments) ? attachments.slice(0, 8) : []
      const archivedAttachments = await this.archiveAttachments(
        session.sessionId,
        value.name,
        safeAttachments,
      )
      const images = []
      const contexts = []
      const sharedContextEnabled = !value.isolatedContext
      const memoryContext =
        sharedContextEnabled && value.enabledTools?.includes('memory_search')
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
      const prompt = contexts.length
        ? `${message}${ATTACHMENT_MARKER}${contexts.join('\n\n')}`
        : message
      const titlePromise = mayAutoTitle
        ? this.generateSessionTitle(
            session.model,
            message,
            safeAttachments,
            temporaryTitle,
            session.sessionId,
          ).catch(() => '')
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
          if (
            generatedTitle &&
            !this.sessionMeta[session.sessionId]?.manual &&
            generatedTitle !== value.name
          ) {
            session.setSessionName(generatedTitle)
            value.name = generatedTitle
            await this.markSessionTitle(session.sessionId, generatedTitle, false)
            emit('session_title', {
              sessionId: session.sessionId,
              name: generatedTitle,
              source: 'generated',
            })
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
      if (this.goalEmitters.get(session.sessionId) === emit)
        this.goalEmitters.delete(session.sessionId)
      if (this.planEmitters.get(session.sessionId) === emit)
        this.planEmitters.delete(session.sessionId)
      if (this.agentEmitters.get(session.sessionId) === emit)
        this.agentEmitters.delete(session.sessionId)
      if (live.streaming) finishLiveRun(live.error)
      this.touchSessionRuntime(value)
      this.evictIdleSessionRuntimes(session.sessionId)
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

  async generateSessionTitle(model, message, attachments, fallback, sessionId) {
    const attachmentText = attachments.length
      ? `\n附件：${attachments.map((item) => safeAttachmentName(item.name)).join('、')}`
      : ''
    const result = await this.modelRuntime.completeSimple(
      model,
      {
        systemPrompt:
          'You generate clear, specific session titles from the user task. Output only a Simplified Chinese title with no quotes, punctuation, explanation, or title prefix. Use at most 20 Chinese characters while preserving necessary filenames, technical terms, and error names.',
        messages: [
          {
            role: 'user',
            content: `${String(message || '').slice(0, 1200)}${attachmentText}`,
            timestamp: Date.now(),
          },
        ],
      },
      {
        ...(model.reasoning ? { reasoning: 'low' } : { temperature: 0.2 }),
        maxTokens: 128,
      },
    )
    if (sessionId)
      await this.recordUsage(
        localDayKey(result.timestamp || Date.now()),
        `title:${sessionId}`,
        result.usage,
      )
    if (result.errorMessage) return fallback
    return cleanSessionTitle(textFromContent(result.content)) || fallback
  }

  async captureConversationMemory({
    sessionId,
    cwd,
    model,
    user,
    assistant,
    sourceTimestamp = '',
  }) {
    const result = await extractConversationMemories({
      modelRuntime: this.modelRuntime,
      model,
      user,
      assistant,
    })
    if (result.usage)
      await this.recordUsage(
        localDayKey(result.timestamp || Date.now()),
        `memory:${sessionId}:${result.timestamp || Date.now()}`,
        result.usage,
      )
    if (!result.memories.length) return []
    const projectSpaceId = await this.memory.ensureWorkspaceSpace(cwd)
    return result.memories.map((item, index) =>
      this.memory.propose({
        ...item,
        spaceId: item.scope === 'global' ? 'global' : projectSpaceId,
        cwd,
        sessionId,
        sourceId: `${sessionId}:${sourceTimestamp || result.timestamp || Date.now()}:${index}`,
        sourceTimestamp: sourceTimestamp || new Date(result.timestamp || Date.now()).toISOString(),
        sourceType: 'conversation',
      }),
    )
  }
}
