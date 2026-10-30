// 应用侧栏外壳（规格 v2 第 2.1 节）：品牌行 + 收起按钮 · 新任务 ⌘N · 搜索 ⌘K · 全部代理 ⌘1 · 功能 ⌘2 ·
// 分组区（插槽，单独滚动）· 底部设置 ⌘, 与更新提示。宽 268，不可拖动；⌘B 或按钮收起为 60px 窄轨：
// 四个入口图标 + 需要你 / 运行中两个计数（插槽）+ 底部设置。收起状态存在 pisper-sidebar（sidebar-store）。
// 任何宽度下默认展开占位：并行可见度优先，只有用户自己收起才变窄轨。
// 分组区是插槽：会话页与普通页放看板领域的会话分组（随看板 store 懒加载，先画骨架行）；
// 全部代理页的筛选形态与设置目录（A3）换成各自的内容，外壳不变。
// 收起时 ⌘J / ⌥A 临时展开（peeking）：展开的侧栏浮在主区上方，焦点离开或 Esc 时收回，不改变用户的收起偏好。
// Windows 自绘标题栏时品牌行空白处是拖拽区（交互元素由 DesktopWindowControls 排除）。
// 本模块进入口包，只放静态外壳；看板数据层、分组与计数都在插槽里按需加载。
import { useEffect, useLayoutEffect, useRef, type ReactNode } from 'react'
import {
  LayoutGrid,
  List,
  PanelLeftClose,
  PanelLeftOpen,
  Search,
  Settings,
  SquarePen,
  type LucideIcon,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { BrandLogo } from '@/components/BrandLogo'
import { Kbd } from '@/components/ui/kbd'
import { ariaKeyShortcuts } from '@/lib/shortcut-aria'
import { useShortcutLabel } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { useShortcutStore } from '@/stores/shortcut-store'
import { useSidebarStore } from '@/stores/sidebar-store'

export type ShellSidebarEntry = 'board' | 'features' | 'settings'

export type ShellSidebarProps = {
  // 当前页对应的入口（aria-current）；会话页由会话行自己标出当前项。
  active: ShellSidebarEntry | null
  onNewTask: () => void
  onSearch: () => void
  onOpenBoard: () => void
  onOpenFeatures: () => void
  onOpenSettings: () => void
  // 分组区内容（展开时）与窄轨计数（收起时）。
  content: ReactNode
  railCounts: ReactNode
  // 有可用更新时的图标键（沿用 SidebarUpdateStatus），展开与窄轨都显示在设置旁。
  update?: ReactNode
}

const ROW =
  'flex w-full min-w-0 items-center gap-2.5 rounded-[var(--r-md)] px-2.5 text-base text-content transition-colors duration-(--d-fast) hover:bg-surface-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus'
const ICON_BUTTON =
  'grid size-8 flex-none place-items-center rounded-[var(--r-sm)] text-content-secondary transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus pointer-coarse:size-11'
const RAIL_BUTTON =
  'grid size-10 place-items-center rounded-[var(--r-md)] text-content-secondary transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus pointer-coarse:size-11'

export function ShellSidebar(props: ShellSidebarProps) {
  const collapsed = useSidebarStore((state) => state.collapsed)
  const peeking = useSidebarStore((state) => state.peeking)
  const setPeeking = useSidebarStore((state) => state.setPeeking)
  const overlayRef = useRef<HTMLDivElement>(null)
  const rootRef = useRef<HTMLDivElement>(null)
  // 收起 / 展开会换掉整块内容：焦点原本在侧栏里（按了收起键或 ⌘B）时，交给新内容里的收起 / 展开键，
  // 不让键盘焦点掉回 body。
  const focusInsideRef = useRef(false)
  useLayoutEffect(() => {
    if (!focusInsideRef.current) return
    const active = document.activeElement
    if (active && active !== document.body && rootRef.current?.contains(active)) return
    rootRef.current?.querySelector<HTMLElement>('[data-sidebar-toggle]')?.focus()
  }, [collapsed])

  // 临时展开：焦点或指针离开浮层（菜单等弹层除外）、按 Esc 时收回窄轨。
  useEffect(() => {
    if (!collapsed || !peeking) return
    const inside = (target: EventTarget | null) =>
      target instanceof Element &&
      Boolean(
        overlayRef.current?.contains(target) ||
        target.closest('[role="menu"], [role="dialog"], [data-radix-popper-content-wrapper]'),
      )
    const onFocusIn = (event: FocusEvent) => {
      if (!inside(event.target)) setPeeking(false)
    }
    const onPointerDown = (event: PointerEvent) => {
      if (!inside(event.target)) setPeeking(false)
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented || !inside(event.target)) return
      event.preventDefault()
      setPeeking(false)
    }
    document.addEventListener('focusin', onFocusIn)
    document.addEventListener('pointerdown', onPointerDown)
    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.removeEventListener('focusin', onFocusIn)
      document.removeEventListener('pointerdown', onPointerDown)
      document.removeEventListener('keydown', onKeyDown)
    }
  }, [collapsed, peeking, setPeeking])

  return (
    <div
      ref={rootRef}
      data-shell-sidebar={collapsed ? 'rail' : 'expanded'}
      onFocus={() => {
        focusInsideRef.current = true
      }}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null))
          focusInsideRef.current = false
      }}
      className={cn(
        'relative z-20 flex h-full flex-none flex-col bg-sidebar transition-[width] duration-(--d-base) ease-out motion-reduce:transition-none',
        collapsed ? 'w-[60px]' : 'w-[268px]',
        // 深色下侧栏与纸面只差一级，补一条极淡的分界；浅色不加分隔线（规格 2.1）。
        'dark:border-r dark:border-stroke-soft',
      )}
    >
      {collapsed ? <SidebarRail {...props} /> : <SidebarExpanded {...props} />}
      {collapsed && peeking && (
        <div
          ref={overlayRef}
          data-shell-sidebar-peek
          className="absolute inset-y-0 left-0 z-40 flex w-[268px] flex-col bg-sidebar shadow-float"
        >
          <SidebarExpanded {...props} peek />
        </div>
      )}
    </div>
  )
}

