// 捕获任务拥有后台请求与取消生命周期；模型提取、持久化由窄依赖完成。
// 每轮写原文（L0）；用户明确说“记住”时立即抽取；其余轮次等会话空闲或压缩前批量抽取，只用辅助模型。
import { createHash } from 'node:crypto'
import { awaitMemoryOperation } from '../services/memory/abortable-memory-operation.mjs'
import {
  extractBatchMemories,
  extractConversationMemories,
  shouldExtractConversationMemory,
} from '../services/memory/conversation-memory.mjs'

// 这些失败下次还可能成功，游标不动；其余（回复解析不出来、超长、为空）重试多半还是一样，
// 而且每次都要花钱，所以跳过这一批。
const RETRYABLE_BATCH_ERRORS = new Set([
  'aborted',
  'budget_exhausted',
  'model_error',
  'model_unavailable',
])

// 本地日期键（YYYY-MM-DD），用量账本按本地时区分桶。
export function localDayKey(value = new Date()) {
  const date = value instanceof Date ? value : new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  const pad = (part) => String(part).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

export class ConversationMemoryCapture {
  constructor({
    getModelRuntime,
    waitForInitialization,
    memory,
    recordUsage,
    modelClient = null,
    reportFailure = (diagnostic) => console.warn('[memory] capture failed', diagnostic),
    timeoutMs = 120_000,
  }) {
    this.getModelRuntime = getModelRuntime
    this.waitForInitialization = waitForInitialization
    this.memory = memory
    this.recordUsage = recordUsage
    this.modelClient = modelClient
    this.reportFailure = reportFailure
    this.timeoutMs = timeoutMs
    this.tasks = new Map()
    // 正在批量抽取的会话：后台定时和压缩前可能撞上，同一批不花两次钱。
    this.flushing = new Set()
    this.closed = false
  }

  diagnose(code, sessionId) {
    // warning 只含固定错误码和会话哈希，便于关联故障且不泄漏原文、路径或 SDK 异常。
    this.reportFailure({
      code,
      session: createHash('sha256')
        .update(String(sessionId || ''))
        .digest('hex')
        .slice(0, 12),
    })
  }

  // 一个后台任务：带超时、可被 dispose 和外部 signal 取消，异常只留诊断码。
  track(sessionId, work, { fallback, failureCode, signal } = {}) {
    const controller = new AbortController()
    let timedOut = false
    const timer = setTimeout(() => {
      timedOut = true
      controller.abort()
    }, this.timeoutMs)
    const onExternalAbort = () => controller.abort()
    signal?.addEventListener('abort', onExternalAbort, { once: true })
    if (signal?.aborted) controller.abort()
    const task = work(controller.signal)
      .catch(() => {
        if (!controller.signal.aborted) this.diagnose(failureCode, sessionId)
        return fallback
      })
      .finally(() => {
        clearTimeout(timer)
        signal?.removeEventListener('abort', onExternalAbort)
        if (timedOut && !this.closed) this.diagnose('timeout', sessionId)
        this.tasks.delete(task)
      })
    this.tasks.set(task, controller)
    return task
  }

  capture(input) {
    if (this.closed) return Promise.resolve([])
    const extract = shouldExtractConversationMemory(input.user, input.assistant)
    if (!extract && !input.recordTurn) return Promise.resolve([])
    return this.track(input.sessionId, (signal) => this.run(input, signal, extract), {
      fallback: [],
      failureCode: 'capture_failed',
    })
  }

  // 写原文失败不能挡住抽取：原文只是证据，用户明确要记的事更重要。
  async recordTurn({ sessionId, cwd, user, assistant, sourceTimestamp }) {
    try {
      const spaceId = await this.memory.ensureWorkspaceSpace(cwd)
      return (
        this.memory.recordEpisode({
          sessionId,
          cwd,
          spaceId,
          user,
          assistant,
          at: sourceTimestamp,
        })?.id || ''
      )
    } catch {
      this.diagnose('episode_write_failed', sessionId)
      return ''
    }
  }

  async run(input, signal, extract) {
    const { sessionId, cwd, model, user, assistant, sourceTimestamp = '' } = input
    await awaitMemoryOperation(this.waitForInitialization(), signal)
    if (signal.aborted) return []
    const episodeId = input.recordTurn ? await this.recordTurn(input) : ''
    if (signal.aborted || !extract) return []
    const result = await extractConversationMemories({
      modelRuntime: this.getModelRuntime(),
      model,
      user,
      assistant,
      signal,
    })
    if (signal.aborted) return []
    if (result.usage) {
      try {
        await this.recordUsage(
          localDayKey(result.timestamp || Date.now()),
          `memory:${sessionId}:${sourceTimestamp || result.timestamp || Date.now()}`,
          result.usage,
        )
      } catch {
        // 统计落盘失败不应再丢掉已经提取的用户记忆。
        this.diagnose('usage_write_failed', sessionId)
      }
    }
    if (signal.aborted) return []
    if (result.errorCode) {
      if (result.errorCode !== 'aborted') this.diagnose(result.errorCode, sessionId)
      // 这一轮没标记已抽取，后台批量还会再试一次。
      return []
    }
    if (episodeId) this.memory.recordEpisode({ id: episodeId, captured: true })
    if (!result.memories.length) return []
    const projectSpaceId = await this.memory.ensureWorkspaceSpace(cwd)
    if (signal.aborted) return []
    return result.memories.map((item, index) =>
      this.memory.remember({
        ...item,
        spaceId: item.scope === 'global' ? 'global' : projectSpaceId,
        cwd,
        sessionId,
        episodeId,
        sourceId: `${sessionId}:${sourceTimestamp || result.timestamp || Date.now()}:${index}`,
        sourceTimestamp: sourceTimestamp || new Date(result.timestamp || Date.now()).toISOString(),
        sourceType: 'conversation',
      }),
    )
  }

  // 抽取一批。返回 { processed, stop }：stop 表示这轮后台任务别再继续（模型暂时不可用）。
  extractBatch(batch, signal) {
    const { sessionId, episodes } = batch
    if (this.closed || this.flushing.has(sessionId))
      return Promise.resolve({ processed: 0, stop: false })
    this.flushing.add(sessionId)
    return this.track(
      sessionId,
      async (taskSignal) => {
        const result = await extractBatchMemories({
          modelClient: this.modelClient,
          turns: episodes,
          signal: taskSignal,
        })
        if (taskSignal.aborted) return { processed: 0, stop: true }
        if (RETRYABLE_BATCH_ERRORS.has(result.errorCode)) {
          this.diagnose(result.errorCode, sessionId)
          return { processed: 0, stop: true }
        }
        if (result.errorCode) this.diagnose(result.errorCode, sessionId)
        for (const [index, item] of result.memories.entries()) {
          const { projectSpaceId, ...memory } = item
          try {
            this.memory.remember({
              ...memory,
              spaceId: item.scope === 'global' ? 'global' : projectSpaceId,
              sessionId,
              sourceId: `${sessionId}:${item.episodeId}:${index}`,
              sourceType: 'conversation',
            })
          } catch {
            // 一条写不进去（比如空间刚被删）不影响其余条目。
            this.diagnose('remember_failed', sessionId)
          }
        }
        // 没有值得记的也要往前走，否则这段对话会被反复送去抽取。
        this.memory.saveSessionCursor(sessionId, episodes.at(-1).at)
        return { processed: episodes.length, stop: false }
      },
      { fallback: { processed: 0, stop: true }, failureCode: 'flush_failed', signal },
    ).finally(() => this.flushing.delete(sessionId))
  }

  // 后台步骤：空闲或累计够多的会话批量抽取。没配辅助模型时不跑，不能悄悄花主模型的钱。
  async flushIdleSessions({ signal } = {}) {
    if (this.closed || !(await this.modelClient?.available({ allowMain: false })))
      return { processed: 0 }
    let processed = 0
    for (const batch of this.memory.pendingEpisodeBatches()) {
      if (signal?.aborted || this.closed) break
      const outcome = await this.extractBatch(batch, signal)
      processed += outcome.processed
      if (outcome.stop) break
    }
    return { processed }
  }

  // 压缩前把这个会话没抽过的原文过一遍。调用方不等它；任何失败都只留诊断码，不影响压缩。
  async flushSessionBeforeCompaction(sessionId) {
    try {
      if (this.closed || !(await this.modelClient?.available({ allowMain: false })))
        return { processed: 0 }
      const [batch] = this.memory.pendingEpisodeBatches({ sessionId, force: true, limit: 1 })
      if (!batch) return { processed: 0 }
      const { processed } = await this.extractBatch(batch)
      return { processed }
    } catch {
      this.diagnose('flush_failed', sessionId)
      return { processed: 0 }
    }
  }

  async dispose() {
    this.closed = true
    for (const controller of this.tasks.values()) controller.abort()
    await Promise.allSettled(this.tasks.keys())
  }
}
