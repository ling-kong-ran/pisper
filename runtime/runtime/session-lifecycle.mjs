import { stat, unlink } from 'node:fs/promises'
import { resolve, sep } from 'node:path'
import { SessionManager } from './pi-coding-agent.mjs'
import {
  DEFAULT_EXECUTION_MODE,
  permissionModeForExecutionMode,
} from '../security/execution-mode.mjs'
import { isCompletedTurnBoundaryMessage } from './session-derivation.mjs'
import {
  appendTreePosition,
  projectSessionTree,
  projectSessionTreeLabels,
} from './session-tree.mjs'

const DEFAULT_SESSION_NAME = '新会话'
const MAX_RESIDENT_SESSION_RUNTIMES = 3
const SESSION_RUNTIME_IDLE_TTL_MS = 5 * 60 * 1000
const SESSION_HISTORY_CACHE_MEMORY_MULTIPLIER = 4

export class SessionLifecycle {
  constructor({
    cwd,
    sessionDir,
    sessions,
    pendingSessions,
    liveSessions,
    sessionHistoryCache,
    sessionHistoryPaths,
    sessionContextUsageCache,
    agentWakeupTimers,
    getSessionMeta,
    getSettingsManager,
    getGoals,
    getPlans,
    getMultiAgents,
    getPermissions,
    getBrowserAutomation,
    getExecutionMode,
    resolveDirectory,
    cleanSessionTitle,
    listStoredSessions,
    openStoredSession,
    saveSessionMeta,
    saveUsageLedger,
    getUsageLedger,
    createSessionRuntime,
    setSessionModel,
    syncGoalTools,
    pauseSessionGoal,
    invalidateProjection,
    getRuntimeState,
    setRuntimeVersion,
  }) {
    this.cwd = cwd
    this.sessionDir = sessionDir
    this.sessions = sessions
    this.pendingSessions = pendingSessions
    this.liveSessions = liveSessions
    this.sessionHistoryCache = sessionHistoryCache
    this.sessionHistoryPaths = sessionHistoryPaths
    this.sessionContextUsageCache = sessionContextUsageCache
    this.agentWakeupTimers = agentWakeupTimers
    this.getSessionMeta = getSessionMeta
    this.getSettingsManager = getSettingsManager
    this.getGoals = getGoals
    this.getPlans = getPlans
    this.getMultiAgents = getMultiAgents
    this.getPermissions = getPermissions
    this.getBrowserAutomation = getBrowserAutomation
    this.getExecutionMode = getExecutionMode
    this.resolveDirectory = resolveDirectory
    this.cleanSessionTitle = cleanSessionTitle
    this.listStoredSessions = listStoredSessions
    this.openStoredSession = openStoredSession
    this.saveSessionMeta = saveSessionMeta
    this.saveUsageLedger = saveUsageLedger
    this.getUsageLedger = getUsageLedger
    this.createSessionRuntime = createSessionRuntime
    this.setSessionModel = setSessionModel
    this.syncGoalTools = syncGoalTools
    this.pauseSessionGoal = pauseSessionGoal
    this.invalidateProjection = invalidateProjection
    this.getRuntimeState = getRuntimeState
    this.setRuntimeVersion = setRuntimeVersion
  }

  touchSessionRuntime(value) {
    if (value) value.lastAccessedAt = Date.now()
    return value
  }

  sessionRunIsActive(id, value = this.sessions.get(id)) {
    return Boolean(
      value?.runActive || this.liveSessions.get(id)?.streaming || value?.session?.isStreaming,
    )
  }

  sessionRuntimeIsProtected(id, value) {
    return Boolean(
      this.sessionRunIsActive(id, value) ||
      this.getGoals().get(id)?.status === 'active' ||
      this.agentWakeupTimers.has(id) ||
      value?.pendingAgentNotifications?.length ||
      this.getMultiAgents().hasActive?.(id),
    )
  }

