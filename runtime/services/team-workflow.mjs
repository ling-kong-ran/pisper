// Team 工作流服务：在 MultiAgentService 的运行能力之上维护团队任务图、角色、依赖、
// 文件所有权和完成屏障，避免把一组普通 subagent 调用误认为完整的团队执行。
import { randomUUID } from 'node:crypto'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
export const TEAM_EXECUTION_MARKER = '[Pisper internal team execution]'
export const TEAM_WORKFLOW_STATUSES = Object.freeze(
  new Set(['active', 'paused', 'budget_limited', 'stalled', 'complete']),
)
export const TEAM_TASK_STATUSES = Object.freeze(
  new Set(['queued', 'starting', 'running', 'completed', 'failed', 'interrupted', 'blocked']),
)
export const TERMINAL_TEAM_TASK_STATUSES = Object.freeze(
  new Set(['completed', 'failed', 'interrupted']),
)
export const MAX_TEAM_TASKS = 64
export const MAX_TEAM_FILES_PER_TASK = 96
export const MAX_TEAM_DEPENDENCIES = 32
export const MAX_TEAM_ROLE_CHARS = 80
export const MAX_TEAM_COMMUNICATIONS = 128
export const MAX_TEAM_COMMUNICATION_CHARS = 12_000
export const MAX_TEAM_SCRIPT_PATH_CHARS = 240
export const MAX_TEAM_TASK_MESSAGE_CHARS = 12_000
export const MAX_TEAM_TASK_OUTPUT_CHARS = 12_000
export const MAX_TEAM_SUMMARY_CHARS = 12_000
// 失败重试由主 Agent 根据错误证据决定；此服务不以固定次数截断仍可恢复的任务。
export const MAX_TEAM_TASK_ATTEMPTS = null
// 保留旧导出以兼容调用方，但 Team 不再用固定时钟终止工作流。
export const MAX_TEAM_DURATION_MS = null
export const MAX_TEAM_IDLE_MS = null
export const TEAM_TASK_LEASE_MS = 10 * 60 * 1000
export const MAX_TEAM_FILE_CHARS = 240

function clone(value) {
  return value ? JSON.parse(JSON.stringify(value)) : null
}

function nowIso(now = Date.now()) {
  return new Date(now).toISOString()
}

function normalizeName(value) {
  return String(value || '')
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, '_')
    .replace(/_+/g, '_')
    .replace(/^_|_$/g, '')
    .slice(0, 48)
}

