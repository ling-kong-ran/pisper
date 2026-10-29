// 页头：48px 面包屑顶栏（所属分组 / 当前页）+ 搜索框 + 主操作按钮 + 主题/终端等工具。
// 设置分区在顶栏下方另起 20/600 页标题；手机宽度改为 52px 居中标题栏 + 一行搜索与主操作。
// 主操作按页面注册（注册表机制），不同页面显示不同的按钮文案与图标。
import type { ReactNode, RefObject } from 'react'
import {
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
import { getActivityNavigation } from '@/app/activity-navigation'
import { SETTINGS_PAGES } from '@/app/settings-navigation'
import { useI18n } from '@/app/use-i18n'
import type { ThemeMode } from '@/stores/ui-store'
import { ThemeToggleButton } from './ThemeToggleButton'
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
  // 与侧栏一致：从功能栏分组进入的插件等页面包屑归该分组，缺省时按页面集合判断。
  settingsOwned?: boolean
  query: string
  setQuery: (query: string) => void
  configSection: string
  onMenu: () => void
  onPrimary: () => void
  searchSlot?: ReactNode
  actionsSlot?: ReactNode
  theme: ThemeMode
  onCycleTheme: () => void
  searchInputRef: RefObject<HTMLInputElement | null>
  workflowActions: WorkflowActions | null
  desktopPlatform: string
  mobileApp?: boolean
  terminalOpen: boolean
  onToggleTerminal: () => void
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
  theme,
  onCycleTheme,
  searchInputRef,
  workflowActions,
  desktopPlatform,
  mobileApp = false,
  terminalOpen,
  onToggleTerminal,
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
  // 面包屑上一级：归设置的页面归「设置」，其余页面归功能栏分组（长组名用短名）；
  // 自定义组件工具页没有独立导航项，随「组件」归扩展分组。
  const activityPage =
    page === 'workflowCreate' ? 'workflows' : page === 'customComponentTool' ? 'components' : page
  const activityGroup = getActivityNavigation(t).find((group) =>
    group.items.some((item) => item.id === activityPage),
  )
  const parent =
    page === 'chat'
      ? ''
      : (settingsOwned ?? SETTINGS_PAGES.has(page))
        ? t('navigation:navigation.settings')
        : activityGroup?.id === 'workspace'
          ? t('navigation:rail.workspaceShort')
          : activityGroup?.label || ''
  // 设置分区的语义标题是下方 20/600 大标题（随界面字号等比缩放）；顶栏里的当前项只作面包屑显示。
  const titleBlock = page === 'config'
  const CurrentCrumb = titleBlock ? 'span' : 'h1'

  return (
    <header
      ref={elementRef}
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
          // 让开 Windows 自绘窗口按钮：桌面宽度整行留白；窄窗口换成两行后只有首行标题与按钮同高，
          // 留白改由标题行承担，第二行搜索与主操作可用满宽度。
          window.pisperDesktop?.customTitlebar && 'pr-[148px] max-[650px]:pr-4',
          'pl-[calc(16px+var(--workbench-leading-inset,0px))] max-[650px]:pl-4',
        )}
      >
        <div
          className={cn(
            'mr-auto flex min-w-0 items-center gap-1.5 text-[13px]',
            // 手机：52px 居中标题 + 12px 说明；两侧各让出 44px 给抽屉键与主题键。
            'max-[650px]:h-[52px] max-[650px]:basis-full max-[650px]:flex-col max-[650px]:justify-center max-[650px]:gap-0 max-[650px]:px-11 max-[650px]:text-center',
            page === 'chat' && 'max-[650px]:h-auto max-[650px]:basis-auto max-[650px]:px-0',
            page !== 'chat' && window.pisperDesktop?.customTitlebar && 'max-[650px]:pr-[176px]',
          )}
        >
          {parent && (
            <>
              <span className="shrink-0 text-content-muted max-[900px]:hidden">{parent}</span>
              <span aria-hidden="true" className="shrink-0 text-content-faint max-[900px]:hidden">
                /
              </span>
            </>
          )}
          <CurrentCrumb className="max-w-full min-w-[4em] truncate text-[13px] leading-5 font-medium text-content max-[650px]:text-base max-[650px]:font-semibold max-[650px]:tracking-[-.01em]">
            {meta[0]}
          </CurrentCrumb>
          {/* 页面说明：桌面跟在标题后作次要文字，窄于 900px 时收起；手机作为居中标题下的副标题行。 */}
          {page !== 'chat' && page !== 'config' && meta[1] && (
            <p className="ml-1.5 min-w-0 truncate text-xs text-content-muted max-[900px]:hidden max-[650px]:ml-0 max-[650px]:block max-[650px]:max-w-full">
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
            desktopPlatform &&
              desktopPlatform !== 'darwin' &&
              !window.pisperDesktop?.customTitlebar &&
              'pr-[138px] max-[650px]:pr-0',
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
          ) : page === 'chat' || page === 'customComponentTool' ? null : page === 'config' ? (
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
          {page !== 'config' && (
            <ThemeToggleButton
              compact
              theme={theme}
              onCycle={onCycleTheme}
              className={cn(
                page !== 'chat' &&
                  'pointer-coarse:size-11 max-[650px]:absolute max-[650px]:top-1 max-[650px]:right-1 max-[650px]:size-11',
                page !== 'chat' &&
                  window.pisperDesktop?.customTitlebar &&
                  'max-[650px]:right-[148px]',
              )}
            />
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
