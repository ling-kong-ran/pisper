// 定时任务服务：按频率（interval/daily/weekly/monthly）调度会话 prompt 或工作流执行，
// 支持时区、运行记录、重启后补偿未执行任务。
import { randomUUID } from 'node:crypto'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { normalizeExecutionMode } from '../security/execution-mode.mjs'

export const DEFAULT_SCHEDULE_EXECUTION_MODE = 'full-access'

const FREQUENCIES = new Set(['interval', 'daily', 'weekly', 'monthly'])
const INTERVAL_UNITS = new Set(['minutes', 'hours', 'days'])
const INTERVAL_MS = { minutes: 60_000, hours: 60 * 60_000, days: 24 * 60 * 60_000 }
const NOTIFICATION_TARGETS = new Set(['browser', 'feishu', 'weixin'])
const SCHEDULE_TARGETS = new Set(['prompt', 'workflow'])
const TERMINAL_WORKFLOW_STATUSES = new Set(['completed', 'failed', 'cancelled', 'interrupted'])
const RESTART_INTERRUPTED_ERROR = '任务因 Pisper 重启而中断。'
const SHUTDOWN_INTERRUPTED_ERROR = '任务因 Pisper 关闭而中断。'

function defaultState() {
  return { version: 1, tasks: [], runs: [] }
}

// 把时间戳按指定时区拆成年月日时分秒（用于计算下一触发时刻）。
function zonedParts(value, timeZone) {
  const parts = new Intl.DateTimeFormat('en-CA', {
    timeZone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hourCycle: 'h23',
  }).formatToParts(value)
  return Object.fromEntries(
    parts.filter((part) => part.type !== 'literal').map((part) => [part.type, Number(part.value)]),
  )
}

// 把某时区的时刻换算回 UTC 时间戳（多次迭代逼近，处理 DST 与偏移差异）。
function zonedTimeToUtc(parts, timeZone) {
  const target = Date.UTC(parts.year, parts.month - 1, parts.day, parts.hour, parts.minute, 0)
  let guess = target
  for (let index = 0; index < 3; index += 1) {
    const actual = zonedParts(new Date(guess), timeZone)
    const actualUtc = Date.UTC(
      actual.year,
      actual.month - 1,
      actual.day,
      actual.hour,
      actual.minute,
      actual.second,
    )
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
  const [hour, minute] = String(task.time || '09:00')
    .split(':')
    .map(Number)
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
      date = {
        year: nextMonth.getUTCFullYear(),
        month: nextMonth.getUTCMonth() + 1,
        day: Math.min(
          Number(task.dayOfMonth),
          daysInMonth(nextMonth.getUTCFullYear(), nextMonth.getUTCMonth() + 1),
        ),
      }
    }
    candidate = zonedTimeToUtc({ ...date, hour, minute }, task.timezone)
  }
  return candidate.toISOString()
}

