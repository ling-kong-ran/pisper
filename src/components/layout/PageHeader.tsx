// 页头：48px 面包屑顶栏（所属入口 / 当前页）+ 搜索框 + 主操作按钮 + 终端等工具。
// 主题切换不在页头（规格 v2 第 2.5 节）：设置 › 界面与 ⌘K“切换主题”。
// 设置分区在顶栏下方另起 20/600 页标题；手机宽度与看板、更多同一套页头：56px 行内左对齐的 24/650 标题
// + 12px 说明，从“更多”进入时左侧是 44px 的“‹ 更多”文字返回键；下面一行是搜索与主操作。
// 手机页头不放主题键（主题在“设置 → 界面”）。
// 主操作按页面注册（注册表机制），不同页面显示不同的按钮文案与图标。
// 功能页（只属于“功能”，规格 2.6 双归属已退役）面包屑上一级是可点的“‹ 功能”（手机标签栏 A4 前仍叫“更多”）。没有应用标题栏了：Windows 自绘标题栏时
// 页头空白处是拖拽区，右侧预留 144px 给窗口按钮（macOS / Linux 只留 12px）。
import type { ReactNode, RefObject } from 'react'
import {
  ChevronLeft,
  Link2,
  Plus,
  Rocket,
  Save,
  Search,
  Square,
  TerminalSquare,
  Play,
  type LucideIcon,
} from 'lucide-react'
import { MORE_PAGES } from '@/app/more-pages'
import { useI18n } from '@/app/use-i18n'
import { cn } from '@/lib/utils'
import { useShortcutLabel } from '@/lib/shortcuts'

import { Button } from '@/components/ui/button'
import { Kbd } from '@/components/ui/kbd'

type WorkflowActions = {
  busy: boolean
  running: boolean
  save: () => void
  run: () => void
}

type PageHeaderProps = {
  elementRef?: RefObject<HTMLElement | null>
  meta: readonly [string, string]
  page: string
  // 当前路由用设置外壳（/config 下的应用设置分区）；缺省按 page === 'config' 判断。
  settingsOwned?: boolean
  query: string
  setQuery: (query: string) => void
  configSection: string
  onMenu: () => void
  onPrimary: () => void
  searchSlot?: ReactNode
  actionsSlot?: ReactNode
  searchInputRef: RefObject<HTMLInputElement | null>
  workflowActions: WorkflowActions | null
  desktopPlatform: string
  mobileApp?: boolean
  terminalOpen: boolean
  onToggleTerminal: () => void
  // 当前页属于“功能”时提供，渲染“‹ 功能”返回。
  onBackToMore?: () => void
  // 手机外壳（规格 7.1）：页名在外壳顶栏里，这里的标题只给读屏；“‹ 功能”返回保留（iOS 没有返回键）。
  phoneShell?: boolean
}

