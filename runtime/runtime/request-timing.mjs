// 会话请求时序统计：跟踪每轮模型请求的首字耗时（TTFT）与总时长，
// 累加进挂在 sessionMeta 上的持久化对象，随 sessionUsage 上报给前端统计弹窗。

// 创建一份新的时序统计对象（持久化于 sessionMeta[sessionId].timing）。
export function createRequestTiming() {
  return {
    requests: 0, // 模型请求次数（带 usage 的 assistant 消息数）
    firstTokenSamples: 0, // 成功记录到首字时刻的请求次数
    firstTokenTotalMs: 0, // 首字耗时总和
    durationTotalMs: 0, // 请求总时长
    lastFirstTokenMs: null, // 最近一次首字耗时
    lastDurationMs: null, // 最近一次请求时长
  }
}

// 取（或创建）挂在 sessionMeta 上的时序统计：live 与 meta 共享同一对象引用，
// 运行期间逐轮累加、run 结束统一落盘；随会话删除（sessionMeta[id] 清理）一并清除。
export function sessionRequestTiming(sessionMeta, id) {
  return ((sessionMeta[id] ||= {}).timing ??= createRequestTiming())
}

// 一次运行内逐轮请求的计时器：assistant message_start 为请求起点，
// 首个可见输出增量（思考/文本）为首字时刻，message_end 时 settle 结算。
// 缺少 message_start 时退化为用上一轮结束时刻作起点（部分中转不透传该事件）。
export function createRequestTimingTracker() {
  let turnStartedAtMs = 0
  let turnFirstDeltaAtMs = 0
  let lastTurnEndedAtMs = Date.now()
  return {
    onMessageStart(role) {
      if (role !== 'assistant') return
      turnStartedAtMs = Date.now()
      turnFirstDeltaAtMs = 0
    },
    onStreamDelta(type) {
      if (turnFirstDeltaAtMs || (type !== 'text_delta' && type !== 'thinking_delta')) return
      turnFirstDeltaAtMs = Date.now()
      if (!turnStartedAtMs) turnStartedAtMs = lastTurnEndedAtMs
    },
    // 结算当前轮次并累加进持久化统计：未见过首个增量（如请求即失败）时
    // firstTokenMs 为 null、首字样本不计入；统计对象缺失时静默跳过。
    settleInto(timing) {
      const endedAtMs = Date.now()
      const startedAtMs = turnStartedAtMs || lastTurnEndedAtMs
      const firstTokenMs = turnFirstDeltaAtMs ? Math.max(0, turnFirstDeltaAtMs - startedAtMs) : null
      const durationMs = Math.max(0, endedAtMs - startedAtMs)
      turnStartedAtMs = 0
      turnFirstDeltaAtMs = 0
      lastTurnEndedAtMs = endedAtMs
      accumulateRequestTiming(timing, { durationMs, firstTokenMs })
    },
  }
}

// 把一次结算结果累加进持久化统计；统计对象缺失时静默跳过。
export function accumulateRequestTiming(timing, { durationMs, firstTokenMs }) {
  if (!timing) return
  timing.requests += 1
  timing.durationTotalMs += durationMs
  timing.lastDurationMs = durationMs
  if (firstTokenMs != null) {
    timing.firstTokenSamples += 1
    timing.firstTokenTotalMs += firstTokenMs
    timing.lastFirstTokenMs = firstTokenMs
  }
}
