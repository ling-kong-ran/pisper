// 移动端断点判定：900px 以下切换为抽屉式布局。
// 断点值须与历史布局保持一致（迁移 shadcn 期间不能改变既有交互）。
import * as React from 'react'

// Pisper's existing navigation switches to an off-canvas layout at 900px.
// Keep this breakpoint aligned with the legacy layout during the shadcn migration.
const MOBILE_BREAKPOINT = 901

// 移动端判定：宽度 < 900px 视为移动端（断点须与历史布局一致，
// 迁移 shadcn 期间不可改变既有交互边界）。
export function useIsMobile() {
  const [isMobile, setIsMobile] = React.useState<boolean | undefined>(undefined)

  React.useEffect(() => {
    const mql = window.matchMedia(`(max-width: ${MOBILE_BREAKPOINT - 1}px)`)
    const onChange = () => {
      setIsMobile(window.innerWidth < MOBILE_BREAKPOINT)
    }
    mql.addEventListener('change', onChange)
    setIsMobile(window.innerWidth < MOBILE_BREAKPOINT)
    return () => mql.removeEventListener('change', onChange)
  }, [])

  return !!isMobile
}
