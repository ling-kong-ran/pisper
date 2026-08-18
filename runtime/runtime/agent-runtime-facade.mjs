// AgentRuntimeFacade：AgentRuntimeService 的“门面”基类。
// 把各领域服务（记忆/目标/计划/多 Agent/渠道/工作流/定时/插件/MCP/技能/Provider 等）
// 的调用组织成 HTTP API 层可直接调用的方法集合；子类 AgentRuntimeService 在
// 构造器里完成依赖装配后，这些方法即成为对外接口。
import { filterToolsForExecutionMode } from '../security/execution-mode.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { TOOL_PRESETS, toolsFromConfig } from '../tools/registry.mjs'
import { workspacePathKey } from './workspace-directories.mjs'
import { projectSessionCommands } from './session-commands.mjs'
import {
  MAX_COMPACTION_THRESHOLD_PERCENT,
  MIN_COMPACTION_THRESHOLD_PERCENT,
  normalizeCompactionThresholdPercent,
} from './compaction-policy.mjs'

// 隔离上下文模式（工作流/渠道的独立子任务）下屏蔽的记忆相关工具：
// 子任务不应读写主会话的长期记忆。
export const ISOLATED_CONTEXT_BLOCKED_TOOLS = ['memory_search', 'memory_remember']

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

