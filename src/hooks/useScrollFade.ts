// 横向滚动行的淡出提示：滚动条隐藏时，被裁到边缘的内容很难看出“还能滑”。按实际溢出方向给出
// 'start' | 'end' | 'both'，调用方写到 data-fade 上，用 mask-image 渐隐对应一端（只改遮罩，不动布局）。
// 尺寸变化（含换语言、芯片文字变化）与滚动都会重新测量；没有溢出时返回 undefined，遮罩随之去掉。
import { useEffect, useState, type RefObject } from 'react'

export type ScrollFade = 'start' | 'end' | 'both' | undefined

// 与 SegmentedTabs 同一套遮罩写法：渐变只用来取透明度，颜色取主题前景 token，避免裸色值。
export const SCROLL_FADE_CLASS =
  'data-[fade=both]:[mask-image:linear-gradient(to_right,transparent,var(--foreground)_1.25rem,var(--foreground)_calc(100%-1.25rem),transparent)] data-[fade=end]:[mask-image:linear-gradient(to_right,var(--foreground)_calc(100%-1.25rem),transparent)] data-[fade=start]:[mask-image:linear-gradient(to_left,var(--foreground)_calc(100%-1.25rem),transparent)]'

export function useScrollFade(ref: RefObject<HTMLElement | null>, key = '') {
  const [fade, setFade] = useState<ScrollFade>()
  useEffect(() => {
    const element = ref.current
    if (!element || typeof ResizeObserver !== 'function') return
    const update = () => {
      const max = element.scrollWidth - element.clientWidth
      // RTL 下 scrollLeft 为负，按绝对值判断离起点的距离。
      const offset = Math.abs(element.scrollLeft)
      const start = max > 1 && offset > 1
      const end = max > 1 && offset < max - 1
      setFade(start && end ? 'both' : start ? 'start' : end ? 'end' : undefined)
    }
    update()
    const observer = new ResizeObserver(update)
    observer.observe(element)
    // 子元素宽度变化（芯片文字换了）不一定改变容器尺寸，一并观察。
    for (const child of Array.from(element.children)) observer.observe(child)
    element.addEventListener('scroll', update, { passive: true })
    return () => {
      observer.disconnect()
      element.removeEventListener('scroll', update)
    }
  }, [ref, key])
  return fade
}
