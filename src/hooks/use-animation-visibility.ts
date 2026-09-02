// 判定装饰动画「是否应播放」：仅当文档可见且被观察元素处于视口内才返回 true。
// 用于在文档隐藏/元素离屏时暂停 CSS 无限动画、rAF 循环与定时器，降低 WebView 与移动端后台功耗。
// 注意：prefers-reduced-motion 与 data-motion='reduced' 的兜底已由全局样式负责，这里不重复处理。
import { useCallback, useEffect, useState } from 'react'

type AnimationVisibilityOptions = {
  // IntersectionObserver rootMargin：默认略微外扩，避免滚动到边缘时播放状态频繁抖动
  rootMargin?: string
  // 判定为可见所需的最小交叉比例
  threshold?: number
}

export function useAnimationVisibility<T extends Element = HTMLDivElement>(
  options: AnimationVisibilityOptions = {},
) {
  const { rootMargin = '96px 0px', threshold = 0 } = options
  const [element, setElement] = useState<T | null>(null)
  const [documentVisible, setDocumentVisible] = useState(() => !document.hidden)
  // 初始按可见处理：IntersectionObserver 在 observe 后会立即给出真实交叉状态
  const [inView, setInView] = useState(true)

  // 回调 ref：引用稳定，且能感知条件渲染导致的元素替换
  const ref = useCallback((node: T | null) => {
    setElement(node)
  }, [])

  // 文档级可见性：切后台/最小化时立即暂停
  useEffect(() => {
    const sync = () => setDocumentVisible(!document.hidden)
    sync()
    document.addEventListener('visibilitychange', sync)
    return () => document.removeEventListener('visibilitychange', sync)
  }, [])

  // 元素级可见性：滚动离屏时暂停；环境不支持 IntersectionObserver 时按可见处理，避免能力回退
  useEffect(() => {
    if (!element || typeof IntersectionObserver === 'undefined') return
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry) setInView(entry.isIntersecting)
      },
      { rootMargin, threshold },
    )
    observer.observe(element)
    return () => observer.disconnect()
  }, [element, rootMargin, threshold])

  return { ref, playing: documentVisible && inView }
}
