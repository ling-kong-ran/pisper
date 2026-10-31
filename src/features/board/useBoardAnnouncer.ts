// 全部代理（与手机看板）的读屏播报：代理换了状态分组时经 aria-live polite 播一句“{标题} 进入 {分组}”，
// 1.2 秒内的多条合并成一句，避免一次刷新刷屏；首次渲染时已在场的代理不播。
// 桌面上新进入“需要你”的播报由常驻侧栏的 SidebarCommands 负责（侧栏收成窄轨时也播），这里跳过，
// 免得同一件事读两遍；手机外壳没有侧栏，照常播。泳道时期的 FLIP 位移动效已随泳道退役。
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import type { BoardLane } from '@shared/session-board-status.mjs'
import type { BoardItem } from './board-model'

const ANNOUNCE_WINDOW_MS = 1_200

export function useBoardAnnouncer({
  items,
  laneName,
  template,
  separator,
  skipAttention,
}: {
  items: readonly Pick<BoardItem, 'id' | 'title' | 'lane'>[]
  laneName: Record<BoardLane, string>
  template: (title: string, lane: string) => string
  separator: string
  skipAttention: boolean
}) {
  const lanes = useRef<Map<string, BoardLane> | null>(null)
  const queue = useRef<string[]>([])
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const [announcement, setAnnouncement] = useState('')

  const announce = useCallback(
    (message: string) => {
      queue.current.push(message)
      if (timer.current) return
      timer.current = setTimeout(() => {
        timer.current = null
        setAnnouncement(queue.current.splice(0).join(separator))
      }, ANNOUNCE_WINDOW_MS)
    },
    [separator],
  )

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current)
    },
    [],
  )

  useLayoutEffect(() => {
    const previous = lanes.current
    lanes.current = new Map(items.map((item) => [item.id, item.lane]))
    if (!previous) return
    for (const item of items) {
      const before = previous.get(item.id)
      if (before === undefined || before === item.lane) continue
      if (skipAttention && item.lane === 'attention') continue
      announce(template(item.title, laneName[item.lane]))
    }
  }, [announce, items, laneName, skipAttention, template])

  return announcement
}
