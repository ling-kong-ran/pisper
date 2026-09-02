// 移动端断点判定：900px 以下切换为抽屉式布局。
// 断点值须与历史布局保持一致（迁移 shadcn 期间不能改变既有交互）。
import * as React from 'react'

// Pisper's existing navigation switches to an off-canvas layout at 900px.
// Keep this breakpoint aligned with the legacy layout during the shadcn migration.
const MOBILE_BREAKPOINT = 901
const PHONE_BREAKPOINT = 651

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

// 手机布局不能只依赖 Runtime 回显的客户端类型，否则移动浏览器和代理握手失败时
// 会错误退回桌面分栏。原生能力仍应使用 useIsMobileApp 单独判断。
export function useIsPhoneViewport() {
  const [isPhone, setIsPhone] = React.useState(
    () => window.matchMedia(`(max-width: ${PHONE_BREAKPOINT - 1}px)`).matches,
  )

  React.useEffect(() => {
    const media = window.matchMedia(`(max-width: ${PHONE_BREAKPOINT - 1}px)`)
    const update = () => setIsPhone(media.matches)
    update()
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [])

  return isPhone
}

// 触屏判定：(pointer: coarse) 比视口宽度更可靠。
// AppSelect 在触屏上改用原生 select——Radix Select 的触摸交互是
// 「抬手即选中并关闭」，列表里轻滑一下就会误触关闭，原生系统选择器不会。
export function useIsCoarsePointer() {
  const [coarse, setCoarse] = React.useState(() => window.matchMedia('(pointer: coarse)').matches)

  React.useEffect(() => {
    const media = window.matchMedia('(pointer: coarse)')
    const update = () => setCoarse(media.matches)
    update()
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [])

  return coarse
}
