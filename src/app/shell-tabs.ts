// 一级导航（标题栏主标签 / 手机底部标签栏）的纯数据模型：标签顺序、去向与“当前页归哪个标签”。
// 不依赖 Feature 与 React，桌面标题栏、手机标签栏、快捷键和守卫测试共用同一份结论。
// 主页是会话（负责人决定）：“会话”标签排第一，去向是主页 '/'（上次停留的会话或新任务草稿），
// 不是 PAGE_PATHS.session（'/board/new' 进入即新建会话）。
import { MORE_PAGES } from '@/app/more-pages'
import { HOME_PATH, pageFromPath, type PageId } from '@/app/routes'

export const SHELL_TABS = Object.freeze([
  'session',
  'board',
  'components',
  'more',
  'settings',
] as const)
export type ShellTab = (typeof SHELL_TABS)[number]

// 手机底部标签栏：会话 · 看板 · [居中新任务] · 更多 · 设置。“组件”挪进“更多”的磁贴（手机专属），
// /components 路由照常可用，从更多进入时点亮“更多”。
export const PHONE_SHELL_TABS = Object.freeze(['session', 'board', 'more', 'settings'] as const)

// 标签对应的页面 id；“会话”标签另走主页逻辑（见 App.selectShellTab），设置标签落到 config 的默认分区（模型）。
export const SHELL_TAB_PAGES: Readonly<Record<Exclude<ShellTab, 'session'>, PageId>> =
  Object.freeze({
    board: 'board',
    components: 'components',
    more: 'more',
    settings: 'config',
  })

// 当前页面点亮哪个一级标签。插件、星忆等页同时挂在“更多”和“设置”下，
// 归属沿用 settingsOwnsPage 的进入来源判定：settingsOwned 为真时只点亮“设置”。
// 会话页与主页点亮“会话”，看板点亮“看板”；手机没有“组件”标签，组件页归“更多”。
export function activeShellTab(
  page: string,
  settingsOwned: boolean,
  phone = false,
): ShellTab | null {
  if (settingsOwned) return 'settings'
  if (page === 'session' || page === 'chat') return 'session'
  if (page === 'board') return 'board'
  if (page === 'components') return phone ? 'more' : 'components'
  if (page === 'more' || MORE_PAGES.has(page)) return 'more'
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
  // 手机外壳（不含平板）：组件页从“更多”进入，列表返回更多。
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
  const tab = activeShellTab(page, settingsOwned, phone)
  if (tab === 'more' && page !== 'more') return '/more'
  return HOME_PATH
}
