// PlanService：会话计划（任务列表）持久化与校验。
// 计划项支持状态机（pending/in_progress/completed/blocked）与依赖关系（dependsOn），
// 写入时校验依赖图无环；提供旧版 task-list 存储的自动迁移。
import { randomUUID } from 'node:crypto'
import { copyFile, mkdir, rename, rm, stat } from 'node:fs/promises'
import { dirname } from 'node:path'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

export const PLAN_STORE_VERSION = 2
export const PLAN_STATUSES = Object.freeze(['pending', 'in_progress', 'completed', 'blocked'])
export const MAX_PLAN_ITEMS = 50
export const MAX_SUSPENDED_PLANS = 8
export const MAX_PLAN_TITLE_CHARS = 300
export const MAX_PLAN_NOTE_CHARS = 1_000
export const MAX_PLAN_ASSIGNEE_CHARS = 80
export const MAX_PLAN_DEPENDS_ON = 20

const STATUS_SET = new Set(PLAN_STATUSES)
const MISSING_STORE = Symbol('missing-plan-store')

// 原子备份：复制到临时文件再 rename，保证备份文件要么完整要么不存在。
async function atomicBackup(source, target) {
  try {
    const existing = await stat(target)
    if (!existing.isFile()) throw new Error(`Plan migration backup path is not a file: ${target}`)
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }

  await mkdir(dirname(target), { recursive: true })
  const temporary = `${target}.${process.pid}.${randomUUID()}.tmp`
  try {
    await copyFile(source, temporary)
    await rename(temporary, target)
  } finally {
    await rm(temporary, { force: true }).catch(() => {})
  }
}

function clone(value) {
  return JSON.parse(JSON.stringify(value))
}

function nowIso(now = Date.now()) {
  return new Date(now).toISOString()
}

// 计划项 ID 规范化：非法格式回退为新 UUID。
function normalizedId(value) {
  const id = String(value || '').trim()
  return /^[a-zA-Z0-9._:-]{1,80}$/.test(id) ? id : randomUUID()
}

function normalizedAssignee(value) {
  const assignee = String(value || '').trim()
  if (!assignee) return ''
  if (assignee.length > MAX_PLAN_ASSIGNEE_CHARS)
    throw new Error(`Plan item assignee is limited to ${MAX_PLAN_ASSIGNEE_CHARS} characters.`)
  return assignee
}

function normalizedDependsOn(value) {
  if (value == null) return []
  if (!Array.isArray(value))
    throw new Error('Plan item dependsOn must be an array of plan item ids.')
  if (value.length > MAX_PLAN_DEPENDS_ON)
    throw new Error(`Plan item dependsOn is limited to ${MAX_PLAN_DEPENDS_ON} ids.`)
  const seen = new Set()
  const ids = []
  for (const entry of value) {
    const id = String(entry || '').trim()
    if (!id) continue
    if (!/^[a-zA-Z0-9._:-]{1,80}$/.test(id))
      throw new Error(`Invalid dependency plan item id: ${id}`)
    if (seen.has(id)) continue
    seen.add(id)
    ids.push(id)
  }
  return ids
}

// 依赖图校验：自引用/未知依赖/环都抛错，防止计划状态机进入不可推进状态。
function validateDependencyGraph(items) {
  const byId = new Map(items.map((item) => [item.id, item]))
  for (const item of items) {
    for (const dependencyId of item.dependsOn) {
      if (dependencyId === item.id) throw new Error(`Plan item cannot depend on itself: ${item.id}`)
      if (!byId.has(dependencyId))
        throw new Error(`Unknown dependency plan item id: ${dependencyId}`)
    }
  }

  const visitState = new Map()
  const stack = []
  const visit = (id) => {
    if (visitState.get(id) === 2) return
    if (visitState.get(id) === 1) {
      const start = stack.indexOf(id)
      const cycle = [...stack.slice(Math.max(0, start)), id]
      throw new Error(`Plan dependency cycle: ${cycle.join(' -> ')}`)
    }
    visitState.set(id, 1)
    stack.push(id)
    for (const dependencyId of byId.get(id)?.dependsOn || []) visit(dependencyId)
    stack.pop()
    visitState.set(id, 2)
  }
  for (const item of items) visit(item.id)
}

