import { randomUUID } from 'node:crypto'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { normalizeExecutionMode } from '../security/execution-mode.mjs'

export const DEFAULT_SCHEDULE_EXECUTION_MODE = 'full-access'

const FREQUENCIES = new Set(['interval', 'daily', 'weekly', 'monthly'])
const INTERVAL_UNITS = new Set(['minutes', 'hours', 'days'])
const INTERVAL_MS = { minutes: 60_000, hours: 60 * 60_000, days: 24 * 60 * 60_000 }
const NOTIFICATION_TARGETS = new Set(['browser', 'feishu', 'weixin'])

function defaultState() {
  return { version: 1, tasks: [], runs: [] }
}

function zonedParts(value, timeZone) {
  const parts = new Intl.DateTimeFormat('en-CA', {
    timeZone,
    year: 'numeric', month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23',
  }).formatToParts(value)
  return Object.fromEntries(parts.filter((part) => part.type !== 'literal').map((part) => [part.type, Number(part.value)]))
}

function zonedTimeToUtc(parts, timeZone) {
  const target = Date.UTC(parts.year, parts.month - 1, parts.day, parts.hour, parts.minute, 0)
  let guess = target
  for (let index = 0; index < 3; index += 1) {
    const actual = zonedParts(new Date(guess), timeZone)
    const actualUtc = Date.UTC(actual.year, actual.month - 1, actual.day, actual.hour, actual.minute, actual.second)
    guess -= actualUtc - target
  }
  return new Date(guess)
}

function addLocalDays(parts, days) {
  const value = new Date(Date.UTC(parts.year, parts.month - 1, parts.day + days))
  return { year: value.getUTCFullYear(), month: value.getUTCMonth() + 1, day: value.getUTCDate() }
}

function daysInMonth(year, month) {
  return new Date(Date.UTC(year, month, 0)).getUTCDate()
}

export function calculateNextRun(task, from = new Date()) {
  if (task.frequency === 'interval') {
    const unit = INTERVAL_UNITS.has(task.intervalUnit) ? task.intervalUnit : 'hours'
    const value = Math.min(10_000, Math.max(1, Number(task.intervalValue) || 1))
    return new Date(from.getTime() + value * INTERVAL_MS[unit]).toISOString()
  }
  const [hour, minute] = String(task.time || '09:00').split(':').map(Number)
  const current = zonedParts(from, task.timezone)
  let date = { year: current.year, month: current.month, day: current.day }
  if (task.frequency === 'weekly') {
    const weekday = new Date(Date.UTC(date.year, date.month - 1, date.day)).getUTCDay()
    date = addLocalDays(date, (Number(task.dayOfWeek) - weekday + 7) % 7)
  } else if (task.frequency === 'monthly') {
    date.day = Math.min(Number(task.dayOfMonth), daysInMonth(date.year, date.month))
  }
  let candidate = zonedTimeToUtc({ ...date, hour, minute }, task.timezone)
  if (candidate.getTime() <= from.getTime()) {
    if (task.frequency === 'daily') date = addLocalDays(date, 1)
    else if (task.frequency === 'weekly') date = addLocalDays(date, 7)
    else {
      const nextMonth = new Date(Date.UTC(date.year, date.month, 1))
      date = { year: nextMonth.getUTCFullYear(), month: nextMonth.getUTCMonth() + 1, day: Math.min(Number(task.dayOfMonth), daysInMonth(nextMonth.getUTCFullYear(), nextMonth.getUTCMonth() + 1)) }
    }
    candidate = zonedTimeToUtc({ ...date, hour, minute }, task.timezone)
  }
  return candidate.toISOString()
}

