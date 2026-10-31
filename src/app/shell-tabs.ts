// 一级入口的纯数据模型：入口的去向、“当前页归哪个入口”，以及路由竞态用的去向判断。
// 不依赖 Feature 与 React，桌面侧栏入口、手机抽屉、快捷键和守卫测试共用同一份结论。
// 规格 v2 第 2.4、7.1 节：桌面标题栏的标签与手机底部标签栏都已退役，一级入口在侧栏 / 抽屉里；
// “组件”不是一级入口，组件工作室归“功能”点亮。
// 主页是会话（负责人决定）：“会话”的去向是主页 '/'（上次停留的会话或新任务草稿），
// 不是 PAGE_PATHS.session（'/board/new' 进入即新建会话）。
import { MORE_PAGES } from '@/app/more-pages'
import { HOME_PATH, pageFromPath, SETTINGS_LIST_PATH, type PageId } from '@/app/routes'

// 一级入口：会话（主页）· 全部代理 · 功能 · 设置（侧栏与抽屉的顺序）。
export const SHELL_TABS = Object.freeze(['session', 'board', 'more', 'settings'] as const)
export type ShellTab = (typeof SHELL_TABS)[number]

// 入口对应的页面 id；“会话”另走主页逻辑（见 App.selectShellTab），设置落到 config 的默认分区（模型）。
export const SHELL_TAB_PAGES: Readonly<Record<Exclude<ShellTab, 'session'>, PageId>> =
  Object.freeze({
    board: 'board',
    more: 'more',
    settings: 'config',
  })

// 当前页面点亮哪个一级入口（桌面侧栏与手机抽屉的“全部代理 / 功能 / 设置”共用）。
// settingsOwned 来自 settingsOwnsRoute：只有 /config 下的应用设置分区归“设置”（规格 2.6）；
// 插件、星忆等功能页只有一个家（“功能”），/config 下的远程访问 / 手机服务器 / 桌面宠物同样归“功能”。
// 会话页与主页归“会话”，看板归“看板”（侧栏的“全部代理”），组件工作室与其余功能页归“更多”（侧栏的“功能”）。
export function activeShellTab(page: string, settingsOwned: boolean): ShellTab | null {
  if (settingsOwned) return 'settings'
  if (page === 'session' || page === 'chat') return 'session'
  if (page === 'board') return 'board'
  if (page === 'components' || page === 'more' || page === 'config' || MORE_PAGES.has(page))
    return 'more'
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
  // 手机外壳（不含平板）：设置分区返回分区列表（规格 7.4）。
  phone?: boolean
  // 当前历史条目是从主页落地的会话（HOME_ENTRY_STATE），或就是主页本身。
  homeEntry?: boolean
  // 当前历史条目之前是否还有应用内的条目（路由历史下标 > 0）。
  canGoBack?: boolean
}

// 手机原生返回（含平板）在弹层、底部表单都关掉之后按这里返回上一级（规格 7.1，主页改为会话）：
// - 会话主页是根：主页 '/'、从主页落地的会话（含抽屉里 replace 切换后的会话）返回 null，交还平台；
// - 从看板等页面点进来的会话：返回 -1，走历史回到来处；冷启动直接打开的会话链接没有来处，同样交还平台；
// - 功能的子页（含组件工作室）→ 功能；组件详情 → 组件列表；
// - 看板、功能、设置等一级页 → 主页。设置整页挂着 useEscapeLeave 时（平板 App 的设置外壳），原生返回先被它接住，
//   按原路退回进入设置前的那一页，到不了这里；这一条只在手机设置等没有设置外壳时生效。
// 只给原生返回用；键盘 Esc 不离开页面，唯一例外是设置整页侧栏上写明的“← 返回 Esc”（SettingsSidebar，规格 2.3）。
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
  // 手机设置是列表 → 分区（规格 7.4）：分区页返回分区列表，列表再返回主页。
  if (phone && settingsOwned && /^\/config\/[^/]+/.test(pathname)) return SETTINGS_LIST_PATH
  // 组件工作室不再是一级标签（规格 2.4 / 2.7）：桌面、平板与手机的组件列表都回到功能。
  const tab = activeShellTab(page, settingsOwned)
  if (tab === 'more' && page !== 'more') return '/more'
  return HOME_PATH
}
