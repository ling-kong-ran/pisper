// 捕获任务拥有后台请求与取消生命周期；模型提取、持久化由窄依赖完成。
import { createHash } from 'node:crypto'
import { awaitMemoryOperation } from '../services/memory/abortable-memory-operation.mjs'
import {
  extractConversationMemories,
  shouldExtractConversationMemory,
} from '../services/memory/conversation-memory.mjs'

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
    reportFailure = (diagnostic) => console.warn('[memory] capture failed', diagnostic),
    timeoutMs = 120_000,
  }) {
    this.getModelRuntime = getModelRuntime
    this.waitForInitialization = waitForInitialization
    this.memory = memory
    this.recordUsage = recordUsage
    this.reportFailure = reportFailure
    this.timeoutMs = timeoutMs
    this.tasks = new Map()
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

  capture(input) {
    if (this.closed || !shouldExtractConversationMemory(input.user, input.assistant))
      return Promise.resolve([])
    const controller = new AbortController()
    let timedOut = false
    const timer = setTimeout(() => {
      timedOut = true
      controller.abort()
    }, this.timeoutMs)
    const task = this.run(input, controller.signal)
      .catch(() => {
        if (!controller.signal.aborted) this.diagnose('capture_failed', input.sessionId)
        return []
      })
      .finally(() => {
        clearTimeout(timer)
        if (timedOut && !this.closed) this.diagnose('timeout', input.sessionId)
        this.tasks.delete(task)
      })
    this.tasks.set(task, controller)
    return task
  }

  async run({ sessionId, cwd, model, user, assistant, sourceTimestamp = '' }, signal) {
    await awaitMemoryOperation(this.waitForInitialization(), signal)
    if (signal.aborted) return []
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
      return []
    }
    if (!result.memories.length) return []
    const projectSpaceId = await this.memory.ensureWorkspaceSpace(cwd)
    if (signal.aborted) return []
    return result.memories.map((item, index) =>
      this.memory.propose({
        ...item,
        spaceId: item.scope === 'global' ? 'global' : projectSpaceId,
        cwd,
        sessionId,
        sourceId: `${sessionId}:${sourceTimestamp || result.timestamp || Date.now()}:${index}`,
        sourceTimestamp: sourceTimestamp || new Date(result.timestamp || Date.now()).toISOString(),
        sourceType: 'conversation',
      }),
    )
  }

  async dispose() {
    this.closed = true
    for (const controller of this.tasks.values()) controller.abort()
    await Promise.allSettled(this.tasks.keys())
  }
}