function normalizeStoredTask(task, cwd) {
  const timezone = String(task.timezone || 'Asia/Hong_Kong')
  try { new Intl.DateTimeFormat('en', { timeZone: timezone }).format() } catch { throw new Error('定时任务时区无效。') }
  const time = /^([01]\d|2[0-3]):[0-5]\d$/.test(String(task.time || '')) ? String(task.time) : '09:00'
  const frequency = FREQUENCIES.has(task.frequency) ? task.frequency : 'daily'
  const normalized = {
    id: String(task.id || randomUUID()),
    name: String(task.name || '未命名任务').slice(0, 120),
    prompt: String(task.prompt || '').slice(0, 100_000),
    enabled: task.enabled !== false,
    frequency,
    intervalValue: Math.min(10_000, Math.max(1, Number(task.intervalValue) || 1)),
    intervalUnit: INTERVAL_UNITS.has(task.intervalUnit) ? task.intervalUnit : 'hours',
    time,
    timezone,
    dayOfWeek: Math.min(6, Math.max(0, Number.isInteger(Number(task.dayOfWeek)) ? Number(task.dayOfWeek) : 1)),
    dayOfMonth: Math.min(28, Math.max(1, Number(task.dayOfMonth) || 1)),
    cwd: String(task.cwd || cwd),
    executionMode: normalizeExecutionMode(task.executionMode, DEFAULT_SCHEDULE_EXECUTION_MODE),
    model: task.model?.provider && task.model?.model ? { provider: String(task.model.provider), model: String(task.model.model) } : null,
    notifications: [...new Set((Array.isArray(task.notifications) ? task.notifications : []).filter((target) => NOTIFICATION_TARGETS.has(target)))],
    notifyOn: task.notifyOn === 'failure' ? 'failure' : 'always',
    createdAt: task.createdAt || new Date().toISOString(),
    updatedAt: task.updatedAt || new Date().toISOString(),
    nextRunAt: task.nextRunAt || null,
    lastRunAt: task.lastRunAt || null,
    lastStatus: task.lastStatus || 'idle',
    lastSummary: String(task.lastSummary || '').slice(0, 1200),
    lastError: String(task.lastError || '').slice(0, 1200),
    lastNotificationError: String(task.lastNotificationError || '').slice(0, 1200),
  }
  normalized.nextRunAt = normalized.enabled ? calculateNextRun(normalized, normalized.nextRunAt && new Date(normalized.nextRunAt) > new Date() ? new Date(Date.now() - 1000) : new Date()) : null
  return normalized
}

function durationLabel(durationMs) {
  const seconds = Math.max(0, Math.round(durationMs / 1000))
  if (seconds < 60) return `${seconds} 秒`
  return `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`
}

function nextRunLabel(task) {
  if (!task.nextRunAt) return '未安排'
  return new Intl.DateTimeFormat('zh-CN', { timeZone: task.timezone, dateStyle: 'medium', timeStyle: 'short' }).format(new Date(task.nextRunAt))
}

export class ScheduleService {
  constructor({ path, cwd, agent, notifications, tickMs = 15_000 }) {
    this.path = path
    this.cwd = cwd
    this.agent = agent
    this.notifications = notifications
    this.tickMs = tickMs
    this.state = defaultState()
    this.writeQueue = Promise.resolve()
    this.timer = null
    this.running = new Set()
    this.executions = new Set()
  }

  async init() {
    const stored = await readJson(this.path, defaultState())
    this.state = {
      version: 1,
      tasks: (Array.isArray(stored.tasks) ? stored.tasks : []).map((task) => {
        const normalized = normalizeStoredTask(task, this.cwd)
        if (normalized.lastStatus === 'running') normalized.lastStatus = 'interrupted'
        return normalized
      }),
      runs: (Array.isArray(stored.runs) ? stored.runs : []).slice(-200),
    }
    await this.save()
    this.timer = setInterval(() => { void this.tick() }, this.tickMs)
    this.timer.unref?.()
    void this.tick()
  }

  save() {
    const snapshot = JSON.parse(JSON.stringify(this.state))
    this.writeQueue = this.writeQueue.catch(() => {}).then(() => writeJsonAtomic(this.path, snapshot))
    return this.writeQueue
  }

  getState() {
    return JSON.parse(JSON.stringify(this.state))
  }

  async normalizeInput(input, current = {}) {
    const merged = { ...current, ...input }
    const name = String(merged.name || '').trim()
    const prompt = String(merged.prompt || '').trim()
    if (!name) throw new Error('任务名称不能为空。')
    if (!prompt) throw new Error('任务 Prompt 不能为空。')
    if (Object.hasOwn(input || {}, 'cwd')) merged.cwd = await this.agent.validateDirectory(input.cwd)
    const task = normalizeStoredTask({ ...merged, name, prompt, updatedAt: new Date().toISOString() }, this.cwd)
    task.nextRunAt = task.enabled ? calculateNextRun(task) : null
    return task
  }

