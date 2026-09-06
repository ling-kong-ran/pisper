// 流式 UI 调度原语：把高频 SSE 事件合并成低频 React 更新。
// - createStreamingTextScheduler：文本增量合并（约 20fps）；
// - createToolUpdateScheduler：同一 tool id 的多次更新合并成一条 patch；
// - createTypewriterDisplay：正文按约 30fps 平滑展示，积压时限速追赶；
//   文本被重写（redaction）时先对齐完整字符的公共前缀再重排。
// 定时器在页面不可见时挂起，切回前台再恢复，避免后台空转。
type ActivityTimestamp = string | null
type ToolPatch = Record<string, unknown>

// 定时调度器：只在“页面可见且无待触发任务”时启动定时器，
// 页面隐藏时挂起、回前台再补触发，避免后台标签页空转；
// 也支持立即冲刷与取消。
function createTimerScheduler(flush: () => void, intervalMs: number) {
  let timer: ReturnType<typeof setTimeout> | null = null
  let waitingForVisibility = false
  const hasDocument = typeof document !== 'undefined'
  const isVisible = () => !hasDocument || document.visibilityState === 'visible'
  const stopWaitingForVisibility = () => {
    if (!waitingForVisibility) return
    document.removeEventListener('visibilitychange', handleVisibilityChange)
    waitingForVisibility = false
  }
  const startTimer = () => {
    timer = setTimeout(() => {
      timer = null
      flush()
    }, intervalMs)
  }
  const handleVisibilityChange = () => {
    if (!isVisible() || timer != null) return
    stopWaitingForVisibility()
    startTimer()
  }
  const waitForVisibility = () => {
    if (!hasDocument || waitingForVisibility) return
    document.addEventListener('visibilitychange', handleVisibilityChange)
    waitingForVisibility = true
  }
  return {
    schedule() {
      if (timer != null || waitingForVisibility) return
      if (isVisible()) startTimer()
      else waitForVisibility()
    },
    flushNow() {
      if (timer != null) {
        clearTimeout(timer)
        timer = null
      }
      stopWaitingForVisibility()
      flush()
    },
    cancel() {
      if (timer != null) clearTimeout(timer)
      timer = null
      stopWaitingForVisibility()
    },
    get active() {
      return timer != null || waitingForVisibility
    },
  }
}

// 流式文本合并调度器：把高频文本增量合并成 ~20fps 的一次回调，
// 携带最近一次活动时间戳，供渲染层节流更新。
export function createStreamingTextScheduler(
  onFlush: (text: string, activityAt: ActivityTimestamp) => void,
  { intervalMs = 48 }: { intervalMs?: number } = {},
) {
  let pending: string | null = null
  let lastActivityAt: ActivityTimestamp = null
  const timer = createTimerScheduler(() => {
    if (pending == null) return
    const text = pending
    const activityAt = lastActivityAt
    pending = null
    lastActivityAt = null
    onFlush(text, activityAt)
  }, intervalMs)

  return {
    push(text: string, activityAt = new Date().toISOString()) {
      pending = text
      lastActivityAt = activityAt
      timer.schedule()
    },
    flush() {
      timer.flushNow()
    },
    cancel() {
      timer.cancel()
      pending = null
      lastActivityAt = null
    },
  }
}

// 工具事件合并调度器：同一工具 id 的多次 patch 合并成一条，
// 按 interval 批量回调，减少 React 更新次数。
export function createToolUpdateScheduler(
  onFlush: (batch: Map<string, ToolPatch>, activityAt: ActivityTimestamp) => void,
  { intervalMs = 80 }: { intervalMs?: number } = {},
) {
  let pending = new Map<string, ToolPatch>()
  let lastActivityAt: ActivityTimestamp = null
  const timer = createTimerScheduler(() => {
    if (!pending.size) return
    const batch = pending
    const activityAt = lastActivityAt
    pending = new Map()
    lastActivityAt = null
    onFlush(batch, activityAt)
  }, intervalMs)

  return {
    push(id: string, patch: ToolPatch, activityAt = new Date().toISOString()) {
      if (!id) return
      pending.set(id, { ...(pending.get(id) || {}), ...patch })
      lastActivityAt = activityAt
      timer.schedule()
    },
    flush() {
      timer.flushNow()
    },
    cancel() {
      timer.cancel()
      pending = new Map<string, ToolPatch>()
      lastActivityAt = null
    },
  }
}

