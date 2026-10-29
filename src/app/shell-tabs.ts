// 一级导航（标题栏主标签 / 手机底部标签栏）的纯数据模型：标签顺序、去向与“当前页归哪个标签”。
// 不依赖 Feature 与 React，桌面标题栏、手机标签栏、快捷键和守卫测试共用同一份结论。
import { MORE_PAGES } from '@/app/more-pages'
import type { PageId } from '@/app/routes'

export const SHELL_TABS = Object.freeze(['board', 'components', 'more', 'settings'] as const)
export type ShellTab = (typeof SHELL_TABS)[number]

// 标签对应的页面 id；设置标签落到 config 的默认分区（模型）。
export const SHELL_TAB_PAGES: Readonly<Record<ShellTab, PageId>> = Object.freeze({
  board: 'board',
  components: 'components',
  more: 'more',
  settings: 'config',
})

// 当前页面点亮哪个一级标签。插件、星忆等页同时挂在“更多”和“设置”下，
// 归属沿用 settingsOwnsPage 的进入来源判定：settingsOwned 为真时只点亮“设置”。
export function activeShellTab(page: string, settingsOwned: boolean): ShellTab | null {
  if (settingsOwned) return 'settings'
  if (page === 'board' || page === 'session' || page === 'chat') return 'board'
  if (page === 'components') return 'components'
  if (page === 'more' || MORE_PAGES.has(page)) return 'more'
  return null
}