  async create(input) {
    const task = await this.normalizeInput({ ...input, id: randomUUID(), createdAt: new Date().toISOString() })
    this.state.tasks.unshift(task)
    await this.save()
    return task
  }

  async update(id, input) {
    const index = this.state.tasks.findIndex((task) => task.id === id)
    if (index < 0) return null
    const current = this.state.tasks[index]
    const task = await this.normalizeInput(input, current)
    task.id = current.id
    task.createdAt = current.createdAt
    task.lastRunAt = current.lastRunAt
    task.lastStatus = current.lastStatus
    task.lastSummary = current.lastSummary
    task.lastError = current.lastError
    task.lastNotificationError = current.lastNotificationError
    this.state.tasks[index] = task
    await this.save()
    return task
  }

  async remove(id) {
    if (this.running.has(id)) throw new Error('任务正在运行，暂时不能删除。')
    const before = this.state.tasks.length
    this.state.tasks = this.state.tasks.filter((task) => task.id !== id)
    this.state.runs = this.state.runs.filter((run) => run.taskId !== id)
    if (this.state.tasks.length === before) return false
    await this.save()
    return true
  }

  async runNow(id) {
    const task = this.state.tasks.find((item) => item.id === id)
    if (!task) return null
    await this.startRun(task, 'manual')
    return task
  }

  async tick() {
    const now = Date.now()
    for (const task of this.state.tasks) {
      if (task.enabled && task.nextRunAt && new Date(task.nextRunAt).getTime() <= now && !this.running.has(task.id)) await this.startRun(task, 'scheduled')
    }
  }

  async startRun(task, trigger) {
    if (this.running.has(task.id)) throw new Error('任务已经在运行。')
    this.running.add(task.id)
    const run = { id: randomUUID(), taskId: task.id, trigger, status: 'running', startedAt: new Date().toISOString(), finishedAt: null, durationMs: 0, summary: '', error: '', sessionId: '' }
    this.state.runs.push(run)
    this.state.runs = this.state.runs.slice(-200)
    task.lastRunAt = run.startedAt
    task.lastStatus = 'running'
    task.lastError = ''
    if (trigger === 'scheduled') task.nextRunAt = calculateNextRun(task, new Date())
    await this.save()
    const execution = this.execute(task, run)
    this.executions.add(execution)
    execution.finally(() => this.executions.delete(execution)).catch(() => {})
  }

  async execute(task, run) {
    const started = Date.now()
    let event = 'schedule.completed'
    let data
    try {
      const result = await this.agent.prompt({ message: task.prompt, cwd: task.cwd, title: `定时任务 · ${task.name}`, model: task.model, executionMode: task.executionMode })
      const summary = String(result.text || '任务已完成。').trim().slice(0, 1200)
      run.status = 'completed'
      run.summary = summary
      run.sessionId = result.sessionId || ''
      task.lastStatus = 'completed'
      task.lastSummary = summary
      task.lastError = ''
      data = { task: { name: task.name, summary, duration: durationLabel(Date.now() - started), nextRun: nextRunLabel(task), error: '' } }
    } catch (error) {
      event = 'schedule.failed'
      const message = error instanceof Error ? error.message : String(error)
      run.status = 'failed'
      run.error = message
      task.lastStatus = 'failed'
      task.lastError = message
      data = { task: { name: task.name, summary: '', error: message, duration: durationLabel(Date.now() - started), nextRun: nextRunLabel(task) } }
    } finally {
      run.finishedAt = new Date().toISOString()
      run.durationMs = Date.now() - started
      task.updatedAt = new Date().toISOString()
      if (task.notifications.length && (event === 'schedule.failed' || task.notifyOn === 'always')) {
        try {
          await this.notifications.notify(event, data, { platforms: task.notifications })
          run.notificationError = ''
          task.lastNotificationError = ''
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error)
          run.notificationError = message
          task.lastNotificationError = message
        }
      }
      this.running.delete(task.id)
      await this.save()
    }
  }

  async dispose() {
    clearInterval(this.timer)
    this.timer = null
    await Promise.allSettled([...this.executions])
    await this.writeQueue.catch(() => {})
  }
}
