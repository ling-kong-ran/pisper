// AgentRuntimeFacade：AgentRuntimeService 的“门面”基类。
// 把各领域服务（记忆/目标/计划/多 Agent/渠道/工作流/定时/插件/MCP/技能/Provider 等）
// 的调用组织成 HTTP API 层可直接调用的方法集合；子类 AgentRuntimeService 在
// 构造器里完成依赖装配后，这些方法即成为对外接口。
import { randomUUID } from 'node:crypto'
import { mkdir, readFile } from 'node:fs/promises'
import { isAbsolute, join, relative, resolve } from 'node:path'
import { WorkspaceAssetTracker } from '../services/workspace-asset-tracker.mjs'
import { SessionFileChangesService } from '../services/session-file-changes.mjs'
import { TurnCheckpointService } from '../services/turn-checkpoints.mjs'
import { loadAgentProfiles } from '../services/agent-profiles.mjs'
import { revealLocalPath } from '../services/local-path-service.mjs'
import { projectApproval } from '../services/session-card-summary.mjs'
import { EnvironmentError } from '../services/bots/environment-service.mjs'
import { NodeExecutor } from '../services/bots/executor.mjs'
import { WorktreeManager } from '../services/bots/worktree-manager.mjs'
import { projectQuestionSummary } from '../services/session-question-service.mjs'
import {
  bridgeOfficialComputerUsePlugin,
  bridgePiExtensionPackages,
  isPiExtensionPlugin,
} from '../services/pi-extension-bridge.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { TOOL_PRESETS, toolsFromConfig } from '../tools/registry.mjs'
import { projectSessionCommands } from './session-commands.mjs'
import { resolveWorkspaceDirectory } from './workspace-directories.mjs'
import { normalizeSideQuestion, streamSideQuestion } from './side-question.mjs'
import { localDayKey } from './conversation-memory-capture.mjs'
import { finishRunStartTiming, markRunStage, withRunStartTiming } from './run-start-timing.mjs'
import { isInternalParentMessage } from './stream-projection.mjs'
import { sessionInputQueueSnapshot } from './session-input-queue.mjs'
import {
  MAX_COMPACTION_THRESHOLD_PERCENT,
  MIN_COMPACTION_THRESHOLD_PERCENT,
  normalizeCompactionThresholdPercent,
} from './compaction-policy.mjs'
import {
  applyBetaFlagPatch,
  isBetaFeatureActive,
  normalizeStoredBetaFlags,
  projectBetaFeatures,
} from '../../shared/beta-features.mjs'

// 隔离上下文模式（工作流/渠道的独立子任务）下屏蔽的记忆相关工具：
// 子任务不应读写主会话的长期记忆。
export const ISOLATED_CONTEXT_BLOCKED_TOOLS = ['memory_search', 'memory_remember']

// 准备阶段在 /live 里投影的待发消息上限（只用于显示，完整消息照常写进会话）。
const MAX_PREPARING_MESSAGE_CHARS = 20_000

export const DEFAULT_MEMORY_AUTO_APPROVE_CONFIDENCE = 60
export const MIN_MEMORY_AUTO_APPROVE_CONFIDENCE = 0
export const MAX_MEMORY_AUTO_APPROVE_CONFIDENCE = 100

// 记忆自动确认阈值归一化：非法输入回退默认值，并限制在 [0, 100] 区间。
export function normalizeMemoryAutoApproveConfidence(value) {
  const requested = Number(value)
  if (!Number.isFinite(requested)) return DEFAULT_MEMORY_AUTO_APPROVE_CONFIDENCE
  return Math.min(
    MAX_MEMORY_AUTO_APPROVE_CONFIDENCE,
    Math.max(MIN_MEMORY_AUTO_APPROVE_CONFIDENCE, Math.round(requested)),
  )
}

// 已启用的通知渠道集合：只有完成连接或授权的渠道才允许工作流与定时任务选择。
export function enabledNotificationTargets(notificationSettings) {
  return new Set([
    ...(notificationSettings.browser?.enabled ? ['browser'] : []),
    ...(notificationSettings.connections?.feishu?.enabled ? ['feishu'] : []),
    ...(notificationSettings.connections?.weixin?.enabled ? ['weixin'] : []),
    ...(notificationSettings.connections?.qq?.enabled ? ['qq'] : []),
    ...(notificationSettings.connections?.telegram?.enabled ? ['telegram'] : []),
  ])
}

// 定时任务 → 工作流适配器：把 WorkflowService 暴露成 ScheduleService 需要的接口。
export function createScheduleWorkflowAdapter(workflows) {
  return {
    list: () => workflows.getState().workflows,
    run: (id, options) => workflows.runNow(id, options),
    getRun: (id) => workflows.getRun(id),
  }
}

// 工作流通知目标过滤：剔除未启用的通知渠道，防止配置了但没连通的渠道在运行时静默失败。
export function filterWorkflowNotificationTargets(input, enabledTargets) {
  if (!input || typeof input !== 'object') return input
  const filtered = { ...input }
  // 部分 PATCH 未提供的字段保持缺席，不能用 undefined 覆盖已有图和通知配置。
  if (Array.isArray(input.notifications))
    filtered.notifications = input.notifications.filter((target) => enabledTargets.has(target))
  if (Array.isArray(input.nodes))
    filtered.nodes = input.nodes.map((node) =>
      node && typeof node === 'object' && Array.isArray(node.notificationTargets)
        ? {
            ...node,
            notificationTargets: node.notificationTargets.filter((target) =>
              enabledTargets.has(target),
            ),
          }
        : node,
    )
  return filtered
}

export class AgentRuntimeFacade {
  async revealLocalPath(path) {
    if (this.capabilities?.profile !== 'desktop')
      throw new Error('当前 Runtime 不支持打开宿主文件管理器。')
    return await revealLocalPath(path)
  }

  // 旁问（/btw）：读当前会话的上下文快照发一次无头请求，不改主会话，也不登记为可重挂的 run。
  // onStart 在校验通过、即将请求模型时调用：此前的错误按普通请求错误返回。
  async askSideQuestion(sessionId, question, { signal, onStart, onDelta } = {}) {
    const text = normalizeSideQuestion(question)
    const current = await this.getExistingSession(String(sessionId || ''))
    if (!current?.session) throw Object.assign(new Error('会话不存在。'), { statusCode: 404 })
    onStart?.()
    const result = await streamSideQuestion({
      modelRuntime: this.modelRuntime,
      session: current.session,
      question: text,
      signal,
      onDelta,
    })
    // 旁问不进会话文件，用量扫描看不到它；单独记一条，统计里的花费才完整。
    if (result.usage) {
      await this.recordUsage(
        localDayKey(),
        `side-question:${sessionId}:${randomUUID()}`,
        result.usage,
      ).catch(() => {})
    }
    return result
  }

  async getSessionCommands(sessionId) {
    return this.commandsForDirectory(await this.sessionWorkspaceCwd(sessionId), sessionId)
  }

  // 新任务页还没有会话：按所选项目目录列出同一套 Prompt / Skill 命令，目录校验与建会话一致。
  async getWorkspaceCommands(cwd) {
    return this.commandsForDirectory(await resolveWorkspaceDirectory(cwd, this.cwd), '')
  }