// 已启用的通知渠道集合（浏览器/飞书/微信）。
export function enabledNotificationTargets(notificationSettings) {
  return new Set([
    ...(notificationSettings.browser?.enabled ? ['browser'] : []),
    ...(notificationSettings.connections?.feishu?.enabled ? ['feishu'] : []),
    ...(notificationSettings.connections?.weixin?.enabled ? ['weixin'] : []),
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

// 工作流通知目标过滤：剔除未启用的通知渠道（浏览器/飞书/微信），
// 防止配置了但没连通的渠道在运行时静默失败。
export function filterWorkflowNotificationTargets(input, enabledTargets) {
  if (!input || typeof input !== 'object') return input
  return {
    ...input,
    notifications: Array.isArray(input.notifications)
      ? input.notifications.filter((target) => enabledTargets.has(target))
      : input.notifications,
    nodes: Array.isArray(input.nodes)
      ? input.nodes.map((node) =>
          node && typeof node === 'object' && Array.isArray(node.notificationTargets)
            ? {
                ...node,
                notificationTargets: node.notificationTargets.filter((target) =>
                  enabledTargets.has(target),
                ),
              }
            : node,
        )
      : input.nodes,
  }
}

export class AgentRuntimeFacade {
  // 解析会话对应的工作目录；会话不存在时抛错。
  async workspaceTrustCwd(sessionId) {
    const id = String(sessionId || '').trim()
    const known =
      id &&
      (this.sessions.has(id) ||
        this.pendingSessions.has(id) ||
        this.sessionMeta[id] ||
        (await this.findSessionInfo(id)))
    if (!known) throw new Error('会话不存在。')
    return this.sessionWorkspaceCwd(id)
  }

  async getWorkspaceTrust(sessionId) {
    return this.workspaceTrust.getStatus(await this.workspaceTrustCwd(sessionId))
  }

  // 切换工作区信任：影响该目录下的技能/设置加载（trusted 决定项目级技能是否可执行）。
  async setWorkspaceTrust(sessionId, trusted) {
    if (this.sessionRunIsActive(sessionId))
      throw new Error('当前会话正在运行，请等待完成或停止后再更改工作区信任。')
    const cwd = await this.workspaceTrustCwd(sessionId)
    const status = this.workspaceTrust.setTrusted(cwd, trusted)
    if (workspacePathKey(cwd) === workspacePathKey(this.cwd)) {
      this.settingsManager.setProjectTrusted(status.trusted)
      await this.settingsManager.reload()
    }
    this.skills.invalidateDashboardCache()
    this.invalidateSessionRuntimes()
    return status
  }

  async getSessionCommands(sessionId) {
    const cwd = await this.workspaceTrustCwd(sessionId)
    const loader = await this.skills.createResourceLoader(cwd)
    const prompts = loader.getPrompts()
    const skills = loader.getSkills()
    return projectSessionCommands({
      sessionId,
      prompts: prompts.prompts,
      skills: skills.skills,
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
    return this.sessionLifecycle.deleteSession(id)
  }

  // 流式执行一次会话提示：拿到会话运行时 → 校验未在运行 → 执行并清理中止标记。
  async streamPrompt(options) {
    let value = await this.getOrCreateSession(options.sessionId)
    let id = value.session.sessionId
    if (value.forceDisposed && !value.runActive) {
      this.sessionLifecycle.disposeSessionRuntime(id, value, { force: true })
      value = await this.getOrCreateSession(options.sessionId)
      id = value.session.sessionId
    }
    if (this.sessionRunIsActive(id, value))
      throw new Error('当前会话仍在运行，请等待完成或先停止。')
    // 中止标记只属于单次运行：留在常驻运行时上会让下一次 prompt 继承上次的截止时间。
    delete value.abortedAt
    delete value.forceDisposed
    value.runActive = true
    try {
      return await this.runSessionPrompt(value, options)
    } finally {
      value.runActive = false
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
  }

  async getPlugins(sessionId = '') {
    const state = await this.toolPlugins.getState()
    if (!sessionId) return state
    const executionMode = this.getSessionExecutionMode(sessionId)
    const enabledToolNames = this.toolPlugins.enabledTools(
      { enabledTools: state.enabledTools },
      executionMode,
    )
    return {
      ...state,
      callableToolNames: filterToolsForExecutionMode(enabledToolNames, executionMode, (name) =>
        this.getToolRisk(name),
      ),
    }
  }

  // 默认工具集合初始化：为记忆/MCP 管理/Web 搜索等能力登记默认工具开关。
  async initializeToolPlugins() {
    await this.toolPlugins.init()
    await this.toolPlugins.ensureDefaultTools(['memory_search', 'memory_remember'], 'memoryToolsV1')
    await this.toolPlugins.ensureDefaultTools(['mcp_list', 'mcp_manage'], 'mcpManagementToolsV1')
    await this.toolPlugins.ensureDefaultTools(['web_search'], 'webSearchToolV1')
    await this.toolPlugins.ensureDefaultTools(['browser_automation'], 'browserAutomationToolV1')
    await this.toolPlugins.ensureDefaultTools(['skill_create'], 'skillCreateToolV1')
    await this.toolPlugins.ensureDefaultTools(['plugin_create'], 'pluginCreateToolV1')
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
    isolatedContext = false,
    requestedToolNames,
    onSession,
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
      isolatedContext,
      requestedToolNames,
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
    await this.sandbox?.close?.()
    this.pendingSessions.clear()
    await this.mcp.dispose()
    this.toolPlugins.dispose()
    this.memory.dispose()
    await Promise.allSettled([
      this.sessionMetaWrite,
      this.usageWrite,
      this.assetReconcile,
      this.assetWrite,
    ])
  }

  async savePlugins(input) {
    const result = await this.toolPlugins.saveState(input)
    this.invalidateSessionRuntimes()
    return result
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

  async importDiscoveredProvider(discoveryId) {
    return this.providerPreferences.importDiscoveredProvider(discoveryId)
  }

  async getConfig() {
    return this.providerPreferences.getConfig()
  }

  async saveConfig(input) {
    return this.providerPreferences.saveConfig(input, toolsFromConfig, TOOL_PRESETS)
  }

  async setProviderConnection(id, input) {
    return this.providerPreferences.setProviderConnection(id, input)
  }

  async setProviderApiKey(id, input) {
    return this.providerPreferences.setProviderApiKey(id, input)
  }

  async setProviderEnabled(id, enabled) {
    return this.providerPreferences.setProviderEnabled(id, enabled)
  }

  async createProvider(input) {
    return this.providerPreferences.createProvider(input)
  }

  async addProviderModel(providerId, input) {
    return this.providerPreferences.addProviderModels(providerId, [input], {
      skipExisting: false,
    })
  }

  async reconcileDefaultModel() {
    return this.providerPreferences.reconcileDefaultModel()
  }

  async refreshProviderModels() {
    return this.providerPreferences.refreshProviderModels()
  }

  async discoverProviderModels(providerId, input = {}) {
    return this.providerPreferences.discoverProviderModels(providerId, input)
  }

  async addProviderModels(providerId, inputs, options = {}) {
    return this.providerPreferences.addProviderModels(providerId, inputs, options)
  }

  async deleteProvider(id) {
    return this.providerPreferences.deleteProvider(id)
  }
}
