import { useLayoutEffect, useRef, useState } from 'react'

// 欢迎装饰需要给标题、输入框和建议留出空间；分屏后以面板高度判断，不能沿用窗口高度。
const WELCOME_MINIMUM_HEIGHT = 560

export function useSessionViewport() {
  const ref = useRef<HTMLDivElement>(null)
  const [compact, setCompact] = useState(false)

  useLayoutEffect(() => {
    const element = ref.current
    if (!element) return undefined
    const update = () => {
      const height = element.clientHeight
      // 非活动标签可能暂时没有尺寸，保留它上次可见时的布局直到再次显示。
      if (height <= 0) return
      element.style.setProperty('--session-viewport-height', `${height}px`)
      setCompact(height < WELCOME_MINIMUM_HEIGHT)
    }
    update()
    if (typeof ResizeObserver === 'undefined') {
      window.addEventListener('resize', update)
      return () => window.removeEventListener('resize', update)
    }
    const observer = new ResizeObserver(update)
    observer.observe(element)
    return () => observer.disconnect()
  }, [])

  return { ref, compact }
}
