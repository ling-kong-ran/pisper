// 手机内容布局与侧栏响应式布局分别判断，避免收起侧栏时误切手机业务呈现。
import * as React from 'react'

const PHONE_BREAKPOINT = 651
// 平板（规格 7.6）：手机 App 在 ≥960×640 的视口里改用桌面布局（顶部 48px 栏代替底部标签栏）。
// 只对手机 App 生效，浏览器窗口在这个尺寸本来就是桌面布局。
export const TABLET_VIEWPORT_QUERY = '(min-width: 960px) and (min-height: 640px)'

// 订阅一条媒体查询；首帧同步读取，避免先按错误布局渲染再跳变。
export function useMediaQuery(query: string) {
  const [matches, setMatches] = React.useState(() => window.matchMedia(query).matches)

  React.useEffect(() => {
    const media = window.matchMedia(query)
    const update = () => setMatches(media.matches)
    update()
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [query])

  return matches
}

export function useIsCompactSidebarViewport() {
  return useMediaQuery('(max-width: 767px)')
}

// 通用 Sidebar 和浮动组件共用手机视口边界；原生移动 App 由调用方补充判断。
export function useIsMobile() {
  return useIsPhoneViewport()
}

// 手机布局不能只依赖 Runtime 回显的客户端类型，否则移动浏览器和代理握手失败时
// 会错误退回桌面分栏。原生能力仍应使用 useIsMobileApp 单独判断。
export function useIsPhoneViewport() {
  return useMediaQuery(`(max-width: ${PHONE_BREAKPOINT - 1}px)`)
}

export function useIsTabletViewport() {
  return useMediaQuery(TABLET_VIEWPORT_QUERY)
}

// 触屏判定：(pointer: coarse) 比视口宽度更可靠。
// AppSelect 在触屏上改用原生 select——Radix Select 的触摸交互是
// 「抬手即选中并关闭」，列表里轻滑一下就会误触关闭，原生系统选择器不会。
export function useIsCoarsePointer() {
  return useMediaQuery('(pointer: coarse)')
}
