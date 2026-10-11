// 手势与临时翻页变换由布局层持有；业务面板始终挂载，切屏不重启终端或丢失草稿。
import { useLayoutEffect, useRef } from 'react'
import {
  mobileShellPaneIndex,
  moveMobileShellPane,
  shouldIgnoreMobileSwipe,
  swipeDeltaToDirection,
  type MobileShellPane,
} from '@/components/layout/mobile-shell-layout'

export function useMobilePaneTransition(
  phone: boolean,
  pane: MobileShellPane,
  onPaneChange: (pane: MobileShellPane) => void,
) {
  const trackRef = useRef<HTMLDivElement>(null)
  const current = useRef({ pane, onPaneChange })
  current.current = { pane, onPaneChange }
  const controller = useRef<{ settle: (index: number) => void } | null>(null)

  useLayoutEffect(() => {
    const track = trackRef.current
    if (!phone || !track) return
    const panels = Array.from(track.children) as HTMLElement[]
    const media = window.matchMedia('(prefers-reduced-motion: reduce)')
    const reduced = () =>
      document.documentElement.dataset.motion === 'reduced' ||
      (media.matches && document.documentElement.dataset.motion !== 'full')
    let animations: Animation[] = []
    let frame = 0
    let position = mobileShellPaneIndex(current.current.pane)
    let gesture: { id: number; x: number; y: number; position: number; locked: boolean } | null =
      null
    const transform = (offset: number) =>
      `perspective(1100px) rotateY(${Math.max(-1, Math.min(1, offset)) * 72}deg)`
    const stop = () => {
      if (frame) cancelAnimationFrame(frame)
      frame = 0
      for (const animation of animations) {
        animation.onfinish = null
        animation.cancel()
      }
      animations = []
    }
    const place = (index: number, tilt: boolean) => {
      position = index
      track.style.marginLeft = `${-index * 100}%`
      panels.forEach((panel, i) => {
        const offset = i - index
        panel.style.transform = tilt && !reduced() ? transform(offset) : ''
        panel.style.transformOrigin = tilt ? (offset < 0 ? 'right center' : 'left center') : ''
      })
    }
    const visiblePosition = () => {
      const width = track.parentElement?.clientWidth || 1
      return -parseFloat(getComputedStyle(track).marginLeft) / width
    }
    const settle = (index: number) => {
      const from = animations.length ? visiblePosition() : position
      stop()
      gesture = null
      // 完成后移除变换，避免给面板内的 fixed 弹窗留下新的定位包含块。
      place(index, false)
      if (reduced() || Math.abs(from - index) < 0.001) return
      const options = { duration: 280, easing: 'cubic-bezier(.22,.8,.25,1)' }
      const slide = track.animate(
        [{ marginLeft: `${-from * 100}%` }, { marginLeft: `${-index * 100}%` }],
        options,
      )
      animations = [
        slide,
        ...panels.map((panel, i) => {
          const origin = i <= Math.min(from, index) ? 'right center' : 'left center'
          return panel.animate(
            [
              { transform: transform(i - from), transformOrigin: origin },
              { transform: transform(i - index), transformOrigin: origin },
            ],
            options,
          )
        }),
      ]
      slide.onfinish = () => {
        stop()
        place(index, false)
      }
    }
    const cancel = () => {
      if (!gesture) return
      settle(mobileShellPaneIndex(current.current.pane))
    }
    const start = (event: TouchEvent) => {
      if (event.touches.length !== 1) {
        cancel()
        return
      }
      if (shouldIgnoreMobileSwipe(event.target) || window.getSelection()?.isCollapsed === false)
        return
      const touch = event.touches[0]
      const from = animations.length ? visiblePosition() : position
      stop()
      place(from, false)
      gesture = {
        id: touch.identifier,
        x: touch.clientX,
        y: touch.clientY,
        position: from,
        locked: false,
      }
    }
    const move = (event: TouchEvent) => {
      if (!gesture) return
      if (event.touches.length !== 1) {
        cancel()
        return
      }
      const touch = event.touches[0]
      if (touch.identifier !== gesture.id) return
      const dx = touch.clientX - gesture.x
      const dy = touch.clientY - gesture.y
      if (!gesture.locked) {
        if (Math.max(Math.abs(dx), Math.abs(dy)) < 10) return
        if (Math.abs(dx) <= Math.abs(dy) * 1.2) {
          cancel()
          return
        }
        gesture.locked = true
      }
      if (!event.cancelable) {
        cancel()
        return
      }
      event.preventDefault()
      const width = track.parentElement?.clientWidth || 1
      const next = Math.max(0, Math.min(2, gesture.position - dx / width))
      if (frame) cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => {
        frame = 0
        place(next, true)
      })
    }
    const end = (event: TouchEvent) => {
      if (!gesture) return
      const touch = Array.from(event.changedTouches).find((item) => item.identifier === gesture?.id)
      if (!touch) return
      const direction = gesture.locked
        ? swipeDeltaToDirection(touch.clientX - gesture.x, touch.clientY - gesture.y)
        : 0
      const next = direction
        ? moveMobileShellPane(current.current.pane, direction)
        : current.current.pane
      if (direction && next !== current.current.pane) {
        gesture = null
        current.current.onPaneChange(next)
      } else settle(mobileShellPaneIndex(current.current.pane))
    }
    const reset = () => {
      stop()
      gesture = null
      place(mobileShellPaneIndex(current.current.pane), false)
    }
    controller.current = { settle }
    place(position, false)
    const observer = new MutationObserver(reset)
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['data-motion'],
    })
    media.addEventListener('change', reset)
    window.addEventListener('resize', reset)
    track.addEventListener('touchstart', start, { passive: true })
    track.addEventListener('touchmove', move, { passive: false })
    track.addEventListener('touchend', end)
    track.addEventListener('touchcancel', cancel)
    return () => {
      stop()
      observer.disconnect()
      media.removeEventListener('change', reset)
      window.removeEventListener('resize', reset)
      track.removeEventListener('touchstart', start)
      track.removeEventListener('touchmove', move)
      track.removeEventListener('touchend', end)
      track.removeEventListener('touchcancel', cancel)
      track.style.marginLeft = ''
      panels.forEach((panel) => {
        panel.style.transform = ''
        panel.style.transformOrigin = ''
      })
      controller.current = null
    }
  }, [phone])

  useLayoutEffect(() => {
    controller.current?.settle(mobileShellPaneIndex(pane))
  }, [pane, phone])
  return trackRef
}