function isHighSurrogate(code: number) {
  return code >= 0xd800 && code <= 0xdbff
}

// 重写可能只改变代理对的低位，公共前缀不能停在高位代理之后。
function commonPrefixLength(left: string, right: string) {
  const limit = Math.min(left.length, right.length)
  let index = 0
  while (index < limit && left.charCodeAt(index) === right.charCodeAt(index)) index += 1
  return index > 0 && isHighSurrogate(left.charCodeAt(index - 1)) ? index - 1 : index
}

const TYPEWRITER_FRAME_INTERVAL_MS = 1_000 / 30

// 浏览器按绘制帧调度，但正文状态最多约 30fps；显式 flush 仍立即校准终态。
// 速率按 Unicode 码点累计，保留小数额度，避免高刷新率或慢速配置突破上限。
export function createTypewriterDisplay(
  onFrame: (text: string, activityAt: ActivityTimestamp) => void,
  {
    minCharsPerSecond = 36,
    maxCharsPerSecond = 1_200,
    catchUpRemaining = 160,
    snapRemaining,
    requestFrame,
    cancelFrame,
    isCurrent = () => true,
    now = () =>
      typeof performance !== 'undefined' && performance.now ? performance.now() : Date.now(),
  }: {
    minCharsPerSecond?: number
    maxCharsPerSecond?: number
    catchUpRemaining?: number
    snapRemaining?: number
    requestFrame?: typeof requestAnimationFrame
    cancelFrame?: typeof cancelAnimationFrame
    isCurrent?: () => boolean
    now?: () => number
  } = {},
) {
  const useNativeFrames =
    !requestFrame &&
    typeof globalThis.requestAnimationFrame === 'function' &&
    typeof globalThis.cancelAnimationFrame === 'function'
  const scheduleFrame: typeof requestAnimationFrame =
    requestFrame ||
    (useNativeFrames
      ? globalThis.requestAnimationFrame.bind(globalThis)
      : (callback) => setTimeout(() => callback(now()), Math.ceil(TYPEWRITER_FRAME_INTERVAL_MS)))
  const cancelScheduled =
    cancelFrame ||
    (useNativeFrames ? globalThis.cancelAnimationFrame.bind(globalThis) : clearTimeout)
  const page = typeof document !== 'undefined' ? document : null
  const isVisible = () => !page || page.visibilityState === 'visible'
  let target = ''
  let shown = ''
  let activityAt: ActivityTimestamp = null
  let frame: number | null = null
  let lastTs = 0
  let lastOutputTs: number | null = null
  let characterCredit = 0
  let closed = false

  const drainWaiters = new Set<(completed: boolean) => void>()
  const settleDrains = (completed: boolean) => {
    const waiters = [...drainWaiters]
    drainWaiters.clear()
    for (const resolve of waiters) resolve(completed)
  }
  const emit = () => onFrame(shown, activityAt)
  const cancel = () => {
    closed = true
    if (frame != null) cancelScheduled(frame)
    page?.removeEventListener('visibilitychange', handleVisibilityChange)
    frame = null
    characterCredit = 0
    settleDrains(false)
  }
  const flush = () => {
    if (closed) return
    if (!isCurrent()) {
      cancel()
      return
    }
    if (frame != null) cancelScheduled(frame)
    frame = null
    characterCredit = 0
    shown = target
    lastOutputTs = now()
    emit()
    if (shown === target) settleDrains(true)
  }

  const step = (timestamp: number) => {
    frame = null
    if (closed) return
    if (!isCurrent()) {
      cancel()
      return
    }
    if (!isVisible()) {
      if (drainWaiters.size) flush()
      return
    }
    if (target === shown) {
      settleDrains(true)
      return
    }
    const elapsed = timestamp - lastTs
    // 留出浮点时间戳的舍入误差，120Hz 等高刷新率也不能每帧写 React 状态。
    if (
      elapsed + 0.001 < TYPEWRITER_FRAME_INTERVAL_MS ||
      (lastOutputTs != null && timestamp - lastOutputTs + 0.001 < TYPEWRITER_FRAME_INTERVAL_MS)
    ) {
      frame = scheduleFrame(step)
      return
    }
    lastTs = timestamp
    const previous = shown
    if (!target.startsWith(shown)) {
      shown = target.slice(0, commonPrefixLength(shown, target))
      characterCredit = 0
    }

    const remaining = target.length - shown.length
    if (snapRemaining != null && remaining >= snapRemaining) {
      // 只有调用方显式选择此模式时才整块补齐，默认 burst 始终受速率上限约束。
      shown = target
      characterCredit = 0
    } else {
      const cps = Math.max(
        0,
        Math.min(
          maxCharsPerSecond,
          minCharsPerSecond + remaining * (remaining >= catchUpRemaining ? 10 : 4),
        ),
      )
      // 卡顿或前后台切换不能储存无限额度，恢复时每次最多消费 80ms 的预算。
      const budget = characterCredit + cps * Math.min(0.08, Math.max(0, elapsed / 1_000))
      const count = Math.floor(budget + 1e-9)
      characterCredit = Math.max(0, budget - count)
      let end = shown.length
      for (let revealed = 0; revealed < count && end < target.length; revealed += 1) {
        const point = target.codePointAt(end) ?? 0
        // SSE 可能在代理对中间拆分文本，等待下一次增量补全尾部高位代理。
        if (isHighSurrogate(point) && end + 1 === target.length && !drainWaiters.size) break
        end += point > 0xffff ? 2 : 1
      }
      shown = target.slice(0, end)
    }

    if (shown !== previous) {
      lastOutputTs = timestamp
      emit()
    }
    if (shown === target) settleDrains(true)
    const waitingForPair =
      !drainWaiters.size &&
      shown.length + 1 === target.length &&
      isHighSurrogate(target.charCodeAt(shown.length))
    if (!closed && frame == null && shown !== target && !waitingForPair && isVisible()) {
      frame = scheduleFrame(step)
    } else if (frame == null) {
      characterCredit = 0
    }
  }

  const schedule = () => {
    if (closed || frame != null || !isVisible() || target === shown) return
    lastTs = now()
    frame = scheduleFrame(step)
  }
  const handleVisibilityChange = () => {
    if (isVisible()) {
      schedule()
    } else if (drainWaiters.size) {
      // 后台没有可见动画，直接校准并释放发送链，避免排队输入无限等待前台恢复。
      flush()
    } else {
      if (frame != null) cancelScheduled(frame)
      frame = null
      characterCredit = 0
    }
  }
  page?.addEventListener('visibilitychange', handleVisibilityChange)

  return {
    setTarget(text: unknown, nextActivityAt = new Date().toISOString()) {
      if (closed) return
      target = String(text || '')
      activityAt = nextActivityAt
      schedule()
    },
    drain(): Promise<boolean> {
      if (closed || !isCurrent()) {
        cancel()
        return Promise.resolve(false)
      }
      if (shown === target) return Promise.resolve(true)
      return new Promise((resolve) => {
        drainWaiters.add(resolve)
        if (isVisible()) schedule()
        else flush()
      })
    },
    flush,
    cancel,
    getShown: () => shown,
    getTarget: () => target,
  }
}
