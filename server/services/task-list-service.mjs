import { randomUUID } from 'node:crypto'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

export const TASK_LIST_STATUSES = Object.freeze(['pending', 'in_progress', 'completed', 'blocked'])
export const MAX_TASK_LIST_ITEMS = 50
export const MAX_TASK_TITLE_CHARS = 300
export const MAX_TASK_NOTE_CHARS = 1_000
export const MAX_TASK_ASSIGNEE_CHARS = 80
export const MAX_TASK_DEPENDS_ON = 20

const STATUS_SET = new Set(TASK_LIST_STATUSES)

function clone(value) {
  return JSON.parse(JSON.stringify(value))
}

function nowIso(now = Date.now()) {
  return new Date(now).toISOString()
}

function normalizedId(value) {
  const id = String(value || '').trim()
  return /^[a-zA-Z0-9._:-]{1,80}$/.test(id) ? id : randomUUID()
}

function normalizedAssignee(value) {
  const assignee = String(value || '').trim()
  if (!assignee) return ''
  if (assignee.length > MAX_TASK_ASSIGNEE_CHARS)
    throw new Error(`Task assignee is limited to ${MAX_TASK_ASSIGNEE_CHARS} characters.`)
  return assignee
}

function normalizedDependsOn(value) {
  if (value == null) return []
  if (!Array.isArray(value)) throw new Error('Task dependsOn must be an array of task ids.')
  if (value.length > MAX_TASK_DEPENDS_ON)
    throw new Error(`Task dependsOn is limited to ${MAX_TASK_DEPENDS_ON} ids.`)
  const seen = new Set()
  const ids = []
  for (const entry of value) {
    const id = String(entry || '').trim()
    if (!id) continue
    if (!/^[a-zA-Z0-9._:-]{1,80}$/.test(id)) throw new Error(`Invalid dependency task id: ${id}`)
    if (seen.has(id)) continue
    seen.add(id)
    ids.push(id)
  }
  return ids
}

function validateDependencyGraph(items) {
  const byId = new Map(items.map((item) => [item.id, item]))
  for (const item of items) {
    for (const dependencyId of item.dependsOn) {
      if (dependencyId === item.id) throw new Error(`Task cannot depend on itself: ${item.id}`)
      if (!byId.has(dependencyId)) throw new Error(`Unknown dependency task id: ${dependencyId}`)
    }
  }

  const visitState = new Map()
  const stack = []
  const visit = (id) => {
    if (visitState.get(id) === 2) return
    if (visitState.get(id) === 1) {
      const start = stack.indexOf(id)
      const cycle = [...stack.slice(Math.max(0, start)), id]
      throw new Error(`Task dependency cycle: ${cycle.join(' -> ')}`)
    }
    visitState.set(id, 1)
    stack.push(id)
    for (const dependencyId of byId.get(id)?.dependsOn || []) visit(dependencyId)
    stack.pop()
    visitState.set(id, 2)
  }
  for (const item of items) visit(item.id)
}

function taskCounts(items) {
  const byId = new Map(items.map((item) => [item.id, item]))
  const dependencyBlocked = (item) => item.status !== 'completed'
    && item.dependsOn.some((dependencyId) => byId.get(dependencyId)?.status !== 'completed')
  const completed = items.filter((item) => item.status === 'completed').length
  const blocked = items.filter((item) => item.status !== 'completed'
    && (item.status === 'blocked' || dependencyBlocked(item))).length
  const inProgress = items.filter((item) => item.status === 'in_progress' && !dependencyBlocked(item)).length
  const pending = items.filter((item) => item.status === 'pending' && !dependencyBlocked(item)).length
  return { pending, inProgress, completed, blocked, total: items.length }
}

function normalizeItem(value, previous, now) {
  const title = String(value?.title || '').trim()
  if (!title) throw new Error('Task title cannot be empty.')
  if (title.length > MAX_TASK_TITLE_CHARS) throw new Error(`Task title is limited to ${MAX_TASK_TITLE_CHARS} characters.`)
  const note = String(value?.note || '').trim()
  if (note.length > MAX_TASK_NOTE_CHARS) throw new Error(`Task note is limited to ${MAX_TASK_NOTE_CHARS} characters.`)
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

function emptyList(sessionId) {
  return {
    sessionId: String(sessionId || ''),
    items: [],
    counts: { pending: 0, inProgress: 0, completed: 0, blocked: 0, total: 0 },
    updatedAt: null,
  }
}

function publicList(sessionId, value) {
  if (!value) return emptyList(sessionId)
  const items = clone(value.items || [])
  return {
    sessionId: String(sessionId || ''),
    items,
    counts: taskCounts(items),
    updatedAt: value.updatedAt || null,
  }
}

function normalizedState(input) {
  const lists = input && typeof input === 'object' && input.lists && typeof input.lists === 'object' ? input.lists : {}
  const result = {}
  for (const [sessionId, value] of Object.entries(lists)) {
    if (!value || !Array.isArray(value.items)) continue
    const now = value.updatedAt || nowIso()
    const seen = new Set()
    const items = []
    for (const item of value.items.slice(0, MAX_TASK_LIST_ITEMS)) {
      try {
        const normalized = normalizeItem(item, item, item.updatedAt || now)
        if (seen.has(normalized.id)) normalized.id = randomUUID()
        seen.add(normalized.id)
        items.push(normalized)
      } catch {
        // Ignore invalid persisted items while preserving the rest of the list.
      }
    }
    if (items.length) result[sessionId] = { items, updatedAt: now }
  }
  return { version: 1, lists: result }
}

export class TaskListService {
  constructor({ path, now = () => Date.now() } = {}) {
    this.path = path
    this.now = now
    this.state = { version: 1, lists: {} }
    this.write = Promise.resolve()
  }

  async init() {
    this.state = normalizedState(await readJson(this.path, { version: 1, lists: {} }))
  }

  save() {
    const snapshot = clone(this.state)
    this.write = this.write.catch(() => {}).then(() => writeJsonAtomic(this.path, snapshot))
    return this.write
  }

  get(sessionId) {
    const id = String(sessionId || '')
    return publicList(id, this.state.lists[id])
  }

  async replace(sessionId, input = []) {
    const id = String(sessionId || '')
    if (!id) throw new Error('Task list requires a session.')
    if (!Array.isArray(input)) throw new Error('Task list items must be an array.')
    if (input.length > MAX_TASK_LIST_ITEMS) throw new Error(`Task list is limited to ${MAX_TASK_LIST_ITEMS} items.`)
    const previousItems = new Map((this.state.lists[id]?.items || []).map((item) => [item.id, item]))
    const now = nowIso(this.now())
    const seen = new Set()
    const items = input.map((item) => {
      const previous = previousItems.get(String(item?.id || ''))
      const normalized = normalizeItem(item, previous, now)
      if (seen.has(normalized.id)) throw new Error(`Duplicate task id: ${normalized.id}`)
      seen.add(normalized.id)
      return normalized
    })
    validateDependencyGraph(items)
    if (items.length) this.state.lists[id] = { items, updatedAt: now }
    else delete this.state.lists[id]
    await this.save()
    return this.get(id)
  }

  async remove(sessionId) {
    const id = String(sessionId || '')
    if (!this.state.lists[id]) return emptyList(id)
    delete this.state.lists[id]
    await this.save()
    return emptyList(id)
  }
}
