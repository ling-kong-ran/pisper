// SessionLifecycle：会话生命周期管理——创建/列出/查找/重命名/删除会话，
// 常驻会话运行时（resident runtime）的缓存与回收，会话树导航/标签，
// 以及权限/执行模式/工作目录切换等元数据操作。
import { readFile, rename, stat, unlink, writeFile } from 'node:fs/promises'
import { dirname, resolve, sep } from 'node:path'
import { SessionManager } from './pi-coding-agent.mjs'
import {
  DEFAULT_EXECUTION_MODE,
  permissionModeForExecutionMode,
} from '../security/execution-mode.mjs'
import { isCompletedTurnBoundaryMessage } from './session-derivation.mjs'
import {
  appendTreePosition,
  findPendingTreePosition,
  isLineBoundary,
  projectSessionTree,
  scanSessionTreeLabels,
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
    upsertStoredSession,
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
    this.upsertStoredSession = upsertStoredSession
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
    // 会话文件标签扫描缓存：path → { mtimeMs, size, scannedBytes, lastId,
    // activeChain, assistantIds, entries }。扫描结果同步持久化到索引文件，
    // 重启后只需 stat 校验，不再全量遍历会话文件。
    this.sessionLabelScanCache = new Map()
    this.sessionLabelScans = new Map()
    this.sessionLabelIndex = null
    this.sessionLabelIndexDirty = false
    this.sessionLabelIndexFlush = null
  }

  touchSessionRuntime(value) {
    if (value) value.lastAccessedAt = Date.now()
    return value
  }

  touchStoredSession(id, modified) {
    const state = this.getRuntimeState()
    const cache = state.storedSessionsCache
    if (!Array.isArray(cache)) return
    const index = cache.findIndex((session) => session.id === id)
    if (index < 0) return
    const timestamp = new Date(modified)
    if (!Number.isFinite(timestamp.getTime())) return
    const next = [...cache]
    next[index] = { ...next[index], modified: timestamp }
    state.storedSessionsCache = next
  }

  // 会话是否在运行：显式 runActive、实时流、或引擎会话 isStreaming 任一为真。
  sessionRunIsActive(id, value = this.sessions.get(id)) {
    return Boolean(
      value?.runActive || this.liveSessions.get(id)?.streaming || value?.session?.isStreaming,
    )
  }

  // 会话运行时是否受保护（不可被闲置回收）：运行中、目标激活、
  // 有待唤醒的 Agent 完成通知或活动子 Agent 时保持驻留。
  sessionRuntimeIsProtected(id, value) {
    return Boolean(
      this.sessionRunIsActive(id, value) ||
      this.getGoals().get(id)?.status === 'active' ||
      this.agentWakeupTimers.has(id) ||
      value?.pendingAgentNotifications?.length ||
      this.getMultiAgents().hasActive?.(id),
    )
  }

  // 释放会话运行时：运行中且非 force 时拒绝；释放时清理权限审批、历史缓存与投影。
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

  // 闲置回收：先按空闲 TTL 驱逐超时运行时的 LRU，再按驻留上限淘汰溢出部分。
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

  // 关闭全部会话（进程退出路径）：暂停目标、终止子 Agent、释放权限并等标签索引落盘。
  async disposeSessions() {
    for (const [id, value] of this.sessions) {
      await this.pauseSessionGoal(id)
      this.getMultiAgents().abortParent(id)
      this.getPermissions().resolveSession(id, false, 'Agent Runtime 正在重新加载，工具未执行。')
      value.session.dispose()
      this.invalidateProjection(id)
    }
    this.sessions.clear()
    // 等标签索引的异步落盘链写完后才允许退出，避免临时文件残留。
    if (this.sessionLabelIndexFlush) {
      try {
        await this.sessionLabelIndexFlush
      } catch {}
    }
  }

  // 使会话运行时失效（工具/插件/MCP 等资源变更后）：版本号 +1，
  // 非运行中的会话全部释放，下次打开时按新资源重建。
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

  // 运行时诊断信息（内存/驻留会话/历史缓存），供 /api/runtime/diagnostics 使用。
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

  // 会话工作目录解析优先级：活动运行时 > 待物化会话 > 元数据 > 存储会话 > 默认 cwd。
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

  // 会话列表：磁盘会话 + 活动运行时 + 待物化会话合并，附目标/计划/Agent/血缘信息。
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

  // 创建会话：只物化 SessionManager 与元数据，真正的运行时等首次消息时才装配。
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
    // 新会话文件已落盘；把它的信息增量插入存储会话缓存，使 listSessions /
    // findSessionInfo 立即可见，而无需为单个新文件全量重扫所有会话（listAll
    // 会逐行读完每个 .jsonl，会话多时非常慢）。
    this.upsertStoredSession({
      path: manager.getSessionFile(),
      id,
      cwd: effectiveCwd,
      name: resolvedName,
      parentSessionPath: undefined,
      created: new Date(now),
      modified: new Date(now),
      messageCount: 0,
      firstMessage: '',
      allMessagesText: '',
    })
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

  // 获取会话的 SessionManager：活动运行时优先，其次待物化，最后从磁盘打开。
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

  // 会话谱系只读取元数据，不加载其它会话树，避免打开追忆时产生级联磁盘读取。
  getSessionLineage(id) {
    const sessionId = String(id || '').trim()
    const sessionMeta = this.getSessionMeta()
    const current = sessionMeta[sessionId] || {}
    const childSessionIds = Object.entries(sessionMeta)
      .filter(([, meta]) => String(meta?.parentSessionId || '') === sessionId)
      .map(([childId]) => childId)
    return current.parentSessionId || childSessionIds.length
      ? {
          parentSessionId: String(current.parentSessionId || ''),
          sourceEntryId: String(current.derivedFromEntryId || ''),
          sourceSessionName: String(current.derivedFromSessionName || ''),
          derivedAt: current.derivedAt || null,
          childSessionIds,
        }
      : null
  }

  // 会话树投影（带运行状态和跨会话谱系标记）。
  async getSessionTree(id) {
    const sessionId = String(id || '').trim()
    const manager = await this.sessionTreeManager(sessionId)
    return {
      ...projectSessionTree(manager, {
        sessionId,
        streaming: this.sessionRunIsActive(sessionId, this.sessions.get(sessionId)),
      }),
      lineage: this.getSessionLineage(sessionId),
    }
  }

  // 标签索引路径：与会话目录同级，独立于会话文件本身。
  sessionLabelIndexPath() {
    return resolve(dirname(this.sessionDir), 'pisper-session-label-index.json')
  }

  // 加载标签索引：磁盘快照直接装入内存缓存，重启后文件 (mtime, size) 未变就无需重扫。
  async ensureSessionLabelIndex() {
    if (this.sessionLabelIndex) return this.sessionLabelIndex
    let files = {}
    try {
      const parsed = JSON.parse(await readFile(this.sessionLabelIndexPath(), 'utf8'))
      if (parsed && parsed.version === 1 && parsed.files && typeof parsed.files === 'object') {
        files = parsed.files
      }
    } catch {
      // 索引不存在或损坏时从零重建。
    }
    // 磁盘索引中的快照直接装入内存缓存：重启后只要文件 (mtime, size)
    // 未变，搜索就完全不需要读会话文件。
    for (const [path, snapshot] of Object.entries(files)) {
      if (
        !snapshot ||
        !Array.isArray(snapshot.entries) ||
        !Array.isArray(snapshot.activeChain) ||
        !Array.isArray(snapshot.assistantIds)
      )
        continue
      this.sessionLabelScanCache.set(path, snapshot)
    }
    this.sessionLabelIndex = files
    this.sessionLabelIndexDirty = false
    return this.sessionLabelIndex
  }

  // 标签索引落盘：串行化 + 临时文件 rename 原子替换。
  flushSessionLabelIndex() {
    if (!this.sessionLabelIndexDirty || !this.sessionLabelIndex) return
    this.sessionLabelIndexDirty = false
    const snapshot = JSON.stringify({ version: 1, files: this.sessionLabelIndex })
    // 串行化写入：并发变更按顺序落盘，后写者胜。
    this.sessionLabelIndexFlush = (this.sessionLabelIndexFlush || Promise.resolve())
      .then(async () => {
        const target = this.sessionLabelIndexPath()
        const tmp = `${target}.${process.pid}.${Date.now()}.tmp`
        try {
          await writeFile(tmp, snapshot)
          await rename(tmp, target)
        } catch {
          this.sessionLabelIndexDirty = true
        }
      })
      .catch(() => {})
    return this.sessionLabelIndexFlush
  }

  // 重新扫描单个会话文件的标签：可增量则只扫新增字节，否则全量；结果写入缓存与索引。
  async rescanSessionLabelFile(path, fileStats, cached) {
    // 文件变大且上次扫描边界在完整行尾时只扫新追加的字节；否则（变小、
    // 原地重写、边界在半行中间）全量扫描，保证增量边界可靠。
    const canTail = Boolean(
      cached && cached.size < fileStats.size && (await isLineBoundary(path, cached.scannedBytes)),
    )
    const scanned = await scanSessionTreeLabels(path, canTail ? { previous: cached } : {})
    const entries = scanned.labels.map((entry) => ({
      sessionId: entry.sessionId,
      sessionName: entry.sessionName,
      sessionCreated: entry.sessionCreated,
      entryId: entry.entryId,
      label: entry.label,
      nodeTimestamp: entry.nodeTimestamp,
      active: entry.active,
    }))
    const next = {
      mtimeMs: fileStats.mtimeMs,
      size: fileStats.size,
      scannedBytes: scanned.scannedBytes,
      lastId: scanned.lastId,
      activeChain: scanned.activeChain,
      assistantIds: scanned.assistantIds,
      entries,
    }
    this.sessionLabelScanCache.set(path, next)
    while (this.sessionLabelScanCache.size > 400) {
      const oldest = this.sessionLabelScanCache.keys().next().value
      if (!oldest) break
      this.sessionLabelScanCache.delete(oldest)
      delete this.sessionLabelIndex?.[oldest]
    }
    const index = await this.ensureSessionLabelIndex()
    index[path] = next
    this.sessionLabelIndexDirty = true
    void this.flushSessionLabelIndex()
    return entries
  }

  // 读取某会话文件的标签条目（带 mtime/size 缓存与并发去重）。
  async sessionTreeLabelEntries(path) {
    let fileStats = null
    try {
      fileStats = await stat(path)
    } catch {
      // 文件在目录列举与扫描之间被删除。
      this.sessionLabelScanCache.delete(path)
      const index = await this.ensureSessionLabelIndex()
      if (index[path]) {
        delete index[path]
        this.sessionLabelIndexDirty = true
        void this.flushSessionLabelIndex()
      }
      return []
    }
    const cached = this.sessionLabelScanCache.get(path)
    if (cached && cached.mtimeMs === fileStats.mtimeMs && cached.size === fileStats.size)
      return cached.entries
    // 同一文件的并发扫描共享一次结果（连续按键会触发多次搜索）。
    let inflight = this.sessionLabelScans.get(path)
    if (!inflight) {
      inflight = this.rescanSessionLabelFile(path, fileStats, cached)
      this.sessionLabelScans.set(path, inflight)
      void inflight.finally(() => {
        if (this.sessionLabelScans.get(path) === inflight) this.sessionLabelScans.delete(path)
      })
    }
    return inflight
  }

  // 标签搜索：空关键字列出全部；带关键字时按最近修改排序、分批并发扫描（每批 8 个文件）。
  async searchSessionTreeLabels(query, options = {}) {
    const keyword = String(query || '')
      .replace(/\s+/g, ' ')
      .trim()
      .toLocaleLowerCase()
      .slice(0, 80)
    const requestedLimit = Number(options?.limit)
    // 空关键字 = 列出全部标签（标签页用）；有关键字时默认限制 20 条。
    const limit = Number.isFinite(requestedLimit)
      ? Math.max(1, Math.min(1000, Math.floor(requestedLimit)))
      : keyword
        ? 20
        : 500
    // 流式只扫 label 行 + id→parentId 索引，不加载会话树或消息内容；
    // 扫描结果按文件 (mtime, size) 缓存，未变化的会话不会重复读取；
    // 冷启动时按批次并发扫描（每批 8 个文件），把首次搜索延迟压下去。
    const storedById = new Map((await this.listStoredSessions()).map((s) => [s.id, s]))
    const sessions = (await this.listSessions()).sort(
      (left, right) => Date.parse(right.modified || '') - Date.parse(left.modified || ''),
    )
    const candidates = []
    for (const session of sessions) {
      const stored = storedById.get(session.id)
      if (!stored?.path) continue
      candidates.push({ session, path: stored.path })
    }
    const matches = []
    const SCAN_CONCURRENCY = 8
    for (
      let start = 0;
      start < candidates.length && matches.length < limit;
      start += SCAN_CONCURRENCY
    ) {
      const batch = candidates.slice(start, start + SCAN_CONCURRENCY)
      const found = await Promise.all(
        batch.map(async ({ session, path }) => {
          try {
            const entries = await this.sessionTreeLabelEntries(path)
            return entries
              .filter((entry) => entry.label.toLocaleLowerCase().includes(keyword))
              .map((entry) => ({ session, entry }))
          } catch {
            // A session may be deleted between the catalog and file scans.
            return []
          }
        }),
      )
      for (const group of found) {
        for (const { session, entry } of group) {
          matches.push({
            sessionId: session.id || entry.sessionId,
            sessionName: String(session.name || entry.sessionName || ''),
            sessionCreated: String(session.created || entry.sessionCreated || ''),
            sessionModified: String(session.modified || ''),
            entryId: entry.entryId,
            label: entry.label,
            summary: '',
            nodeTimestamp: entry.nodeTimestamp,
            active: entry.active,
          })
          if (matches.length >= limit) return matches
        }
      }
    }
    return matches
  }

  // 树导航（切换分支）：通过引擎的 navigateTree 重建会话，可选择是否摘要新分支。
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
    const pendingPositionId = findPendingTreePosition(manager, entryId)
    const navigationEntryId = pendingPositionId || entryId
    if (manager.getLeafId() === navigationEntryId) {
      const navigation = { cancelled: false, editorText: null }
      return options?.includeTree === false
        ? navigation
        : {
            ...projectSessionTree(manager, { sessionId, streaming: false }),
            lineage: this.getSessionLineage(sessionId),
            ...navigation,
          }
    }

    value.runActive = true
    try {
      const result = await value.session.navigateTree(navigationEntryId, {
        summarize: Boolean(options?.summarize),
      })
      if (!result.cancelled && !result.summaryEntry && !pendingPositionId)
        appendTreePosition(manager, entryId)
      value.modified = new Date().toISOString()
      this.touchStoredSession(sessionId, value.modified)
      this.sessionHistoryPaths.delete(sessionId)
      this.sessionContextUsageCache.delete(sessionId)
      if (value.session.sessionFile) this.sessionHistoryCache.delete(value.session.sessionFile)
      this.invalidateProjection(sessionId)
      const navigation = {
        cancelled: Boolean(result.cancelled),
        editorText: typeof result.editorText === 'string' ? result.editorText : null,
      }
      return options?.includeTree === false
        ? navigation
        : {
            ...projectSessionTree(manager, { sessionId, streaming: false }),
            lineage: this.getSessionLineage(sessionId),
            ...navigation,
          }
    } finally {
      value.runActive = false
    }
  }

  // 设置节点标签：空标签 = 删除（appendLabelChange 写墓碑）；返回更新后的树。
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
      return {
        ...projectSessionTree(manager, { sessionId, streaming: false }),
        lineage: this.getSessionLineage(sessionId),
      }
    }
    value.runActive = true
    try {
      manager.appendLabelChange(entryId, normalizedLabel || undefined)
      value.modified = new Date().toISOString()
      this.touchStoredSession(sessionId, value.modified)
      this.sessionHistoryPaths.delete(sessionId)
      this.sessionContextUsageCache.delete(sessionId)
      if (value.session.sessionFile) this.sessionHistoryCache.delete(value.session.sessionFile)
      this.invalidateProjection(sessionId)
      return {
        ...projectSessionTree(manager, { sessionId, streaming: false }),
        lineage: this.getSessionLineage(sessionId),
      }
    } finally {
      value.runActive = false
    }
  }

  // 从历史节点衍生新会话：只允许已完成回复的边界；源会话运行中也允许（基于磁盘快照分支）。
  async deriveSession(id, boundaryEntryId, name) {
    const sourceId = String(id || '').trim()
    const entryId = String(boundaryEntryId || '').trim()
    if (!sourceId || !entryId) throw new Error('衍生边界无效。')
    const active = this.sessions.get(sourceId)
    if (this.pendingSessions.has(sourceId)) throw new Error('当前会话还没有可衍生的完整回复。')

    const info = await this.findSessionInfo(sourceId)
    const sourcePath = active?.session?.sessionFile || info?.path
    const sourceFile = sourcePath ? await stat(sourcePath).catch(() => null) : null
    if (!sourceFile?.isFile()) throw new Error('源会话不存在或尚未持久化。')

    // 从已完成的历史节点衍生时，即使源会话仍在流式生成也允许：
    // 历史节点已持久化到磁盘，openStoredSession 只会读到最新写入点，
    // 而 createBranchedSession 在独立的 manager 副本上写新文件，不触碰源会话。
    const manager = this.openStoredSession(sourcePath)
    if (manager.getSessionId() !== sourceId) throw new Error('源会话标识不匹配。')
    const branch = manager.getBranch(entryId)
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
    // 衍生会话文件已写入；用内存中的条目统计消息数后增量插入缓存，
    // 避免全量重扫。buildSessionContext 基于 openStoredSession 已加载的
    // fileEntries，不会重读磁盘。
    let derivedMessages = []
    let derivedInfoOk = true
    try {
      derivedMessages = manager.buildSessionContext().messages || []
    } catch {
      // 上下文构造失败时回退到全量重扫，保证衍生会话仍可被列出。
      derivedInfoOk = false
      await this.listStoredSessions({ refresh: true })
    }
    if (derivedInfoOk) {
      const firstUser = derivedMessages.find((message) => message?.role === 'user')
      this.upsertStoredSession({
        path: derivedPath,
        id: derivedId,
        cwd: sessionMeta[derivedId]?.cwd,
        name: derivedName,
        parentSessionPath: sourcePath,
        created: new Date(now),
        modified: new Date(now),
        messageCount: derivedMessages.filter((message) =>
          ['user', 'assistant'].includes(message?.role),
        ).length,
        firstMessage: firstUser ? String(firstUser.content || '') : '',
        allMessagesText: '',
      })
    }
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

  // 查找会话信息：缓存 miss 时回退到磁盘全量扫描（新会话文件可能晚于缓存构建）。
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

  // 重命名会话：同时更新运行时/待物化/存储三处名称并写回元数据。
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

  // 切换权限模式：非 ask 模式立即结算（approve/deny）全部待审批项。
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

  // 切换执行模式：运行中则仅标记 runtimeVersion 失效（下次 prompt 重建工具集），
  // 否则直接释放运行时；并同步权限模式。
  async setSessionExecutionMode(id, executionMode) {
    if (!executionMode) throw new Error('执行模式无效。')
    if (
      !this.sessions.has(id) &&
      !this.pendingSessions.has(id) &&
      !(await this.findSessionInfo(id))
    )
      return null
    const active = this.sessions.get(id)
    const running = this.sessionRunIsActive(id, active)
    const permissionMode = permissionModeForExecutionMode(executionMode)
    const sessionMeta = this.getSessionMeta()
    sessionMeta[id] = { ...(sessionMeta[id] || {}), executionMode, permissionMode }
    await this.saveSessionMeta()
    if (running) {
      // Authorization reads session metadata for every tool call. Keep this run alive,
      // then force the next prompt to rebuild the mode-specific tool catalog.
      active.runtimeVersion = -1
    } else if (active) {
      this.disposeSessionRuntime(id, active)
    }
    this.getPermissions().resolveSession(
      id,
      executionMode === 'full-access',
      executionMode === 'full-access'
        ? '已切换为完全访问。'
        : '执行模式已切换，请按新权限重新发起工具调用。',
    )
    this.invalidateProjection(id, { transcript: false, activity: true, usage: false })
    return { id, executionMode, permissionMode }
  }

  // 切换会话工作目录：重建会话运行时（新 cwd 的工具/资源），保留原模型。
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

  // 获取或创建会话运行时：优先活动缓存，其次待物化，再磁盘恢复，最后新建。
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
        // 第一次 prompt 已将会话写盘；把最新信息增量插入存储会话缓存，使该
        // 会话在常驻运行时被回收（闲置清扫/强制中断）后仍可被列出与查找，
        // 无需全量重扫所有会话文件。
        const messages = value.session?.messages || []
        const firstUser = messages.find((message) => message?.role === 'user')
        this.upsertStoredSession({
          path: pending.manager.getSessionFile() || value.session?.sessionFile,
          id,
          cwd: value.cwd,
          name: value.name,
          parentSessionPath: undefined,
          created: new Date(value.created),
          modified: new Date(value.modified),
          messageCount: messages.filter((message) => ['user', 'assistant'].includes(message?.role))
            .length,
          firstMessage: firstUser ? String(firstUser.content || '') : '',
          allMessagesText: '',
        })
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

  // 中止会话运行：清理唤醒定时器、暂停目标、终止子 Agent、结算审批并 abort 引擎会话。
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

  // 删除会话：清理目标/计划/浏览器会话/子 Agent/审批，删除会话文件（限定在会话目录内）。
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
