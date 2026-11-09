// 定时任务服务：按频率（once/interval/daily/weekly/monthly）调度会话 prompt 或工作流执行，
// 支持时区、运行记录、重启后补偿未执行任务。
// 一次性任务（once）到点执行一次后自动停用；Pisper 没在运行而错过的，启动时标为“已错过”并停用，
// 不补跑——“9 点发布”这类任务晚几个小时再执行可能有害，需要的话由人手动立即运行。
import { randomUUID } from 'node:crypto'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { pisperError } from '../../shared/errors.mjs'

export const DEFAULT_SCHEDULE_EXECUTION_MODE = 'full-access'

const FREQUENCIES = new Set(['once', 'interval', 'daily', 'weekly', 'monthly'])
const INTERVAL_UNITS = new Set(['minutes', 'hours', 'days'])
const INTERVAL_MS = { minutes: 60_000, hours: 60 * 60_000, days: 24 * 60 * 60_000 }
const NOTIFICATION_TARGETS = new Set(['browser', 'feishu', 'weixin', 'qq', 'telegram'])
// bot（B3）：到点给某个 Bot 派一件事。要做的事写在 botMessage 里、prompt 留空——旧版本不认识 bot 会把它当成
// prompt 任务，空的 prompt 只会失败，不会以完全访问权限去执行这段话。
const SCHEDULE_TARGETS = new Set(['prompt', 'workflow', 'bot'])
const MAX_BOT_MESSAGE_CHARS = 12_000
const TERMINAL_WORKFLOW_STATUSES = new Set(['completed', 'failed', 'cancelled', 'interrupted'])
const RESTART_INTERRUPTED_ERROR = 'Interrupted because Pisper restarted.'
const SHUTDOWN_INTERRUPTED_ERROR = 'Interrupted because Pisper closed.'
const MISSED_ONCE_ERROR =
  'Skipped: Pisper was not running at the scheduled time. Run it now manually if needed.'
const DATE_PATTERN = /^\d{4}-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])$/

function defaultState() {
  return { version: 1, tasks: [], runs: [] }
}

