import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { TOOL_PRESETS, toolsFromConfig } from '../tools/registry.mjs'
import {
  MAX_COMPACTION_THRESHOLD_PERCENT,
  MIN_COMPACTION_THRESHOLD_PERCENT,
  normalizeCompactionThresholdPercent,
} from './compaction-policy.mjs'

export const ISOLATED_CONTEXT_BLOCKED_TOOLS = ['memory_search', 'memory_remember']

export class AgentRuntimeFacade {
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
    // Interruption markers belong to one prompt run. Leaving them on the resident
    // makes the next prompt inherit the previous abort deadline.
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
        // The disposed Agent may report isStreaming until its abandoned prompt
        // settles, but this resident can no longer be reused.
        this.sessionLifecycle.disposeSessionRuntime(id, value, { force: true })
      } else {
        this.touchSessionRuntime(value)
        this.evictIdleSessionRuntimes(id)
      }
    }
  }

  async getPlugins() {
    return this.toolPlugins.getState()
  }

  testWebSearch(input) {
    return this.webSearch.test(input)
  }

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
        path: asset.filePath,
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
