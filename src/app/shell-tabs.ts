// 一级入口的纯数据模型：手机底部标签栏的标签顺序与去向、“当前页归哪个入口”，以及路由竞态用的去向判断。
// 不依赖 Feature 与 React，桌面侧栏入口、手机标签栏、快捷键和守卫测试共用同一份结论。
// 规格 v2 第 2.4 节：桌面标题栏的标签（会话 / 看板 / 组件 / 更多 / 设置）已退役，一级入口在侧栏里；
// 这里只剩手机标签栏（A4 改为抽屉）沿用的四个标签。“组件”不是一级入口，组件工作室归“功能”点亮。
// 主页是会话（负责人决定）：“会话”的去向是主页 '/'（上次停留的会话或新任务草稿），
// 不是 PAGE_PATHS.session（'/board/new' 进入即新建会话）。
import { MORE_PAGES } from '@/app/more-pages'
import { HOME_PATH, pageFromPath, type PageId } from '@/app/routes'

// 手机底部标签栏：会话 · 看板 · [居中新任务] · 更多 · 设置。
export const PHONE_SHELL_TABS = Object.freeze(['session', 'board', 'more', 'settings'] as const)
export type ShellTab = (typeof PHONE_SHELL_TABS)[number]

// 入口对应的页面 id；“会话”另走主页逻辑（见 App.selectShellTab），设置落到 config 的默认分区（模型）。
export const SHELL_TAB_PAGES: Readonly<Record<Exclude<ShellTab, 'session'>, PageId>> =
  Object.freeze({
    board: 'board',
    more: 'more',
    settings: 'config',
  })

// 当前页面点亮哪个一级入口（桌面侧栏的“全部代理 / 功能 / 设置”与手机标签栏共用）。
// 插件、星忆等页同时挂在“功能”和“设置”下，归属沿用 settingsOwnsPage 的进入来源判定：
// settingsOwned 为真时只点亮“设置”。会话页与主页归“会话”，看板归“看板”（侧栏的“全部代理”），
// 组件工作室与其余功能页归“更多”（侧栏的“功能”）。
export function activeShellTab(page: string, settingsOwned: boolean): ShellTab | null {
  if (settingsOwned) return 'settings'
  if (page === 'session' || page === 'chat') return 'session'
  if (page === 'board') return 'board'
  if (page === 'components' || page === 'more' || MORE_PAGES.has(page)) return 'more'
  return null
}

// 数据路由 router.state 的子集。navigate() 同步改它：懒加载路由还在加载时 navigation 带着目标，
// 加载完才写入历史并换掉 location；React 却在 transition 里稍后才提交，渲染出的 location / page 会落后
// （与 SessionShell 的 effectiveStripCurrent 同一个问题）。
export type RouterLocationState = {
  location: { pathname: string; search?: string; state?: unknown }
  navigation: { state: string; location?: { pathname: string } }
}

// 最新请求的去向：有进行中的导航以它的目标为准，否则是路由器已提交的位置（界面可能还没换过去）。
export function requestedPathname({ location, navigation }: RouterLocationState): string {
  return (navigation.state !== 'idle' && navigation.location?.pathname) || location.pathname
}

// “会话”标签与 ⌘1 的去向（null 表示不导航）：
// - 已在或正要去会话页 / 主页：不再压入历史；
// - 离开会话页的导航还在加载、没写入历史：原地 replace（带上当前条目的状态）撤销它，不压一条重复的条目；
// - 其余回到主页（上次停留的会话或新任务草稿）。
export function sessionTabNavigation({
  location,
  navigation,
}: RouterLocationState): { to: string; replace?: boolean; state?: unknown } | null {
  if (pageFromPath(requestedPathname({ location, navigation })) === 'session') return null
  if (navigation.state !== 'idle' && pageFromPath(location.pathname) === 'session')
    return { to: location.pathname + (location.search || ''), replace: true, state: location.state }
  return { to: HOME_PATH }
}

// 组件详情路由：/components/:componentId（手机上列表与详情各占一屏；桌面在双栏里选中同一组件）。
export const COMPONENT_DETAIL_PATTERN = /^\/components\/([^/]+)\/?$/

export type ShellBackContext = {
  // 手机外壳（不含平板）：组件列表返回更多；桌面与平板的组件列表是功能页之一，同样回到更多。
  phone?: boolean
  // 当前历史条目是从主页落地的会话（HOME_ENTRY_STATE），或就是主页本身。
  homeEntry?: boolean
  // 当前历史条目之前是否还有应用内的条目（路由历史下标 > 0）。
  canGoBack?: boolean
}

// 手机原生返回（含平板）在弹层、底部表单都关掉之后按这里返回上一级（规格 7.1，主页改为会话）：
// - 会话主页是根：主页 '/'、从主页落地的会话（含泳道条 replace 切换后的会话）返回 null，交还平台；
// - 从看板等页面点进来的会话：返回 -1，走历史回到来处；冷启动直接打开的会话链接没有来处，同样交还平台；
// - 更多的子页 → 更多；组件详情 → 组件列表；手机上组件列表 → 更多；
// - 看板、组件（桌面 / 平板）、更多、设置等一级页 → 主页。
// 只给原生返回用，键盘 Esc 永远不离开页面。
export function shellBackPath(
  page: string,
  pathname: string,
  settingsOwned: boolean,
  { phone = false, homeEntry = false, canGoBack = false }: ShellBackContext = {},
): string | -1 | null {
  if (page === 'session') {
    if (homeEntry || pathname === HOME_PATH || pathname === '') return null
    return canGoBack ? -1 : null
  }
  if (page === 'components' && COMPONENT_DETAIL_PATTERN.test(pathname)) return '/components'
  // 桌面 / 平板的组件列表原是一级标签，返回主页；手机上它从“更多”进入，返回更多（行为不变）。
  if (page === 'components' && !phone) return HOME_PATH
  const tab = activeShellTab(page, settingsOwned)
  if (tab === 'more' && page !== 'more') return '/more'
  return HOME_PATH
}