// 计划计数：依赖未完成（dependencyBlocked）的项计入 blocked 而非其自身状态。
function planCounts(items) {
  const byId = new Map(items.map((item) => [item.id, item]))
  const dependencyBlocked = (item) =>
    item.status !== 'completed' &&
    item.dependsOn.some((dependencyId) => byId.get(dependencyId)?.status !== 'completed')
  const completed = items.filter((item) => item.status === 'completed').length
  const blocked = items.filter(
    (item) => item.status !== 'completed' && (item.status === 'blocked' || dependencyBlocked(item)),
  ).length
  const inProgress = items.filter(
    (item) => item.status === 'in_progress' && !dependencyBlocked(item),
  ).length
  const pending = items.filter(
    (item) => item.status === 'pending' && !dependencyBlocked(item),
  ).length
  return { pending, inProgress, completed, blocked, total: items.length }
}

function normalizeItem(value, previous, now) {
  const title = String(value?.title || '').trim()
  if (!title) throw new Error('Plan item title cannot be empty.')
  if (title.length > MAX_PLAN_TITLE_CHARS)
    throw new Error(`Plan item title is limited to ${MAX_PLAN_TITLE_CHARS} characters.`)
  const note = String(value?.note || '').trim()
  if (note.length > MAX_PLAN_NOTE_CHARS)
    throw new Error(`Plan item note is limited to ${MAX_PLAN_NOTE_CHARS} characters.`)
  const status = STATUS_SET.has(value?.status) ? value.status : 'pending'
  return {
    id: normalizedId(value?.id || previous?.id),
    title,
    status,
    note,
    assignee: normalizedAssignee(value?.assignee ?? previous?.assignee),
    dependsOn: normalizedDependsOn(value?.dependsOn ?? previous?.dependsOn),
    createdAt: previous?.createdAt || now,
    updatedAt: now,
  }
}

function hasUnfinishedItems(items) {
  return items.some((item) => item.status !== 'completed')
}

function emptyPlan(sessionId) {
  return {
    sessionId: String(sessionId || ''),
    items: [],
    counts: { pending: 0, inProgress: 0, completed: 0, blocked: 0, total: 0 },
    suspendedCount: 0,
    updatedAt: null,
  }
}

function publicPlan(sessionId, value, transition = null) {
  if (!value) return emptyPlan(sessionId)
  const items = clone(value.items || [])
  return {
    sessionId: String(sessionId || ''),
    items,
    counts: planCounts(items),
    suspendedCount: value.suspended?.length || 0,
    updatedAt: value.updatedAt || null,
    ...(transition || {}),
  }
}

// 兼容旧存储格式：既接受 plans 也接受 lists 键。
function persistedPlans(input) {
  if (!input || typeof input !== 'object') return null
  if (input.plans && typeof input.plans === 'object' && !Array.isArray(input.plans))
    return input.plans
  if (input.lists && typeof input.lists === 'object' && !Array.isArray(input.lists))
    return input.lists
  return null
}

function normalizePersistedItems(input, fallbackUpdatedAt) {
  const seen = new Set()
  const items = []
  for (const item of (Array.isArray(input) ? input : []).slice(0, MAX_PLAN_ITEMS)) {
    try {
      const normalized = normalizeItem(item, item, item.updatedAt || fallbackUpdatedAt)
      if (seen.has(normalized.id)) normalized.id = randomUUID()
      seen.add(normalized.id)
      items.push(normalized)
    } catch {
      // 忽略单个损坏条目，保留同一计划中的其余有效任务。
    }
  }
  return items
}

function normalizedState(input) {
  const plans = persistedPlans(input) || {}
  const result = {}
  for (const [sessionId, value] of Object.entries(plans)) {
    if (!value || !Array.isArray(value.items)) continue
    const updatedAt = value.updatedAt || nowIso()
    const items = normalizePersistedItems(value.items, updatedAt)
    if (!items.length) continue
    const suspended = (Array.isArray(value.suspended) ? value.suspended : [])
      .slice(-MAX_SUSPENDED_PLANS)
      .map((snapshot) => {
        const snapshotUpdatedAt = snapshot?.updatedAt || updatedAt
        return {
          items: normalizePersistedItems(snapshot?.items, snapshotUpdatedAt),
          updatedAt: snapshotUpdatedAt,
        }
      })
      .filter((snapshot) => snapshot.items.length && hasUnfinishedItems(snapshot.items))
    result[sessionId] = { items, updatedAt, suspended }
  }
  return { version: PLAN_STORE_VERSION, plans: result }
}

export class PlanService {
  constructor({ path, legacyPath, now = () => Date.now() } = {}) {
    this.path = path
    this.legacyPath = legacyPath
    this.now = now
    this.state = { version: PLAN_STORE_VERSION, plans: {} }
    this.write = Promise.resolve()
    this.readingLegacy = false
    this.legacyMigrationPending = false
  }

