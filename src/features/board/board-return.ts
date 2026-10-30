// 从看板打开会话前记下的位置：各泳道滚动位置与来源卡片。回到看板（看板入口、⌘[ / ⇧⌘B、标签、浏览器后退）时
// 页面重新挂载，据此恢复滚动并把焦点还给来源卡片。只保存在内存里，刷新页面后不恢复。
// 打开 / 返回的 FLIP 也经这里交接：origin 是看板上来源卡片的位置（泳道条据此把卡片缩进芯片），
// from 是离开会话页时泳道条里当前芯片的位置（看板据此把卡片从芯片位置展开回去）。
export type BoardReturnRect = { left: number; top: number; width: number; height: number }

export type BoardReturnState = {
  focusId: string
  scroll: Record<string, number>
  origin?: BoardReturnRect & { at: number }
  from?: BoardReturnRect & { at: number }
}

// 交接的位置只在路由切换的这一瞬间有效；超过这个时长（例如中途停在别的页面）就不再播放动效。
export const BOARD_RETURN_RECT_TTL_MS = 1_500

let pending: BoardReturnState | null = null

export function rememberBoardReturn(state: BoardReturnState) {
  pending = state
}

// 只读查看：开发模式的 StrictMode 会重复调用渲染与 effect，读取与清除必须分开。
export function peekBoardReturn() {
  return pending
}

export function clearBoardReturn() {
  pending = null
}

export function boardReturnRect(element: Element | null | undefined): BoardReturnRect | null {
  const rect = element?.getBoundingClientRect()
  if (!rect || rect.width <= 0 || rect.height <= 0) return null
  return { left: rect.left, top: rect.top, width: rect.width, height: rect.height }
}

export function freshReturnRect<T extends { at: number }>(rect: T | undefined, now = Date.now()) {
  return rect && now - rect.at <= BOARD_RETURN_RECT_TTL_MS ? rect : null
}

// 用位移 + 缩放把元素从 from 位置“放回”自己的位置（只动 transform 与 opacity）。
export function flipFrom(element: HTMLElement, from: BoardReturnRect, duration = 200) {
  const to = element.getBoundingClientRect()
  if (!to.width || !to.height || typeof element.animate !== 'function') return
  const dx = from.left - to.left
  const dy = from.top - to.top
  const sx = from.width / to.width
  const sy = from.height / to.height
  element.animate(
    [
      { transformOrigin: 'top left', transform: `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})` },
      { transformOrigin: 'top left', transform: 'none' },
    ],
    { duration, easing: 'cubic-bezier(.19,1,.22,1)' },
  )
}
