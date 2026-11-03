// Bot 任务服务（B2）：Bot 每被叫一次就是一个任务。负责状态机、排队与并发、隔离目录、交付与重启恢复。
// 状态：queued → preparing → running → review / done / failed；review → done / discarded；
// running 遇到重启变 interrupted，可“继续”回到 queued。每个 Bot 同时只跑一个任务，全局最多 MAX_RUNNING 个。
// 会话归会话服务所有，任务只记 sessionId；真正跑代理由注入的 runAgent / stopAgent 完成。
import { randomBytes } from 'node:crypto'
import { readJson, writeJsonAtomic } from '../../storage/json-file.mjs'

export const MAX_RUNNING = 3
export const MAX_TASKS = 500
export const MAX_REQUEST_CHARS = 12_000
const STORE_VERSION = 1
const ACTIVE = new Set(['preparing', 'running'])
// 占着 Bot、不能删除 Bot 的状态：还在跑，或改动等人处理。
const OPEN = new Set(['queued', 'preparing', 'running', 'review'])
const TERMINAL = new Set(['done', 'failed', 'discarded'])
const RESTART_ERROR = {
  code: 'runtime_restarted',
  message: '任务因 Pisper 重启而中断，可以点“继续”。',
}
const STOPPED_ERROR = { code: 'stopped', message: '任务已被停止。' }

export class BotTaskError extends Error {
  constructor(code, message, statusCode = 400) {
    super(message)
    this.name = 'BotTaskError'
    this.code = code
    this.statusCode = statusCode
  }
}

function clone(value) {
  return JSON.parse(JSON.stringify(value))
}

function newTaskId() {
  return `task_${randomBytes(9).toString('base64url')}`
}

function errorOf(error, fallback = 'task_failed') {
  return {
    code: typeof error?.code === 'string' ? error.code : fallback,
    message: String(error?.message || error || '任务失败。').slice(0, 500),
  }
}

// 交给代理的第一条消息：Bot 的固定说明在前，本次请求在后；隔离运行时说明边界。
export function botTaskPrompt(bot, task, { isolated }) {
  const parts = []
  if (bot.instructions.trim()) parts.push(`[${bot.name} 的工作说明]\n${bot.instructions.trim()}`)
  parts.push(`[任务]\n${task.request}`)
  if (isolated)
    parts.push(
      '[工作方式]\n你在这个任务专用的 git 工作树里工作，改动会交给人确认后再合入。不要推送、不要切换或删除分支。',
    )
  return parts.join('\n\n')
}

// 待确认时“接着说”的那一轮：只带修改意见本身，Bot 的说明已在同一会话的前文里。
export function botFollowUpPrompt(text, { isolated }) {
  const parts = [`[接着改]\n${text}`]
  if (isolated)
    parts.push('[工作方式]\n继续在当前分支上修改，改动会再交给人确认。不要推送、不要切换分支。')
  return parts.join('\n\n')
}

export class BotTaskService {
  constructor({
    path,
    getBot,
    worktrees,
    runAgent,
    stopAgent,
    // 运行中的任务在等什么（审批或问题），由 Runtime 从会话里读；没有接线时视为不等待。
    waitingFor = () => null,
    maxRunning = MAX_RUNNING,
    now = () => Date.now(),
  }) {
    this.path = path
    this.getBot = getBot
    this.worktrees = worktrees
    this.runAgent = runAgent
    this.stopAgent = stopAgent
    this.waitingFor = waitingFor
    this.maxRunning = maxRunning
    this.now = now
    this.tasks = []
    this.running = new Map()
    this.writeQueue = Promise.resolve()
    this.disposed = false
  }