// 入口对应的快捷键动作：标注 aria-keyshortcuts 与按键提示，改绑后立即跟着变。
const ENTRY_ACTIONS = {
  newTask: 'primaryAction',
  search: 'commandPalette',
  board: 'openBoard',
  features: 'openMore',
  settings: 'openSettings',
} as const

function useEntryLabels() {
  const { t } = useI18n()
  return {
    newTask: t('navigation:shell.newTask'),
    search: t('navigation:sidebar.search'),
    board: t('navigation:sidebar.allAgents'),
    features: t('navigation:sidebar.features'),
    settings: t('navigation:navigation.settings'),
  }
}

function SidebarExpanded({
  active,
  onNewTask,
  onSearch,
  onOpenBoard,
  onOpenFeatures,
  onOpenSettings,
  content,
  update,
  peek = false,
}: ShellSidebarProps & { peek?: boolean }) {
  const { t } = useI18n()
  const labels = useEntryLabels()
  const toggleCollapsed = useSidebarStore((state) => state.toggleCollapsed)
  const setPeeking = useSidebarStore((state) => state.setPeeking)
  const bindings = useShortcutStore((state) => state.bindings)
  const toggleBinding = bindings.toggleSidebar
  const toggleShortcut = useShortcutLabel('toggleSidebar')
  const shortcuts = {
    newTask: useShortcutLabel('primaryAction'),
    search: useShortcutLabel('commandPalette'),
    board: useShortcutLabel('openBoard'),
    features: useShortcutLabel('openMore'),
    settings: useShortcutLabel('openSettings'),
  }
  // 临时展开的浮层里“收起”键的意思是收回浮层；常规展开时是收起成窄轨。
  const collapseLabel = peek ? t('navigation:sidebar.expand') : t('navigation:sidebar.collapse')
  const entry = (
    id: 'newTask' | 'search' | ShellSidebarEntry,
    Icon: LucideIcon,
    onSelect: () => void,
  ) => {
    const label = id === 'board' ? labels.board : id === 'features' ? labels.features : labels[id]
    const shortcut = shortcuts[id]
    const current = id === active
    return (
      <button
        type="button"
        data-sidebar-entry={id}
        aria-current={current ? 'page' : undefined}
        aria-keyshortcuts={ariaKeyShortcuts(bindings[ENTRY_ACTIONS[id]])}
        className={cn(
          ROW,
          'h-9 pointer-coarse:h-11',
          current && 'bg-surface-active font-medium hover:bg-surface-active',
        )}
        onClick={onSelect}
      >
        <Icon
          className="size-[18px] flex-none text-content-secondary"
          strokeWidth={1.75}
          aria-hidden="true"
        />
        <span className="min-w-0 flex-1 truncate text-left">{label}</span>
        {shortcut && (
          <Kbd
            aria-hidden="true"
            className="border-transparent bg-transparent text-content-faint shadow-none pointer-coarse:hidden"
          >
            {shortcut}
          </Kbd>
        )}
      </button>
    )
  }
  return (
    <nav aria-label={t('navigation:sidebar.label')} className="flex min-h-0 flex-1 flex-col">
      <div
        data-window-drag-region
        className="flex h-[52px] flex-none items-center gap-2 pr-2.5 pl-4"
      >
        <BrandLogo size={22} />
        <span className="text-[15px] font-semibold tracking-[-.01em] text-content">Pisper</span>
        <span className="flex-1" />
        <button
          type="button"
          data-sidebar-toggle={peek ? undefined : 'collapse'}
          aria-expanded={!peek || undefined}
          className={ICON_BUTTON}
          aria-label={collapseLabel}
          aria-keyshortcuts={peek ? undefined : ariaKeyShortcuts(toggleBinding)}
          title={!peek && toggleShortcut ? `${collapseLabel} (${toggleShortcut})` : collapseLabel}
          onClick={() => (peek ? setPeeking(false) : toggleCollapsed())}
        >
          {peek ? (
            <PanelLeftOpen className="size-[18px]" strokeWidth={1.75} aria-hidden="true" />
          ) : (
            <PanelLeftClose className="size-[18px]" strokeWidth={1.75} aria-hidden="true" />
          )}
        </button>
      </div>
      <div className="flex flex-none flex-col gap-px px-2 pt-0.5 pb-1.5">
        {entry('newTask', SquarePen, onNewTask)}
        {entry('search', Search, onSearch)}
        {entry('board', List, onOpenBoard)}
        {entry('features', LayoutGrid, onOpenFeatures)}
      </div>
      <div
        data-sidebar-scroll
        className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto overscroll-contain px-2 pt-1.5"
      >
        {content}
      </div>
      <div className="flex flex-none items-center gap-1 p-2">
        <div className="min-w-0 flex-1">{entry('settings', Settings, onOpenSettings)}</div>
        {update}
      </div>
    </nav>
  )
}

