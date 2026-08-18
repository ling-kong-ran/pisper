// 流式 UI 调度原语：把高频 SSE 事件合并成低频 React 更新。
// - createStreamingTextScheduler：文本增量合并（约 20fps）；
// - createToolUpdateScheduler：同一 tool id 的多次更新合并成一条 patch；
// - createTypewriterDisplay：打字机式平滑展示，目标落后太多时加速追赶、
//   剩余极少时直接补齐；文本被重写（redaction）时先对齐公共前缀再重排。
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

/** Coalesce high-frequency streaming text into ~20fps React updates. */
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

/** Merge rapid tool_update events by tool id before hitting React state. */
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

// 两串文本公共前缀长度（按字符码比较），用于打字机重排对齐。
function commonPrefixLength(left: string, right: string) {
  const limit = Math.min(left.length, right.length)
  let index = 0
  while (index < limit && left.charCodeAt(index) === right.charCodeAt(index)) index += 1
  return index
}

/**
 * Smooth typewriter display for streaming text.
 * - Keeps React updates to at most ~30fps
 * - Speeds up dynamically when the target is far ahead
 * - Snaps immediately on flush (done / tool boundary)
 */
export function createTypewriterDisplay(
  onFrame: (text: string, activityAt: ActivityTimestamp) => void,
  {
    minCharsPerSecond = 36,
    maxCharsPerSecond = 1_200,
    catchUpRemaining = 160,
    snapRemaining = 480,
    requestFrame,
    cancelFrame,
    now = () =>
      typeof performance !== 'undefined' && performance.now ? performance.now() : Date.now(),
  }: {
    minCharsPerSecond?: number
    maxCharsPerSecond?: number
    catchUpRemaining?: number
    snapRemaining?: number
    requestFrame?: typeof requestAnimationFrame
    cancelFrame?: typeof cancelAnimationFrame
    now?: () => number
  } = {},
) {
  const scheduleFrame: typeof requestAnimationFrame =
    requestFrame || ((callback) => setTimeout(() => callback(now()), 16))
  const cancelScheduled = cancelFrame || clearTimeout
  const hasDocument = typeof document !== 'undefined'
  const isVisible = () => !hasDocument || document.visibilityState === 'visible'
  let target = ''
  let shown = ''
  let activityAt: ActivityTimestamp = null
  let frame = 0
  let lastTs = 0
  let closed = false

  const emit = () => onFrame(shown, activityAt)

  const revealCount = (remaining: number, dt: number) => {
    if (remaining <= 0) return 0
    // Huge backlog: snap in one paint so the UI never feels laggy.
    if (remaining >= snapRemaining) return remaining
    // Medium backlog: catch up in ~100-200ms.
    if (remaining >= catchUpRemaining) {
      return Math.min(remaining, Math.max(12, Math.ceil(remaining * Math.min(1, dt * 10))))
    }
    // Small backlog: natural typing with mild acceleration.
    const cps = Math.min(maxCharsPerSecond, minCharsPerSecond + remaining * 4)
    return Math.min(remaining, Math.max(1, Math.ceil(cps * dt)))
  }

  const step = (now: number) => {
    frame = 0
    if (closed) return
    const dt = lastTs ? Math.min(0.08, Math.max(0.012, (now - lastTs) / 1000)) : 0.032
    lastTs = now
    const previous = shown

    if (target === shown) return

    // text_patch / redaction may rewrite earlier text; realign to the common prefix first.
    if (!target.startsWith(shown)) {
      const prefix = commonPrefixLength(shown, target)
      shown = target.slice(0, prefix)
    }

    if (target.length < shown.length) {
      shown = target
    } else {
      const remaining = target.length - shown.length
      if (remaining > 0) shown = target.slice(0, shown.length + revealCount(remaining, dt))
    }

    if (shown !== previous) emit()
    if (!closed && shown !== target) frame = scheduleFrame(step)
  }

  const schedule = () => {
    if (closed || frame || !isVisible()) return
    lastTs = 0
    frame = scheduleFrame(step)
  }
  const handleVisibilityChange = () => {
    if (isVisible()) {
      schedule()
    } else if (frame) {
      cancelScheduled(frame)
      frame = 0
      lastTs = 0
    }
  }
  if (hasDocument) document.addEventListener('visibilitychange', handleVisibilityChange)

  return {
    setTarget(text: unknown, nextActivityAt = new Date().toISOString()) {
      if (closed) return
      target = String(text || '')
      activityAt = nextActivityAt
      schedule()
    },
    flush() {
      if (frame) cancelScheduled(frame)
      frame = 0
      lastTs = 0
      shown = target
      emit()
    },
    cancel() {
      closed = true
      if (frame) cancelScheduled(frame)
      if (hasDocument) document.removeEventListener('visibilitychange', handleVisibilityChange)
      frame = 0
      lastTs = 0
    },
    getShown: () => shown,
    getTarget: () => target,
  }
}
