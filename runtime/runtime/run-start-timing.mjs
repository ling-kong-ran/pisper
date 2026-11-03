// 开跑前计时：从受理一条消息到建立运行状态（live）之间的各阶段耗时。
// PISPER_RUN_TIMING=1 时写到 stderr（只含阶段名与毫秒，不含会话内容），用于定位“发出消息后迟迟不开跑”的瓶颈；
// 未开启时 mark/finish 都是空操作。用异步上下文传递计时器，装配会话运行时等深层步骤不必改函数签名。
import { AsyncLocalStorage } from 'node:async_hooks'

const storage = new AsyncLocalStorage()

export function runStartTimingEnabled() {
  return process.env.PISPER_RUN_TIMING === '1'
}

// 在计时上下文里执行 fn；未开启计时时直接执行。
export function withRunStartTiming(fn) {
  if (!runStartTimingEnabled()) return fn()
  const startedAt = performance.now()
  return storage.run({ startedAt, last: startedAt, stages: [], finished: false }, fn)
}

// 记一个阶段：距上一个阶段的耗时。同名阶段（例如重试）累加。
export function markRunStage(stage) {
  const timer = storage.getStore()
  if (!timer || timer.finished) return
  const now = performance.now()
  timer.stages.push([stage, now - timer.last])
  timer.last = now
}

// 运行状态建立（或准备阶段失败）时结束计时并输出一行。
export function finishRunStartTiming(stage = 'live') {
  const timer = storage.getStore()
  if (!timer || timer.finished) return
  markRunStage(stage)
  timer.finished = true
  const stages = {}
  for (const [name, ms] of timer.stages) stages[name] = Math.round((stages[name] || 0) + ms)
  process.stderr.write(
    `PISPER_RUN_START ${JSON.stringify({
      totalMs: Math.round(timer.last - timer.startedAt),
      stages,
      ...(timer.parallel ? { parallel: timer.parallel } : {}),
    })}\n`,
  )
}

// 并行步骤单独计时：记录 promise 自己的耗时，不推进阶段游标。
export function measureRunStage(stage, promise) {
  const timer = storage.getStore()
  if (!timer || timer.finished) return promise
  const startedAt = performance.now()
  const record = () => {
    timer.parallel ||= {}
    timer.parallel[stage] = Math.round(performance.now() - startedAt)
  }
  return Promise.resolve(promise).finally(record)
}
