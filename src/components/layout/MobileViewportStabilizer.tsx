import { useEffect, type RefObject } from 'react'

type MobileViewportStabilizerProps = {
  enabled: boolean
  shellRef: RefObject<HTMLDivElement | null>
}

export function MobileViewportStabilizer({ enabled, shellRef }: MobileViewportStabilizerProps) {
  useEffect(() => {
    if (!enabled) return undefined
    const shell = shellRef.current
    if (!shell) return undefined
    const viewport = window.visualViewport
    let frame = 0
    let orientationTimer = 0
    let keyboardTransitionTimer = 0
    let viewportBaseline = Math.max(window.innerHeight, viewport?.height ?? 0)
    const composerSelector = '[data-mobile-composer-input]'

    const clearKeyboardTransitionTimer = () => {
      if (keyboardTransitionTimer) window.clearTimeout(keyboardTransitionTimer)
      keyboardTransitionTimer = 0
    }
    const markKeyboardTransition = (state: 'opening' | 'closing') => {
      clearKeyboardTransitionTimer()
      shell.dataset.mobileKeyboardTransition = state
      if (state === 'opening') {
        // 外接键盘或系统未派发 viewport 事件时，也要及时解除保护，避免 Welcome 永久失去交互。
        keyboardTransitionTimer = window.setTimeout(() => {
          if (!shell.dataset.mobileKeyboard) delete shell.dataset.mobileKeyboardTransition
          keyboardTransitionTimer = 0
        }, 500)
      }
    }
    const markComposerTarget = (target: EventTarget | null) => {
      if (!(target instanceof Element) || !target.closest(composerSelector)) return
      markKeyboardTransition('opening')
    }
    const handleTouch = (event: Event) => markComposerTarget(event.target)
    const handleFocus = (event: FocusEvent) => markComposerTarget(event.target)
    const handleBlur = (event: FocusEvent) => {
      const target = event.target
      if (!(target instanceof Element) || !target.closest(composerSelector)) return
      window.requestAnimationFrame(() => {
        const active = document.activeElement
        if (active instanceof Element && active.closest(composerSelector)) return
        markKeyboardTransition('closing')
        keyboardTransitionTimer = window.setTimeout(() => {
          delete shell.dataset.mobileKeyboardTransition
          keyboardTransitionTimer = 0
        }, 240)
      })
    }
    const handleWelcomeClick = (event: MouseEvent) => {
      if (!shell.dataset.mobileKeyboard && !shell.dataset.mobileKeyboardTransition) return
      const target = event.target
      if (!(target instanceof Element) || !target.closest('.agent-welcome button')) return
      event.preventDefault()
      event.stopPropagation()
    }

    // Android WebView 与 iOS WKWebView 对软键盘的布局视口处理不同，统一以 visualViewport 为准。
    const apply = () => {
      frame = 0
      const height = Math.max(1, Math.round(viewport?.height ?? window.innerHeight))
      const offsetTop = Math.max(0, Math.round(viewport?.offsetTop ?? 0))
      viewportBaseline = Math.max(viewportBaseline, window.innerHeight, height + offsetTop)
      const keyboardInset = Math.max(0, viewportBaseline - height - offsetTop)
      const keyboardOpen = keyboardInset >= viewportBaseline * 0.2
      if (keyboardOpen) {
        // iOS 可能只收缩 visualViewport，Android adjustResize 则会同时收缩布局视口；
        // 两种情况下都把外壳底部放到键盘顶部，但不会再叠加一个额外的 inset。
        shell.style.height = `${height}px`
        shell.style.transform = offsetTop ? `translate3d(0, ${offsetTop}px, 0)` : ''
        shell.dataset.mobileKeyboard = 'open'
        clearKeyboardTransitionTimer()
      } else {
        shell.style.height = ''
        shell.style.transform = ''
        delete shell.dataset.mobileKeyboard
        if (shell.dataset.mobileKeyboardTransition === 'opening') {
          markKeyboardTransition('closing')
        }
      }
    }
    const schedule = () => {
      if (frame) window.cancelAnimationFrame(frame)
      frame = window.requestAnimationFrame(apply)
    }
    const resetAfterOrientationChange = () => {
      window.clearTimeout(orientationTimer)
      orientationTimer = window.setTimeout(() => {
        viewportBaseline = 0
        schedule()
      }, 150)
    }

    apply()
    viewport?.addEventListener('resize', schedule)
    viewport?.addEventListener('scroll', schedule)
    window.addEventListener('resize', schedule)
    window.addEventListener('orientationchange', resetAfterOrientationChange)
    document.addEventListener('touchstart', handleTouch, true)
    document.addEventListener('pointerdown', handleTouch, true)
    document.addEventListener('focusin', handleFocus, true)
    document.addEventListener('focusout', handleBlur, true)
    document.addEventListener('click', handleWelcomeClick, true)
    return () => {
      if (frame) window.cancelAnimationFrame(frame)
      window.clearTimeout(orientationTimer)
      clearKeyboardTransitionTimer()
      viewport?.removeEventListener('resize', schedule)
      viewport?.removeEventListener('scroll', schedule)
      window.removeEventListener('resize', schedule)
      window.removeEventListener('orientationchange', resetAfterOrientationChange)
      document.removeEventListener('touchstart', handleTouch, true)
      document.removeEventListener('pointerdown', handleTouch, true)
      document.removeEventListener('focusin', handleFocus, true)
      document.removeEventListener('focusout', handleBlur, true)
      document.removeEventListener('click', handleWelcomeClick, true)
      delete shell.dataset.mobileKeyboard
      delete shell.dataset.mobileKeyboardTransition
    }
  }, [enabled, shellRef])

  return null
}