// 单独导出窄轨，便于守卫测试在不改收起偏好的情况下渲染它。
export function SidebarRail({
  active,
  onNewTask,
  onSearch,
  onOpenBoard,
  onOpenFeatures,
  onOpenSettings,
  railCounts,
  update,
}: ShellSidebarProps) {
  const { t } = useI18n()
  const labels = useEntryLabels()
  const toggleCollapsed = useSidebarStore((state) => state.toggleCollapsed)
  const bindings = useShortcutStore((state) => state.bindings)
  const toggleBinding = bindings.toggleSidebar
  const toggleShortcut = useShortcutLabel('toggleSidebar')
  const expandLabel = t('navigation:sidebar.expand')
  const railButton = (
    id: 'newTask' | 'search' | ShellSidebarEntry,
    Icon: LucideIcon,
    onSelect: () => void,
  ) => {
    const label = id === 'board' ? labels.board : id === 'features' ? labels.features : labels[id]
    const current = id === active
    return (
      <button
        type="button"
        data-sidebar-entry={id}
        aria-keyshortcuts={ariaKeyShortcuts(bindings[ENTRY_ACTIONS[id]])}
        aria-label={label}
        aria-current={current ? 'page' : undefined}
        title={label}
        className={cn(RAIL_BUTTON, current && 'bg-surface-active text-content')}
        onClick={onSelect}
      >
        <Icon className="size-[18px]" strokeWidth={1.75} aria-hidden="true" />
      </button>
    )
  }
  return (
    <nav
      aria-label={t('navigation:sidebar.labelCollapsed')}
      className="flex min-h-0 flex-1 flex-col items-center gap-1 pb-2"
    >
      <div data-window-drag-region className="flex h-[52px] flex-none items-center">
        <button
          type="button"
          data-sidebar-toggle="expand"
          aria-expanded={false}
          className={RAIL_BUTTON}
          aria-label={expandLabel}
          aria-keyshortcuts={ariaKeyShortcuts(toggleBinding)}
          title={toggleShortcut ? `${expandLabel} (${toggleShortcut})` : expandLabel}
          onClick={toggleCollapsed}
        >
          <PanelLeftOpen className="size-[18px]" strokeWidth={1.75} aria-hidden="true" />
        </button>
      </div>
      {railButton('newTask', SquarePen, onNewTask)}
      {railButton('search', Search, onSearch)}
      <span aria-hidden="true" className="my-1.5 h-px w-6 bg-stroke-strong" />
      {railCounts}
      <span aria-hidden="true" className="my-1.5 h-px w-6 bg-stroke-strong" />
      {railButton('board', List, onOpenBoard)}
      {railButton('features', LayoutGrid, onOpenFeatures)}
      <span className="flex-1" />
      {update}
      {railButton('settings', Settings, onOpenSettings)}
    </nav>
  )
}

// 分组区在看板数据块加载前的骨架行：只占位，不说“没有代理”。体积要小，它在入口包里。
export function SidebarSessionsFallback({ collapsed = false }: { collapsed?: boolean }) {
  const { t } = useI18n()
  if (collapsed)
    return (
      <div aria-hidden="true" className="flex flex-col items-center gap-1">
        <span className="size-10 rounded-[var(--r-md)] bg-surface-hover" />
        <span className="size-10 rounded-[var(--r-md)] bg-surface-hover" />
      </div>
    )
  return (
    <div role="status" aria-busy="true" aria-label={t('navigation:sidebar.loading')}>
      {[60, 44, 70].map((width) => (
        <div key={width} aria-hidden="true" className="flex h-9 items-center gap-2.5 px-2.5">
          <span className="size-4 rounded-full bg-surface-hover" />
          <span className="h-3 rounded-sm bg-surface-hover" style={{ width: `${width}%` }} />
        </div>
      ))}
    </div>
  )
}
