// 变化频繁的页面状态（浏览器页签、浏览历史）合并一段时间再写：每次写都会镜像到 Runtime，
// 不能每次跳转、每次滚动都发一个请求。写入内容由调用方在写的那一刻生成，内容没变就不写。
import { pageStateStorage } from '@/lib/page-state-storage'

export function createDebouncedPageStateWriter(key: string, delayMs: number) {
  let timer: ReturnType<typeof setTimeout> | null = null
  let produce: (() => string | null) | null = null
  // 已经在存储里的值；undefined 表示还不知道。
  let stored: string | null | undefined
  let listening = false

  const flush = () => {
    if (timer !== null) clearTimeout(timer)
    timer = null
    const next = produce
    produce = null
    if (!next) return
    let value: string | null
    try {
      value = next()
    } catch {
      return
    }
    if (value === stored) return
    try {
      if (value === null) pageStateStorage.removeItem(key)
      else pageStateStorage.setItem(key, value)
      stored = value
    } catch {
      // localStorage 满了或被禁用：这次不保存，界面照常用内存里的状态。
    }
  }

  return {
    // 读出来的初始值，内容没变时第一次写就能跳过。
    noteStored(value: string | null) {
      stored = value
    },
    schedule(serialize: () => string | null) {
      // Node 测试里直接调用 Store，没有 window：只更新内存。
      if (typeof window === 'undefined') return
      produce = serialize
      if (!listening) {
        listening = true
        // 捕获阶段先于 page-state-storage 的 pagehide 监听，后者才能把这次写入一起用 sendBeacon 发出去。
        window.addEventListener('pagehide', flush, { capture: true })
      }
      if (timer === null) timer = setTimeout(flush, delayMs)
    },
    flush,
  }
}

// 启动时读一个键；没有 window、存储被禁用都当没有。
export function readPageState(key: string): string | null {
  if (typeof window === 'undefined') return null
  try {
    return pageStateStorage.getItem(key)
  } catch {
    return null
  }
}
