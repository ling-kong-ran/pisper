type ScrollFollowOptions = {
  threshold?: number
  onUnreadChange: (unread: boolean) => void
  // 读者离开了最新消息（不在跟随、且离底部超过阈值）：用来显示“回到最新”。跟随中永远是 false，流式时不闪。
  onAwayChange?: (away: boolean) => void
  requestFrame?: typeof requestAnimationFrame
  cancelFrame?: typeof cancelAnimationFrame
  reducedMotion?: () => boolean
}

// 打开（或切回）会话后的这段时间里，虚拟行陆续量出真实高度、代码块和 Markdown 渐次排好，内容可能一下长出几屏。
// 这段时间里贴底直接落到底部，不做平滑跟随：否则读者会看着转录从中间一路滚下来。
const SETTLE_MS = 1200

// 动画与用户意图共用一个控制器，避免内容更新和虚拟行重测分别发起滚动。
export function createScrollFollowController(
  node: HTMLDivElement,
  {
    threshold = 64,
    onUnreadChange,
    onAwayChange,
    requestFrame = (callback) => requestAnimationFrame(callback),
    cancelFrame = (frame) => cancelAnimationFrame(frame),
    reducedMotion,
  }: ScrollFollowOptions,
) {
  const document = node.ownerDocument
  const motionQuery = document.defaultView?.matchMedia?.('(prefers-reduced-motion: reduce)')
  const prefersReducedMotion =
    reducedMotion ||
    (() => {
      const motion = document.documentElement.dataset.motion
      return motion === 'reduced' || (motion !== 'full' && Boolean(motionQuery?.matches))
    })
  let pinned = true
  let unread = false
  let away = false
  let disposed = false
  let frame: number | null = null
  let releaseFrame: number | null = null
  let lastTime: number | null = null
  let lastTop = node.scrollTop
  let programmatic = false
  let pointerDown = false
  let touchPoint: { x: number; y: number } | null = null
  // 刚打开时的安顿期截止时刻（帧时间戳）；null = 不在安顿期。
  let settleUntil: number | null = null
  let settleFrame: number | null = null

  const bottom = () => Math.max(0, node.scrollHeight - node.clientHeight)
  const interacting = () => pointerDown || touchPoint !== null
  // 程序滚动期间 scroll 事件不参与判断；这段时间里有人把位置往上改了（恢复阅读位置、跳到某条消息，
  // 事件可能还没派发或被当成程序滚动忽略），不是本控制器写的：要放弃贴底，否则下一帧会把它拉回底部。
  // 虚拟行重测也会在这段时间里把位置往上校正，但幅度小；只把明显的跳动（超过半屏、且离底部超过阈值）当成外部改动。
  const movedUpExternally = () =>
    lastTop - node.scrollTop > Math.max(threshold, node.clientHeight / 2) &&
    bottom() - node.scrollTop > threshold
  const setUnread = (value: boolean) => {
    if (unread === value) return
    unread = value
    onUnreadChange(value)
  }
  const syncAway = () => {
    const next = !disposed && !pinned && bottom() - node.scrollTop > threshold
    if (away === next) return
    away = next
    onAwayChange?.(next)
  }
  const clearRelease = () => {
    if (releaseFrame !== null) cancelFrame(releaseFrame)
    releaseFrame = null
  }
  const stopAnimation = () => {
    if (frame !== null) cancelFrame(frame)
    frame = null
    lastTime = null
    clearRelease()
    programmatic = false
    lastTop = node.scrollTop
  }
  const markProgrammatic = () => {
    clearRelease()
    programmatic = true
  }
  const releaseProgrammatic = () => {
    // 虚拟行的异步重测可能继续调整位置，覆盖两帧，不能把这些事件当成上翻。
    clearRelease()
    releaseFrame = requestFrame(() => {
      releaseFrame = requestFrame(() => {
        releaseFrame = null
        if (movedUpExternally()) {
          pauseFollowing()
          return
        }
        // 抵达后仍可能有 Markdown 或虚拟行布局收尾；目标稳定后才交还事件判断。
        if (pinned && !interacting() && Math.abs(bottom() - node.scrollTop) > 1) {
          maintainBottom()
          return
        }
        programmatic = false
        lastTop = node.scrollTop
      })
    })
  }
  const writeTop = (top: number) => {
    const before = node.scrollTop
    node.scrollTo({ top, behavior: 'instant' })
    lastTop = node.scrollTop
    // 位置没变就不补发：浏览器这时也不会派发 scroll 事件。补发会让不满一屏的页面一打开就被当成“滚到了顶”，
    // 自动去翻更早的历史，并因此停掉跟随。
    if (lastTop === before) return
    // 浏览器下一帧才派发 scroll 事件。转录的虚拟器只从 scroll 事件里更新它记的位置，在那之前按旧位置做行高校正，
    // 会把这里刚写的位置改回去（切回长会话时从底部跳到中间，跟随也因此被当成上翻而停掉）。
    // 写完立刻补发一次，让其它 scroll 监听者同步拿到新位置；稍后的真实事件位置相同，不会重复处理。
    node.dispatchEvent(new Event('scroll'))
  }
  const step = (timestamp: number) => {
    frame = null
    if (disposed || !pinned || interacting()) return
    if (movedUpExternally()) {
      pauseFollowing()
      return
    }
    const target = bottom()
    const gap = target - node.scrollTop
    const elapsed = lastTime === null ? 1000 / 60 : Math.min(64, timestamp - lastTime)
    lastTime = timestamp
    // 按帧间隔校正 0.15 的逼近比例，让 60Hz 与高刷屏保持相同节奏。
    const alpha = 1 - Math.pow(0.85, Math.max(0, elapsed) / (1000 / 60))
    const settling = settleUntil !== null && timestamp < settleUntil
    if (prefersReducedMotion() || settling || Math.abs(gap) <= 1) {
      if (gap !== 0) writeTop(target)
      lastTime = null
      releaseProgrammatic()
      return
    }
    // 部分 WebView 把滚动位置取整；至少移动一个像素才能收敛并停止调度。
    writeTop(
      node.scrollTop + Math.sign(gap) * Math.min(Math.abs(gap), Math.max(1, Math.abs(gap) * alpha)),
    )
    frame = requestFrame(step)
  }
  const maintainBottom = () => {
    if (disposed || !pinned || interacting()) return
    markProgrammatic()
    if (frame === null) frame = requestFrame(step)
  }
  const endSettle = () => {
    if (settleFrame !== null) cancelFrame(settleFrame)
    settleFrame = null
    settleUntil = null
  }
  const pauseFollowing = () => {
    if (disposed) return
    endSettle()
    pinned = false
    stopAnimation()
    syncAway()
  }
  const resumeFollowing = () => {
    pinned = true
    setUnread(false)
    syncAway()
    maintainBottom()
  }
  const onScroll = () => {
    if (disposed) return
    // 程序滚动期间不更新 lastTop：本控制器的写入已经记过，留着它才能认出外部的跳动。
    if (programmatic) {
      if (movedUpExternally()) pauseFollowing()
      return
    }
    const delta = node.scrollTop - lastTop
    lastTop = node.scrollTop
    // 小幅上翻也要脱离，不能因为仍在贴底阈值内而立即抢回滚动权。
    // 但视口变高（如输入区收起）时浏览器会把 scrollTop 向上夹回底部，这不是用户意图；
    // 真实上翻已由滚轮、键盘和触摸各自处理，这里只在离开底部时才视为上翻。
    if (delta < -0.5 && bottom() - node.scrollTop > 1) pauseFollowing()
    else if (delta > 0.5 && bottom() - node.scrollTop <= threshold) resumeFollowing()
    else syncAway()
  }
  const onWheel = (event: WheelEvent) => {
    if (event.ctrlKey || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return
    if (event.deltaY < 0) pauseFollowing()
    else if (event.deltaY > 0 && bottom() - node.scrollTop <= 1) resumeFollowing()
  }
  const onTouchStart = (event: TouchEvent) => {
    const touch = event.touches[0]
    if (!touch) return
    touchPoint = { x: touch.clientX, y: touch.clientY }
    stopAnimation()
  }
  const onTouchMove = (event: TouchEvent) => {
    const touch = event.touches[0]
    if (!touch || !touchPoint) return
    const dx = touch.clientX - touchPoint.x
    const dy = touch.clientY - touchPoint.y
    if (Math.abs(dy) < 2) return
    if (dy > 0 && dy > Math.abs(dx)) pauseFollowing()
    touchPoint = { x: touch.clientX, y: touch.clientY }
  }
  const onTouchEnd = () => {
    touchPoint = null
    maintainBottom()
  }
  const onPointerDown = (event: PointerEvent) => {
    if (event.pointerType === 'touch' || event.button > 1) return
    pointerDown = true
    stopAnimation()
  }
  const onPointerUp = () => {
    if (!pointerDown) return
    pointerDown = false
    maintainBottom()
  }
  const onKeyDown = (event: KeyboardEvent) => {
    const target = event.target as HTMLElement | null
    if (event.defaultPrevented || target?.closest?.('input, textarea, select, [contenteditable]'))
      return
    if (
      event.key === 'ArrowUp' ||
      event.key === 'PageUp' ||
      event.key === 'Home' ||
      (event.key === ' ' && event.shiftKey)
    )
      pauseFollowing()
  }

  const scrollToBottom = (behavior: ScrollBehavior = 'auto') => {
    if (disposed) return
    stopAnimation()
    pinned = true
    setUnread(false)
    syncAway()
    if (behavior === 'smooth') maintainBottom()
    else {
      markProgrammatic()
      writeTop(bottom())
      releaseProgrammatic()
    }
  }

  node.addEventListener('scroll', onScroll, { passive: true })
  node.addEventListener('wheel', onWheel, { passive: true })
  node.addEventListener('touchstart', onTouchStart, { passive: true })
  node.addEventListener('touchmove', onTouchMove, { passive: true })
  node.addEventListener('touchend', onTouchEnd, { passive: true })
  node.addEventListener('touchcancel', onTouchEnd, { passive: true })
  node.addEventListener('pointerdown', onPointerDown, { passive: true })
  node.addEventListener('keydown', onKeyDown)
  document.addEventListener('pointerup', onPointerUp, { passive: true })
  document.addEventListener('pointercancel', onPointerUp, { passive: true })
  motionQuery?.addEventListener?.('change', maintainBottom)
  const observer =
    typeof ResizeObserver === 'undefined'
      ? null
      : new ResizeObserver(() => {
          maintainBottom()
          syncAway()
        })
  observer?.observe(node)

  return {
    maintainBottom,
    pauseFollowing,
    // 正贴着最新消息跟随：位置由本控制器定，别的写入者（虚拟行重测）不必再挪。
    // 读者按着鼠标或手指时本控制器不动位置（见 maintainBottom），这段时间不算在跟随：
    // 虚拟行的校正得照常落到容器上，不然虚拟器记的位置和真实位置对不上。
    isFollowing: () => !disposed && pinned && !interacting(),
    restorePosition(top: number) {
      if (disposed || !Number.isFinite(top)) return
      pauseFollowing()
      setUnread(false)
      writeTop(Math.max(0, top))
      syncAway()
    },
    contentChanged() {
      if (disposed) return
      if (pinned) maintainBottom()
      else {
        setUnread(true)
        syncAway()
      }
    },
    // 打开或切回会话：直接落到最新消息，之后一小段时间里内容长高也直接贴底（见 SETTLE_MS）。
    settleAtBottom() {
      if (disposed) return
      scrollToBottom('auto')
      endSettle()
      settleFrame = requestFrame((timestamp) => {
        settleFrame = null
        if (pinned) settleUntil = timestamp + SETTLE_MS
      })
    },
    scrollToBottom,
    dispose() {
      disposed = true
      endSettle()
      stopAnimation()
      observer?.disconnect()
      node.removeEventListener('scroll', onScroll)
      node.removeEventListener('wheel', onWheel)
      node.removeEventListener('touchstart', onTouchStart)
      node.removeEventListener('touchmove', onTouchMove)
      node.removeEventListener('touchend', onTouchEnd)
      node.removeEventListener('touchcancel', onTouchEnd)
      node.removeEventListener('pointerdown', onPointerDown)
      node.removeEventListener('keydown', onKeyDown)
      document.removeEventListener('pointerup', onPointerUp)
      document.removeEventListener('pointercancel', onPointerUp)
      motionQuery?.removeEventListener?.('change', maintainBottom)
    },
  }
}