  disposeSessionRuntime(id, value, { force = false } = {}) {
    if (!value || this.sessions.get(id) !== value || (!force && this.sessionRunIsActive(id, value)))
      return false
    this.getPermissions().resolveSession(id, false, '会话运行时已从内存释放，请重新发送消息。')
    try {
      value.session.dispose()
    } finally {
      this.sessions.delete(id)
      this.sessionContextUsageCache.delete(id)
      this.invalidateProjection(id)
      const sessionPath = value.session.sessionFile
      if (sessionPath) this.sessionHistoryCache.delete(sessionPath)
    }
    return true
  }

  evictIdleSessionRuntimes(exceptId = '', now = Date.now()) {
    const state = this.getRuntimeState()
    const maximum = Math.max(
      1,
      Number(state.maxResidentSessionRuntimes) || MAX_RESIDENT_SESSION_RUNTIMES,
    )
    const idleTtlMs = Math.max(
      0,
      Number(state.sessionRuntimeIdleTtlMs) || SESSION_RUNTIME_IDLE_TTL_MS,
    )
    const candidates = () =>
      [...this.sessions.entries()]
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

  async disposeSessions() {
    for (const [id, value] of this.sessions) {
      await this.pauseSessionGoal(id)
      this.getMultiAgents().abortParent(id)
      this.getPermissions().resolveSession(id, false, 'Agent Runtime 正在重新加载，工具未执行。')
      value.session.dispose()
      this.invalidateProjection(id)
    }
    this.sessions.clear()
  }

  invalidateSessionRuntimes() {
    const state = this.getRuntimeState()
    this.setRuntimeVersion(state.sessionRuntimeVersion + 1)
    for (const [id, value] of this.sessions) {
      if (this.sessionRunIsActive(id, value)) continue
      this.getMultiAgents().abortParent(id)
      this.getPermissions().resolveSession(
        id,
        false,
        'Agent Runtime resources changed before the tool could run.',
      )
      value.session.dispose()
      this.sessions.delete(id)
      this.invalidateProjection(id)
    }
  }

  getRuntimeDiagnostics() {
    const now = Date.now()
    const memory = process.memoryUsage()
    const state = this.getRuntimeState()
    const resident = [...this.sessions.entries()]
    const protectedSessions = resident.filter(([id, value]) =>
      this.sessionRuntimeIsProtected(id, value),
    ).length
    const idleAges = resident
      .filter(([id, value]) => !this.sessionRuntimeIsProtected(id, value))
      .map(([, value]) => Math.max(0, now - (value.lastAccessedAt || now)))
    const historySourceBytes = [...this.sessionHistoryCache.values()].reduce(
      (total, entry) => total + entry.size,
      0,
    )
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
        maxResident: state.maxResidentSessionRuntimes,
        idleTtlMs: state.sessionRuntimeIdleTtlMs,
      },
      historyCache: {
        entries: this.sessionHistoryCache.size,
        sourceBytes: historySourceBytes,
        estimatedBytes: historySourceBytes * SESSION_HISTORY_CACHE_MEMORY_MULTIPLIER,
        maxEntries: state.maxSessionHistoryCacheEntries,
        maxSourceBytes: state.maxSessionHistoryCacheSourceBytes,
        maxEstimatedBytes: state.maxSessionHistoryCacheEstimatedBytes,
      },
    }
  }

  async sessionWorkspaceCwd(id) {
    if (!id) return this.cwd
    const activeCwd = this.sessions.get(id)?.cwd
    if (activeCwd) return activeCwd
    const pendingCwd = this.pendingSessions.get(id)?.cwd
    if (pendingCwd) return pendingCwd
    const metaCwd = this.getSessionMeta()[id]?.cwd
    if (metaCwd) return metaCwd
    const stored = await this.findSessionInfo(id)
    return stored?.cwd || this.cwd
  }

  async listSessions() {
    const sessions = await this.listStoredSessions()
    const settings = this.getSettingsManager().getGlobalSettings()
    const sessionMeta = this.getSessionMeta()
    const goals = this.getGoals()
    const plans = this.getPlans()
    const multiAgents = this.getMultiAgents()
    const defaultModel =
      settings.defaultProvider && settings.defaultModel
        ? `${settings.defaultProvider}/${settings.defaultModel}`
        : ''
    const defaultThinkingLevel = settings.defaultThinkingLevel || 'medium'
    const childrenByParent = new Map()
    for (const [childId, meta] of Object.entries(sessionMeta)) {
      const parentId = String(meta?.parentSessionId || '')
      if (!parentId) continue
      childrenByParent.set(parentId, [...(childrenByParent.get(parentId) || []), childId])
    }
    const sessionValue = (id, value) => ({
      permissionMode:
        sessionMeta[id]?.permissionMode ||
        permissionModeForExecutionMode(this.getExecutionMode(id)),
      executionMode: this.getExecutionMode(id),
      goal: goals.get(id),
      plan: plans.get(id),
      agents: multiAgents
        .summaries(id)
        .filter((agent) => ['queued', 'starting', 'running'].includes(agent.status)),
      lineage: sessionMeta[id]?.parentSessionId
        ? {
            parentSessionId: sessionMeta[id].parentSessionId,
            sourceEntryId: sessionMeta[id].derivedFromEntryId || '',
            sourceSessionName: sessionMeta[id].derivedFromSessionName || '',
            derivedAt: sessionMeta[id].derivedAt || null,
            childSessionIds: childrenByParent.get(id) || [],
          }
        : childrenByParent.has(id)
          ? { childSessionIds: childrenByParent.get(id) }
          : null,
      ...value,
    })
    const result = sessions.map((session) => {
      const active = this.sessions.get(session.id)
      const contextModel = active?.session.model
        ? `${active.session.model.provider}/${active.session.model.id}`
        : sessionMeta[session.id]?.model || defaultModel
      return sessionValue(session.id, {
        id: session.id,
        name: active?.name || session.name || session.firstMessage || DEFAULT_SESSION_NAME,
        firstMessage: session.firstMessage || '',
        messageCount: active
          ? active.session.messages.filter((message) =>
              ['user', 'assistant'].includes(message.role),
            ).length
          : session.messageCount,
        model: contextModel,
        thinkingLevel:
          active?.session.thinkingLevel ||
          sessionMeta[session.id]?.thinkingLevel ||
          defaultThinkingLevel,
        cwd: active?.cwd || sessionMeta[session.id]?.cwd || session.cwd || this.cwd,
        created: session.created.toISOString(),
        modified: active?.modified || session.modified.toISOString(),
        streaming: this.sessionRunIsActive(session.id, active),
      })
    })
    const persistedIds = new Set(result.map((session) => session.id))
    for (const [id, value] of this.sessions) {
      if (persistedIds.has(id)) continue
      result.unshift(
        sessionValue(id, {
          id,
          name: value.name || DEFAULT_SESSION_NAME,
          firstMessage: '',
          messageCount: value.session.messages.filter((message) =>
            ['user', 'assistant'].includes(message.role),
          ).length,
          model: value.session.model
            ? `${value.session.model.provider}/${value.session.model.id}`
            : defaultModel,
          thinkingLevel: value.session.thinkingLevel || defaultThinkingLevel,
          cwd: value.cwd || this.cwd,
          created: value.created,
          modified: value.modified,
          streaming: this.sessionRunIsActive(id, value),
        }),
      )
    }
    for (const [id, value] of this.pendingSessions) {
      if (persistedIds.has(id) || this.sessions.has(id)) continue
      result.unshift(
        sessionValue(id, {
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
          agents: [],
        }),
      )
    }
    result.sort(
      (left, right) => new Date(right.modified).getTime() - new Date(left.modified).getTime(),
    )
    return result
  }

  async createSession(name, cwd) {
    const resolvedName = this.cleanSessionTitle(name) || DEFAULT_SESSION_NAME
    const effectiveCwd = await this.resolveDirectory(cwd, this.cwd)
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
    const sessionMeta = this.getSessionMeta()
    sessionMeta[id] = {
      ...(sessionMeta[id] || {}),
      name: resolvedName,
      manual: resolvedName !== DEFAULT_SESSION_NAME,
      cwd: effectiveCwd,
      executionMode: DEFAULT_EXECUTION_MODE,
      permissionMode: permissionModeForExecutionMode(DEFAULT_EXECUTION_MODE),
    }
    await this.saveSessionMeta()
    // The session file already exists on disk; refresh the stored-session
    // cache so the new conversation is visible to listSessions and
    // findSessionInfo even before its first prompt materializes it.
    await this.listStoredSessions({ refresh: true })
    const settings = this.getSettingsManager().getGlobalSettings()
    const model =
      settings.defaultProvider && settings.defaultModel
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
      permissionMode: sessionMeta[id].permissionMode,
      executionMode: this.getExecutionMode(id),
      goal: null,
      plan: this.getPlans().get(id),
      agents: [],
      contextUsage: null,
    }
  }

  async sessionTreeManager(id) {
    const sessionId = String(id || '').trim()
    if (!sessionId) throw new Error('会话不存在。')
    const active = this.sessions.get(sessionId)
    if (active?.session?.sessionManager) return active.session.sessionManager
    const pending = this.pendingSessions.get(sessionId)
    if (pending?.manager) return pending.manager
    const info = await this.findSessionInfo(sessionId)
    if (!info?.path) throw new Error('会话不存在。')
    const manager = this.openStoredSession(info.path)
    if (manager.getSessionId() !== sessionId) throw new Error('会话标识不匹配。')
    return manager
  }

  async getSessionTree(id) {
    const sessionId = String(id || '').trim()
    const manager = await this.sessionTreeManager(sessionId)
    return projectSessionTree(manager, {
      sessionId,
      streaming: this.sessionRunIsActive(sessionId, this.sessions.get(sessionId)),
    })
  }

  async searchSessionTreeLabels(query, options = {}) {
    const keyword = String(query || '')
      .replace(/\s+/g, ' ')
      .trim()
      .toLocaleLowerCase()
      .slice(0, 80)
    if (!keyword) return []
    const requestedLimit = Number(options?.limit)
    const limit = Number.isFinite(requestedLimit)
      ? Math.max(1, Math.min(50, Math.floor(requestedLimit)))
      : 20
    const sessions = (await this.listSessions()).sort(
      (left, right) => Date.parse(right.modified || '') - Date.parse(left.modified || ''),
    )
    const matches = []
    for (const session of sessions) {
      try {
        const manager = await this.sessionTreeManager(session.id)
        for (const label of projectSessionTreeLabels(manager, session)) {
          if (!label.label.toLocaleLowerCase().includes(keyword)) continue
          matches.push(label)
          if (matches.length >= limit) return matches
        }
      } catch {
        // A session may be deleted between the catalog and tree reads.
      }
    }
    return matches
  }

  async navigateSessionTree(id, targetEntryId, options = {}) {
    const sessionId = String(id || '').trim()
    const entryId = String(targetEntryId || '').trim()
    if (!sessionId || !entryId) throw new Error('会话树节点无效。')
    const existing = this.sessions.get(sessionId)
    if (this.sessionRunIsActive(sessionId, existing)) {
      throw new Error('当前会话正在运行，请等待完成或停止后再切换分支。')
    }
    const value = await this.getOrCreateSession(sessionId)
    if (this.sessionRunIsActive(sessionId, value)) {
      throw new Error('当前会话正在运行，请等待完成或停止后再切换分支。')
    }
    const manager = value.session.sessionManager
    if (!manager.getEntry(entryId)) throw new Error('会话树节点不存在。')
    if (manager.getLeafId() === entryId) {
      return {
        ...projectSessionTree(manager, { sessionId, streaming: false }),
        cancelled: false,
        editorText: null,
      }
    }

    value.runActive = true
    try {
      const result = await value.session.navigateTree(entryId, {
        summarize: Boolean(options?.summarize),
      })
      if (!result.cancelled && !result.summaryEntry) appendTreePosition(manager, entryId)
      value.modified = new Date().toISOString()
      this.sessionHistoryPaths.delete(sessionId)
      this.sessionContextUsageCache.delete(sessionId)
      if (value.session.sessionFile) this.sessionHistoryCache.delete(value.session.sessionFile)
      this.invalidateProjection(sessionId)
      await this.listStoredSessions({ refresh: true })
      return {
        ...projectSessionTree(manager, { sessionId, streaming: false }),
        cancelled: Boolean(result.cancelled),
        editorText: typeof result.editorText === 'string' ? result.editorText : null,
      }
    } finally {
      value.runActive = false
    }
  }

  async setSessionTreeLabel(id, targetEntryId, label) {
    const sessionId = String(id || '').trim()
    const entryId = String(targetEntryId || '').trim()
    const normalizedLabel = String(label || '')
      .replace(/\s+/g, ' ')
      .trim()
    if (!sessionId || !entryId) throw new Error('会话树节点无效。')
    if (normalizedLabel.length > 80) throw new Error('节点标签不能超过 80 个字符。')
    const existing = this.sessions.get(sessionId)
    if (this.sessionRunIsActive(sessionId, existing)) {
      throw new Error('当前会话正在运行，请等待完成或停止后再修改标签。')
    }
    const value = await this.getOrCreateSession(sessionId)
    if (this.sessionRunIsActive(sessionId, value)) {
      throw new Error('当前会话正在运行，请等待完成或停止后再修改标签。')
    }
    const manager = value.session.sessionManager
    if (!manager.getEntry(entryId)) throw new Error('会话树节点不存在。')
    if ((manager.getLabel(entryId) || '') === normalizedLabel) {
      return projectSessionTree(manager, { sessionId, streaming: false })
    }
    value.runActive = true
    try {
      manager.appendLabelChange(entryId, normalizedLabel || undefined)
      value.modified = new Date().toISOString()
      this.sessionHistoryPaths.delete(sessionId)
      this.sessionContextUsageCache.delete(sessionId)
      if (value.session.sessionFile) this.sessionHistoryCache.delete(value.session.sessionFile)
      this.invalidateProjection(sessionId)
      await this.listStoredSessions({ refresh: true })
      return projectSessionTree(manager, { sessionId, streaming: false })
    } finally {
      value.runActive = false
    }
  }

  async deriveSession(id, boundaryEntryId, name) {
    const sourceId = String(id || '').trim()
    const entryId = String(boundaryEntryId || '').trim()
    if (!sourceId || !entryId) throw new Error('衍生边界无效。')
    const active = this.sessions.get(sourceId)
    if (this.sessionRunIsActive(sourceId, active)) {
      throw new Error('当前会话正在运行，请等待完成或停止后再衍生。')
    }
    if (this.pendingSessions.has(sourceId)) throw new Error('当前会话还没有可衍生的完整回复。')

    const info = await this.findSessionInfo(sourceId)
    const sourcePath = active?.session?.sessionFile || info?.path
    const sourceFile = sourcePath ? await stat(sourcePath).catch(() => null) : null
    if (!sourceFile?.isFile()) throw new Error('源会话不存在或尚未持久化。')
    if (this.sessionRunIsActive(sourceId, this.sessions.get(sourceId))) {
      throw new Error('当前会话正在运行，请等待完成或停止后再衍生。')
    }

    const manager = this.openStoredSession(sourcePath)
    if (manager.getSessionId() !== sourceId) throw new Error('源会话标识不匹配。')
    const branch = manager.getBranch()
    const boundary = branch.find((entry) => entry?.id === entryId)
    if (boundary?.type !== 'message' || !isCompletedTurnBoundaryMessage(boundary.message)) {
      throw new Error('只能从已完成的 Agent 回复衍生会话。')
    }

    const sessionMeta = this.getSessionMeta()
    const sourceMeta = sessionMeta[sourceId] || {}
    const sourceName =
      active?.name || sourceMeta.name || info?.name || info?.firstMessage || DEFAULT_SESSION_NAME
    const derivedName =
      this.cleanSessionTitle(name) || `${this.cleanSessionTitle(sourceName)} · 衍生`
    const context = manager.buildSessionContext()
    const now = new Date().toISOString()
    let derivedPath = ''
    let derivedId = ''
    try {
      derivedPath = manager.createBranchedSession(entryId) || ''
      derivedId = manager.getSessionId()
      if (!derivedPath || !derivedId || derivedId === sourceId) {
        throw new Error('无法创建衍生会话文件。')
      }
      manager.appendSessionInfo(derivedName)
      const model =
        sourceMeta.model ||
        (context.model?.provider && context.model.modelId
          ? `${context.model.provider}/${context.model.modelId}`
          : '')
      const executionMode = this.getExecutionMode(sourceId)
      sessionMeta[derivedId] = {
        name: derivedName,
        manual: true,
        cwd: sourceMeta.cwd || active?.cwd || info?.cwd || manager.getCwd() || this.cwd,
        executionMode,
        permissionMode: sourceMeta.permissionMode || permissionModeForExecutionMode(executionMode),
        ...(model ? { model } : {}),
        thinkingLevel:
          context.thinkingLevel ||
          this.getSettingsManager().getGlobalSettings().defaultThinkingLevel ||
          'medium',
        parentSessionId: sourceId,
        derivedFromEntryId: entryId,
        derivedFromSessionName: sourceName,
        derivedAt: now,
      }
      await this.saveSessionMeta()
    } catch (error) {
      if (derivedId && sessionMeta[derivedId]) delete sessionMeta[derivedId]
      if (derivedPath) await unlink(derivedPath).catch(() => {})
      throw error
    }

    this.invalidateProjection(derivedId)
    await this.listStoredSessions({ refresh: true })
    const derived = (await this.listSessions()).find((session) => session.id === derivedId)
    if (derived) return derived
    return {
      id: derivedId,
      name: derivedName,
      firstMessage: info?.firstMessage || '',
      messageCount: context.messages.filter((message) =>
        ['user', 'assistant'].includes(message.role),
      ).length,
      model: sessionMeta[derivedId].model || '',
      thinkingLevel: context.thinkingLevel,
      cwd: sessionMeta[derivedId].cwd,
      created: now,
      modified: now,
      streaming: false,
      permissionMode: sessionMeta[derivedId].permissionMode,
      executionMode: sessionMeta[derivedId].executionMode,
      goal: null,
      plan: null,
      agents: [],
      lineage: {
        parentSessionId: sourceId,
        sourceEntryId: entryId,
        sourceSessionName: sourceName,
        derivedAt: now,
        childSessionIds: [],
      },
    }
  }

  async findSessionInfo(id) {
    const sessions = await this.listStoredSessions()
    let session = sessions.find((item) => item.id === id)
    if (!session) {
      // The stored-session cache may have been built before this session file
      // appeared on disk (a fresh session materialized after its first prompt).
      // Fall back to a disk scan so freshly created sessions never read as
      // "not found" once their resident runtime is released.
      session = (await this.listStoredSessions({ refresh: true })).find((item) => item.id === id)
    }
    return session || null
  }

  async renameSession(id, name, { manual = true } = {}) {
    const title = this.cleanSessionTitle(name)
    if (!title) throw new Error('会话标题不能为空。')
    const active = this.sessions.get(id)
    const pending = this.pendingSessions.get(id)
    if (this.sessionRunIsActive(id, active)) {
      throw new Error('当前会话正在运行，请完成或停止后再修改标题。')
    }
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
    const sessionMeta = this.getSessionMeta()
    sessionMeta[id] = { ...(sessionMeta[id] || {}), name: title, manual: Boolean(manual) }
    await this.saveSessionMeta()
    this.invalidateProjection(id, { transcript: false, activity: true, usage: false })
    return { id, name: title, manual: Boolean(manual) }
  }

  async setSessionPermission(id, mode, permissionModes) {
    const permissionMode = String(mode || '')
    if (!permissionModes.has(permissionMode)) throw new Error('权限模式无效。')
    if (
      !this.sessions.has(id) &&
      !this.pendingSessions.has(id) &&
      !(await this.findSessionInfo(id))
    )
      return null
    const active = this.sessions.get(id)
    if (this.sessionRunIsActive(id, active)) {
      throw new Error('当前会话正在运行，请完成或停止后再切换权限模式。')
    }
    const sessionMeta = this.getSessionMeta()
    sessionMeta[id] = { ...(sessionMeta[id] || {}), permissionMode }
    await this.saveSessionMeta()
    if (permissionMode !== 'ask') {
      this.getPermissions().resolveSession(
        id,
        true,
        `权限模式已切换为${permissionMode === 'ignore' ? '忽略' : '自动'}。`,
      )
    }
    this.invalidateProjection(id, { transcript: false, activity: true, usage: false })
    return { id, permissionMode, executionMode: this.getExecutionMode(id) }
  }

  async setSessionExecutionMode(id, executionMode) {
    if (!executionMode) throw new Error('执行模式无效。')
    if (
      !this.sessions.has(id) &&
      !this.pendingSessions.has(id) &&
      !(await this.findSessionInfo(id))
    )
      return null
    const active = this.sessions.get(id)
    if (this.sessionRunIsActive(id, active)) {
      throw new Error('当前会话正在运行，请完成或停止后再切换执行模式。')
    }
    const permissionMode = permissionModeForExecutionMode(executionMode)
    const sessionMeta = this.getSessionMeta()
    sessionMeta[id] = { ...(sessionMeta[id] || {}), executionMode, permissionMode }
    await this.saveSessionMeta()
    if (active) this.disposeSessionRuntime(id, active)
    this.getPermissions().resolveSession(
      id,
      executionMode === 'full-access',
      executionMode === 'full-access'
        ? '已切换为完全访问。'
        : '执行模式已切换，请重新发起工具调用。',
    )
    this.invalidateProjection(id, { transcript: false, activity: true, usage: false })
    return { id, executionMode, permissionMode }
  }

  async setSessionCwd(id, input) {
    const cwd = await this.resolveDirectory(input, this.cwd)
    const active = this.sessions.get(id)
    const pending = this.pendingSessions.get(id)
    const sessionMeta = this.getSessionMeta()
    if (pending) {
      pending.cwd = cwd
      pending.modified = new Date().toISOString()
      sessionMeta[id] = { ...(sessionMeta[id] || {}), cwd }
      await this.saveSessionMeta()
      this.invalidateProjection(id, { transcript: false, activity: true, usage: false })
      return { id, cwd }
    }
    if (this.sessionRunIsActive(id, active)) {
      throw new Error('当前会话正在运行，请完成或停止后再切换工作目录。')
    }
    const activeSessionFile = active?.session.sessionFile
    const activeSessionFileInfo = activeSessionFile
      ? await stat(activeSessionFile).catch(() => null)
      : null
    const info = activeSessionFileInfo?.isFile()
      ? { path: activeSessionFile, name: active.name }
      : await this.findSessionInfo(id)
    if (!active && !info) return null

    const name = active?.name || info?.name || sessionMeta[id]?.name || DEFAULT_SESSION_NAME
    const previousModel = active?.session.model
    if (active) {
      active.session.dispose()
      this.sessions.delete(id)
    }
    sessionMeta[id] = { ...(sessionMeta[id] || {}), cwd }
    await this.saveSessionMeta()

    const manager = info?.path
      ? this.openStoredSession(info.path)
      : SessionManager.create(this.cwd, this.sessionDir, { id })
    const next = await this.createSessionRuntime(manager, name)
    if (!info?.path) next.session.setSessionName(name)
    if (
      previousModel &&
      (!next.session.model ||
        previousModel.provider !== next.session.model.provider ||
        previousModel.id !== next.session.model.id)
    ) {
      await this.setSessionModel(id, previousModel.provider, previousModel.id)
    }
    this.invalidateProjection(id)
    return { id, cwd: next.cwd }
  }

  async getOrCreateSession(id) {
    const state = this.getRuntimeState()
    if (id && this.sessions.has(id)) {
      const current = this.sessions.get(id)
      if (
        this.sessionRunIsActive(id, current) ||
        (current.runtimeVersion ?? state.sessionRuntimeVersion) === state.sessionRuntimeVersion
      )
        return this.touchSessionRuntime(current)
      current.session.dispose()
      this.sessions.delete(id)
      this.invalidateProjection(id)
    }
    if (id && this.pendingSessions.has(id)) {
      const pending = this.pendingSessions.get(id)
      this.pendingSessions.delete(id)
      try {
        const value = await this.createSessionRuntime(pending.manager, pending.name)
        value.created = pending.created
        value.modified = pending.modified
        // The first prompt flushed the session transcript to disk; refresh the
        // stored-session cache so the materialized session survives resident
        // eviction (idle sweep or forced interruption) without turning into a
        // "session not found" on the next workspace switch.
        await this.listStoredSessions({ refresh: true })
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

  async abortSession(id) {
    const wakeupTimer = this.agentWakeupTimers.get(id)
    if (wakeupTimer) {
      clearTimeout(wakeupTimer)
      this.agentWakeupTimers.delete(id)
    }
    const value = this.sessions.get(id)
    if (!value) return false
    value.abortedAt = Date.now()
    await this.pauseSessionGoal(id)
    this.getMultiAgents().abortParent(id)
    this.getPermissions().resolveSession(id, false, '会话已停止，工具未执行。')
    value.session.clearQueue?.()
    await value.session.abort()
    this.invalidateProjection(id)
    return true
  }

  async deleteSession(id) {
    await this.getGoals().remove(id)
    await this.getPlans().remove(id)
    await this.getBrowserAutomation().closeSession(id)
    await this.getMultiAgents().removeParent(id)
    this.getPermissions().resolveSession(id, false, '会话已删除，工具未执行。')
    const active = this.sessions.get(id)
    const pending = this.pendingSessions.get(id)
    this.pendingSessions.delete(id)
    let sessionFile = active?.session.sessionFile
    if (active) {
      if (this.sessionRunIsActive(id, active)) await active.session.abort()
      active.session.dispose()
      this.sessions.delete(id)
    }
    if (!sessionFile) sessionFile = (await this.findSessionInfo(id))?.path
    this.sessionHistoryPaths.delete(id)
    this.sessionContextUsageCache.delete(id)
    if (sessionFile) this.sessionHistoryCache.delete(sessionFile)
    this.invalidateProjection(id)
    const sessionMeta = this.getSessionMeta()
    const usageLedger = this.getUsageLedger()
    const removeMetadata = async () => {
      if (sessionMeta[id]) {
        delete sessionMeta[id]
        await this.saveSessionMeta()
      }
      if (usageLedger.sessionScans?.[id]) {
        delete usageLedger.sessionScans[id]
        await this.saveUsageLedger()
      }
    }
    if (!sessionFile) {
      await removeMetadata()
      const state = this.getRuntimeState()
      if (state.storedSessionsCache) {
        state.storedSessionsCache = state.storedSessionsCache.filter((session) => session.id !== id)
      }
      return Boolean(active || pending)
    }
    const root = resolve(this.sessionDir)
    const target = resolve(sessionFile)
    if (target !== root && !target.startsWith(`${root}${sep}`)) {
      throw new Error('拒绝删除会话目录之外的文件。')
    }
    try {
      await unlink(target)
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
    await removeMetadata()
    const state = this.getRuntimeState()
    if (state.storedSessionsCache) {
      state.storedSessionsCache = state.storedSessionsCache.filter((session) => session.id !== id)
    }
    return true
  }
}
