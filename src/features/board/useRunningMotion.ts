// 运行中卡片的动效闸门（规格 6.6）：状态图标旋转、沿边扫光与尾巴光标闪烁只在卡片进入可视区时运行，
// 离屏即停；扫光同时最多 6 张，按进入可视区的先后分配，前面的卡片离开后后面的自动补上。
// 减少动效偏好下扫光不挂载；页面隐藏时由全局的 data-page-visibility 规则暂停所有动画。
import { useEffect, useId, useState, useSyncExternalStore, type RefObject } from 'react'
import { motionReduced } from './row-motion'

export const MAX_SWEEPING_CARDS = 6

// 可视的运行中卡片，按进入可视区的顺序；只有前 MAX_SWEEPING_CARDS 个可以扫光。
const visibleRunning: string[] = []
const listeners = new Set<() => void>()

function emit() {
  for (const listener of listeners) listener()
}

function claim(key: string) {
  if (visibleRunning.includes(key)) return
  visibleRunning.push(key)
  emit()
}

function release(key: string) {
  const index = visibleRunning.indexOf(key)
  if (index < 0) return
  visibleRunning.splice(index, 1)
  emit()
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

// 供测试：当前分到扫光的卡片数。
export function sweepingCardCount() {
  return Math.min(visibleRunning.length, MAX_SWEEPING_CARDS)
}

export function useRunningMotion(ref: RefObject<HTMLElement | null>, running: boolean) {
  const key = useId()
  const [visible, setVisible] = useState(false)
  useEffect(() => {
    const node = ref.current
    if (!running || !node) {
      setVisible(false)
      return
    }
    // 没有 IntersectionObserver 的环境（旧 WebView）按可见处理：宁可多转，也不让运行中看起来停住。
    if (typeof IntersectionObserver === 'undefined') {
      setVisible(true)
      return
    }
    const observer = new IntersectionObserver(([entry]) =>
      setVisible(Boolean(entry?.isIntersecting)),
    )
    observer.observe(node)
    return () => observer.disconnect()
  }, [ref, running])
  const active = running && visible
  useEffect(() => {
    if (!active) return
    claim(key)
    return () => release(key)
  }, [active, key])
  const granted = useSyncExternalStore(
    subscribe,
    () => visibleRunning.indexOf(key) >= 0 && visibleRunning.indexOf(key) < MAX_SWEEPING_CARDS,
    () => false,
  )
  return { spin: active, sweep: active && granted && !motionReduced() }
}