function normalizeScheduleExecutionMode(value) {
  if (value !== DEFAULT_SCHEDULE_EXECUTION_MODE) return DEFAULT_SCHEDULE_EXECUTION_MODE
  return value
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

// 一次性任务的执行时刻：日期 + 时间按任务时区换算；日期非法时返回 null。
function onceRunAt(task) {
  if (!DATE_PATTERN.test(String(task.date || ''))) return null
  const [year, month, day] = String(task.date).split('-').map(Number)
  if (day > daysInMonth(year, month)) return null
  const [hour, minute] = String(task.time || '09:00')
    .split(':')
    .map(Number)
  return zonedTimeToUtc({ year, month, day, hour, minute }, task.timezone)
}

// 下次运行时刻；一次性任务过了执行时刻返回 null（不再安排）。
export function calculateNextRun(task, from = new Date()) {
  if (task.frequency === 'once') {
    const at = onceRunAt(task)
    return at && at.getTime() > from.getTime() ? at.toISOString() : null
  }
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
    throw pisperError('invalid', 'schedule', 'invalid time zone')
  }
  const time = /^([01]\d|2[0-3]):[0-5]\d$/.test(String(task.time || ''))
    ? String(task.time)
    : '09:00'
  const frequency = FREQUENCIES.has(task.frequency) ? task.frequency : 'daily'
  const normalized = {
    id: String(task.id || randomUUID()),
    name: String(task.name || 'Untitled task').slice(0, 120),
    targetType: SCHEDULE_TARGETS.has(task.targetType) ? task.targetType : 'prompt',
    prompt: String(task.prompt || '').slice(0, 100_000),
    workflowId: String(task.workflowId || '').slice(0, 200),
    botId: String(task.botId || '').slice(0, 64),
    botMessage: String(task.botMessage || '').slice(0, MAX_BOT_MESSAGE_CHARS),
    // Bot 在群里给自己定的：到点送回那个群里的席位（旧版本不认识这个字段，照常发进 Bot 的长期对话）。
    ...(task.targetType === 'bot' && /^room_[A-Za-z0-9_-]{6,40}$/.test(String(task.roomId || ''))
      ? { roomId: String(task.roomId) }
      : {}),
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
    date: DATE_PATTERN.test(String(task.date || '')) ? String(task.date) : '',
    cwd: String(task.cwd || cwd),
    executionMode: normalizeScheduleExecutionMode(task.executionMode),
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
    // 谁建的：代理通过 schedule_manage 建的任务记下会话，界面里标出来，便于追查。
    createdBy:
      task.createdBy?.kind === 'agent'
        ? { kind: 'agent', sessionId: String(task.createdBy.sessionId || '').slice(0, 200) }
        : { kind: 'user' },
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

// 通知模板里的耗时：用不分语言的 45s / 3m 12s，聊天软件里的中文模板与英文都看得懂。
function durationLabel(durationMs) {
  const seconds = Math.max(0, Math.round(durationMs / 1000))
  if (seconds < 60) return `${seconds}s`
  return `${Math.floor(seconds / 60)}m ${seconds % 60}s`
}

function nextRunLabel(task) {
  if (!task.nextRunAt) return '-'
  return new Intl.DateTimeFormat('zh-CN', {
    timeZone: task.timezone,
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(new Date(task.nextRunAt))
}

export class ScheduleService {
  constructor({ path, cwd, agent, workflows, notifications, bots = null, tickMs = 15_000 }) {
    this.path = path
    this.cwd = cwd
    this.agent = agent
    this.workflows = workflows
    this.bots = bots
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
        // 启用中的一次性任务已经没有下次运行：执行时刻在 Pisper 关闭期间过去了。
        if (normalized.frequency === 'once' && normalized.enabled && !normalized.nextRunAt) {
          normalized.enabled = false
          normalized.lastStatus = 'missed'
          normalized.lastError = MISSED_ONCE_ERROR
          normalized.updatedAt = interruptedAt.toISOString()
        }
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
    const botId = String(merged.botId || '').trim()
    const botMessage = String(merged.botMessage || '').trim()
    const workflowInputs =
      merged.workflowInputs &&
      typeof merged.workflowInputs === 'object' &&
      !Array.isArray(merged.workflowInputs)
        ? merged.workflowInputs
        : {}
    if (!name) throw pisperError('invalid', 'schedule', 'name is required')
    if (targetType === 'prompt' && !prompt)
      throw pisperError('invalid', 'schedule', 'prompt is required')
    if (targetType === 'workflow') {
      if (!workflowId) throw pisperError('invalid', 'schedule', 'choose a workflow')
      const workflow = this.workflows?.list().find((item) => item.id === workflowId)
      if (!workflow) throw pisperError('not_found', 'workflow')
      if (workflow.status !== 'published')
        throw pisperError('invalid', 'workflow', 'only published workflows can be scheduled')
      const missingInput = (workflow.inputs || []).find((definition) => {
        const value = Object.hasOwn(workflowInputs, definition.name)
          ? workflowInputs[definition.name]
          : definition.defaultValue
        return definition.required && (value === undefined || value === null || value === '')
      })
      if (missingInput) throw pisperError('invalid', 'input', `${missingInput.label} is required`)
    }
    if (targetType === 'bot') {
      if (!this.bots?.enabled())
        throw pisperError('unavailable', 'bot', 'turn on Bots in Settings › Labs')
      if (!this.bots.get(botId)) throw pisperError('not_found', 'bot')
      if (!botMessage) throw pisperError('invalid', 'message', 'say what the bot should do')
      if (botMessage.length > MAX_BOT_MESSAGE_CHARS)
        throw pisperError('too_large', 'message', `max ${MAX_BOT_MESSAGE_CHARS} characters`)
    }
    if (targetType === 'prompt' && Object.hasOwn(input || {}, 'cwd'))
      merged.cwd = await this.agent.validateDirectory(input.cwd)
    const task = normalizeStoredTask(
      {
        ...merged,
        name,
        targetType,
        prompt: targetType === 'bot' ? '' : prompt,
        workflowId,
        workflowInputs,
        botId: targetType === 'bot' ? botId : '',
        botMessage: targetType === 'bot' ? botMessage : '',
        updatedAt: new Date().toISOString(),
      },
      this.cwd,
    )
    task.nextRunAt = task.enabled ? calculateNextRun(task) : null
    if (task.frequency === 'once') {
      if (!task.date)
        throw pisperError('invalid', 'schedule', 'date is required for a one-time task')
      if (task.enabled && !task.nextRunAt)
        throw pisperError('invalid', 'schedule', 'the time has passed; choose a future time')
    }
    return task
  }

  // createdBy 只由调用方（HTTP 路由或代理工具）给出，不从请求体读取。
  async create(input, { createdBy = { kind: 'user' } } = {}) {
    const task = await this.normalizeInput({
      ...input,
      id: randomUUID(),
      createdAt: new Date().toISOString(),
      createdBy,
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
    task.createdBy = current.createdBy
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
    if (this.running.has(id)) throw pisperError('busy', 'schedule')
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
    if (this.running.has(task.id)) throw pisperError('busy', 'schedule')
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
    // 一次性任务到点执行这一次后停用；手动“立即运行”不影响原计划。
    if (trigger === 'scheduled' && task.frequency === 'once') {
      task.enabled = false
      task.nextRunAt = null
    }
    await this.save()
    const execution = this.execute(task, run)
    this.executions.add(execution)
    execution.finally(() => this.executions.delete(execution)).catch(() => {})
  }

  // 到点对 Bot 说一句话：话进它的长期会话，用它自己的设定（电脑、权限上限、工具）去做；
  // 它正在说话时排在这一轮之后。不走渠道白名单（计划由本机用户配置），也不用定时任务的完全访问默认值。
  async executeBot(task) {
    if (!this.bots?.enabled()) throw pisperError('unavailable', 'bot', 'Bots are turned off')
    const bot = this.bots.get(task.botId)
    if (!bot) throw pisperError('not_found', 'bot')
    const sent = await this.bots.message(task.botId, {
      text: task.botMessage,
      source: {
        kind: 'schedule',
        scheduleTaskId: task.id,
        ...(task.roomId ? { roomId: task.roomId } : {}),
      },
      // 等到它真的开始：开不起来（模型不可用等）时这次运行记为失败，不写“已发送”。
      waitForStart: true,
    })
    return {
      text:
        sent.status === 'queued'
          ? `${bot.name} is busy; queued after its current turn.`
          : `Sent to ${bot.name}.`,
      sessionId: sent.sessionId,
    }
  }

  async deleteForBot(botId) {
    const before = this.state.tasks.length
    this.state.tasks = this.state.tasks.filter(
      (task) => !(task.targetType === 'bot' && task.botId === botId),
    )
    if (this.state.tasks.length !== before) await this.save()
    return before - this.state.tasks.length
  }

  async execute(task, run) {
    const started = Date.now()
    let event = 'schedule.completed'
    let data
    try {
      const result =
        task.targetType === 'workflow'
          ? await this.executeWorkflow(task, run)
          : task.targetType === 'bot'
            ? await this.executeBot(task)
            : await this.agent.prompt({
                message: task.prompt,
                cwd: task.cwd,
                title: `Schedule · ${task.name}`,
                model: task.model,
                executionMode: task.executionMode,
                isolatedContext: true,
                // 定时任务没有人在场，不提供 ask_user（规格 8.4）。
                unattended: true,
              })
      const summary = String(result.text || 'Task completed.')
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
    if (!workflow) throw pisperError('not_found', 'workflow')
    if (workflow.status !== 'published')
      throw pisperError('invalid', 'workflow', 'only published workflows can be scheduled')
    const workflowRun = await this.workflows.run(task.workflowId, {
      trigger: 'schedule',
      sourceMessage: task.name,
      inputs: task.workflowInputs,
    })
    if (!workflowRun) throw pisperError('not_found', 'workflow')
    scheduleRun.workflowRunId = workflowRun.id
    await this.save()

    while (true) {
      if (this.disposed)
        throw Object.assign(new Error(SHUTDOWN_INTERRUPTED_ERROR), {
          code: 'SCHEDULE_INTERRUPTED',
        })
      const current = this.workflows.getRun(workflowRun.id)
      if (!current) throw pisperError('not_found', 'workflow_run')
      if (TERMINAL_WORKFLOW_STATUSES.has(current.status)) {
        if (current.status !== 'completed')
          throw current.error
            ? new Error(current.error)
            : pisperError('failed', 'workflow_run', current.status)
        return {
          text: current.summary || 'Workflow completed.',
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