  async commandsForDirectory(cwd, sessionId) {
    const loader = await this.skills.createResourceLoader(cwd)
    const prompts = loader.getPrompts()
    const skills = loader.getSkills()
    return projectSessionCommands({
      sessionId,
      prompts: prompts.prompts,
      skills: skills.skills.filter((skill) => !this.skills.isBuiltinSkill?.(skill)),
      diagnostics: prompts.diagnostics,
    })
  }

  deriveSession(id, boundaryEntryId, name) {
    return this.sessionLifecycle.deriveSession(id, boundaryEntryId, name)
  }

  getSessionTree(id) {
    return this.sessionLifecycle.getSessionTree(id)
  }

  searchSessionTreeLabels(query, options) {
    return this.sessionLifecycle.searchSessionTreeLabels(query, options)
  }

  navigateSessionTree(id, targetEntryId, options) {
    return this.sessionLifecycle.navigateSessionTree(id, targetEntryId, options)
  }

  prepareLastTurnRetry(id) {
    return this.sessionLifecycle.prepareLastTurnRetry(id)
  }

  setSessionTreeLabel(id, targetEntryId, label) {
    return this.sessionLifecycle.setSessionTreeLabel(id, targetEntryId, label)
  }

  // 归档附件：上传类附件落盘为资产；路径引用型附件跳过（只保留引用）。
  // 归档失败不阻塞聊天（best-effort）。
  async archiveAttachments(sessionId, sessionName, attachments = []) {
    const archived = []
    for (const attachment of attachments) {
      if (attachment.kind === 'path') {
        archived.push(null)
        continue
      }
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

  resolveDefaultModel() {
    return this.providerPreferences.resolveDefaultModel()
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
    return this.sessionLifecycle.abortSession(id)
  }

  async deleteSession(id) {
    // 仅在会话生命周期删除成功后清理快照，避免删除失败时丢失可恢复基线。
    const remove = async () => {
      const result = await this.sessionLifecycle.deleteSession(id)
      await this.getFileChangesService().clear(id)
      await this.getTurnCheckpoints().clear(id)
      if (result) {
        // 删除后丢掉内存摘要，并让看板事件流对在线客户端发 removed。
        this.sessionCards?.forget(id, { deleted: true })
        this.boardEvents?.touch(id)
        this.boardEvents?.catalogChanged()
        // 删掉的会话不再排队；排在它后面的会话视为前置已结束，照常启动。
        void this.sessionQueue?.onSessionDeleted(id).catch(() => {})
      }
      return result
    }
    return this.sideChats ? this.sideChats.deleteWithChildren(id, remove) : remove()
  }

  installWorkspaceAssetCapture(session, cwd, sessionId = session.sessionId) {
    this.workspaceAssetTracker ||= new WorkspaceAssetTracker({
      dataDir: this.dataDir,
      archive: (id, path) =>
        this.recordGeneratedFile(
          id,
          this.sessions.get(id) || { name: this.sessionMeta[id]?.name },
          path,
        ),
    })
    this.workspaceAssetTracker.install(session, { sessionId, cwd })
    // 同一安装点顺手挂上文件变更追踪：子会话也归属父会话 ID，审批视图自动聚合。
    this.fileChanges ||= new SessionFileChangesService({ dataDir: this.dataDir })
    this.fileChanges.install(session, { sessionId, cwd })
    // 按轮回退文件的快照也挂在这里：子 Agent 的写入同样记在父会话下。
    this.getTurnCheckpoints().install(session, { sessionId, cwd })
  }

  getTurnCheckpoints() {
    this.turnCheckpoints ||= new TurnCheckpointService({
      dataDir: this.dataDir,
      warn: () => console.warn('文件回退快照写入失败，该文件之后不能按轮回退。'),
    })
    return this.turnCheckpoints
  }

  // 回退文件的目标时间：某条消息（通常是一轮结束时的回复）的时间。会话运行中不允许。
  async fileRewindTarget(id, entryId) {
    const sessionId = String(id || '').trim()
    const targetId = String(entryId || '').trim()
    if (!sessionId || !targetId) throw new Error('会话树节点无效。')
    if (this.sessionRunIsActive(sessionId, this.sessions.get(sessionId)))
      throw new Error('当前会话正在运行，请等待完成或停止后再回退文件。')
    const value = await this.getOrCreateSession(sessionId)
    if (this.sessionRunIsActive(sessionId, value))
      throw new Error('当前会话正在运行，请等待完成或停止后再回退文件。')
    const entry = value.session.sessionManager.getEntry(targetId)
    if (entry?.type !== 'message') throw new Error('会话树节点不存在。')
    const at = Number.isFinite(entry.message?.timestamp)
      ? entry.message.timestamp
      : Date.parse(entry.timestamp)
    if (!Number.isFinite(at)) throw new Error('这条消息没有时间信息，无法回退文件。')
    return { sessionId, at }
  }

  async previewFileRewind(id, entryId) {
    const { sessionId, at } = await this.fileRewindTarget(id, entryId)
    return this.getTurnCheckpoints().preview(sessionId, at)
  }

  async rewindFiles(id, entryId) {
    const { sessionId, at } = await this.fileRewindTarget(id, entryId)
    const result = await this.getTurnCheckpoints().restore(sessionId, at)
    // 工作区文件变了：变更视图与看板摘要重新计算。
    this.streamProjection.invalidate(sessionId)
    this.boardEvents?.touch(sessionId)
    return result
  }

  // 流式执行一次会话提示：登记准备阶段 → 拿到会话运行时 → 校验未在运行 → 执行并清理中止标记。
  async streamPrompt(options) {
    return withRunStartTiming(() => this.streamPromptTimed(options))
  }

  async streamPromptTimed(options) {
    // 受理即算运行中：装配运行时、选工具等准备工作可能要几秒（新会话首轮、MCP 连接），这段时间里
    // 会话列表、看板、/messages 与 /live 都要报告“在跑”，否则别处打开会话会读到“空页、未运行”。
    // 同一会话的第二次 prompt 在准备阶段就拒绝，不再两轮并发装配。
    const requestedId = typeof options.sessionId === 'string' ? options.sessionId : ''
    if (requestedId && this.startingRuns.has(requestedId))
      throw new Error('当前会话仍在运行，请等待完成或先停止。')
    const message = typeof options.message === 'string' ? options.message : ''
    const starting = requestedId
      ? {
          aborted: false,
          // 供 /live 在准备阶段投影这条还没写进会话的消息；内部唤醒消息对用户隐藏，不投影。
          message: isInternalParentMessage(message)
            ? ''
            : message.slice(0, MAX_PREPARING_MESSAGE_CHARS),
          acceptedAt: new Date().toISOString(),
        }
      : null
    if (starting) {
      this.startingRuns.set(requestedId, starting)
      this.streamProjection.invalidate(requestedId)
    }
    try {
      return await this.runStartedPrompt(options)
    } finally {
      // 正常路径在建立 live 时已移除；准备阶段失败或被拒绝时在这里收回，并让读者看到“已结束”。
      if (starting && this.startingRuns.get(requestedId) === starting) {
        this.startingRuns.delete(requestedId)
        this.streamProjection.invalidate(requestedId)
      }
      finishRunStartTiming('ended')
      if (starting?.aborted) void this.discardStoppedEmptySession(requestedId)
    }
  }

  // 第一条消息在准备阶段就被停止的新会话一条消息也没有：删掉，不在侧栏留下点进去是空白的会话。
  // 已有历史或还有排队输入的会话不动；删除失败只记一笔，不影响停止本身。
  async discardStoppedEmptySession(id) {
    try {
      const value = this.sessions.get(id)
      const messages = value ? value.session?.messages || [] : null
      const queued = value ? sessionInputQueueSnapshot(value.session).inputs.length : 0
      const empty = value
        ? messages.length === 0 && queued === 0 && !this.sessionRunIsActive(id, value)
        : this.pendingSessions.has(id) && !this.sessionRunIsActive(id)
      if (empty) await this.deleteSession(id)
    } catch {
      console.warn('停止后清理空会话失败。')
    }
  }

  async runStartedPrompt(options) {
    const finishSideChatRun = await this.sideChats?.beginRun(options.sessionId)
    markRunStage('sideChat')
    try {
      let value = await this.getOrCreateSession(options.sessionId)
      markRunStage('session')
      let id = value.session.sessionId
      if (value.forceDisposed && !value.runActive) {
        this.sessionLifecycle.disposeSessionRuntime(id, value, { force: true })
        value = await this.getOrCreateSession(options.sessionId)
        id = value.session.sessionId
      }
      if (this.sessionLifecycle.sessionEngineRunIsActive(id, value))
        throw new Error('当前会话仍在运行，请等待完成或先停止。')
      // 历史会话不允许自动切换模型：绑定模型不可用时拒绝执行并显式告知，
      // 而不是静默用默认模型消耗额度、并把回退结果写回会话绑定。
      if (value.blockedModel) {
        const ref = `${value.blockedModel.provider}/${value.blockedModel.modelId}`
        const reason =
          value.blockedModel.reason === 'missing-auth'
            ? 'Provider 缺少可用凭据'
            : '模型不存在或 Provider 未配置'
        throw new Error(`会话绑定的模型 ${ref} 当前不可用（${reason}），请重新选择模型后再发送。`)
      }
      // 中止标记只属于单次运行：留在常驻运行时上会让下一次 prompt 继承上次的截止时间。
      delete value.abortedAt
      delete value.forceDisposed
      value.runActive = true
      try {
        this.installWorkspaceAssetCapture(value.session, value.cwd)
        markRunStage('assetCapture')
        return await this.runSessionPrompt(value, options)
      } finally {
        value.runActive = false
        // 运行收尾的两次落盘发生在 done/error 帧之后：落盘慢于看板 flush 时，最后一帧
        // 仍读到 runActive=true。这里在真正放下运行标记后再失效一次活动投影，
        // 经 onInvalidate 让看板事件流补发一帧“已结束”，/live 缓存也随之失效。
        this.streamProjection.invalidate(id, { transcript: false, activity: true, usage: false })
        // 运行期间注入失败、放回队列的 Agent 完成通知：本轮结束后补发；用户主动停止时不自动再起一轮。
        if (!value.abortedAt && value.pendingAgentNotifications?.length)
          this.scheduleAgentWakeup(id)
        // 这一轮结束（完成、失败、停止都算）：启动排在它后面的会话（sessionQueue）。
        void this.sessionQueue?.onRunFinished(id).catch(() => console.warn('排队的会话启动失败。'))
        const forceDisposed = value.forceDisposed
        delete value.abortedAt
        delete value.forceDisposed
        if (forceDisposed) {
          // 被强制 dispose 的 Agent 在其遗留 prompt 落定前可能仍报告 isStreaming，
          // 但这个常驻运行时已不可复用，直接释放。
          this.sessionLifecycle.disposeSessionRuntime(id, value, { force: true })
        } else {
          this.touchSessionRuntime(value)
          this.evictIdleSessionRuntimes(id)
        }
      }
    } finally {
      await finishSideChatRun?.()
    }
  }

  async getPlugins(sessionId = '') {
    const state = await this.toolPlugins.getState()
    const stateTools = Array.isArray(state.tools) ? state.tools : []
    const statePlugins = Array.isArray(state.plugins) ? state.plugins : []
    const configuredToolNames = Array.isArray(state.enabledTools) ? state.enabledTools : []
    const supportedToolNames = new Set(
      Array.isArray(this.capabilities?.tools)
        ? this.capabilities.tools
        : stateTools.length
          ? stateTools.map((tool) => tool.id)
          : configuredToolNames,
    )
    const customPluginsAvailable = this.capabilities?.features?.plugins !== false
    const piExtensionsAvailable = this.capabilities?.features?.extensions !== false
    let extensionDashboard = { packages: [] }
    if (piExtensionsAvailable) {
      try {
        extensionDashboard = await this.skills.extensionDashboard({
          cwd: await this.sessionWorkspaceCwd(sessionId),
        })
      } catch {}
    }
    const piPlugins = [
      ...bridgePiExtensionPackages(extensionDashboard.packages).map((plugin) => ({
        ...plugin,
        enabled: state.piExtensions?.[plugin.packageSource] !== false,
        capabilities: plugin.capabilities.map((capability) => ({
          ...capability,
          enabled: state.piExtensions?.[plugin.packageSource] !== false,
        })),
      })),
      ...(this.capabilities?.features?.computerUse
        ? [bridgeOfficialComputerUsePlugin({ enabled: state.computerUseEnabled !== false })]
        : []),
    ]
    const plugins = [
      ...statePlugins
        .filter((plugin) => plugin.builtIn || customPluginsAvailable)
        .map((plugin) => ({
          ...plugin,
          capabilities: plugin.capabilities.filter(
            (capability) => !plugin.builtIn || supportedToolNames.has(capability.name),
          ),
        })),
      ...piPlugins,
    ].filter((plugin) => plugin.capabilities.length > 0)
    const projectedToolNames = plugins
      .filter((plugin) => !isPiExtensionPlugin(plugin))
      .flatMap((plugin) => plugin.capabilities.map((capability) => capability.name))
    const visibleToolNames = new Set(
      projectedToolNames.length
        ? projectedToolNames
        : configuredToolNames.filter((tool) => supportedToolNames.has(tool)),
    )
    const visibleState = {
      ...state,
      computerUseEnabled:
        this.capabilities?.features?.computerUse !== false && state.computerUseEnabled !== false,
      plugins,
      tools: stateTools.filter((tool) => visibleToolNames.has(tool.id)),
      presets: Object.fromEntries(
        Object.entries(state.presets || {}).map(([preset, tools]) => [
          preset,
          tools.filter((tool) => visibleToolNames.has(tool)),
        ]),
      ),
      enabledTools: state.enabledTools.filter((tool) => visibleToolNames.has(tool)),
    }
    if (!sessionId) return visibleState
    const enabledToolNames = this.toolPlugins
      .enabledTools({ enabledTools: visibleState.enabledTools })
      .filter((tool) => visibleToolNames.has(tool))
    const callablePiToolNames = piPlugins
      .filter((plugin) => plugin.name === 'Computer Use' && plugin.enabled !== false)
      .flatMap((plugin) => plugin.capabilities.map((capability) => capability.name))
    return {
      ...visibleState,
      // 可调用工具不随执行模式变化：模式只决定调用时是否逐次确认。
      callableToolNames: [...new Set([...enabledToolNames, ...callablePiToolNames])],
    }
  }

  // 默认工具集合初始化：为记忆/MCP 管理/Web 搜索等能力登记默认工具开关。
  async initializeToolPlugins() {
    await this.toolPlugins.init()
    await this.toolPlugins.ensureDefaultTools(['memory_search', 'memory_remember'], 'memoryToolsV1')
    await this.toolPlugins.ensureDefaultTools(['mcp_list', 'mcp_manage'], 'mcpManagementToolsV1')
    // 定时任务工具：既有安装若已物化 enabledTools，也补上（代理此前没有办法建定时任务）。
    await this.toolPlugins.ensureDefaultTools(
      ['schedule_list', 'schedule_manage'],
      'scheduleToolsV1',
    )
    await this.toolPlugins.ensureDefaultTools(['workflow_list', 'workflow_run'], 'workflowToolsV1')
    await this.toolPlugins.ensureDefaultTools(['web_search'], 'webSearchToolV1')
    await this.toolPlugins.ensureDefaultTools(['browser_automation'], 'browserAutomationToolV1')
    await this.toolPlugins.ensureDefaultTools(['skill_create'], 'skillCreateToolV1')
    await this.toolPlugins.ensureDefaultTools(['plugin_create'], 'pluginCreateToolV1')
    // 原生组件写入工具（A7）：既有安装若已物化 enabledTools，也补上它。
    await this.toolPlugins.ensureDefaultTools(['component_create'], 'componentCreateToolV1')
    await this.toolPlugins.ensureDefaultTools(['mobile_device'], 'mobileDeviceToolV2')
    // 生图默认开放：既有安装若已物化 enabledTools（如旧版 workspace 预设），需迁移补齐。
    await this.toolPlugins.ensureDefaultTools(['generate_visual'], 'visualGenerateToolV1')
  }

  getToolRisk(name) {
    return this.toolPlugins.getToolRisk(name) || this.mcp.getToolRisk(name)
  }

  testWebSearch(input) {
    return this.webSearch.test(input)
  }

  // 渠道（飞书/微信）发起的会话提示：不存在则建会话，支持隔离上下文与指定模型。
  async promptFromChannel({
    sessionId,
    message,
    attachments = [],
    cwd,
    title,
    model,
    executionMode,
    goalMode = false,
    teamMode = false,
    isolatedContext = false,
    requestedToolNames,
    allowedToolNames = null,
    // 执行节点（B6）：{ environmentId, cwd } 或 null；undefined 表示不改。
    executor,
    unattended = false,
    onSession,
    onEvent,
  }) {
    let id = String(sessionId || '')
    if (
      id &&
      !this.sessions.has(id) &&
      !this.pendingSessions.has(id) &&
      !(await this.findSessionInfo(id))
    )
      id = ''
    if (!id) {
      const created = await this.createSession(title || '飞书会话')
      id = created.id
      if (executor !== undefined) await this.setSessionExecutor(id, executor)
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
      if (
        active.session.model?.provider !== model.provider ||
        active.session.model?.id !== model.model
      )
        await this.setSessionModel(id, model.provider, model.model)
    }
    onSession?.(id)
    let actualId = id
    let text = ''
    const assetIds = new Set()
    await this.streamPrompt({
      sessionId: id,
      message,
      attachments,
      goalMode,
      teamMode,
      isolatedContext,
      requestedToolNames,
      allowedToolNames,
      unattended,
      send: (event, data) => {
        if ((event === 'meta' || event === 'done') && data?.sessionId) actualId = data.sessionId
        if (event === 'text_delta') text += data?.delta || ''
        if (event === 'generated_asset' && data?.id) assetIds.add(data.id)
        onEvent?.(event, data)
      },
    })
    if (!text.trim()) {
      const messages = await this.getSessionMessages(actualId)
      text = [...messages].reverse().find((item) => item.role === 'agent')?.text || ''
    }
    const runtime = this.sessions.get(actualId)
    const assets = [...assetIds]
      .map((assetId) => this.assetIndex.assets.find((asset) => asset.id === assetId))
      .filter(Boolean)
      .map((asset) => ({
        id: asset.id,
        name: asset.name,
        path: asset.storagePath,
        mimeType: asset.mimeType,
      }))
      .filter((asset) => asset.path)
    return {
      sessionId: actualId,
      text: text.trim(),
      cwd: runtime?.cwd || this.sessionMeta[actualId]?.cwd || this.cwd,
      model: runtime?.session.model
        ? `${runtime.session.model.provider}/${runtime.session.model.id}`
        : '',
      assets,
    }
  }

  async getChannels() {
    const state = this.channels.getState()
    const config = await this.getConfig()
    return {
      providers: state.providers,
      connections: state.connections,
      scopes: state.scopes,
      models: config.providers
        .filter((provider) => provider.type !== 'visual' && provider.enabled && provider.configured)
        .flatMap((provider) =>
          provider.models
            .filter((model) => model.kind === 'chat')
            .map((model) => ({
              provider: provider.id,
              model: model.id,
              label: `${provider.name} / ${model.name}`,
            })),
        ),
    }
  }

  startChannelOnboarding(platform, input = undefined) {
    return this.channels.startOnboarding(platform, input)
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

  getChatDockLayout() {
    return readJson(this.chatDockLayoutPath, null)
  }

  async saveChatDockLayout(input) {
    if (!input || typeof input !== 'object' || Array.isArray(input)) {
      throw new Error('Dock 布局必须是 JSON 对象。')
    }
    const snapshot = structuredClone(input)
    this.chatDockLayoutWrite = this.chatDockLayoutWrite
      .catch(() => {})
      .then(() => writeJsonAtomic(this.chatDockLayoutPath, snapshot))
    await this.chatDockLayoutWrite
    return snapshot
  }

  getMemoryPreference() {
    return {
      autoApproveConfidence: this.memoryAutoApproveConfidence,
      minConfidence: MIN_MEMORY_AUTO_APPROVE_CONFIDENCE,
      maxConfidence: MAX_MEMORY_AUTO_APPROVE_CONFIDENCE,
    }
  }

  async updateMemoryPreference(input) {
    const requested = Number(input?.autoApproveConfidence)
    if (
      !Number.isFinite(requested) ||
      requested < MIN_MEMORY_AUTO_APPROVE_CONFIDENCE ||
      requested > MAX_MEMORY_AUTO_APPROVE_CONFIDENCE
    ) {
      throw new Error(
        `记忆自动确认阈值必须在 ${MIN_MEMORY_AUTO_APPROVE_CONFIDENCE} 到 ${MAX_MEMORY_AUTO_APPROVE_CONFIDENCE} 之间。`,
      )
    }
    const autoApproveConfidence = normalizeMemoryAutoApproveConfidence(requested)
    const appConfig = await readJson(this.appConfigPath, {})
    await writeJsonAtomic(this.appConfigPath, {
      ...appConfig,
      memoryAutoApproveConfidence: autoApproveConfidence,
    })
    this.memoryAutoApproveConfidence = autoApproveConfidence
    return this.getMemoryPreference()
  }

  getBetaFeatures() {
    return {
      features: projectBetaFeatures(
        this.betaFeatureFlags,
        this.capabilities?.features,
        this.betaFeatureCatalog,
      ),
    }
  }

  // 服务在做决定的那一刻调用（例如渠道消息是否交给 Bot），不缓存，改开关立即生效。
  isBetaFeatureEnabled(id) {
    return isBetaFeatureActive(
      this.betaFeatureFlags,
      this.capabilities?.features,
      id,
      this.betaFeatureCatalog,
    )
  }

  // betaFeatureCatalog 默认为空（undefined → 用 shared 里的正式清单），只有测试会注入。
  // 写入串行化：连续切换两个开关时，后一次必须基于前一次写入后的配置，否则会丢更新。
  // 先校验再排队，非法输入不占写入队列；配置里清单之外的旧键原样保留。
  async updateBetaFeatures(input) {
    applyBetaFlagPatch(this.betaFeatureFlags, input, this.betaFeatureCatalog)
    const write = this.betaFeatureWrite
      .catch(() => {})
      .then(async () => {
        const appConfig = await readJson(this.appConfigPath, {})
        const flags = applyBetaFlagPatch(
          normalizeStoredBetaFlags(appConfig.betaFeatures),
          input,
          this.betaFeatureCatalog,
        )
        await writeJsonAtomic(this.appConfigPath, { ...appConfig, betaFeatures: flags })
        this.betaFeatureFlags = flags
      })
    this.betaFeatureWrite = write
    await write
    return this.getBetaFeatures()
  }

  // 列表里带上每个 Bot 的最新动态，侧栏据此显示最近一句与“需要你”。
  listBots() {
    return this.bots.list().map((bot) => ({ ...bot, activity: this.botTasks.activity(bot.id) }))
  }

  sendBotMessage(botId, input) {
    return this.botTasks.message(botId, { text: input?.text, source: { kind: 'chat' } })
  }

  getBot(id) {
    return this.bots.get(id)
  }

  createBot(input) {
    return this.bots.create(input)
  }

  updateBot(id, input) {
    return this.bots.update(id, input)
  }

  // 删除 Bot 时一并删掉指向它的定时任务（README「Bot」）。
  async deleteBot(id) {
    const removed = await this.bots.remove(id)
    if (removed) await this.schedules?.deleteForBot?.(id)
    return removed
  }

  // 渠道与定时任务用的 Bot 接口（B3）：都在 Bots 实验功能打开时才生效。
  botChannelAdapter() {
    return {
      enabled: () => Boolean(this.capabilities.features.bots && this.isBetaFeatureEnabled('bots')),
      list: () => this.bots?.list() || [],
      get: (id) => this.bots?.get(id) || null,
      activity: (botId) => this.botTasks?.activity(botId) ?? null,
      message: (botId, input) => this.botTasks.message(botId, input),
      dispatch: (botId, input) => this.botTasks.dispatch(botId, input),
    }
  }

  // Bot 任务的一次代理运行：独立会话（隔离上下文），模型按 Bot 的档位或指定模型解析，
  // 解析不到时沿用默认模型；执行权限取 Bot 的上限。审批与提问照常进看板“需要你”。
  async runBotTaskAgent({
    bot,
    task,
    cwd,
    environmentId = 'local',
    sessionId = '',
    message,
    onSession,
  }) {
    const resolved = await this.providerPreferences.resolveSubagentModel(
      bot.model.ref ? { modelRef: bot.model.ref } : { tier: bot.model.tier },
    )
    // 电脑是执行节点（B6）：工具在那台电脑的 cwd 上执行，大脑这边的会话目录只是占位。
    const remoteExecutor =
      environmentId && environmentId !== 'local' ? { environmentId, cwd } : null
    const sessionCwd = remoteExecutor ? await this.executorSessionDirectory(environmentId) : cwd
    // 长期会话已存在时，先换电脑与工作目录（会重建会话的工具）；新建会话时由 promptFromChannel 设置。
    if (sessionId && (this.sessions.has(sessionId) || (await this.findSessionInfo(sessionId)))) {
      await this.setSessionExecutor(sessionId, remoteExecutor)
      await this.setSessionCwd(sessionId, sessionCwd)
    }
    const result = await this.promptFromChannel({
      sessionId,
      message,
      cwd: sessionCwd,
      executor: remoteExecutor,
      title: `${bot.name} · ${task.title}`,
      model: resolved.model
        ? { provider: resolved.model.provider, model: resolved.model.id }
        : null,
      executionMode: bot.executionMode,
      goalMode: bot.runMode === 'goal',
      teamMode: bot.runMode === 'team',
      isolatedContext: true,
      // Bot 的工具范围：null 表示用当前启用的全部工具。
      allowedToolNames: bot.tools,
      onSession: (id) => {
        onSession?.(id)
        void this.bots.setThread(bot.id, id).catch(() => {})
      },
    })
    await this.bots.setThread(bot.id, result.sessionId)
    return result
  }

  // Bot 的电脑（B6）：本机，或已配对的执行节点。节点不存在时也返回执行器，调用会以“离线”失败，
  // 绝不悄悄退回在本机执行。
  executorFor(environmentId) {
    if (!environmentId || environmentId === 'local') return this.localExecutor
    const environment = this.environments.get(environmentId) || {
      id: environmentId,
      name: '',
      capabilities: {},
    }
    return new NodeExecutor({ hub: this.executorHub, environment })
  }

  botWorktreesFor(environmentId) {
    if (!environmentId || environmentId === 'local') return this.localWorktrees
    const environment = this.environments.get(environmentId)
    if (!environment?.worktreeRoot)
      throw new EnvironmentError(
        'environment_not_ready',
        '这台电脑还没有连上过，等它运行 pisper executor 后再试。',
        409,
      )
    return new WorktreeManager({
      executor: this.executorFor(environmentId),
      root: environment.worktreeRoot,
    })
  }

  listEnvironments() {
    const bots = this.bots.list()
    const usedBy = (id) =>
      bots
        .filter((bot) => (bot.workspace.environmentId || 'local') === id)
        .map((bot) => ({ id: bot.id, name: bot.name }))
    return {
      environments: [
        {
          id: 'local',
          kind: 'local',
          name: '',
          online: true,
          roots: [],
          capabilities: { shell: true, git: true },
          usedBy: usedBy('local'),
        },
        ...this.environments.list().map((environment) => ({
          ...environment,
          online: this.executorHub.online(environment.id),
          usedBy: usedBy(environment.id),
        })),
      ],
    }
  }

  // 只改会话元数据；调用方随后用 setSessionCwd 重建会话，新工具才生效。
  async setSessionExecutor(id, executor) {
    const meta = { ...(this.sessionMeta[id] || {}) }
    if (executor) meta.remoteExecutor = { environmentId: executor.environmentId, cwd: executor.cwd }
    else delete meta.remoteExecutor
    this.sessionMeta[id] = meta
    await this.saveSessionMeta()
  }

  // 远程会话在大脑这边的占位目录：每台电脑一个，里面不放任何东西。
  async executorSessionDirectory(environmentId) {
    const directory = join(this.dataDir, 'executor-sessions', environmentId)
    await mkdir(directory, { recursive: true })
    return directory
  }

  createEnvironmentPairing() {
    return this.environments.createPairing()
  }

  async removeEnvironment(id) {
    const inUseBy = this.bots
      .list()
      .filter((bot) => bot.workspace.environmentId === id)
      .map((bot) => bot.name)
    const removed = await this.environments.remove(id, { inUseBy })
    if (removed) this.executorHub.disconnect(id)
    return removed
  }

  pairExecutor(input) {
    return this.environments.pair(input)
  }

  authenticateExecutor(bearer) {
    return this.environments.authenticate(bearer)
  }

  async pollExecutor(environment, report, signal) {
    await this.environments.report(environment.id, report)
    return this.executorHub.poll(environment.id, { signal })
  }

  deliverExecutorResults(environment, results) {
    return this.executorHub.deliver(environment.id, results)
  }

  // Bot 任务在等的东西：最早一条待审批（与看板卡片同一份摘要）与最早一组待回答的问题。
  botTaskWaiting(sessionId) {
    const approval = this.permissions
      .getPending(sessionId)
      .toSorted((left, right) => String(left.createdAt).localeCompare(String(right.createdAt)))
      .map(projectApproval)
      .find(Boolean)
    const question = projectQuestionSummary(this.questions?.getPending(sessionId) || [])
    return { approval: approval ?? null, question }
  }

  listBotTasks(query) {
    return this.botTasks.list(query)
  }

  getBotTask(id) {
    return this.botTasks.get(id)
  }

  dispatchBotTask(botId, input) {
    return this.botTasks.dispatch(botId, { text: input?.text, source: { kind: 'manual' } })
  }

  getBotTaskDiff(id) {
    return this.botTasks.diff(id)
  }

  acceptBotTask(id, input) {
    return this.botTasks.accept(id, { apply: input?.apply === true })
  }

  discardBotTask(id) {
    return this.botTasks.discard(id)
  }

  resumeBotTask(id) {
    return this.botTasks.resume(id)
  }

  stopBotTask(id) {
    return this.botTasks.stop(id)
  }

  async updateCompactionPreference(input) {
    const requested = Number(input?.thresholdPercent)
    if (
      !Number.isFinite(requested) ||
      requested < MIN_COMPACTION_THRESHOLD_PERCENT ||
      requested > MAX_COMPACTION_THRESHOLD_PERCENT
    ) {
      throw new Error(
        `自动压缩阈值必须在 ${MIN_COMPACTION_THRESHOLD_PERCENT}% 到 ${MAX_COMPACTION_THRESHOLD_PERCENT}% 之间。`,
      )
    }
    const thresholdPercent = normalizeCompactionThresholdPercent(requested)
    const appConfig = await readJson(this.appConfigPath, {})
    await writeJsonAtomic(this.appConfigPath, {
      ...appConfig,
      compactionThresholdPercent: thresholdPercent,
    })
    this.compactionThresholdPercent = thresholdPercent
    this.sessionContextUsageCache.clear()
    this.streamProjection.invalidateAllUsage()
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
      workflows: this.workflows
        .getState()
        .workflows.filter((workflow) => workflow.status === 'published')
        .map(({ id, name, description, revision, inputs }) => ({
          id,
          name,
          description,
          revision,
          inputs,
        })),
      models: config.providers
        .filter((provider) => provider.type !== 'visual' && provider.enabled && provider.configured)
        .flatMap((provider) =>
          provider.models
            .filter((model) => model.kind === 'chat')
            .map((model) => ({
              provider: provider.id,
              model: model.id,
              label: `${provider.name} / ${model.name}`,
            })),
        ),
      notificationTargets: {
        browser: { enabled: notificationSettings.browser.enabled },
        feishu: { enabled: Boolean(notificationSettings.connections.feishu?.enabled) },
        weixin: { enabled: Boolean(notificationSettings.connections.weixin?.enabled) },
        qq: { enabled: Boolean(notificationSettings.connections.qq?.enabled) },
        telegram: { enabled: Boolean(notificationSettings.connections.telegram?.enabled) },
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
    const skills = await this.skills.dashboard({ cwd: this.cwd })
    return {
      ...this.workflows.getState(),
      skills: skills.skills
        .filter((skill) => skill.enabled && skill.command)
        .map((skill) => ({ id: skill.id, name: skill.name, description: skill.description })),
      cwd: this.cwd,
      models: config.providers
        .filter((provider) => provider.type !== 'visual' && provider.enabled && provider.configured)
        .flatMap((provider) =>
          provider.models
            .filter((model) => model.kind === 'chat')
            .map((model) => ({
              provider: provider.id,
              model: model.id,
              label: `${provider.name} / ${model.name}`,
            })),
        ),
      notificationTargets: {
        browser: { enabled: notificationSettings.browser.enabled },
        feishu: { enabled: Boolean(notificationSettings.connections.feishu?.enabled) },
        weixin: { enabled: Boolean(notificationSettings.connections.weixin?.enabled) },
        qq: { enabled: Boolean(notificationSettings.connections.qq?.enabled) },
        telegram: { enabled: Boolean(notificationSettings.connections.telegram?.enabled) },
      },
    }
  }

  async createWorkflow(input) {
    const notificationSettings = await this.notificationSettings.getState()
    const workflow = await this.workflows.create(
      filterWorkflowNotificationTargets(input, enabledNotificationTargets(notificationSettings)),
    )
    return { workflow, state: await this.getWorkflows() }
  }

  async updateWorkflow(id, input) {
    const notificationSettings = await this.notificationSettings.getState()
    const workflow = await this.workflows.update(
      id,
      filterWorkflowNotificationTargets(input, enabledNotificationTargets(notificationSettings)),
    )
    return workflow ? { workflow, state: await this.getWorkflows() } : null
  }

  deleteWorkflow(id) {
    return this.workflows.remove(id)
  }

  async runWorkflow(id, input = {}) {
    const run = await this.workflows.runNow(id, input)
    return run ? { started: true, run } : null
  }

  getWorkflowRun(id) {
    return this.workflows.getRun(id)
  }

  async retryWorkflowRun(id) {
    const run = await this.workflows.retryRun(id)
    return run ? { started: true, run } : null
  }

  async resolveWorkflowApproval(runId, nodeId, input) {
    const run = await this.workflows.resolveApproval(runId, nodeId, input?.approved, input?.comment)
    return run ? { resolved: true, run } : null
  }

  async duplicateWorkflow(id, input) {
    const workflow = await this.workflows.duplicate(id, input)
    return workflow ? { workflow, state: await this.getWorkflows() } : null
  }

  exportWorkflow(id) {
    return this.workflows.exportWorkflow(id)
  }

  async importWorkflow(input) {
    const workflow = await this.workflows.importWorkflow(input)
    return { workflow, state: await this.getWorkflows() }
  }

  getSessionWorkflowRuns(sessionId) {
    return { runs: this.workflows.getState({ sessionId }).runs }
  }

  async stopWorkflowRun(id) {
    const run = await this.workflows.stop(id)
    return run ? { stopping: true, run } : null
  }

  notifyChannels(event, data, options) {
    return this.notificationSettings.notify(event, data, options)
  }

  // 全局关闭流程：清理定时器、后台服务与挂起的写盘任务，保证进程可干净退出。
  async dispose() {
    // 先断开看板事件流：关闭期间不再为在线客户端生成卡片增量。
    this.boardEvents?.dispose()
    this.sessionCards?.dispose()
    await this.memoryCapture.dispose()
    this.auxiliaryTitles?.dispose()
    if (this.sessionRuntimeSweepTimer) clearInterval(this.sessionRuntimeSweepTimer)
    this.sessionRuntimeSweepTimer = null
    await this.sideChats?.dispose()
    for (const timer of this.agentWakeupTimers.values()) clearTimeout(timer)
    this.agentWakeupTimers.clear()
    this.providerModelDiscovery.abort?.()
    await this.providerModelRefreshPromise?.catch(() => {})
    await this.providerModelCatalog.dispose()
    await this.modelMetadata.dispose()
    await this.workflows.dispose()
    await this.gameAssets?.dispose()
    await this.imageAssets?.dispose()
    await this.workflowImageOperations?.dispose()
    await this.gameAssetImageOperations?.dispose()
    await this.agentImageOperations?.dispose()
    await this.workflowImageProcessor?.dispose()
    await this.spriteEngines?.dispose()
    await this.workflowMedia?.dispose()
    await this.gameAssetMedia?.dispose()
    await this.agentImageMedia?.dispose()
    this.executorHub?.dispose()
    await this.schedules.dispose()
    await this.channels.dispose()
    await this.goals.pauseAllActive()
    await this.teamWorkflows?.pauseActive?.()
    await this.multiAgents.dispose()
    await this.browserAutomation.dispose()
    this.mobileOperations.dispose()
    this.permissions.dispose()
    this.questions?.dispose()
    await this.sessionQueue?.dispose()
    await this.disposeSessions()
    await this.sandbox?.close?.()
    this.pendingSessions.clear()
    await this.mcp.dispose()
    this.toolPlugins.dispose()
    await this.memory.dispose()
    await Promise.allSettled([
      this.sessionMetaWrite,
      this.usageWrite,
      this.assetReconcile,
      this.assetWrite,
    ])
  }

  async savePlugins(input) {
    const [current, visible] = await Promise.all([this.toolPlugins.getState(), this.getPlugins()])
    const visibleToolNames = new Set(visible.tools.map((tool) => tool.id))
    const preservedToolNames = current.enabledTools.filter((tool) => !visibleToolNames.has(tool))
    const requested = { ...input }
    if (this.capabilities?.features?.computerUse === false) delete requested.computerUseEnabled
    await this.toolPlugins.saveState({
      ...requested,
      enabledTools: [...new Set([...(input?.enabledTools || []), ...preservedToolNames])],
    })
    this.invalidateSessionRuntimes()
    return await this.getPlugins()
  }

  inspectPlugin(input) {
    return this.toolPlugins.inspect(input?.path)
  }

  async installPlugin(input) {
    const result = await this.toolPlugins.install(input?.inspectionId)
    this.invalidateSessionRuntimes()
    return result
  }

  async setPluginEnabled(id, enabled) {
    const result = await this.toolPlugins.setPluginEnabled(id, enabled)
    this.invalidateSessionRuntimes()
    return result
  }

  async setPluginCapabilityEnabled(id, name, enabled) {
    const result = await this.toolPlugins.setCapabilityEnabled(id, name, enabled)
    this.invalidateSessionRuntimes()
    return result
  }

  async uninstallPlugin(id) {
    const result = await this.toolPlugins.uninstall(id)
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

  async sessionGitCwd(id) {
    return this.sessionWorkspaceCwd(id)
  }

  async getSessionGitChanges(id) {
    return this.gitChanges.getChanges(await this.sessionGitCwd(id))
  }

  async commitSessionGitChanges(id, message) {
    if (this.sessionRunIsActive(id)) throw new Error('当前会话正在运行，请完成或停止后再提交改动。')
    return this.gitChanges.commit(await this.sessionGitCwd(id), message)
  }

  async pushSessionGitChanges(id) {
    return this.gitChanges.push(await this.sessionGitCwd(id))
  }

  async revertSessionGitChanges(id) {
    if (this.sessionRunIsActive(id)) throw new Error('当前会话正在运行，请完成或停止后再撤销改动。')
    return this.gitChanges.revert(await this.sessionGitCwd(id))
  }

  async getSessionVcsChanges(id) {
    return this.vcsChanges.getChanges(await this.sessionGitCwd(id))
  }

  // 单文件差异：会话文件 chip「查看改动」的数据源；
  // 路径必须落在会话工作区内，避免借会话读取任意目录的 diff。
  // 非 VCS 工作区回退到会话内修改前快照，保证无 Git/SVN 也能预览。
  async getSessionFileDiff(id, filePath) {
    const cwd = await this.sessionGitCwd(id)
    const target = resolve(cwd, String(filePath || ''))
    const relativePath = relative(cwd, target)
    if (!String(filePath || '') || relativePath.startsWith('..') || isAbsolute(relativePath))
      throw new Error('文件路径超出会话工作区范围。')
    const vcs = await this.vcsChanges.getFileDiff(cwd, target)
    if (vcs.isRepo && vcs.diff?.trim())
      return { ...vcs, source: vcs.vcs || 'vcs', canRevert: false }
    const relPath = relativePath.replace(/\\/g, '/')
    const snapshot = await this.getFileChangesService().diff(id, cwd, relPath)
    if (snapshot.found && snapshot.diff.trim())
      return {
        isRepo: false,
        vcs: '',
        diff: snapshot.diff,
        diffTruncated: snapshot.diffTruncated,
        source: 'snapshot',
        canRevert: true,
      }
    return { ...vcs, source: vcs.isRepo ? vcs.vcs || 'vcs' : '' }
  }

  getFileChangesService() {
    this.fileChanges ||= new SessionFileChangesService({ dataDir: this.dataDir })
    return this.fileChanges
  }

  // 会话文件变更清单（无 VCS 审批视图的数据源）。
  async getSessionFileChanges(id) {
    const cwd = await this.sessionWorkspaceCwd(id)
    return { cwd, ...(await this.getFileChangesService().list(id, cwd)) }
  }

  // 会话目录的轻量改动摘要只来自本会话写工具快照，不混用工作区 Git/SVN 状态。
  async getSessionChangeSummary(id) {
    const cwd = await this.sessionWorkspaceCwd(id)
    return this.getFileChangesService().summary(id, cwd)
  }

  async getSessionFileChangeDiff(id, filePath) {
    const cwd = await this.sessionWorkspaceCwd(id)
    const relPath = this.fileChangeRelPath(cwd, filePath)
    return this.getFileChangesService().diff(id, cwd, relPath)
  }

  async revertSessionFileChanges(id, filePath) {
    if (this.sessionRunIsActive(id)) throw new Error('当前会话正在运行，请完成或停止后再撤销改动。')
    const cwd = await this.sessionWorkspaceCwd(id)
    const relPath = filePath ? this.fileChangeRelPath(cwd, filePath) : ''
    const result = await this.getFileChangesService().revert(id, cwd, relPath || undefined)
    // HTTP 面保持清单响应平铺，前端不需要了解服务层的内部包装。
    return { ...result.files, reverted: result.reverted }
  }

  async approveSessionFileChanges(id, filePath) {
    const cwd = await this.sessionWorkspaceCwd(id)
    const relPath = filePath ? this.fileChangeRelPath(cwd, filePath) : ''
    const result = await this.getFileChangesService().approve(id, cwd, relPath || undefined)
    return result.files
  }

  // 文件变更接口统一的路径校验：必须落在会话工作区内。
  fileChangeRelPath(cwd, filePath) {
    const target = resolve(cwd, String(filePath || ''))
    const relativePath = relative(cwd, target)
    if (!String(filePath || '') || relativePath.startsWith('..') || isAbsolute(relativePath))
      throw new Error('文件路径超出会话工作区范围。')
    return relativePath.replace(/\\/g, '/')
  }

  async commitSessionVcsChanges(id, message) {
    if (this.sessionRunIsActive(id)) throw new Error('当前会话正在运行，请完成或停止后再提交改动。')
    return this.vcsChanges.commit(await this.sessionGitCwd(id), message)
  }

  async pushSessionVcsChanges(id) {
    return this.vcsChanges.push(await this.sessionGitCwd(id))
  }

  async revertSessionVcsChanges(id) {
    if (this.sessionRunIsActive(id)) throw new Error('当前会话正在运行，请完成或停止后再撤销改动。')
    return this.vcsChanges.revert(await this.sessionGitCwd(id))
  }

  async getSkillsDashboard(sessionId = '') {
    return this.skills.dashboard({ cwd: await this.sessionWorkspaceCwd(sessionId) })
  }

  extensionMarketplace(input) {
    return this.skills.extensionMarketplace(input)
  }

  async getExtensionDashboard(sessionId = '') {
    return this.skills.extensionDashboard({ cwd: await this.sessionWorkspaceCwd(sessionId) })
  }

  async installExtension(input, sessionId = '') {
    const result = await this.skills.installExtension(input, {
      cwd: await this.sessionWorkspaceCwd(sessionId),
    })
    await this.refreshSessionRuntimes()
    return result
  }

  async removeExtension(source, scope = 'user', sessionId = '') {
    const removed = await this.skills.removeExtension(source, {
      cwd: await this.sessionWorkspaceCwd(sessionId),
      scope,
    })
    if (removed) await this.refreshSessionRuntimes()
    return removed
  }

  async installSkill(input, sessionId = '') {
    const result = await this.skills.install(input, {
      cwd: await this.sessionWorkspaceCwd(sessionId),
    })
    this.invalidateSessionRuntimes()
    return result
  }

  async updateSkill(id, input, sessionId = '') {
    const result = await this.skills.update(id, input, {
      cwd: await this.sessionWorkspaceCwd(sessionId),
    })
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
    return this.providerPreferences.getProviderDiscovery()
  }

  async importLocalProviders() {
    return this.providerPreferences.importLocalProviders()
  }

  async importDiscoveredProvider(discoveryId) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.importDiscoveredProvider(discoveryId),
    )
  }

  async getConfig() {
    return this.providerPreferences.getConfig()
  }

  listPermissionRules() {
    return this.permissionRules.list()
  }

  addPermissionRule(input) {
    return this.permissionRules.add(input)
  }

  removePermissionRule(id) {
    return this.permissionRules.remove(id)
  }

  async setFallbackModels(input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setFallbackModels(input),
    )
  }

  async setSubagentModelTiers(input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setSubagentModelTiers(input),
    )
  }

  async setAuxiliaryModel(input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setAuxiliaryModel(input),
    )
  }