export function PageHeader({
  elementRef,
  meta,
  page,
  settingsOwned,
  query,
  setQuery,
  configSection,
  onPrimary,
  searchSlot,
  actionsSlot,
  searchInputRef,
  workflowActions,
  desktopPlatform,
  mobileApp = false,
  terminalOpen,
  onToggleTerminal,
  onBackToMore,
  phoneShell = false,
}: PageHeaderProps) {
  const { t } = useI18n()
  const primaryShortcut = useShortcutLabel('primaryAction')
  const terminalShortcut = useShortcutLabel('toggleTerminal')
  const searchShortcut = useShortcutLabel('focusSearch')
  const primaryActions: Partial<Record<string, [string, LucideIcon]>> = {
    chatHistory: [t('navigation:pageHeader.newChat'), Plus],
    assets: [t('navigation:pageHeader.addLink'), Link2],
    channels: [t('navigation:pageHeader.connectChannel'), Plus],
    schedules: [t('navigation:pageHeader.newTask'), Plus],
    config: [t('navigation:pageHeader.quickSetup'), Plus],
    plugins: [t('navigation:pageHeader.savePolicy'), Save],
    memory: [t('navigation:pageHeader.addMemory'), Plus],
    mcp: [t('navigation:pageHeader.addService'), Plus],
    skills: [t('navigation:pageHeader.installSkill'), Plus],
    workflows: [t('navigation:pageHeader.newWorkflow'), Plus],
    workflowCreate: [t('navigation:pageHeader.publish'), Rocket],
  }
  const primary: [string, LucideIcon] | null | undefined =
    page === 'config' && configSection !== 'models'
      ? null
      : page === 'chat'
        ? mobileApp
          ? [t('navigation:pageHeader.newChat'), Plus]
          : null
        : primaryActions[page]
  const desktop = Boolean(desktopPlatform)
  // 面包屑上一级：设置分区归「设置」，功能页（含 /config 下的远程访问等）归「功能」（可点返回）；
  // 一级页（组件、功能本身）没有上一级。手机没有“组件”标签，组件页从“更多”进入，
  // 应用壳此时传入 onBackToMore，同样显示“‹ 更多”。
  const inSettings = settingsOwned ?? page === 'config'
  const parent =
    page === 'chat'
      ? ''
      : inSettings
        ? t('navigation:navigation.settings')
        : MORE_PAGES.has(page) || onBackToMore
          ? t('navigation:sidebar.features')
          : ''
  // 设置分区的语义标题是下方 20/600 大标题（随界面字号等比缩放）；顶栏里的当前项只作面包屑显示。
  const titleBlock = page === 'config'
  const CurrentCrumb = titleBlock ? 'span' : 'h1'

  return (
    <header
      ref={elementRef}
      data-focus-region="header"
      data-window-drag-region
      className={cn(
        '@container/page-header relative z-[2] flex flex-none flex-col',
        desktop && '[-webkit-app-region:drag]',
      )}
    >
      <div
        className={cn(
          'flex min-h-12 items-center gap-2 border-b border-stroke-soft py-1.5 pr-2.5 in-data-[density=compact]:min-h-11 max-[650px]:min-h-0 max-[650px]:flex-wrap max-[650px]:gap-x-2 max-[650px]:gap-y-0 max-[650px]:border-b-0 max-[650px]:px-4 max-[650px]:pt-0 max-[650px]:pb-3',
          page === 'chat' && '!min-h-12 px-4 !py-1.5 max-[650px]:!flex-nowrap',
          'pl-[calc(16px+var(--workbench-leading-inset,0px))] max-[650px]:pl-4',
          // Windows 自绘标题栏：右上角 144px 是窗口按钮。
          '[[data-custom-titlebar]_&]:pr-[144px]',
        )}
      >
        <div
          className={cn(
            'mr-auto flex min-w-0 items-center gap-1.5 text-[13px]',
            // 手机：56px 行，左对齐大标题，说明跟在标题后（放不下时先截说明）。
            'max-[650px]:h-14 max-[650px]:basis-full max-[650px]:gap-2',
            page === 'chat' && 'max-[650px]:h-auto max-[650px]:basis-auto max-[650px]:px-0',
            // 手机外壳：标题在顶栏里，这一行只剩“‹ 功能”（44px）；没有返回时整行只留给读屏的标题。
            phoneShell &&
              (parent && onBackToMore && !inSettings ? 'max-[650px]:h-11' : 'max-[650px]:h-0'),
          )}
        >
          {parent && onBackToMore && !inSettings ? (
            <>
              {/* 桌面是面包屑里的“‹ 更多”；手机是标题左侧 44px 高的文字返回键（规格 2.1）。 */}
              <button
                type="button"
                data-back-to-more
                className="-ml-1.5 flex h-7 shrink-0 items-center gap-0.5 rounded-sm pr-1.5 pl-1 text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus max-[650px]:-ml-3 max-[650px]:h-11 max-[650px]:min-w-11 max-[650px]:pr-2 max-[650px]:text-sm max-[650px]:text-content-secondary"
                aria-label={t('navigation:shell.backToFeaturesLabel')}
                onClick={onBackToMore}
              >
                <ChevronLeft className="size-4 max-[650px]:size-5" aria-hidden="true" />
                <span>{parent}</span>
              </button>
              <span aria-hidden="true" className="shrink-0 text-content-faint max-[650px]:hidden">
                /
              </span>
            </>
          ) : (
            parent && (
              <>
                <span className="shrink-0 text-content-muted max-[900px]:hidden">{parent}</span>
                <span aria-hidden="true" className="shrink-0 text-content-faint max-[900px]:hidden">
                  /
                </span>
              </>
            )
          )}
          <CurrentCrumb
            className={cn(
              'max-w-full min-w-[4em] truncate text-[13px] leading-5 font-medium text-content max-[650px]:min-w-0 max-[650px]:text-2xl max-[650px]:leading-8 max-[650px]:font-[650] max-[650px]:tracking-[-.015em]',
              phoneShell && 'sr-only',
            )}
          >
            {meta[0]}
          </CurrentCrumb>
          {/* 页面说明：桌面跟在标题后作次要文字，窄于 900px 时收起；手机跟在大标题后，只占剩余宽度。 */}
          {page !== 'chat' && page !== 'config' && meta[1] && !phoneShell && (
            <p className="ml-1.5 min-w-0 truncate text-xs text-content-muted max-[900px]:hidden max-[650px]:ml-0 max-[650px]:block max-[650px]:flex-1 max-[650px]:basis-0">
              {meta[1]}
            </p>
          )}
        </div>
        <div
          className={cn(
            'flex min-w-0 items-center justify-end gap-1.5 max-[650px]:grid max-[650px]:w-full max-[650px]:grid-cols-[minmax(0,1fr)_auto] max-[650px]:justify-stretch max-[650px]:gap-2',
            page === 'workflowCreate' &&
              'max-[650px]:grid-cols-[repeat(3,auto)] max-[650px]:justify-end',
            page === 'chat' && 'max-[650px]:!flex max-[650px]:!w-auto max-[650px]:shrink-0',
            desktop && '[-webkit-app-region:no-drag]',
          )}
        >
          {page === 'workflowCreate' ? (
            <>
              <Button
                variant="outline"
                className="max-[650px]:col-auto max-[650px]:row-start-1 max-[650px]:h-11"
                disabled={!workflowActions || workflowActions.busy || workflowActions.running}
                onClick={() => workflowActions?.save()}
              >
                <Save size={15} />
                {t('navigation:pageHeader.saveDraft')}
              </Button>
              <Button
                className="max-[650px]:col-auto max-[650px]:row-start-1 max-[650px]:h-11"
                disabled={!workflowActions || workflowActions.busy}
                onClick={() => workflowActions?.run()}
              >
                {workflowActions?.running ? <Square size={15} /> : <Play size={15} />}
                {workflowActions?.running
                  ? t('navigation:pageHeader.stop')
                  : t('navigation:pageHeader.testRun')}
              </Button>
            </>
          ) : page === 'chat' ||
            page === 'customComponentTool' ||
            page === 'components' ? null : page === 'config' ? (
            searchSlot
          ) : (
            <label
              className="flex h-8 w-[min(240px,24vw)] min-w-32 shrink items-center gap-2 rounded-sm border border-control bg-panel px-2.5 text-content-muted focus-within:border-focus focus-within:ring-3 focus-within:ring-focus-ring max-[900px]:w-[190px] max-[650px]:col-start-1 max-[650px]:row-start-1 max-[650px]:h-11 max-[650px]:w-full max-[650px]:min-w-0 max-[650px]:rounded-[10px] max-[650px]:border-transparent max-[650px]:bg-surface-muted max-[650px]:px-3"
              title={t('navigation:pageHeader.search')}
            >
              <Search size={15} className="shrink-0" />
              <input
                className="w-full min-w-0 border-0 bg-transparent text-[length:var(--app-font-size)] font-normal text-content caret-focus outline-none max-[650px]:text-[16px]"
                ref={searchInputRef}
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={
                  page === 'mcp'
                    ? t('navigation:pageHeader.searchServicesOrTools')
                    : page === 'memory'
                      ? t('navigation:pageHeader.searchMemoriesOrFiles')
                      : t('navigation:pageHeader.searchPage', { page: meta[0] })
                }
              />
              {searchShortcut && !query && (
                <Kbd aria-hidden="true" className="max-[650px]:hidden pointer-coarse:hidden">
                  {searchShortcut}
                </Kbd>
              )}
            </label>
          )}
          {actionsSlot}
          {primary && (
            <Button
              variant={
                page === 'config' ? 'ghost' : page === 'workflowCreate' ? 'outline' : 'default'
              }
              className={cn(
                'max-[650px]:col-start-2 max-[650px]:row-start-1 max-[650px]:h-11',
                page === 'chat' && mobileApp && 'hidden',
              )}
              title={
                primaryShortcut
                  ? t('navigation:pageHeader.primaryActionShortcut', {
                      action: primary[0],
                      shortcut: primaryShortcut,
                    })
                  : primary[0]
              }
              onClick={onPrimary}
              disabled={page === 'workflowCreate' && (!workflowActions || workflowActions.busy)}
            >
              {(() => {
                const PrimaryIcon = primary[1]
                return <PrimaryIcon size={15} />
              })()}
              {primary[0]}
            </Button>
          )}
          {desktop && (
            <Button
              variant="ghost"
              size="icon"
              className={cn(
                'text-content-muted max-[650px]:hidden',
                terminalOpen && 'bg-surface-hover text-accent-fg',
              )}
              title={
                terminalShortcut
                  ? `${t('navigation:pageHeader.toggleTerminal')} (${terminalShortcut})`
                  : t('navigation:pageHeader.toggleTerminal')
              }
              aria-label={t('navigation:pageHeader.toggleTerminal')}
              aria-pressed={terminalOpen}
              onClick={onToggleTerminal}
            >
              <TerminalSquare size={16} />
            </Button>
          )}
        </div>
      </div>
      {titleBlock && (
        <div
          className={cn(
            'mx-auto w-full px-4 pt-7 pb-5 @min-[760px]/page-header:px-10 max-[650px]:sr-only',
            configSection === 'models' ? 'max-w-[1040px]' : 'max-w-[776px]',
          )}
        >
          <h1 className="text-[length:calc(var(--ui-font-size)*10/7)] leading-[1.3] font-semibold tracking-[-.015em] text-content">
            {meta[0]}
          </h1>
        </div>
      )}
    </header>
  )
}