  #at() {
    return new Date(this.now()).toISOString()
  }

  async init() {
    const stored = await readJson(this.path, { version: STORE_VERSION, tasks: [] })
    const at = this.#at()
    this.tasks = (Array.isArray(stored?.tasks) ? stored.tasks : [])
      .filter((task) => task && typeof task.id === 'string' && typeof task.botId === 'string')
      .map((task) => {
        // 准备中：worktree 可能建了一半，回到排队重新准备；运行中：命令可能只执行了一半，交给人决定。
        if (task.status === 'preparing') return { ...task, status: 'queued', updatedAt: at }
        if (task.status === 'running')
          return { ...task, status: 'interrupted', error: RESTART_ERROR, updatedAt: at }
        return task
      })
    await this.save()
    this.schedule()
  }

  save() {
    // 超出上限时先删最早的终态任务，未结束的任务不删。
    while (this.tasks.length > MAX_TASKS) {
      const index = this.tasks.findIndex((task) => TERMINAL.has(task.status))
      if (index < 0) break
      this.tasks.splice(index, 1)
    }
    const snapshot = { version: STORE_VERSION, tasks: clone(this.tasks) }
    this.writeQueue = this.writeQueue
      .catch(() => {})
      .then(() => writeJsonAtomic(this.path, snapshot, { mode: 0o600 }))
    return this.writeQueue
  }

  list({ botId = '', status = '', limit = 100 } = {}) {
    return this.tasks
      .filter((task) => (!botId || task.botId === botId) && (!status || task.status === status))
      .slice(-Math.max(1, Math.min(500, limit)))
      .reverse()
      .map((task) => this.#public(task))
  }

  get(id) {
    const task = this.tasks.find((item) => item.id === id)
    return task ? this.#public(task) : null
  }

  // 对外的任务：存储里的字段 + 运行中时正在等的审批或问题（只在内存里，不落盘）。
  #public(task) {
    const copy = clone(task)
    const waiting = this.#waiting(task)
    if (waiting) copy.waiting = waiting
    return copy
  }

  #waiting(task) {
    if (task.status !== 'running' || !task.sessionId) return null
    const waiting = this.waitingFor(task.sessionId)
    return waiting && (waiting.approval || waiting.question) ? waiting : null
  }

  isBusy(botId) {
    return this.tasks.some((task) => task.botId === botId && OPEN.has(task.status))
  }

  #find(id) {
    const task = this.tasks.find((item) => item.id === id)
    if (!task) throw new BotTaskError('bot_task_not_found', '任务不存在。', 404)
    return task
  }

  #set(task, patch) {
    Object.assign(task, patch, { updatedAt: this.#at() })
  }

  // 派活：校验 Bot 与请求后排队，立即尝试调度。source 由调用方（路由、渠道、定时任务）给出。
  async dispatch(botId, { text, source = { kind: 'manual' } } = {}) {
    const bot = this.getBot(botId)
    if (!bot) throw new BotTaskError('bot_not_found', 'Bot 不存在。', 404)
    if (!bot.enabled) throw new BotTaskError('bot_paused', `${bot.name} 已暂停，不接新任务。`, 409)
    const request = String(text ?? '').trim()
    if (!request) throw new BotTaskError('bot_task_empty', '要让 Bot 做什么？请写下任务内容。')
    if (request.length > MAX_REQUEST_CHARS)
      throw new BotTaskError('bot_task_too_long', `任务内容最多 ${MAX_REQUEST_CHARS} 个字符。`)
    const at = this.#at()
    const task = {
      id: newTaskId(),
      botId,
      botName: bot.name,
      status: 'queued',
      source,
      title: request.split('\n')[0].slice(0, 80),
      request,
      // 对话里的每一轮：第一轮是派活，之后是待确认时“接着说”的修改意见；reply 是 Bot 这一轮的回复。
      turns: [{ text: request, at }],
      sessionId: null,
      workspace: null,
      result: null,
      error: null,
      createdAt: at,
      updatedAt: at,
      finishedAt: null,
    }
    this.tasks.push(task)
    await this.save()
    this.schedule()
    return clone(task)
  }

  // 在 Bot 的对话里说一句话：有改动正等确认时，这句话是对那份改动的修改意见，在同一个任务、
  // 同一个分支上接着改；否则就是一件新的事，排成新任务。
  async message(botId, { text, source = { kind: 'chat' } } = {}) {
    const bot = this.getBot(botId)
    if (!bot) throw new BotTaskError('bot_not_found', 'Bot 不存在。', 404)
    const pending = this.tasks.findLast((task) => task.botId === botId && task.status === 'review')
    if (!pending) return this.dispatch(botId, { text, source })
    if (!bot.enabled) throw new BotTaskError('bot_paused', `${bot.name} 已暂停，不接新任务。`, 409)
    const request = String(text ?? '').trim()
    if (!request) throw new BotTaskError('bot_task_empty', '要让 Bot 做什么？请写下任务内容。')
    if (request.length > MAX_REQUEST_CHARS)
      throw new BotTaskError('bot_task_too_long', `任务内容最多 ${MAX_REQUEST_CHARS} 个字符。`)
    pending.turns = [...(pending.turns || [{ text: pending.request, at: pending.createdAt }])]
    pending.turns.push({ text: request, at: this.#at() })
    this.#set(pending, { status: 'queued', error: null })
    await this.save()
    this.schedule()
    return clone(pending)
  }

  // 给侧栏的一句话动态：最近一个任务的状态，以及是否需要你（待确认、失败、中断）。
  activity(botId) {
    const latest = this.tasks.findLast((task) => task.botId === botId)
    if (!latest) return null
    const waiting = this.#waiting(latest)
    return {
      taskId: latest.id,
      status: latest.status,
      title: latest.title,
      changedFiles: latest.result?.changedFiles || 0,
      branch: latest.workspace?.branch || '',
      needsYou: Boolean(waiting) || ['review', 'failed', 'interrupted'].includes(latest.status),
      // 'approval' | 'question' | null：侧栏据此写“等你批准”或“有问题问你”。
      waiting: waiting ? (waiting.approval ? 'approval' : 'question') : null,
      updatedAt: latest.updatedAt,
    }
  }

  // 调度：按排队先后，跳过所属 Bot 已在忙的任务，直到占满全局并发。
  schedule() {
    if (this.disposed) return
    for (const task of this.tasks) {
      if (this.running.size >= this.maxRunning) return
      if (task.status !== 'queued') continue
      const busy = this.tasks.some(
        (other) => other !== task && other.botId === task.botId && ACTIVE.has(other.status),
      )
      if (busy) continue
      // 执行中的意外（例如状态文件写不进去）只记日志，不让一个任务的失败变成未处理的异常。
      const execution = this.#execute(task).catch((error) => {
        console.warn('[bots] 任务执行出错：', error instanceof Error ? error.message : error)
      })
      this.running.set(task.id, { execution, stopped: false })
      execution.finally(() => {
        this.running.delete(task.id)
        this.schedule()
      })
    }
  }

  async #execute(task) {
    const bot = this.getBot(task.botId)
    if (!bot) {
      this.#set(task, {
        status: 'failed',
        error: { code: 'bot_not_found', message: 'Bot 已被删除。' },
        finishedAt: this.#at(),
      })
      await this.save()
      return
    }
    this.#set(task, { status: 'preparing', error: null })
    await this.save()
    const isolated = bot.workspace.isolation === 'worktree'
    try {
      // “继续”一个已有 worktree 的任务时沿用原来的目录与分支。
      if (!task.workspace || task.workspace.state !== 'ready')
        task.workspace = isolated
          ? await this.worktrees.create({
              cwd: bot.workspace.cwd,
              taskId: task.id,
              botName: bot.name,
            })
          : { kind: 'shared', path: bot.workspace.cwd, state: 'ready' }
    } catch (error) {
      this.#set(task, { status: 'failed', error: errorOf(error), finishedAt: this.#at() })
      await this.save()
      return
    }
    if (this.running.get(task.id)?.stopped) return this.#finishStopped(task)
    this.#set(task, { status: 'running' })
    await this.save()
    let outcome
    try {
      const turns = task.turns || [{ text: task.request }]
      outcome = await this.runAgent({
        bot,
        task: clone(task),
        cwd: task.workspace.path,
        // 同一个 Bot 的所有任务都在它的长期会话里跑；第一次还没有会话时由代理新建。
        sessionId: task.sessionId || bot.threadSessionId || '',
        message:
          turns.length > 1
            ? botFollowUpPrompt(turns.at(-1).text, { isolated })
            : botTaskPrompt(bot, task, { isolated }),
        onSession: (sessionId) => {
          task.sessionId = sessionId
          this.save().catch(() => {})
        },
      })
    } catch (error) {
      if (this.running.get(task.id)?.stopped) return this.#finishStopped(task)
      this.#set(task, { status: 'failed', error: errorOf(error), finishedAt: this.#at() })
      await this.save()
      return
    }
    if (this.running.get(task.id)?.stopped) return this.#finishStopped(task)
    if (outcome?.sessionId) task.sessionId = outcome.sessionId
    const summary = String(outcome?.text || '')
      .trim()
      .slice(0, 1_200)
    if (task.turns?.length) task.turns.at(-1).reply = summary
    try {
      await this.#deliver(task, bot, summary)
    } catch (error) {
      this.#set(task, { status: 'failed', error: errorOf(error), finishedAt: this.#at() })
    }
    await this.save()
  }

  // 交付：只回复或共享目录直接完成；隔离目录里没改动也直接完成并清掉 worktree；
  // 有改动进 review 等人确认（commit 交付先在任务分支上提交）。
  // 先清理再标为完成：状态不能领先于实际（看到“完成”时 worktree 与分支已经删掉）。
  async #deliver(task, bot, summary) {
    if (task.workspace.kind !== 'worktree' || bot.delivery === 'reply-only') {
      if (task.workspace.kind === 'worktree') await this.#removeWorkspace(task, true)
      this.#set(task, {
        status: 'done',
        result: { summary, changedFiles: 0 },
        finishedAt: this.#at(),
      })
      return
    }
    const { files } = await this.worktrees.changes(task.workspace)
    if (files.length === 0) {
      await this.#removeWorkspace(task, true)
      this.#set(task, {
        status: 'done',
        result: { summary, changedFiles: 0 },
        finishedAt: this.#at(),
      })
      return
    }
    const commit =
      bot.delivery === 'commit'
        ? await this.worktrees.commit(task.workspace, `${bot.name}: ${task.title}`)
        : undefined
    this.#set(task, {
      status: 'review',
      result: { summary, changedFiles: files.length, ...(commit ? { commit } : {}) },
    })
  }

  async #removeWorkspace(task, deleteBranch) {
    await this.worktrees.remove(task.workspace, { deleteBranch })
    task.workspace = { ...task.workspace, state: 'removed' }
  }

  async #finishStopped(task) {
    this.#set(task, { status: 'failed', error: STOPPED_ERROR, finishedAt: this.#at() })
    await this.save()
  }

  async diff(id) {
    const task = this.#find(id)
    if (task.workspace?.kind !== 'worktree' || task.workspace.state !== 'ready')
      throw new BotTaskError('invalid_state', '这个任务没有可查看的改动。', 409)
    return this.worktrees.changes(task.workspace)
  }

  // 接受：在任务分支上提交（若还没提交），可选应用到用户当前分支；之后删掉 worktree、保留分支。
  async accept(id, { apply = false } = {}) {
    const task = this.#find(id)
    if (task.status !== 'review')
      throw new BotTaskError('invalid_state', '只有待确认的任务能接受。', 409)
    const commit =
      task.result?.commit ||
      (await this.worktrees.commit(task.workspace, `${task.botName}: ${task.title}`))
    if (!commit) throw new BotTaskError('invalid_state', '这个任务没有改动可以接受。', 409)
    if (apply) await this.worktrees.apply(task.workspace, commit)
    await this.#removeWorkspace(task, false)
    this.#set(task, {
      status: 'done',
      result: { ...task.result, commit, applied: Boolean(apply), branch: task.workspace.branch },
      finishedAt: this.#at(),
    })
    await this.save()
    return clone(task)
  }

  // 放弃：删掉 worktree 与分支。排队中、待确认、失败、中断的任务都可以放弃。
  async discard(id) {
    const task = this.#find(id)
    if (!['queued', 'review', 'failed', 'interrupted'].includes(task.status))
      throw new BotTaskError('invalid_state', '运行中的任务请先停止。', 409)
    if (task.workspace?.kind === 'worktree') await this.#removeWorkspace(task, true)
    this.#set(task, { status: 'discarded', finishedAt: this.#at() })
    await this.save()
    return clone(task)
  }

  // 继续：中断或失败的任务回到排队，沿用原 worktree（如果还在）。
  async resume(id) {
    const task = this.#find(id)
    if (!['interrupted', 'failed'].includes(task.status))
      throw new BotTaskError('invalid_state', '只有中断或失败的任务能继续。', 409)
    this.#set(task, { status: 'queued', error: null, finishedAt: null })
    await this.save()
    this.schedule()
    return clone(task)
  }

  // 停止：排队中的直接放弃；运行中的通知代理停下，由执行流程收尾为“已停止”。
  async stop(id) {
    const task = this.#find(id)
    if (task.status === 'queued') return this.discard(id)
    const running = this.running.get(id)
    if (!running) throw new BotTaskError('invalid_state', '这个任务没有在运行。', 409)
    running.stopped = true
    if (task.sessionId) await this.stopAgent(task.sessionId)
    await running.execution
    return clone(task)
  }

  async dispose() {
    this.disposed = true
    await Promise.allSettled([...this.running.values()].map((item) => item.execution))
    await this.writeQueue.catch(() => {})
  }
}