  // 从磁盘加载：优先新存储；缺失时读旧版 task-list 并标记待迁移。
  async init() {
    const stored = await readJson(this.path, MISSING_STORE)
    if (stored !== MISSING_STORE) {
      this.state = normalizedState(stored)
      return
    }
    if (!this.legacyPath) return

    let legacy
    try {
      legacy = await readJson(this.legacyPath, MISSING_STORE)
    } catch {
      this.readingLegacy = true
      this.legacyMigrationPending = true
      return
    }
    if (legacy === MISSING_STORE || !persistedPlans(legacy)) return

    this.state = normalizedState(legacy)
    this.readingLegacy = true
    this.legacyMigrationPending = true
    try {
      await this.save()
    } catch {
      // Keep serving normalized legacy data. A later write retries backup before migration.
    }
  }

  // 落盘前先备份旧存储（仅迁移路径），随后原子写入新文件。
  async persist(snapshot) {
    if (this.legacyMigrationPending) await atomicBackup(this.legacyPath, `${this.legacyPath}.bak`)
    await writeJsonAtomic(this.path, snapshot)
    this.legacyMigrationPending = false
    this.readingLegacy = false
  }

  save() {
    this.write = this.write.catch(() => {}).then(() => this.persist(clone(this.state)))
    return this.write
  }

  get(sessionId) {
    const id = String(sessionId || '')
    return publicPlan(id, this.state.plans[id])
  }

  // 整体更新当前计划。完全不同的新计划会暂挂未完成计划；临时计划完成后自动恢复。
  // 空数组仍是显式取消，清除当前计划及全部暂挂计划。
  async replace(sessionId, input = [], { mode = 'auto' } = {}) {
    const id = String(sessionId || '')
    if (!id) throw new Error('Plan requires a session.')
    if (!Array.isArray(input)) throw new Error('Plan items must be an array.')
    if (!['auto', 'replace'].includes(mode))
      throw new Error(`Unsupported plan update mode: ${mode}`)
    if (input.length > MAX_PLAN_ITEMS)
      throw new Error(`Plan is limited to ${MAX_PLAN_ITEMS} items.`)
    const requestedItems = clone(input)
    this.write = this.write
      .catch(() => {})
      .then(async () => {
        const currentPlan = this.state.plans[id]
        const previousItems = new Map((currentPlan?.items || []).map((item) => [item.id, item]))
        const now = nowIso(this.now())
        const seen = new Set()
        const items = requestedItems.map((item) => {
          const previous = previousItems.get(String(item?.id || ''))
          const normalized = normalizeItem(item, previous, now)
          if (seen.has(normalized.id)) throw new Error(`Duplicate plan item id: ${normalized.id}`)
          seen.add(normalized.id)
          return normalized
        })
        validateDependencyGraph(items)

        const nextState = clone(this.state)
        if (!items.length) {
          delete nextState.plans[id]
          await this.persist(nextState)
          this.state = nextState
          return emptyPlan(id)
        }

        const suspended = mode === 'replace' ? [] : clone(currentPlan?.suspended || [])
        const continuesCurrentPlan = items.some((item) => previousItems.has(item.id))
        if (
          mode === 'auto' &&
          currentPlan &&
          hasUnfinishedItems(currentPlan.items) &&
          !continuesCurrentPlan
        ) {
          if (suspended.length >= MAX_SUSPENDED_PLANS) {
            throw new Error(`Plan suspension is limited to ${MAX_SUSPENDED_PLANS} nested plans.`)
          }
          suspended.push({
            items: clone(currentPlan.items),
            updatedAt: currentPlan.updatedAt || now,
          })
        }

        let transition = null
        if (!hasUnfinishedItems(items) && suspended.length) {
          const resumedPlan = suspended.pop()
          nextState.plans[id] = { ...resumedPlan, suspended, updatedAt: now }
          transition = {
            resumed: true,
            completedPlan: {
              items: clone(items),
              counts: planCounts(items),
              updatedAt: now,
            },
            resumeInstruction:
              'A previously unfinished plan was restored. Continue its current item unless the user explicitly cancelled or redirected it.',
          }
        } else {
          nextState.plans[id] = { items, suspended, updatedAt: now }
        }

        await this.persist(nextState)
        this.state = nextState
        return publicPlan(id, nextState.plans[id], transition)
      })
    return this.write
  }

  async remove(sessionId) {
    const id = String(sessionId || '')
    this.write = this.write
      .catch(() => {})
      .then(async () => {
        if (!this.state.plans[id]) return emptyPlan(id)
        const nextState = clone(this.state)
        delete nextState.plans[id]
        await this.persist(nextState)
        this.state = nextState
        return emptyPlan(id)
      })
    return this.write
  }
}