function normalizeStoredTask(task, cwd) {
  const timezone = String(task.timezone || 'Asia/Hong_Kong')
  try {
    new Intl.DateTimeFormat('en', { timeZone: timezone }).format()
  } catch {
    throw new Error('定时任务时区无效。')
  }
  const time = /^([01]\d|2[0-3]):[0-5]\d$/.test(String(task.time || ''))
    ? String(task.time)
    : '09:00'
  const frequency = FREQUENCIES.has(task.frequency) ? task.frequency : 'daily'
  const normalized = {
    id: String(task.id || randomUUID()),
    name: String(task.name || '未命名任务').slice(0, 120),
    targetType: SCHEDULE_TARGETS.has(task.targetType) ? task.targetType : 'prompt',
    prompt: String(task.prompt || '').slice(0, 100_000),
    workflowId: String(task.workflowId || '').slice(0, 200),
    workflowInputs:
      task.workflowInputs &&
      typeof task.workflowInputs === 'object' &&
      !Array.isArray(task.workflowInputs)
        ? Object.fromEntries(
            Object.entries(task.workflowInputs)
              .slice(0, 100)
              .map(([key, value]) => [String(key).slice(0, 120), value]),
          )
        : {},
    enabled: task.enabled !== false,
    frequency,
    intervalValue: Math.min(10_000, Math.max(1, Number(task.intervalValue) || 1)),
    intervalUnit: INTERVAL_UNITS.has(task.intervalUnit) ? task.intervalUnit : 'hours',
    time,
    timezone,
    dayOfWeek: Math.min(
      6,
      Math.max(0, Number.isInteger(Number(task.dayOfWeek)) ? Number(task.dayOfWeek) : 1),
    ),
    dayOfMonth: Math.min(28, Math.max(1, Number(task.dayOfMonth) || 1)),
    cwd: String(task.cwd || cwd),
    executionMode: normalizeExecutionMode(task.executionMode, DEFAULT_SCHEDULE_EXECUTION_MODE),
    model:
      task.model?.provider && task.model?.model
        ? { provider: String(task.model.provider), model: String(task.model.model) }
        : null,
    notifications: [
      ...new Set(
        (Array.isArray(task.notifications) ? task.notifications : []).filter((target) =>
          NOTIFICATION_TARGETS.has(target),
        ),
      ),
    ],
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
  normalized.nextRunAt = normalized.enabled
    ? calculateNextRun(
        normalized,
        normalized.nextRunAt && new Date(normalized.nextRunAt) > new Date()
          ? new Date(Date.now() - 1000)
          : new Date(),
      )
    : null
  return normalized
}

function durationLabel(durationMs) {
  const seconds = Math.max(0, Math.round(durationMs / 1000))
  if (seconds < 60) return `${seconds} 秒`
  return `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`
}

function nextRunLabel(task) {
  if (!task.nextRunAt) return '未安排'
  return new Intl.DateTimeFormat('zh-CN', {
    timeZone: task.timezone,
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(new Date(task.nextRunAt))
}

export class ScheduleService {
  constructor({ path, cwd, agent, workflows, notifications, tickMs = 15_000 }) {
    this.path = path
    this.cwd = cwd
    this.agent = agent
    this.workflows = workflows
    this.notifications = notifications
    this.tickMs = tickMs
    this.state = defaultState()
    this.writeQueue = Promise.resolve()
    this.timer = null
    this.running = new Set()
    this.executions = new Set()
    this.disposed = false
  }

  async init() {
    const stored = await readJson(this.path, defaultState())
    const interruptedAt = new Date()
    const runs = (Array.isArray(stored.runs) ? stored.runs : []).slice(-200).map((run) => {
      if (run.status !== 'running') return run
      const startedAt = new Date(run.startedAt).getTime()
      const elapsed = Number.isFinite(startedAt)
        ? Math.max(0, interruptedAt.getTime() - startedAt)
        : 0
      return {
        ...run,
        status: 'interrupted',
        finishedAt: interruptedAt.toISOString(),
        durationMs: Math.max(Number(run.durationMs) || 0, elapsed),
        error: RESTART_INTERRUPTED_ERROR,
      }
    })
    this.state = {
      version: 1,
      tasks: (Array.isArray(stored.tasks) ? stored.tasks : []).map((task) => {
        const normalized = normalizeStoredTask(task, this.cwd)
        const interruptedRun = runs.findLast(
          (run) =>
            run.taskId === normalized.id &&
            run.status === 'interrupted' &&
            (!normalized.lastRunAt || run.startedAt === normalized.lastRunAt),
        )
        if (normalized.lastStatus === 'running') {
          normalized.lastStatus = 'interrupted'
          normalized.lastError = RESTART_INTERRUPTED_ERROR
          normalized.updatedAt = interruptedAt.toISOString()
        }
        if (normalized.lastStatus === 'interrupted' && interruptedRun && !normalized.lastError)
          normalized.lastError = RESTART_INTERRUPTED_ERROR
        return normalized
      }),
      runs,
    }
    await this.save()
    this.timer = setInterval(() => {
      void this.tick()
    }, this.tickMs)
    this.timer.unref?.()
    void this.tick()
  }

  save() {
    const snapshot = JSON.parse(JSON.stringify(this.state))
    this.writeQueue = this.writeQueue
      .catch(() => {})
      .then(() => writeJsonAtomic(this.path, snapshot))
    return this.writeQueue
  }

  getState() {
    return JSON.parse(JSON.stringify(this.state))
  }

  async normalizeInput(input, current = {}) {
    const merged = { ...current, ...input }
    const name = String(merged.name || '').trim()
    const targetType = SCHEDULE_TARGETS.has(merged.targetType) ? merged.targetType : 'prompt'
    const prompt = String(merged.prompt || '').trim()
    const workflowId = String(merged.workflowId || '').trim()
    const workflowInputs =
      merged.workflowInputs &&
      typeof merged.workflowInputs === 'object' &&
      !Array.isArray(merged.workflowInputs)
        ? merged.workflowInputs
        : {}
    if (!name) throw new Error('任务名称不能为空。')
    if (targetType === 'prompt' && !prompt) throw new Error('任务 Prompt 不能为空。')
    if (targetType === 'workflow') {
      if (!workflowId) throw new Error('请选择要运行的工作流。')
      const workflow = this.workflows?.list().find((item) => item.id === workflowId)
      if (!workflow) throw new Error('选择的工作流不存在。')
      if (workflow.status !== 'published') throw new Error('定时任务只能调用已发布的工作流。')
      const missingInput = (workflow.inputs || []).find((definition) => {
        const value = Object.hasOwn(workflowInputs, definition.name)
          ? workflowInputs[definition.name]
          : definition.defaultValue
        return definition.required && (value === undefined || value === null || value === '')
      })
      if (missingInput) throw new Error(`工作流输入「${missingInput.label}」不能为空。`)
    }
    if (targetType === 'prompt' && Object.hasOwn(input || {}, 'cwd'))
      merged.cwd = await this.agent.validateDirectory(input.cwd)
    const task = normalizeStoredTask(
      {
        ...merged,
        name,
        targetType,
        prompt,
        workflowId,
        workflowInputs,
        updatedAt: new Date().toISOString(),
      },
      this.cwd,
    )
    task.nextRunAt = task.enabled ? calculateNextRun(task) : null
    return task
  }

  async create(input) {
    const task = await this.normalizeInput({
      ...input,
      id: randomUUID(),
      createdAt: new Date().toISOString(),
    })
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
      if (
        task.enabled &&
        task.nextRunAt &&
        new Date(task.nextRunAt).getTime() <= now &&
        !this.running.has(task.id)
      )
        await this.startRun(task, 'scheduled')
    }
  }

  async startRun(task, trigger) {
    if (this.running.has(task.id)) throw new Error('任务已经在运行。')
    this.running.add(task.id)
    const run = {
      id: randomUUID(),
      taskId: task.id,
      trigger,
      status: 'running',
      startedAt: new Date().toISOString(),
      finishedAt: null,
      durationMs: 0,
      summary: '',
      error: '',
      sessionId: '',
      workflowRunId: '',
    }
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
      const result =
        task.targetType === 'workflow'
          ? await this.executeWorkflow(task, run)
          : await this.agent.prompt({
              message: task.prompt,
              cwd: task.cwd,
              title: `定时任务 · ${task.name}`,
              model: task.model,
              executionMode: task.executionMode,
              isolatedContext: true,
            })
      const summary = String(result.text || '任务已完成。')
        .trim()
        .slice(0, 1200)
      run.status = 'completed'
      run.summary = summary
      run.sessionId = result.sessionId || ''
      run.workflowRunId = result.workflowRunId || ''
      task.lastStatus = 'completed'
      task.lastSummary = summary
      task.lastError = ''
      data = {
        task: {
          name: task.name,
          summary,
          duration: durationLabel(Date.now() - started),
          nextRun: nextRunLabel(task),
          error: '',
        },
      }
    } catch (error) {
      const interrupted = error?.code === 'SCHEDULE_INTERRUPTED'
      event = interrupted ? 'schedule.interrupted' : 'schedule.failed'
      const message = error instanceof Error ? error.message : String(error)
      run.status = interrupted ? 'interrupted' : 'failed'
      run.error = message
      task.lastStatus = interrupted ? 'interrupted' : 'failed'
      task.lastError = message
      data = {
        task: {
          name: task.name,
          summary: '',
          error: message,
          duration: durationLabel(Date.now() - started),
          nextRun: nextRunLabel(task),
        },
      }
    } finally {
      run.finishedAt = new Date().toISOString()
      run.durationMs = Date.now() - started
      task.updatedAt = new Date().toISOString()
      if (
        task.notifications.length &&
        (event === 'schedule.failed' ||
          (event === 'schedule.completed' && task.notifyOn === 'always'))
      ) {
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

  async executeWorkflow(task, scheduleRun) {
    const workflow = this.workflows?.list().find((item) => item.id === task.workflowId)
    if (!workflow) throw new Error('选择的工作流不存在。')
    if (workflow.status !== 'published') throw new Error('定时任务只能调用已发布的工作流。')
    const workflowRun = await this.workflows.run(task.workflowId, {
      trigger: 'schedule',
      sourceMessage: task.name,
      inputs: task.workflowInputs,
    })
    if (!workflowRun) throw new Error('选择的工作流不存在。')
    scheduleRun.workflowRunId = workflowRun.id
    await this.save()

    while (true) {
      if (this.disposed)
        throw Object.assign(new Error(SHUTDOWN_INTERRUPTED_ERROR), {
          code: 'SCHEDULE_INTERRUPTED',
        })
      const current = this.workflows.getRun(workflowRun.id)
      if (!current) throw new Error('工作流运行记录不存在。')
      if (TERMINAL_WORKFLOW_STATUSES.has(current.status)) {
        if (current.status !== 'completed')
          throw new Error(current.error || `工作流运行${current.status}。`)
        return {
          text: current.summary || '工作流已完成。',
          sessionId: current.sessionId || '',
          workflowRunId: current.id,
        }
      }
      await new Promise((resolve) => setTimeout(resolve, 250))
    }
  }

  async dispose() {
    this.disposed = true
    clearInterval(this.timer)
    this.timer = null
    await Promise.allSettled([...this.executions])
    await this.writeQueue.catch(() => {})
  }
}
