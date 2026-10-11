// 手势与临时翻页变换由布局层持有；业务面板始终挂载，切屏不重启终端或丢失草稿。
import { startTransition, useCallback, useLayoutEffect, useRef } from 'react'
import {
  MOBILE_SHELL_PANES,
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
  const controller = useRef<{ settle: (index: number) => void; index: () => number } | null>(null)
  const requestPane = useCallback((next: MobileShellPane) => {
    // 点击的这一拍就启动合成动画，不等待 App/聊天子树提交新状态。
    controller.current?.settle(mobileShellPaneIndex(next))
    startTransition(() => current.current.onPaneChange(next))
  }, [])
  const stepPane = useCallback(
    (direction: -1 | 1) => {
      const index = controller.current?.index() ?? mobileShellPaneIndex(current.current.pane)
      requestPane(moveMobileShellPane(MOBILE_SHELL_PANES[index], direction))
    },
    [requestPane],
  )

  useLayoutEffect(() => {
    const track = trackRef.current
    if (!phone || !track) return
    const panels = Array.from(track.children) as HTMLElement[]
    const media = window.matchMedia('(prefers-reduced-motion: reduce)')
    const reduced = () =>
      document.documentElement.dataset.motion === 'reduced' ||
      (media.matches && document.documentElement.dataset.motion !== 'full')
    let animations: Animation[] = []
    let directFlip: { from: number; to: number; animation: Animation } | null = null
    let frame = 0
    let position = mobileShellPaneIndex(current.current.pane)
    let baseIndex = position
    let targetIndex = position
    let width = track.parentElement?.clientWidth || 1
    const translation = (offset: number) => `translate3d(${offset * width}px,0,0)`
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
      directFlip = null
    }
    const place = (index: number, tilt: boolean) => {
      position = index
      if (tilt) {
        track.style.transform = translation(baseIndex - index)
        track.style.willChange = 'transform'
      } else {
        baseIndex = index
        track.style.marginLeft = `${-index * 100}%`
        track.style.transform = ''
        track.style.willChange = ''
      }
      panels.forEach((panel, i) => {
        const offset = i - index
        const turning = tilt && !reduced() && Math.abs(offset) <= 1
        panel.style.transform = turning ? transform(offset) : ''
        panel.style.willChange = turning ? 'transform' : ''
        panel.style.transformOrigin = tilt ? (offset < 0 ? 'right center' : 'left center') : ''
      })
    }
    const visiblePosition = () => {
      if (directFlip) {
        // 跨屏时轨道静止，必须从面板动效取进度，否则连点会闪到尚未到达的终点。
        const progress = directFlip.animation.effect?.getComputedTiming().progress ?? 0
        return directFlip.from + (directFlip.to - directFlip.from) * progress
      }
      const matrix = new DOMMatrixReadOnly(getComputedStyle(track).transform)
      return baseIndex - matrix.m41 / width
    }
    const settle = (index: number) => {
      // 受控值提交后不重启动画；连续点击以已请求的目标为准。
      if (
        index === targetIndex &&
        !gesture &&
        (animations.length || Math.abs(position - index) < 0.001)
      )
        return
      const from = animations.length ? visiblePosition() : position
      targetIndex = index
      stop()
      gesture = null
      // 只在起止时定位一次，帧间只改变 transform，不再逐帧排版长消息。
      place(index, false)
      if (reduced() || Math.abs(from - index) < 0.001) return
      const options = { duration: 220, easing: 'cubic-bezier(.2,.8,.2,1)' }
      if (Math.abs(from - index) > 1.9) {
        // 跨两屏直接翻出发页与目标页，不把中间的聊天长列表拉过视口。
        const outgoingIndex = Math.round(from)
        const outgoing = panels[outgoingIndex]
        const incoming = panels[index]
        const direction = index > from ? 1 : -1
        outgoing.style.willChange = 'transform, opacity'
        incoming.style.willChange = 'transform'
        animations = [
          incoming.animate(
            [
              {
                transform: transform(direction),
                transformOrigin: direction > 0 ? 'left center' : 'right center',
              },
              {
                transform: transform(0),
                transformOrigin: direction > 0 ? 'left center' : 'right center',
              },
            ],
            options,
          ),
          outgoing.animate(
            [
              {
                transform: `${translation(index - outgoingIndex)} ${transform(0)}`,
                opacity: 1,
                transformOrigin: direction > 0 ? 'right center' : 'left center',
              },
              {
                transform: `${translation(index - outgoingIndex)} ${transform(-direction)}`,
                opacity: 0,
                transformOrigin: direction > 0 ? 'right center' : 'left center',
              },
            ],
            options,
          ),
        ]
        directFlip = { from, to: index, animation: animations[0] }
      } else {
        track.style.willChange = 'transform'
        animations = [
          track.animate(
            [{ transform: translation(index - from) }, { transform: translation(0) }],
            options,
          ),
        ]
        panels.forEach((panel, i) => {
          if (i < Math.floor(Math.min(from, index)) || i > Math.ceil(Math.max(from, index))) return
          const origin = i <= Math.min(from, index) ? 'right center' : 'left center'
          panel.style.willChange = 'transform'
          animations.push(
            panel.animate(
              [
                { transform: transform(i - from), transformOrigin: origin },
                { transform: transform(i - index), transformOrigin: origin },
              ],
              options,
            ),
          )
        })
      }
      animations[0].onfinish = () => {
        stop()
        place(index, false)
      }
    }
    const cancel = () => {
      if (!gesture) return
      settle(targetIndex)
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
        ? moveMobileShellPane(MOBILE_SHELL_PANES[targetIndex], direction)
        : MOBILE_SHELL_PANES[targetIndex]
      if (direction && next !== MOBILE_SHELL_PANES[targetIndex]) {
        gesture = null
        requestPane(next)
      } else settle(targetIndex)
    }
    const reset = () => {
      stop()
      gesture = null
      width = track.parentElement?.clientWidth || 1
      place(targetIndex, false)
    }
    controller.current = { settle, index: () => targetIndex }
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
      track.style.transform = ''
      track.style.willChange = ''
      panels.forEach((panel) => {
        panel.style.transform = ''
        panel.style.transformOrigin = ''
        panel.style.willChange = ''
      })
      controller.current = null
    }
  }, [phone, requestPane])

  useLayoutEffect(() => {
    controller.current?.settle(mobileShellPaneIndex(pane))
  }, [pane, phone])
  return { trackRef, requestPane, stepPane }
}
