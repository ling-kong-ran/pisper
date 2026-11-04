// Bot 对话的命令不阻塞（v2.1）：对话自己跑的命令超过 10 秒、或者这一步被新消息打断时，命令不杀掉，转到后台接着跑，
// 工具结果先告诉对话“已转到后台”；跑完后把结果作为一条内部消息交回对话（和子 agent 做完一样）。
// 后台命令在时间线里是一件活（“在跑：npm test”），可以停。只在内存里，Runtime 重启后不恢复。
import { randomBytes } from 'node:crypto'

export const BOT_COMMAND_BACKGROUND_AFTER_MS = 10_000
export const BOT_COMMAND_RESULT_MARKER = '[Pisper internal command result]'
const MAX_OUTPUT_CHARS = 4_000
const MAX_FINISHED_PER_SESSION = 20

export function isBotCommandResultMessage(content) {
  return String(content || '').startsWith(BOT_COMMAND_RESULT_MARKER)
}

function textOf(result) {
  return (Array.isArray(result?.content) ? result.content : [])
    .filter((part) => part?.type === 'text')
    .map((part) => part.text || '')
    .join('')
}

function tail(value) {
  const text = String(value || '')
  return text.length > MAX_OUTPUT_CHARS ? `…${text.slice(-MAX_OUTPUT_CHARS)}` : text
}

// 后台命令跑完交回对话的那条消息（转录里不显示，对话据此汇报）。
export function botCommandResultPrompt(job) {
  return [
    BOT_COMMAND_RESULT_MARKER,
    `Background command ${job.id} (\`${job.message}\`) ${job.status === 'completed' ? 'finished' : job.status === 'interrupted' ? 'was stopped' : 'failed'}.`,
    job.output ? `Output (tail):\n${job.output}` : 'No output.',
    job.error ? `Error: ${job.error}` : '',
    'Use this result to continue; tell the user briefly if it matters to them.',
  ]
    .filter(Boolean)
    .join('\n')
}

export class BotCommandJobs {
  constructor({
    backgroundAfterMs = BOT_COMMAND_BACKGROUND_AFTER_MS,
    now = () => Date.now(),
    onFinished,
  } = {}) {
    this.backgroundAfterMs = backgroundAfterMs
    this.now = now
    this.onFinished = onFinished || (() => {})
    this.jobs = new Map()
  }

  // 包住对话的 bash：结果在时限内回来就照常返回；超时或这一步被打断就转后台。
  wrap(bashTool, { sessionId }) {
    return {
      ...bashTool,
      description: `${bashTool.description}\nIn a bot conversation, a command still running after ${Math.round(this.backgroundAfterMs / 1000)}s (or when the user sends a new message) keeps running in the background; you get its result later as a message. Do not wait for it or poll it.`,
      execute: (toolCallId, params, signal, onUpdate, ctx) =>
        this.run({ bashTool, sessionId, toolCallId, params, signal, onUpdate, ctx }),
    }
  }

  async run({ bashTool, sessionId, toolCallId, params, signal, onUpdate, ctx }) {
    const controller = new AbortController()
    const job = {
      id: `cmd_${randomBytes(6).toString('hex')}`,
      sessionId,
      command: String(params?.command || '').slice(0, 300),
      status: 'running',
      output: '',
      error: '',
      background: false,
      startedAt: new Date(this.now()).toISOString(),
      completedAt: null,
      controller,
    }
    let forward = true
    const running = Promise.resolve().then(() =>
      bashTool.execute(
        toolCallId,
        params,
        controller.signal,
        (update) => {
          job.output = tail(textOf(update))
          if (forward) onUpdate?.(update)
        },
        ctx,
      ),
    )
    let timer
    const moved = new Promise((resolve) => {
      timer = setTimeout(() => resolve('timeout'), this.backgroundAfterMs)
      timer.unref?.()
      if (signal?.aborted) resolve('interrupted')
      else signal?.addEventListener?.('abort', () => resolve('interrupted'), { once: true })
    })
    const settled = running.then(
      (result) => ({ result }),
      (error) => ({ error }),
    )
    const first = await Promise.race([settled, moved])
    clearTimeout(timer)
    if (typeof first === 'object') {
      if (first.error) throw first.error
      return first.result
    }
    // 转后台：命令接着跑，跑完把结果交回对话。
    forward = false
    job.background = true
    this.jobs.set(job.id, job)
    void settled.then(({ result, error }) => {
      job.completedAt = new Date(this.now()).toISOString()
      if (job.status === 'interrupted') {
        // 用户停掉的：照样告诉对话一声。
      } else if (error) {
        job.status = 'failed'
        job.error = String(error instanceof Error ? error.message : error).slice(0, 1_000)
      } else {
        job.status = 'completed'
        job.output = tail(textOf(result)) || job.output
      }
      delete job.controller
      this.prune(sessionId)
      try {
        this.onFinished(sessionId, this.public(job))
      } catch {}
    })
    return {
      content: [
        {
          type: 'text',
          text: `Still running ${first === 'timeout' ? `after ${Math.round(this.backgroundAfterMs / 1000)}s` : 'when the user sent a new message'}; moved to the background as ${job.id}. Its result will come back to you as a message when it finishes. Do not wait for it: carry on, or answer the user.`,
        },
      ],
      details: { backgroundCommand: job.id },
    }
  }

  stop(sessionId, id) {
    const job = this.jobs.get(id)
    if (!job || job.sessionId !== sessionId || job.status !== 'running') return null
    job.status = 'interrupted'
    job.controller?.abort(new Error('stopped'))
    return this.public(job)
  }

  list(sessionId) {
    return [...this.jobs.values()]
      .filter((job) => job.sessionId === sessionId)
      .map((job) => this.public(job))
  }

  hasActive(sessionId) {
    return [...this.jobs.values()].some(
      (job) => job.sessionId === sessionId && job.status === 'running',
    )
  }

  // 时间线用的形状与子 agent 摘要一致（kind 区分），界面写“在跑：命令”。
  public(job) {
    return {
      id: job.id,
      kind: 'command',
      taskName: job.command,
      title: job.command,
      status: job.status,
      message: job.command,
      output: job.output,
      error: job.error,
      startedAt: job.startedAt,
      completedAt: job.completedAt,
      toolCallCount: 0,
      currentActivity: null,
    }
  }

  prune(sessionId) {
    const finished = [...this.jobs.values()].filter(
      (job) => job.sessionId === sessionId && job.status !== 'running',
    )
    for (const job of finished.slice(0, Math.max(0, finished.length - MAX_FINISHED_PER_SESSION)))
      this.jobs.delete(job.id)
  }

  stopSession(sessionId) {
    for (const job of this.jobs.values())
      if (job.sessionId === sessionId && job.status === 'running') this.stop(sessionId, job.id)
  }
}
