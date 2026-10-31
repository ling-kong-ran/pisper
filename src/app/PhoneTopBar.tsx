// 手机外壳顶栏（规格 v2 第 7.1 节，56px）：左 ☰（带需要你计数，点开抽屉），中间页面标题，
// 右边新任务 ✎（打开派发底部表单）；全部代理页在 ✎ 前多一个搜索（打开 ⌘K 面板，规格 7.3）。
// 会话页与主页有自己的顶栏（标题 ▾ 与改动芯片），不用这一条。底部标签栏已退役。
// 标题只是看得见的页名，不是标题元素：每页自己的 h1 在手机上视觉隐藏但仍给读屏，避免一页两个 h1。
// 本模块读看板数据层（☰ 的计数），只在手机外壳里懒加载，不进入口包。
import { Search, SquarePen } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { PhoneMenuButton } from './PhoneMenuButton'

export type PhoneTopBarProps = {
  title: string
  onNewTask: () => void
  // 给出时在 ✎ 前显示搜索（全部代理页）。
  onSearch?: () => void
}

const ICON_BUTTON =
  'grid size-11 flex-none place-items-center rounded-[var(--r-md)] text-content-secondary focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus active:bg-surface-hover'

export function PhoneTopBar({ title, onNewTask, onSearch }: PhoneTopBarProps) {
  const { t } = useI18n()
  return (
    <header
      data-phone-topbar
      data-focus-region="header"
      className="relative grid h-14 flex-none grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center bg-panel px-1"
    >
      <div className="flex min-w-0 items-center">
        <PhoneMenuButton />
      </div>
      <p
        data-phone-title
        aria-hidden="true"
        className="max-w-[min(56vw,240px)] truncate text-center text-base leading-6 font-semibold text-content"
      >
        {title}
      </p>
      <div className="flex min-w-0 items-center justify-end">
        {onSearch && (
          <button
            type="button"
            data-phone-search
            aria-label={t('navigation:sidebar.search')}
            className={ICON_BUTTON}
            onClick={onSearch}
          >
            <Search className="size-5" strokeWidth={1.75} aria-hidden="true" />
          </button>
        )}
        <button
          type="button"
          data-phone-new-task
          aria-label={t('navigation:shell.newTask')}
          className={ICON_BUTTON}
          onClick={onNewTask}
        >
          <SquarePen className="size-5" strokeWidth={1.75} aria-hidden="true" />
        </button>
      </div>
    </header>
  )
}
