// 收尾屏障：agent_end / message_end 里异步排入的隐藏消息（目标延续、预算总结）必须赶在
// Pi 判断“本次运行是否继续”之前入队，否则消息会停在队列里，直到用户下次发消息才被带出去。
// Pi 在 agent_before_settle 处理器全部完成后才再次检查队列，这里等待登记过的入队操作。
export class SettleBarrier {
  constructor() {
    this.pending = new Map()
  }

  // 登记一个会让会话继续运行的异步操作；结束（成功或失败）后自动移除。
  track(sessionId, work) {
    const promise = Promise.resolve(work).catch(() => {})
    let set = this.pending.get(sessionId)
    if (!set) {
      set = new Set()
      this.pending.set(sessionId, set)
    }
    set.add(promise)
    void promise.finally(() => {
      set.delete(promise)
      if (!set.size && this.pending.get(sessionId) === set) this.pending.delete(sessionId)
    })
    return promise
  }

  // 等待已登记的操作；等待期间新登记的（例如用量结算后排入的预算总结）也一并等完。
  async settle(sessionId) {
    while (this.pending.get(sessionId)?.size) {
      await Promise.all(this.pending.get(sessionId))
    }
  }
}

export function createSettleBarrierExtension(barrier) {
  return (pi) => {
    pi.on('agent_before_settle', async (_event, context) => {
      const sessionId = context.sessionManager?.getSessionId?.()
      if (sessionId) await barrier.settle(sessionId)
    })
  }
}
