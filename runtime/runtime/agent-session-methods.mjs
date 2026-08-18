import { normalizeExecutionMode } from '../security/execution-mode.mjs'
import { PERMISSION_MODES } from '../services/session-permission-service.mjs'
import { finishedCompaction, queuedSessionInputs, startedCompaction } from './stream-projection.mjs'
import { listWorkspaceDirectories, listWorkspaceEntries } from './workspace-directories.mjs'

export const agentSessionMethods = {
  async listSessions() {
    return this.sessionLifecycle.listSessions()
  },

  async createSession(name, cwd) {
    return this.sessionLifecycle.createSession(name, cwd)
  },

  async findSessionInfo(id) {
    return this.sessionLifecycle.findSessionInfo(id)
  },

  async getSessionMessages(id) {
    return this.streamProjection.getSessionMessages(id)
  },

  trimSessionHistoryCache(protectedPath = '') {
    return this.streamProjection.trimSessionHistoryCache(protectedPath)
  },

  async readSessionHistoryEntries(path) {
    return this.streamProjection.readSessionHistoryEntries(path)
  },

  async getSessionHistoryMessages(id) {
    return this.streamProjection.getSessionHistoryMessages(id)
  },

  compactionAwareContextUsage(session, compaction = null) {
    return this.streamProjection.compactionAwareContextUsage(session, compaction)
  },

  decorateContextUsage(raw, compaction = null) {
    return this.streamProjection.decorateContextUsage(raw, compaction)
  },

  async getSessionContextUsage(id, compaction = null) {
    return this.streamProjection.getSessionContextUsage(id, compaction)
  },

  async getSessionMessagePage(id, options = {}) {
    return this.streamProjection.getSessionMessagePage(id, options)
  },

  async getSessionLive(id) {
    return this.streamProjection.getSessionLive(id)
  },

  // 手动触发上下文压缩：压缩期间以 live.compaction 状态对外可见，
  // 失败时保留压缩状态与错误信息，便于前端展示与重试。
  async compactSession(id) {
    const value = await this.getOrCreateSession(id)
    const { session } = value
    const existingLive = this.liveSessions.get(session.sessionId)
    if (this.sessionRunIsActive(session.sessionId, value))
      throw new Error('当前会话仍在运行，请等待完成后再压缩上下文。')
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
  },

  async renameSession(id, name, options = {}) {
    return this.sessionLifecycle.renameSession(id, name, options)
  },

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
  },

  async getSessionThinkingState(id) {
    return this.providerPreferences.getSessionThinkingState(id)
  },

  async setSessionThinkingLevel(id, level) {
    return this.providerPreferences.setSessionThinkingLevel(id, level)
  },

  async setSessionPermission(id, mode) {
    return this.sessionLifecycle.setSessionPermission(id, mode, PERMISSION_MODES)
  },

  async setSessionExecutionMode(id, mode) {
    return this.sessionLifecycle.setSessionExecutionMode(id, normalizeExecutionMode(mode, ''))
  },

  resolveToolApproval(sessionId, approvalId, approved) {
    return this.permissions.resolve(sessionId, approvalId, approved)
  },

  async setSessionCwd(id, input) {
    return this.sessionLifecycle.setSessionCwd(id, input)
  },

  listDirectories(input) {
    return listWorkspaceDirectories(input, this.cwd)
  },

  listWorkspaceEntries(input) {
    return listWorkspaceEntries(input, this.cwd)
  },
}
