// 泳道迁移动效与读屏播报：卡片换泳道时 FLIP（--d-lane 240ms，cubic-bezier(.19,1,.22,1)），
// 落位后状态边做 2 次不透明度脉冲；本客户端刚派发的卡片从启动台位置飞入“运行中”。
// 动效只用 transform / opacity，减少动效偏好、页面隐藏或卡片离屏时直接跳过，且从不循环。
// 播报走 aria-live polite，并在 1.2 秒内合并多条，避免一次轮询刷屏。
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react'
import type { BoardLane } from '@shared/session-board-status.mjs'
import { motionReduced } from '@/lib/motion'
import type { BoardItem } from './board-model'

// 看板各处沿用从这里导入；判断本身放在 lib，会话页的底部表单退场也用同一份。
export { motionReduced }

const LANE_EASE = 'cubic-bezier(.19,1,.22,1)'
const LANE_DURATION_MS = 240
const PULSE_MS = 600
const COLLAPSE_MS = 180
const ANNOUNCE_WINDOW_MS = 1_200

function motionAllowed() {
  return !motionReduced() && document.visibilityState !== 'hidden'
}

function onScreen(rect: DOMRect) {
  return (
    rect.bottom > 0 &&
    rect.top < window.innerHeight &&
    rect.right > 0 &&
    rect.left < window.innerWidth
  )
}

function cards(root: HTMLElement) {
  return [...root.querySelectorAll<HTMLElement>('[data-board-card]')]
}

// 审批允许后的收起：scaleY + 透明度 180ms。动画结束（或被跳过）后才继续后续动作。
export function collapseCard(element: HTMLElement | null) {
  if (!element || !motionAllowed() || typeof element.animate !== 'function')
    return Promise.resolve()
  const animation = element.animate(
    [
      { transform: 'scaleY(1)', opacity: 1 },
      { transform: 'scaleY(.9)', opacity: 0 },
    ],
    { duration: COLLAPSE_MS, easing: 'ease-in', fill: 'forwards' },
  )
  element.style.transformOrigin = 'top center'
  return animation.finished.then(
    () => undefined,
    () => undefined,
  )
}

type LaneMotionOptions = {
  rootRef: RefObject<HTMLElement | null>
  items: readonly BoardItem[]
  laneName: Record<BoardLane, string>
  announceTemplate: (title: string, lane: string) => string
  // 被允许后先收起的卡片：落到新泳道时淡入，不再从原位置飞过去。
  skipFlipRef?: RefObject<Set<string>>
}

export function useLaneMotion({
  rootRef,
  items,
  laneName,
  announceTemplate,
  skipFlipRef,
}: LaneMotionOptions) {
  const rects = useRef(new Map<string, DOMRect>())
  const lanes = useRef(new Map<string, BoardLane>())
  const primed = useRef(false)
  const [announcement, setAnnouncement] = useState('')
  const queue = useRef<string[]>([])
  const announceTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  const measure = useCallback(() => {
    const root = rootRef.current
    if (!root) return
    const next = new Map<string, DOMRect>()
    for (const card of cards(root))
      next.set(card.dataset.boardCard || '', card.getBoundingClientRect())
    rects.current = next
  }, [rootRef])

  // 滚动后旧坐标失效：在下一帧重新记录，保证 FLIP 的起点是用户眼中卡片所在的位置。
  useEffect(() => {
    const root = rootRef.current
    if (!root) return
    let frame = 0
    const schedule = () => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(measure)
    }
    root.addEventListener('scroll', schedule, { capture: true, passive: true })
    window.addEventListener('resize', schedule)
    return () => {
      cancelAnimationFrame(frame)
      root.removeEventListener('scroll', schedule, { capture: true })
      window.removeEventListener('resize', schedule)
    }
  }, [measure, rootRef])

  const announce = useCallback((message: string) => {
    queue.current.push(message)
    if (announceTimer.current) return
    announceTimer.current = setTimeout(() => {
      announceTimer.current = null
      setAnnouncement(queue.current.splice(0).join('；'))
    }, ANNOUNCE_WINDOW_MS)
  }, [])

  useEffect(
    () => () => {
      if (announceTimer.current) clearTimeout(announceTimer.current)
    },
    [],
  )

  useLayoutEffect(() => {
    const root = rootRef.current
    const previousLanes = lanes.current
    const nextLanes = new Map(items.map((item) => [item.id, item.lane]))
    lanes.current = nextLanes
    if (!root) return
    // 首次渲染只记录位置，不对已经在场的卡片做动画或播报。
    if (!primed.current) {
      primed.current = true
      measure()
      return
    }
    const titles = new Map(items.map((item) => [item.id, item]))
    const animate = motionAllowed()
    const launch = root.querySelector<HTMLElement>('[data-launch-console]')?.getBoundingClientRect()
    for (const card of cards(root)) {
      const id = card.dataset.boardCard || ''
      const item = titles.get(id)
      if (!item) continue
      const before = previousLanes.get(id)
      const moved = before !== undefined && before !== item.lane
      if (moved) announce(announceTemplate(item.title, laneName[item.lane]))
      if (!animate || typeof card.animate !== 'function') continue
      const now = card.getBoundingClientRect()
      if (!onScreen(now)) continue
      const skip = skipFlipRef?.current?.has(id)
      if (skip) skipFlipRef?.current?.delete(id)
      const from =
        moved && !skip
          ? rects.current.get(id)
          : before === undefined && item.localRun
            ? launch
            : null
      if (from && (moved || before === undefined)) {
        const dx = from.left - now.left
        const dy = from.top - now.top
        card.animate(
          [
            { transform: `translate(${dx}px, ${dy}px)`, opacity: before === undefined ? 0.4 : 1 },
            { transform: 'translate(0, 0)', opacity: 1 },
          ],
          { duration: LANE_DURATION_MS, easing: LANE_EASE },
        )
      } else if (moved && skip) {
        card.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 120, easing: 'ease-out' })
      }
      if (moved) {
        // 落位后状态边做两次不透明度脉冲（2 × 600ms），不循环。
        card
          .querySelector<HTMLElement>('[data-card-edge]')
          ?.animate([{ opacity: 1 }, { opacity: 0.25 }, { opacity: 1 }], {
            duration: PULSE_MS,
            iterations: 2,
            delay: LANE_DURATION_MS,
            easing: 'ease-in-out',
          })
      }
    }
    measure()
  }, [announce, announceTemplate, items, laneName, measure, rootRef, skipFlipRef])

  return announcement
}