function normalizeFileScope(value) {
  const text = String(value || '')
    .trim()
    .replaceAll('\\', '/')
    .replace(/^\.\//, '')
    .replace(/\/+$/, '')
  return text.slice(0, MAX_TEAM_FILE_CHARS)
}

function normalizeFiles(files) {
  return [
    ...new Set(
      (Array.isArray(files) ? files : [])
        .map(normalizeFileScope)
        .filter(Boolean)
        .slice(0, MAX_TEAM_FILES_PER_TASK),
    ),
  ]
}

function normalizeDependencies(dependencies) {
  return [
    ...new Set(
      (Array.isArray(dependencies) ? dependencies : [])
        .map(normalizeName)
        .filter(Boolean)
        .slice(0, MAX_TEAM_DEPENDENCIES),
    ),
  ]
}

function scopeOverlaps(left, right) {
  const a = normalizeFileScope(left)
  const b = normalizeFileScope(right)
  if (!a || !b) return false
  return a === b || a.startsWith(`${b}/`) || b.startsWith(`${a}/`)
}

function scopesOverlap(left, right) {
  return left.some((a) => right.some((b) => scopeOverlaps(a, b)))
}

function dependenciesComplete(team, task) {
  return task.dependsOn.every((dependency) => taskByName(team, dependency)?.status === 'completed')
}

function dependencyIncludes(team, task, candidateName, visited = new Set()) {
  for (const dependency of task.dependsOn) {
    if (dependency === candidateName) return true
    if (visited.has(dependency)) continue
    visited.add(dependency)
    const parent = taskByName(team, dependency)
    if (parent && dependencyIncludes(team, parent, candidateName, visited)) return true
  }
  return false
}

function hasDependencyCycle(team, task, dependencies) {
  return dependencies.some((dependency) => {
    const parent = taskByName(team, dependency)
    return (
      parent &&
      (parent.taskName === task.taskName || dependencyIncludes(team, parent, task.taskName))
    )
  })
}

function durableTask(task) {
  return {
    id: task.id,
    taskName: task.taskName,
    role: task.role,
    message: task.message,
    files: [...task.files],
    dependsOn: [...task.dependsOn],
    agentId: task.agentId,
    leaseId: task.leaseId,
    claimedAt: task.claimedAt,
    leaseExpiresAt: task.leaseExpiresAt,
    blockedReason: task.blockedReason,
    status: task.status,
    output: task.output,
    error: task.error,
    attempts: task.attempts,
    createdAt: task.createdAt,
    updatedAt: task.updatedAt,
    completedAt: task.completedAt,
    autoStart: task.autoStart === true,
    workflowFingerprint: task.workflowFingerprint,
  }
}

function normalizeCommunication(value) {
  const message = String(value?.message || '')
    .trim()
    .slice(0, MAX_TEAM_COMMUNICATION_CHARS)
  if (!message) return null
  return {
    id: String(value?.id || randomUUID()),
    fromAgentId: String(value?.fromAgentId || '').trim(),
    fromTaskName: String(value?.fromTaskName || '')
      .trim()
      .slice(0, 48),
    toAgentId: String(value?.toAgentId || '').trim(),
    toTaskName: String(value?.toTaskName || '')
      .trim()
      .slice(0, 48),
    message,
    status: ['queued', 'delivered'].includes(value?.status) ? value.status : 'queued',
    sentAt: value?.sentAt || nowIso(),
  }
}

function durableTeam(team) {
  return {
    id: team.id,
    sessionId: team.sessionId,
    goalId: team.goalId,
    objective: team.objective,
    status: team.status,
    tokenBudget: team.tokenBudget,
    tokenBudgetExplicit: team.tokenBudgetExplicit === true,
    tasks: Object.fromEntries(
      Object.entries(team.tasks || {}).map(([id, task]) => [id, durableTask(task)]),
    ),
    conflicts: [...(team.conflicts || [])],
    summaryText: team.summaryText,
    scriptPath: team.scriptPath,
    communications: (team.communications || []).map(normalizeCommunication).filter(Boolean),
    startedAt: team.startedAt,
    createdAt: team.createdAt,
    updatedAt: team.updatedAt,
    lastProgressAt: team.lastProgressAt,
  }
}

function normalizeTask(value) {
  const taskName = normalizeName(value?.taskName)
  const status = TEAM_TASK_STATUSES.has(value?.status) ? value.status : 'failed'
  if (!taskName || !String(value?.id || '').trim()) return null
  return {
    id: String(value.id),
    taskName,
    role: String(value?.role || '')
      .trim()
      .slice(0, MAX_TEAM_ROLE_CHARS),
    message: String(value?.message || '')
      .trim()
      .slice(0, MAX_TEAM_TASK_MESSAGE_CHARS),
    files: normalizeFiles(value?.files),
    dependsOn: normalizeDependencies(value?.dependsOn),
    agentId: String(value?.agentId || '').trim(),
    leaseId: String(value?.leaseId || '').trim(),
    claimedAt: value?.claimedAt || null,
    leaseExpiresAt: value?.leaseExpiresAt || null,
    blockedReason: String(value?.blockedReason || '').slice(0, 1_000),
    status,
    output: String(value?.output || '').slice(0, MAX_TEAM_TASK_OUTPUT_CHARS),
    error: String(value?.error || '').slice(0, 1_000),
    attempts: Math.max(0, Math.floor(Number(value?.attempts) || 0)),
    createdAt: value?.createdAt || nowIso(),
    updatedAt: value?.updatedAt || nowIso(),
    completedAt: value?.completedAt || null,
    autoStart: value?.autoStart === true,
    workflowFingerprint: String(value?.workflowFingerprint || '').slice(0, 64),
  }
}

function normalizeTeam(value, sessionId) {
  if (!value || typeof value !== 'object' || !String(value.id || '').trim()) return null
  const tasks = {}
  for (const [id, task] of Object.entries(value.tasks || {})) {
    const normalized = normalizeTask({ ...task, id: task?.id || id })
    if (normalized) tasks[normalized.id] = normalized
  }
  const explicitBudget =
    value.tokenBudgetExplicit === true && positiveBudget(value.tokenBudget) != null
  const storedStatus = TEAM_WORKFLOW_STATUSES.has(value.status) ? value.status : 'paused'
  const status = storedStatus === 'budget_limited' && !explicitBudget ? 'paused' : storedStatus
  return {
    id: String(value.id),
    sessionId,
    goalId: String(value.goalId || ''),
    objective: String(value.objective || '').trim(),
    status,
    tokenBudget: explicitBudget ? positiveBudget(value.tokenBudget) : null,
    tokenBudgetExplicit: explicitBudget,
    tasks,
    createdAt: value.createdAt || nowIso(),
    updatedAt: value.updatedAt || nowIso(),
    lastProgressAt: value.lastProgressAt || value.updatedAt || nowIso(),
    conflicts: Array.isArray(value.conflicts) ? value.conflicts.slice(-24) : [],
    summaryText: String(value.summaryText || '').slice(0, MAX_TEAM_SUMMARY_CHARS),
    scriptPath: String(value.scriptPath || '')
      .trim()
      .slice(0, MAX_TEAM_SCRIPT_PATH_CHARS),
    communications: (Array.isArray(value.communications) ? value.communications : [])
      .map(normalizeCommunication)
      .filter(Boolean)
      .slice(-MAX_TEAM_COMMUNICATIONS),
    startedAt: value.startedAt || value.createdAt || nowIso(),
  }
}

function positiveBudget(value) {
  const budget = Math.round(Number(value))
  return Number.isFinite(budget) && budget > 0 ? budget : null
}

function publicTask(task) {
  if (!task) return null
  return { ...durableTask(task), files: [...task.files], dependsOn: [...task.dependsOn] }
}

function publicTeam(team, now = Date.now()) {
  if (!team) return null
  const tasks = Object.values(team.tasks || {}).map(publicTask)
  const completed = tasks.filter((task) => task.status === 'completed').length
  const active = tasks.filter((task) => !TERMINAL_TEAM_TASK_STATUSES.has(task.status)).length
  return {
    id: team.id,
    sessionId: team.sessionId,
    goalId: team.goalId,
    objective: team.objective,
    status: team.status,
    tokenBudget: team.tokenBudget,
    tokenBudgetExplicit: team.tokenBudgetExplicit === true,
    tasks,
    completedTaskCount: completed,
    activeTaskCount: active,
    taskCount: tasks.length,
    updatedAt: team.updatedAt,
    lastProgressAt: team.lastProgressAt,
    startedAt: team.startedAt,
    elapsedMs: Math.max(0, now - new Date(team.startedAt).getTime()),
    stalled: team.status === 'stalled',
    conflicts: [...(team.conflicts || [])],
    scriptPath: team.scriptPath || '',
    communications: (team.communications || []).map(normalizeCommunication).filter(Boolean),
    blockers: tasks
      .filter((task) => task.status !== 'completed')
      .map((task) => ({
        taskName: task.taskName,
        status: task.status,
        reason:
          task.error ||
          task.blockedReason ||
          (task.dependsOn.length ? `等待依赖：${task.dependsOn.join(', ')}` : '工作流尚未完成'),
      })),
    summary: {
      text: team.summaryText,
      completed: tasks
        .filter((task) => task.status === 'completed')
        .map((task) => ({ taskName: task.taskName, role: task.role, output: task.output })),
    },
  }
}

export function compactTeamProjection(team) {
  if (!team) return null
  const tasks = (team.tasks || []).map((task) => ({
    id: task.id,
    taskName: task.taskName,
    role: task.role,
    status: task.status,
    message: String(task.message || '').slice(0, 500),
    dependsOn: Array.isArray(task.dependsOn) ? task.dependsOn.slice(0, 8) : [],
    agentId: task.agentId,
    leaseId: task.leaseId,
    blockedReason: String(task.blockedReason || '').slice(0, 250),
    output: String(task.output || '').slice(0, 500),
    error: String(task.error || '').slice(0, 250),
    attempts: task.attempts,
    createdAt: task.createdAt,
    updatedAt: task.updatedAt,
    completedAt: task.completedAt,
    autoStart: task.autoStart,
  }))
  return {
    ...team,
    objective: String(team.objective || '').slice(0, 2_000),
    tasks,
    communications: (team.communications || []).slice(-8).map((communication) => ({
      ...communication,
      message: String(communication.message || '').slice(0, 500),
    })),
    conflicts: (team.conflicts || []).slice(-8).map((conflict) => ({
      taskName: conflict.taskName,
      files: Array.isArray(conflict.files) ? conflict.files.slice(0, 4) : [],
      message: String(conflict.message || '').slice(0, 500),
      at: conflict.at,
    })),
    blockers: (team.blockers || []).map((blocker) => ({
      taskName: blocker.taskName,
      reason: String(blocker.reason || '').slice(0, 250),
    })),
    summary: {
      text: String(team.summary?.text || '').slice(0, 4_000),
      completed: tasks
        .filter((task) => task.status === 'completed')
        .map(({ taskName, role }) => ({ taskName, role })),
    },
  }
}

export function projectGoalTeam(team, goal, { compact = false } = {}) {
  if (!team || goal?.mode !== 'team') return null
  const projection = {
    ...team,
    tokenUsed: goal.tokensUsed,
    tokenBudget: goal.teamTokenBudget ?? team.tokenBudget,
  }
  return compact ? compactTeamProjection(projection) : projection
}

// 判断当前轮次是否应附带团队快照：goal/team 请求或目标仍 active 时附带；
// 否则附带 null（客户端把 null 解读为清除信号），避免陈旧团队面板（如重启后
// 被暂停的团队）泄漏到 plan/普通模式的轮次。
export function shouldAttachTeamSnapshot({ goalMode = false, teamMode = false, goal } = {}) {
  return Boolean(goalMode || teamMode || goal?.status === 'active')
}

// 会话列表投影守卫：团队记录只跟随仍活跃的目标投影给客户端。
// 已完成/取消目标的团队属于当时的运行轮次，若随列表回灌，
// 刷新或重新同步后会残留「团队已完成」面板。
export function projectStoredTeam(goal, team) {
  if (!team) return null
  if (!goal) return null
  if (goal.status === 'complete' || goal.status === 'cancelled') return null
  return team
}

function interruptActiveTasks(team, now = Date.now()) {
  let changed = false
  for (const task of Object.values(team?.tasks || {})) {
    if (!['starting', 'running', 'interrupted'].includes(task.status)) continue
    task.status = 'interrupted'
    task.agentId = ''
    task.leaseId = ''
    task.claimedAt = null
    task.leaseExpiresAt = null
    task.updatedAt = nowIso(now)
    changed = true
  }
  return changed
}

function taskByName(team, taskName) {
  const normalized = normalizeName(taskName)
  return Object.values(team?.tasks || {}).find((task) => task.taskName === normalized) || null
}

function taskByAgent(team, agentId) {
  const id = String(agentId || '').trim()
  if (!id) return null
  return Object.values(team?.tasks || {}).find((task) => task.agentId === id) || null
}

export class TeamWorkflowService {
  constructor({ path, now = () => Date.now() } = {}) {
    this.path = path
    this.now = now
    this.state = { version: 1, teams: {} }
    this.write = Promise.resolve()
  }

  async init({ pauseActive = false } = {}) {
    const stored = await readJson(this.path, { version: 1, teams: {} })
    const teams = {}
    let changed = false
    for (const [sessionId, value] of Object.entries(stored?.teams || {})) {
      const team = normalizeTeam(value, sessionId)
      if (!team) continue
      if (pauseActive && team.status === 'active') {
        team.status = 'paused'
        interruptActiveTasks(team, this.now())
        team.updatedAt = nowIso(this.now())
        team.lastProgressAt = team.updatedAt
        changed = true
      }
      teams[sessionId] = team
    }
    this.state = { version: 1, teams }
    if (changed) await this.save()
  }

  save() {
    const snapshot = {
      version: 1,
      teams: Object.fromEntries(
        Object.entries(this.state.teams).map(([id, team]) => [id, durableTeam(team)]),
      ),
    }
    this.write = this.write.catch(() => {}).then(() => writeJsonAtomic(this.path, snapshot))
    return this.write
  }

  get(sessionId) {
    return clone(publicTeam(this.state.teams[String(sessionId || '')]))
  }

  getTask(sessionId, taskId) {
    const team = this.state.teams[String(sessionId || '')]
    return clone(publicTask(team?.tasks[String(taskId || '')]))
  }

  findTask(sessionId, taskName) {
    return clone(taskByName(this.state.teams[String(sessionId || '')], taskName))
  }

  isActive(sessionId) {
    return this.state.teams[String(sessionId || '')]?.status === 'active'
  }

  async ensure(sessionId, { goalId, objective, tokenBudget } = {}) {
    const id = String(sessionId || '').trim()
    if (!id) throw new Error('Team requires a session.')
    const current = this.state.teams[id]
    if (
      current &&
      ['active', 'paused', 'stalled', 'budget_limited'].includes(current.status) &&
      (!goalId || current.goalId === String(goalId))
    ) {
      current.goalId = String(goalId || current.goalId || '')
      current.objective = String(objective || current.objective).trim()
      if (tokenBudget !== undefined) {
        current.tokenBudget = positiveBudget(tokenBudget)
        current.tokenBudgetExplicit = current.tokenBudget != null
      }
      current.status = 'active'
      this.recoverInterruptedTasks(current)
      this.unblockTasks(current)
      current.updatedAt = nowIso(this.now())
      await this.save()
      return clone(publicTeam(current))
    }
    const now = nowIso(this.now())
    const team = {
      id: randomUUID(),
      sessionId: id,
      goalId: String(goalId || ''),
      objective: String(objective || '').trim(),
      status: 'active',
      tokenBudget: positiveBudget(tokenBudget),
      tokenBudgetExplicit: positiveBudget(tokenBudget) != null,
      tasks: {},
      createdAt: now,
      updatedAt: now,
      lastProgressAt: now,
      conflicts: [],
      summaryText: '',
      scriptPath: '',
      communications: [],
      startedAt: now,
    }
    this.state.teams[id] = team
    await this.save()
    return clone(publicTeam(team))
  }

  async remove(sessionId) {
    const id = String(sessionId || '')
    if (!this.state.teams[id]) return false
    delete this.state.teams[id]
    await this.save()
    return true
  }

  async pauseActive() {
    let changed = false
    const now = nowIso(this.now())
    for (const team of Object.values(this.state.teams)) {
      if (team.status !== 'active') continue
      team.status = 'paused'
      interruptActiveTasks(team, this.now())
      team.updatedAt = now
      team.lastProgressAt = now
      changed = true
    }
    if (changed) await this.save()
  }

  async pause(sessionId) {
    const team = this.state.teams[String(sessionId || '')]
    if (!team || team.status !== 'active') return clone(publicTeam(team))
    team.status = 'paused'
    interruptActiveTasks(team, this.now())
    team.updatedAt = nowIso(this.now())
    team.lastProgressAt = team.updatedAt
    await this.save()
    return clone(publicTeam(team))
  }

  async markBudgetLimited(sessionId, tokenBudget) {
    const team = this.state.teams[String(sessionId || '')]
    if (!team) return null
    if (tokenBudget !== undefined) {
      team.tokenBudget = positiveBudget(tokenBudget)
      team.tokenBudgetExplicit = team.tokenBudget != null
    }
    if (team.status !== 'active' || !team.tokenBudgetExplicit) return clone(publicTeam(team))
    team.status = 'budget_limited'
    interruptActiveTasks(team, this.now())
    team.updatedAt = nowIso(this.now())
    team.lastProgressAt = team.updatedAt
    await this.save()
    return clone(publicTeam(team))
  }

  // Team 不因经过固定时间而失败；时间和空闲信息只用于观察与人工判断。
  teamExpired() {
    return false
  }

  refreshLimits() {
    return null
  }

  recoverInterruptedTasks(team) {
    let changed = false
    for (const task of Object.values(team?.tasks || {})) {
      if (task.status !== 'interrupted') continue
      task.status = dependenciesComplete(team, task) ? 'queued' : 'blocked'
      task.agentId = ''
      task.leaseId = ''
      task.claimedAt = null
      task.leaseExpiresAt = null
      task.blockedReason = task.status === 'blocked' ? '等待依赖完成后自动认领' : ''
      task.updatedAt = nowIso(this.now())
      changed = true
    }
    return changed
  }

  unblockTasks(team) {
    let changed = false
    for (const task of Object.values(team?.tasks || {})) {
      if (task.status !== 'blocked' || !dependenciesComplete(team, task)) continue
      task.status = 'queued'
      task.blockedReason = ''
      task.updatedAt = nowIso(this.now())
      changed = true
    }
    return changed
  }

  taskReady(sessionId, taskId) {
    const team = this.state.teams[String(sessionId || '')]
    const task = team?.tasks[String(taskId || '')]
    if (!team || !task || team.status !== 'active') return false
    return ['queued', 'blocked'].includes(task.status) && dependenciesComplete(team, task)
  }

  readyTasks(sessionId) {
    const team = this.state.teams[String(sessionId || '')]
    if (!team || team.status !== 'active') return []
    this.unblockTasks(team)
    return Object.values(team.tasks)
      .filter((task) => this.taskReady(sessionId, task.id))
      .sort((left, right) => left.createdAt.localeCompare(right.createdAt))
      .map(publicTask)
  }

  requeueExpiredLease(team, task) {
    const expiresAt = task?.leaseExpiresAt ? new Date(task.leaseExpiresAt).getTime() : 0
    if (!expiresAt || expiresAt > this.now()) return null
    if (!['starting', 'running'].includes(task.status)) return null
    const expired = {
      taskId: task.id,
      taskName: task.taskName,
      agentId: task.agentId,
      leaseId: task.leaseId,
    }
    task.status = dependenciesComplete(team, task) ? 'queued' : 'blocked'
    task.agentId = ''
    task.leaseId = ''
    task.claimedAt = null
    task.leaseExpiresAt = null
    task.blockedReason = task.status === 'blocked' ? '等待依赖完成后重新认领' : ''
    task.updatedAt = nowIso(this.now())
    team.updatedAt = task.updatedAt
    return expired
  }

  async requeueExpiredLeases(sessionId) {
    const team = this.state.teams[String(sessionId || '')]
    if (!team || team.status !== 'active') return []
    const expired = Object.values(team.tasks)
      .map((task) => this.requeueExpiredLease(team, task))
      .filter(Boolean)
    if (expired.length) await this.save()
    return clone(expired)
  }

  async claimTask(
    sessionId,
    taskId,
    { leaseId = randomUUID(), leaseMs = TEAM_TASK_LEASE_MS } = {},
  ) {
    const team = this.state.teams[String(sessionId || '')]
    const task = team?.tasks[String(taskId || '')]
    if (!team || !task || team.status !== 'active') return null
    if (!['queued', 'blocked'].includes(task.status) || !dependenciesComplete(team, task))
      return null
    const now = this.now()
    task.status = 'starting'
    task.leaseId = String(leaseId)
    task.claimedAt = nowIso(now)
    task.leaseExpiresAt = nowIso(now + Math.max(60_000, Number(leaseMs) || TEAM_TASK_LEASE_MS))
    task.blockedReason = ''
    task.updatedAt = nowIso(now)
    team.updatedAt = task.updatedAt
    team.lastProgressAt = task.updatedAt
    await this.save()
    return clone(publicTask(task))
  }

  async releaseTask(sessionId, taskId, error = '', { leaseId } = {}) {
    const team = this.state.teams[String(sessionId || '')]
    const task = team?.tasks[String(taskId || '')]
    if (!team || !task) return null
    if (leaseId && task.leaseId !== String(leaseId)) return null
    task.status = dependenciesComplete(team, task) ? 'queued' : 'blocked'
    task.agentId = ''
    task.leaseId = ''
    task.claimedAt = null
    task.leaseExpiresAt = null
    task.error = String(error || '').slice(0, 1_000)
    task.blockedReason = task.status === 'blocked' ? '等待依赖完成后重新认领' : ''
    task.updatedAt = nowIso(this.now())
    team.updatedAt = task.updatedAt
    await this.save()
    return clone(publicTask(task))
  }

  async recordConflict(team, { taskName, files, message }) {
    const now = nowIso(this.now())
    team.conflicts = [
      ...(team.conflicts || []),
      {
        taskName: normalizeName(taskName),
        files: normalizeFiles(files),
        message: String(message || '').slice(0, 1_000),
        at: now,
      },
    ].slice(-24)
    team.updatedAt = now
    await this.save()
  }

  async registerTask(
    sessionId,
    {
      taskName,
      role,
      files = [],
      dependsOn = [],
      message = '',
      autoStart = false,
      workflowFingerprint = '',
    } = {},
  ) {
    const team = this.state.teams[String(sessionId || '')]
    if (!team || team.status !== 'active') throw new Error('No active Team is available.')
    const normalizedName = normalizeName(taskName)
    if (!normalizedName) throw new Error('Team taskName cannot be empty.')
    if (taskByName(team, normalizedName))
      throw new Error(`Team task already exists: ${normalizedName}. Use followup_task instead.`)
    if (Object.keys(team.tasks).length >= MAX_TEAM_TASKS)
      throw new Error(`Team task limit is ${MAX_TEAM_TASKS}.`)
    const dependencies = normalizeDependencies(dependsOn)
    const missing = dependencies.filter((dependency) => !taskByName(team, dependency))
    if (missing.length) throw new Error(`Unknown Team task dependencies: ${missing.join(', ')}.`)
    if (hasDependencyCycle(team, { taskName: normalizedName }, dependencies))
      throw new Error(`Team task dependency cycle detected: ${normalizedName}.`)
    const normalizedFiles = normalizeFiles(files)
    const activeConflict = Object.values(team.tasks).find(
      (task) =>
        !TERMINAL_TEAM_TASK_STATUSES.has(task.status) &&
        !dependencies.includes(task.taskName) &&
        scopesOverlap(normalizedFiles, task.files),
    )
    if (activeConflict) {
      const conflictMessage = `Team file ownership conflict with ${activeConflict.taskName}: ${normalizedFiles.join(', ')}.`
      await this.recordConflict(team, {
        taskName: normalizedName,
        files: normalizedFiles,
        message: conflictMessage,
      })
      throw new Error(conflictMessage)
    }
    const now = nowIso(this.now())
    const ready = dependencies.every(
      (dependency) => taskByName(team, dependency)?.status === 'completed',
    )
    const task = {
      id: randomUUID(),
      taskName: normalizedName,
      role: String(role || '')
        .trim()
        .slice(0, MAX_TEAM_ROLE_CHARS),
      message: String(message || '')
        .trim()
        .slice(0, MAX_TEAM_TASK_MESSAGE_CHARS),
      files: normalizedFiles,
      dependsOn: dependencies,
      agentId: '',
      leaseId: '',
      claimedAt: null,
      leaseExpiresAt: null,
      blockedReason: ready ? '' : '等待依赖完成后自动认领',
      status: ready ? 'queued' : 'blocked',
      output: '',
      error: '',
      attempts: 0,
      createdAt: now,
      updatedAt: now,
      completedAt: null,
      autoStart: autoStart === true,
      workflowFingerprint: String(workflowFingerprint || '').slice(0, 64),
    }
    team.tasks[task.id] = task
    team.updatedAt = now
    team.lastProgressAt = now
    await this.save()
    return clone(publicTask(task))
  }

  async discardTask(sessionId, taskId) {
    const team = this.state.teams[String(sessionId || '')]
    const id = String(taskId || '')
    if (!team || !team.tasks[id]) return null
    delete team.tasks[id]
    team.updatedAt = nowIso(this.now())
    this.unblockTasks(team)
    await this.save()
    return clone(publicTeam(team))
  }

  async updateTask(
    sessionId,
    taskId,
    { taskName, role, files, dependsOn, message, workflowFingerprint, autoStart } = {},
  ) {
    const team = this.state.teams[String(sessionId || '')]
    const task = team?.tasks[String(taskId || '')]
    if (!team || !task || team.status !== 'active')
      throw new Error('No active Team task is available.')
    if (['starting', 'running'].includes(task.status))
      throw new Error('Running Team tasks must be interrupted before changing their graph entry.')
    const previousName = task.taskName
    const nextName = taskName === undefined ? previousName : normalizeName(taskName)
    if (!nextName) throw new Error('Team taskName cannot be empty.')
    const duplicate = Object.values(team.tasks).find(
      (candidate) => candidate.id !== task.id && candidate.taskName === nextName,
    )
    if (duplicate) throw new Error(`Team task already exists: ${nextName}.`)
    const nextDependencies =
      dependsOn === undefined ? [...task.dependsOn] : normalizeDependencies(dependsOn)
    if (nextDependencies.includes(previousName) || nextDependencies.includes(nextName))
      throw new Error(`Team task ${nextName} cannot depend on itself.`)
    const missing = nextDependencies.filter(
      (dependency) =>
        dependency !== previousName && dependency !== nextName && !taskByName(team, dependency),
    )
    if (missing.length) throw new Error(`Unknown Team task dependencies: ${missing.join(', ')}.`)
    const dependencyTask = { ...task, taskName: nextName }
    if (hasDependencyCycle(team, dependencyTask, nextDependencies))
      throw new Error(`Team task dependency cycle detected: ${nextName}.`)
    const nextFiles = files === undefined ? [...task.files] : normalizeFiles(files)
    const activeConflict = Object.values(team.tasks).find(
      (candidate) =>
        candidate.id !== task.id &&
        !TERMINAL_TEAM_TASK_STATUSES.has(candidate.status) &&
        !nextDependencies.includes(candidate.taskName) &&
        scopesOverlap(nextFiles, candidate.files),
    )
    if (activeConflict) {
      const conflictMessage = `Team file ownership conflict with ${activeConflict.taskName}.`
      await this.recordConflict(team, {
        taskName: nextName,
        files: nextFiles,
        message: conflictMessage,
      })
      throw new Error(conflictMessage)
    }
    task.taskName = nextName
    task.role = String(role === undefined ? task.role : role || '')
      .trim()
      .slice(0, MAX_TEAM_ROLE_CHARS)
    task.message = String(message === undefined ? task.message : message || '')
      .trim()
      .slice(0, MAX_TEAM_TASK_MESSAGE_CHARS)
    task.files = nextFiles
    task.dependsOn = nextDependencies.filter((dependency) => dependency !== previousName)
    task.workflowFingerprint = String(
      workflowFingerprint === undefined ? task.workflowFingerprint : workflowFingerprint || '',
    ).slice(0, 64)
    if (autoStart !== undefined) task.autoStart = autoStart === true
    if (previousName !== nextName) {
      for (const candidate of Object.values(team.tasks)) {
        if (candidate.id === task.id) continue
        candidate.dependsOn = candidate.dependsOn.map((dependency) =>
          dependency === previousName ? nextName : dependency,
        )
      }
    }
    task.agentId = ''
    task.leaseId = ''
    task.claimedAt = null
    task.leaseExpiresAt = null
    task.output = ''
    task.error = ''
    task.completedAt = null
    task.status = dependenciesComplete(team, task) ? 'queued' : 'blocked'
    task.blockedReason = task.status === 'blocked' ? '等待依赖完成后自动认领' : ''
    task.updatedAt = nowIso(this.now())
    team.updatedAt = task.updatedAt
    team.lastProgressAt = task.updatedAt
    this.unblockTasks(team)
    await this.save()
    return clone(publicTask(task))
  }

  async bindAgent(sessionId, taskId, agent, { leaseId } = {}) {
    const team = this.state.teams[String(sessionId || '')]
    const task = team?.tasks[String(taskId || '')]
    if (!team || !task) return null
    if (leaseId && task.leaseId !== String(leaseId)) return null
    task.agentId = String(agent?.id || '').trim()
    task.status = TEAM_TASK_STATUSES.has(agent?.status) ? agent.status : 'running'
    task.output = String(agent?.output || '').slice(0, MAX_TEAM_TASK_OUTPUT_CHARS)
    task.error = String(agent?.error || '').slice(0, 1_000)
    task.attempts += 1
    task.completedAt = TERMINAL_TEAM_TASK_STATUSES.has(task.status) ? nowIso(this.now()) : null
    if (TERMINAL_TEAM_TASK_STATUSES.has(task.status)) {
      task.leaseId = ''
      task.claimedAt = null
      task.leaseExpiresAt = null
    }
    task.updatedAt = nowIso(this.now())
    team.updatedAt = task.updatedAt
    this.unblockTasks(team)
    team.lastProgressAt = task.updatedAt
    await this.save()
    return clone(publicTask(task))
  }

  async prepareRetry(sessionId, agent) {
    const team = this.state.teams[String(sessionId || '')]
    const task = taskByAgent(team, agent?.id)
    if (!team || !task) return null
    task.attempts += 1
    task.status = dependenciesComplete(team, task) ? 'queued' : 'blocked'
    task.error = ''
    task.output = ''
    task.completedAt = null
    task.leaseId = ''
    task.claimedAt = null
    task.leaseExpiresAt = null
    task.blockedReason = task.status === 'blocked' ? '等待依赖完成后自动认领' : ''
    task.updatedAt = nowIso(this.now())
    team.updatedAt = task.updatedAt
    team.lastProgressAt = task.updatedAt
    await this.save()
    return clone(publicTask(task))
  }

  applyAgentUpdate(team, task, agent) {
    task.status = TEAM_TASK_STATUSES.has(agent?.status) ? agent.status : task.status
    task.output = String(agent?.output || '').slice(0, MAX_TEAM_TASK_OUTPUT_CHARS)
    task.error = String(agent?.error || '').slice(0, 1_000)
    task.updatedAt = nowIso(this.now())
    if (TERMINAL_TEAM_TASK_STATUSES.has(task.status)) {
      task.completedAt = task.updatedAt
      task.leaseId = ''
      task.claimedAt = null
      task.leaseExpiresAt = null
    } else if (['starting', 'running'].includes(task.status) && task.leaseId) {
      task.leaseExpiresAt = nowIso(this.now() + TEAM_TASK_LEASE_MS)
    }
    team.updatedAt = task.updatedAt
    team.lastProgressAt = task.updatedAt
    this.unblockTasks(team)
  }

  async updateAgent(sessionId, agent) {
    const team = this.state.teams[String(sessionId || '')]
    const task = taskByAgent(team, agent?.id)
    if (!team || !task) return clone(publicTeam(team))
    this.applyAgentUpdate(team, task, agent)
    await this.save()
    return clone(publicTeam(team))
  }

  async updateLeasedAgent(sessionId, taskId, leaseId, agent) {
    const team = this.state.teams[String(sessionId || '')]
    const task = team?.tasks[String(taskId || '')]
    if (
      !team ||
      !task ||
      !leaseId ||
      task.leaseId !== String(leaseId) ||
      !task.agentId ||
      task.agentId !== String(agent?.id || '')
    )
      return null
    this.applyAgentUpdate(team, task, agent)
    await this.save()
    return clone({ task: publicTask(task), team: publicTeam(team) })
  }

  async syncAgents(sessionId, agents = []) {
    for (const agent of agents) await this.updateAgent(sessionId, agent)
    return this.get(sessionId)
  }

  async setSummary(sessionId, summary) {
    const team = this.state.teams[String(sessionId || '')]
    if (!team) return null
    team.summaryText = String(summary || '')
      .trim()
      .slice(0, MAX_TEAM_SUMMARY_CHARS)
    team.updatedAt = nowIso(this.now())
    await this.save()
    return clone(publicTeam(team))
  }

  async setScriptPath(sessionId, scriptPath) {
    const team = this.state.teams[String(sessionId || '')]
    if (!team) return null
    team.scriptPath = String(scriptPath || '')
      .trim()
      .slice(0, MAX_TEAM_SCRIPT_PATH_CHARS)
    team.updatedAt = nowIso(this.now())
    team.lastProgressAt = team.updatedAt
    await this.save()
    return clone(publicTeam(team))
  }

  async recordCommunication(sessionId, input = {}) {
    const team = this.state.teams[String(sessionId || '')]
    const communication = normalizeCommunication({
      ...input,
      sentAt: input.sentAt || nowIso(this.now()),
    })
    if (!team || !communication) return null
    team.communications = [...(team.communications || []), communication].slice(
      -MAX_TEAM_COMMUNICATIONS,
    )
    team.updatedAt = communication.sentAt
    team.lastProgressAt = communication.sentAt
    await this.save()
    return clone(communication)
  }

  async markCommunicationsDelivered(sessionId, agentId) {
    const team = this.state.teams[String(sessionId || '')]
    const target = String(agentId || '').trim()
    if (!team || !target) return []
    const delivered = []
    for (const communication of team.communications || []) {
      if (communication.toAgentId !== target || communication.status !== 'queued') continue
      communication.status = 'delivered'
      delivered.push(communication)
    }
    if (delivered.length) {
      team.updatedAt = nowIso(this.now())
      team.lastProgressAt = team.updatedAt
      await this.save()
    }
    return clone(delivered)
  }

  async markComplete(sessionId) {
    const team = this.state.teams[String(sessionId || '')]
    if (!team) return null
    team.status = 'complete'
    team.updatedAt = nowIso(this.now())
    await this.save()
    return clone(publicTeam(team))
  }

  canComplete(sessionId) {
    const team = this.state.teams[String(sessionId || '')]
    if (!team || team.status !== 'active')
      return {
        ok: false,
        reason: 'No active Team is available.',
        team: publicTeam(team),
      }
    const tasks = Object.values(team.tasks)
    const incomplete = tasks.filter((task) => task.status !== 'completed')
    if (incomplete.length) {
      // 区分「仍在跑的任务」与「已终态但未完成的任务」：后者给出恢复路径，
      // 避免 lead 面对中断/失败任务时只能靠重新 spawn 占位才能解锁目标。
      const activeTasks = incomplete.filter((task) => !TERMINAL_TEAM_TASK_STATUSES.has(task.status))
      const terminalTasks = incomplete.filter((task) =>
        TERMINAL_TEAM_TASK_STATUSES.has(task.status),
      )
      const name = (task) => `${task.taskName} (${task.status})`
      const reason = activeTasks.length
        ? `Team 仍有未完成工作流：${activeTasks.map(name).join(', ')}。`
        : `Team 仍有未完成工作流：${terminalTasks.map(name).join(', ')}。可用 followup_task 继续，或用 spawn_agent 以相同 taskName 重启收尾。`
      return { ok: false, reason, team: publicTeam(team) }
    }
    const missingEvidence = tasks.filter(
      (task) => task.status === 'completed' && !String(task.output || '').trim(),
    )
    if (missingEvidence.length)
      return {
        ok: false,
        reason: `Team 完成任务缺少可验证结果：${missingEvidence.map((task) => task.taskName).join(', ')}。`,
        team: publicTeam(team),
      }
    return { ok: true, team: publicTeam(team) }
  }
}

// Team 模式提示：主 Agent 负责动态拆解、并行委派、结果整合和最终验收。
export function teamExecutionPrompt(goal, activeAgents = [], team = null) {
  const objective = String(goal?.objective || '').trim()
  const agents = Array.isArray(activeAgents)
    ? activeAgents
        .map((agent) => `${agent.canonicalName || agent.taskName || agent.id}: ${agent.status}`)
        .join(', ')
    : ''
  const tasks = Array.isArray(team?.tasks)
    ? team.tasks
        .map(
          (task) =>
            `${task.taskName}${task.role ? ` [${task.role}]` : ''} ${task.status}${task.dependsOn?.length ? ` (depends on ${task.dependsOn.join(', ')})` : ''}`,
        )
        .join('; ')
    : ''
  return `${TEAM_EXECUTION_MARKER}
You are the lead of a dynamic software engineering team. The user objective below is the team's mission, not a request for a one-turn answer.

<team_objective>
${objective}
</team_objective>

Team operating rules:
- Start by maintaining a concrete Plan with non-overlapping workstreams, deliverables, dependencies, and acceptance evidence.
- Decompose the objective only as far as the repository and acceptance criteria require. A single direct task is valid; create multiple tasks when parallel work or independent evidence makes it useful.
- Give each subagent a bounded task, relevant file scope, expected output, and verification criteria. Roles are optional labels chosen from the actual work, not a fixed cast. Include role, files, and dependsOn in spawn_agent when applicable so the runtime can enforce ownership and dependencies.
- Members can inspect the teammate roster and send direct messages to one another. Use member-to-member messages for handoffs, questions, and evidence; do not route every coordination step through the lead.
- Tasks whose prerequisites are unfinished remain queued and are claimed automatically when they become ready. Let independent tasks run in parallel. Do not busy-poll; use completion notifications or a targeted wait only when a decision depends on a result.
- Treat subagent output as evidence and suggestions, not as proof that the project is complete. Inspect and integrate changes, resolve conflicts, run the relevant tests, and request follow-up work when a result is incomplete.
- Keep the parent session responsible for final integration, user-facing decisions, and the completion audit. Never call update_goal while a teammate is queued or running; call it with status "complete" only after every explicit requirement has concrete evidence and no required teammate result remains unresolved.
- If a task is too small to justify multiple agents, explain that decision internally and complete it directly instead of creating artificial work.
- When the task needs dynamic fan-out, branching, aggregation, or repeated verification, write a workspace JavaScript workflow and run it with run_team_workflow. The restricted script API provides agent, pipeline, parallel, phase, log, and args; the script itself has no file or Shell access. Prefer this when the orchestration logic should be visible, repeatable, and driven by intermediate results.

The runtime persists the task graph, prevents unsafe overlapping active file ownership, recovers abandoned task leases, schedules ready work, and blocks team completion until registered work converges.
${
  tasks
    ? `
Current workflow status: ${tasks}`
    : ''
}
${
  agents
    ? `
Current teammate status: ${agents}`
    : ''
}`
}
