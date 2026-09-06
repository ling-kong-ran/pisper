type ScrollFollowOptions = {
  threshold?: number
  onUnreadChange: (unread: boolean) => void
  requestFrame?: typeof requestAnimationFrame
  cancelFrame?: typeof cancelAnimationFrame
  reducedMotion?: () => boolean
}

// 动画与用户意图共用一个控制器，避免内容更新和虚拟行重测分别发起滚动。
export function createScrollFollowController(
  node: HTMLDivElement,
  {
    threshold = 64,
    onUnreadChange,
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
  let disposed = false
  let frame: number | null = null
  let releaseFrame: number | null = null
  let lastTime: number | null = null
  let lastTop = node.scrollTop
  let programmatic = false
  let pointerDown = false
  let touchPoint: { x: number; y: number } | null = null

  const bottom = () => Math.max(0, node.scrollHeight - node.clientHeight)
  const interacting = () => pointerDown || touchPoint !== null
  const setUnread = (value: boolean) => {
    if (unread === value) return
    unread = value
    onUnreadChange(value)
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
    node.scrollTo({ top, behavior: 'instant' })
    lastTop = node.scrollTop
  }
  const step = (timestamp: number) => {
    frame = null
    if (disposed || !pinned || interacting()) return
    const target = bottom()
    const gap = target - node.scrollTop
    const elapsed = lastTime === null ? 1000 / 60 : Math.min(64, timestamp - lastTime)
    lastTime = timestamp
    // 按帧间隔校正 0.15 的逼近比例，让 60Hz 与高刷屏保持相同节奏。
    const alpha = 1 - Math.pow(0.85, Math.max(0, elapsed) / (1000 / 60))
    if (prefersReducedMotion() || Math.abs(gap) <= 1) {
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
  const pauseFollowing = () => {
    if (disposed) return
    pinned = false
    stopAnimation()
  }
  const resumeFollowing = () => {
    pinned = true
    setUnread(false)
    maintainBottom()
  }
  const onScroll = () => {
    const delta = node.scrollTop - lastTop
    lastTop = node.scrollTop
    if (programmatic || disposed) return
    // 小幅上翻也要脱离，不能因为仍在贴底阈值内而立即抢回滚动权。
    if (delta < -0.5) pauseFollowing()
    else if (delta > 0.5 && bottom() - node.scrollTop <= threshold) resumeFollowing()
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
  const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(maintainBottom)
  observer?.observe(node)

  return {
    maintainBottom,
    pauseFollowing,
    contentChanged() {
      if (disposed) return
      if (pinned) maintainBottom()
      else setUnread(true)
    },
    scrollToBottom(behavior: ScrollBehavior = 'auto') {
      if (disposed) return
      stopAnimation()
      pinned = true
      setUnread(false)
      if (behavior === 'smooth') maintainBottom()
      else {
        markProgrammatic()
        writeTop(bottom())
        releaseProgrammatic()
      }
    },
    dispose() {
      disposed = true
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
