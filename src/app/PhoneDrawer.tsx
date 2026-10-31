// 手机外壳的抽屉（规格 v2 第 7.1 节）：从左侧滑出，宽 min(320, 85vw)，内容与桌面侧栏相同——
// 顶部搜索框（打开 ⌘K 面板）与新任务 ✎，然后全部代理、功能、会话分组（行高 44 / 56，普通批准可一步处理），
// 底部设置与更新提示。只用 ☰ 打开，不支持边缘滑动（避免与 iOS 返回手势冲突）。
// 抽屉是 Radix 对话框：Esc 与 Android 返回键（BackNavigation 先关弹层）都会关上它，焦点回到 ☰。
// 换页（点会话行、入口、历史会话）时由应用壳按路由变化关上抽屉，这里不逐个拦截点击。
// 本模块只在手机外壳里懒加载，不进入口包。
import type { ReactNode } from 'react'
import { LayoutGrid, List, Search, Settings, SquarePen, type LucideIcon } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { cn } from '@/lib/utils'
import { useSidebarStore } from '@/stores/sidebar-store'
import type { ShellSidebarEntry } from './ShellSidebar'

export type PhoneDrawerProps = {
  active: ShellSidebarEntry | null
  onSearch: () => void
  onNewTask: () => void
  onOpenBoard: () => void
  onOpenFeatures: () => void
  onOpenSettings: () => void
  // 会话分组（看板领域，随看板 store 懒加载）。
  content: ReactNode
  // 全部代理右侧的代理总数（可选）。
  boardCount?: ReactNode
  update?: ReactNode
}

const ROW =
  'flex h-11 w-full min-w-0 items-center gap-3 rounded-[var(--r-md)] px-2.5 text-base text-content transition-colors duration-(--d-fast) focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus active:bg-surface-hover'

export function PhoneDrawer({
  active,
  onSearch,
  onNewTask,
  onOpenBoard,
  onOpenFeatures,
  onOpenSettings,
  content,
  boardCount,
  update,
}: PhoneDrawerProps) {
  const { t } = useI18n()
  const open = useSidebarStore((state) => state.drawerOpen)
  const setOpen = useSidebarStore((state) => state.setDrawerOpen)
  // 入口先关抽屉再执行：打开面板、派发表单或换页时抽屉不留在下面。
  const run = (action: () => void) => () => {
    setOpen(false)
    action()
  }
  const entry = (
    id: ShellSidebarEntry,
    Icon: LucideIcon,
    label: string,
    onClick: () => void,
    trailing?: ReactNode,
  ) => (
    <button
      type="button"
      data-drawer-entry={id}
      aria-current={active === id ? 'page' : undefined}
      className={cn(ROW, active === id && 'bg-surface-active')}
      onClick={run(onClick)}
    >
      <Icon
        className="size-[18px] flex-none text-content-secondary"
        strokeWidth={1.75}
        aria-hidden="true"
      />
      <span className="min-w-0 flex-1 truncate text-left">{label}</span>
      {trailing}
    </button>
  )
  return (
    <Sheet open={open} onOpenChange={setOpen}>
      <SheetContent
        side="left"
        showCloseButton={false}
        data-phone-drawer
        aria-describedby={undefined}
        className="w-[min(320px,85vw)] gap-0 border-stroke-soft bg-sidebar p-0 pt-[var(--pisper-safe-area-top)] pb-[var(--pisper-safe-area-bottom)] pl-[var(--pisper-safe-area-left)] data-[side=left]:w-[min(320px,85vw)] data-[side=left]:max-w-none"
      >
        <SheetTitle className="sr-only">{t('navigation:phone.drawer')}</SheetTitle>
        <SheetDescription className="sr-only">{t('navigation:phone.drawerHint')}</SheetDescription>
        <div className="flex flex-none items-center gap-2 px-3 pt-3 pb-2">
          <button
            type="button"
            data-drawer-search
            className="flex h-11 min-w-0 flex-1 items-center gap-2 rounded-[10px] bg-surface-muted px-3 text-left text-base text-content-muted focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus"
            onClick={run(onSearch)}
          >
            <Search className="size-[18px] flex-none" strokeWidth={1.75} aria-hidden="true" />
            <span className="truncate">{t('navigation:sidebar.search')}</span>
          </button>
          <button
            type="button"
            data-drawer-new-task
            aria-label={t('navigation:shell.newTask')}
            className="grid size-11 flex-none place-items-center rounded-[var(--r-md)] text-content-secondary focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus active:bg-surface-hover"
            onClick={run(onNewTask)}
          >
            <SquarePen className="size-5" strokeWidth={1.75} aria-hidden="true" />
          </button>
        </div>
        <nav aria-label={t('navigation:shell.primaryNavigation')} className="flex-none px-2 pb-2">
          {entry(
            'board',
            List,
            t('navigation:sidebar.allAgents'),
            onOpenBoard,
            boardCount ? (
              <span className="flex-none text-sm text-content-muted tabular-nums">
                {boardCount}
              </span>
            ) : undefined,
          )}
          {entry('features', LayoutGrid, t('navigation:sidebar.features'), onOpenFeatures)}
        </nav>
        <div
          data-drawer-sessions
          className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-2 pt-1"
        >
          {content}
        </div>
        <div className="flex flex-none items-center gap-1 border-t border-stroke-soft px-2 py-2">
          <div className="min-w-0 flex-1">
            {entry('settings', Settings, t('navigation:navigation.settings'), onOpenSettings)}
          </div>
          {update}
        </div>
      </SheetContent>
    </Sheet>
  )
}
