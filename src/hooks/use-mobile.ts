// 手机视口使用抽屉式导航；窄桌面窗口仍保留可折叠的图标侧栏。
import * as React from 'react'

const PHONE_BREAKPOINT = 651

// 通用 Sidebar 和浮动组件共用手机视口边界；原生移动 App 由调用方补充判断。
export function useIsMobile() {
  return useIsPhoneViewport()
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