  // 辅助模型解析失败（读配置出错等）一律当作未设置，后台杂活退回原来的模型。
  async resolveAuxiliaryModel() {
    return this.providerPreferences.resolveAuxiliaryModel().catch(() => null)
  }

  // 列出子代理定义及坏文件原因：不带会话或项目时只列全局定义（设置页）；
  // 带上时再叠加该项目的定义（同名覆盖全局），目录校验与新建会话一致，不接受任意路径。
  async listAgentProfiles({ sessionId = '', cwd = '' } = {}) {
    const directory = sessionId
      ? await this.sessionWorkspaceCwd(sessionId)
      : cwd
        ? await resolveWorkspaceDirectory(cwd, this.cwd)
        : ''
    return loadAgentProfiles({ agentDir: this.dataDir, cwd: directory })
  }

  async exportProviderConfig() {
    return this.providerPreferences.exportProviderConfig()
  }

  async importProviderConfig(input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.importProviderConfig(input),
    )
  }

  async saveConfig(input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.saveConfig(input, toolsFromConfig, TOOL_PRESETS),
    )
  }

  async setProviderConnection(id, input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setProviderConnection(id, input),
    )
  }

  async setProviderApiKey(id, input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setProviderApiKey(id, input),
    )
  }

  async setProviderEnabled(id, enabled) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setProviderEnabled(id, enabled),
    )
  }

  async cloneProvider(id, input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.cloneProvider(id, input),
    )
  }

  async createProvider(input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.createProvider(input),
    )
  }

  async setProviderModelOptions(providerId, input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setProviderModelOptions(providerId, input),
    )
  }

  async addProviderModel(providerId, input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.addProviderModels(providerId, [input], { skipExisting: false }),
    )
  }

  async deleteProviderModel(providerId, modelId) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.deleteProviderModel(providerId, modelId),
    )
  }

  async reconcileDefaultModel() {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.reconcileDefaultModel(),
    )
  }

  async refreshProviderModels() {
    return this.providerPreferences.refreshProviderModels()
  }

  async discoverProviderModels(providerId, input = {}) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.discoverProviderModels(providerId, input),
    )
  }

  async discoverConnectionModels(input = {}) {
    return this.providerPreferences.discoverConnectionModels(input)
  }

  async addProviderModels(providerId, inputs, options = {}) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.addProviderModels(providerId, inputs, options),
    )
  }

  async deleteProvider(id) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.deleteProvider(id),
    )
  }

  // 视觉模型状态：返回当前选中的模型和全部候选，供配置页切换视觉模型。
  async getVisualModelStatus() {
    const [image, video] = await Promise.all([
      this.visualGeneration.getModelStatus('image'),
      this.visualGeneration.getModelStatus('video'),
    ])
    return {
      image: image.model,
      video: video.model,
      imageModels: image.models,
      videoModels: video.models,
      imageSelection: image.selection,
      videoSelection: video.selection,
    }
  }

  async setVisualModel(kind, requestedModel) {
    return this.visualGeneration.setPreferredModel(kind, requestedModel)
  }

  // 视觉生成冒烟测试：用最小参数真实生成一张图，验证密钥与模型链路可用。
  // 产物写到数据目录（不污染用户工作区），并附带 base64 预览（超大文件跳过预览）。
  async testVisualGeneration() {
    const cwd = join(this.dataDir, 'visual-test')
    const result = await this.visualGeneration.generate({
      kind: 'image',
      prompt:
        'a small friendly robot mascot waving, flat vector illustration, soft pastel colors, plain background',
      outputName: 'config-test',
      cwd,
    })
    let previewDataUrl = ''
    try {
      const buffer = await readFile(result.path)
      if (buffer.length <= 6 * 1024 * 1024) {
        previewDataUrl = `data:${result.mimeType};base64,${buffer.toString('base64')}`
      }
    } catch {
      // 预览读取失败不影响测试结论：文件已生成即视为成功。
    }
    return { ...result, previewDataUrl }
  }
}
