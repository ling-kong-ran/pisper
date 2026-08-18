// 页头：当前页标题/描述 + 搜索框 + 主操作按钮 + 主题/终端/菜单等工具。
// 主操作按页面注册（注册表机制），不同页面显示不同的按钮文案与图标。
import type { RefObject } from 'react'
import {
  Link2,
  Menu,
  Clock,
  Moon,
  Plus,
  Rocket,
  Save,
  Search,
  Square,
  Sun,
  TerminalSquare,
  Play,
  type LucideIcon,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { ThemeMode } from '@/stores/ui-store'
import { SidebarTrigger } from '@/components/ui/sidebar'
import { cn } from '@/lib/utils'

import { Button } from '@/components/ui/button'

const THEME_META: Record<ThemeMode, LucideIcon> = {
  system: Clock,
  light: Sun,
  dark: Moon,
}

type WorkflowActions = {
  busy: boolean
  running: boolean
  save: () => void
  run: () => void
}

type PageHeaderProps = {
  meta: readonly [string, string]
  page: string
  query: string
  setQuery: (query: string) => void
  configSection: string
  onMenu: () => void
  onPrimary: () => void
  theme: ThemeMode
  onCycleTheme: () => void
  searchInputRef: RefObject<HTMLInputElement | null>
  workflowActions: WorkflowActions | null
  desktopPlatform: string
  terminalOpen: boolean
  onToggleTerminal: () => void
}

export function PageHeader({
  meta,
  page,
  query,
  setQuery,
  configSection,
  onMenu,
  onPrimary,
  theme,
  onCycleTheme,
  searchInputRef,
  workflowActions,
  desktopPlatform,
  terminalOpen,
  onToggleTerminal,
}: PageHeaderProps) {
  const { t } = useI18n()
  const primaryActions: Partial<Record<string, [string, LucideIcon]>> = {
    chatHistory: [t('navigation:pageHeader.newChat'), Plus],
    assets: [t('navigation:pageHeader.addLink'), Link2],
    channels: [t('navigation:pageHeader.connectChannel'), Plus],
    schedules: [t('navigation:pageHeader.newTask'), Plus],
    config: [t('navigation:pageHeader.addProvider'), Plus],
    plugins: [t('navigation:pageHeader.savePolicy'), Save],
    memory: [t('navigation:pageHeader.addMemory'), Plus],
    mcp: [t('navigation:pageHeader.addService'), Plus],
    skills: [t('navigation:pageHeader.installSkill'), Plus],
    workflows: [t('navigation:pageHeader.newWorkflow'), Plus],
    workflowCreate: [t('navigation:pageHeader.publish'), Rocket],
  }
  const primary = page === 'config' && configSection !== 'models' ? null : primaryActions[page]
  const ThemeIcon = THEME_META[theme]
  const themeLabel =
    theme === 'light'
      ? t('navigation:pageHeader.light')
      : theme === 'dark'
        ? t('navigation:pageHeader.dark')
        : t('navigation:pageHeader.system')
  const desktop = Boolean(desktopPlatform)

  return (
    <header
      className={cn(
        'relative z-[2] flex min-h-14 flex-none items-center gap-3.5 px-6 pt-[9px] pb-[7px] in-data-[density=compact]:min-h-[50px] in-data-[density=compact]:pt-1.5 in-data-[density=compact]:pb-[5px] max-[650px]:min-h-[126px] max-[650px]:flex-wrap max-[650px]:content-center max-[650px]:gap-2.5 max-[650px]:px-4 max-[650px]:py-2.5',
        page === 'chat' &&
          'min-h-[52px] px-5 pt-2 pb-1.5 in-data-[density=compact]:min-h-[46px] max-[650px]:min-h-[126px] max-[650px]:px-4 max-[650px]:pt-3.5 max-[650px]:pb-2.5',
        desktop && '[-webkit-app-region:drag]',
        desktopPlatform === 'darwin' && 'pl-[74px]',
      )}
    >
      <SidebarTrigger
        className={cn(
          'hidden size-[34px] place-items-center rounded-[var(--r-sm)] border border-[var(--stroke)] bg-[var(--solid)] max-[900px]:grid',
          desktop && '[-webkit-app-region:no-drag]',
        )}
        onClick={onMenu}
      >
        <Menu size={19} />
      </SidebarTrigger>
      <div className="mr-auto flex min-w-[170px] items-baseline gap-2.5 max-[650px]:block max-[650px]:min-w-0 max-[650px]:flex-1">
        <h1 className="text-base leading-[1.15] font-bold tracking-[0] max-[650px]:text-[21px]">
          {meta[0]}
        </h1>
        <p className="mt-0 min-w-0 overflow-hidden text-[12px] text-ellipsis whitespace-nowrap text-[var(--text-muted)] max-[650px]:mt-[3px] max-[650px]:whitespace-normal">
          {meta[1]}
        </p>
      </div>
      <div
        className={cn(
          'flex min-w-0 items-center justify-end gap-2 max-[650px]:grid max-[650px]:w-full max-[650px]:grid-cols-[minmax(0,1fr)_auto] max-[650px]:justify-stretch',
          page === 'workflowCreate' &&
            'max-[650px]:grid-cols-[repeat(3,auto)] max-[650px]:justify-end',
          desktop && '[-webkit-app-region:no-drag]',
          desktopPlatform && desktopPlatform !== 'darwin' && 'pr-[138px]',
        )}
      >
        {page === 'workflowCreate' ? (
          <>
            <Button
              variant="outline"
              size="lg"
              className="bg-surface-subtle max-[650px]:col-auto max-[650px]:row-start-1"
              disabled={!workflowActions || workflowActions.busy || workflowActions.running}
              onClick={() => workflowActions?.save()}
            >
              <Save size={15} />
              {t('navigation:pageHeader.saveDraft')}
            </Button>
            <Button
              size="lg"
              className="max-[650px]:col-auto max-[650px]:row-start-1"
              disabled={!workflowActions || workflowActions.busy}
              onClick={() => workflowActions?.run()}
            >
              {workflowActions?.running ? <Square size={15} /> : <Play size={15} />}
              {workflowActions?.running
                ? t('navigation:pageHeader.stop')
                : t('navigation:pageHeader.testRun')}
            </Button>
          </>
        ) : page === 'chat' ? null : (
          <label
            className="flex h-[34px] w-[min(250px,24vw)] items-center gap-[7px] rounded-[var(--r-sm)] border border-[var(--stroke)] bg-[var(--search-bg)] px-2.5 text-[var(--text-muted)] focus-within:border-[var(--focus)] focus-within:shadow-[0_0_0_3px_var(--focus-ring)] in-data-[density=compact]:h-[30px] max-[900px]:w-[190px] max-[650px]:col-start-1 max-[650px]:row-start-1 max-[650px]:w-full max-[650px]:min-w-0"
            title={t('navigation:pageHeader.search')}
          >
            <Search size={15} />
            <input
              className="w-full min-w-0 border-0 bg-transparent text-[13px] text-[var(--text)] outline-none"
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
          </label>
        )}
        {primary && (
          <Button
            size="lg"
            className="max-[650px]:col-start-2 max-[650px]:row-start-1"
            title={t('navigation:pageHeader.primaryActionShortcut', { action: primary[0] })}
            onClick={onPrimary}
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
            className={terminalOpen ? 'bg-surface-hover text-brand' : undefined}
            title={t('navigation:pageHeader.toggleTerminal')}
            aria-label={t('navigation:pageHeader.toggleTerminal')}
            aria-pressed={terminalOpen}
            onClick={onToggleTerminal}
          >
            <TerminalSquare size={16} />
          </Button>
        )}
        <Button
          variant="outline"
          size="icon"
          className="bg-[var(--solid)] max-[650px]:absolute max-[650px]:top-3.5 max-[650px]:right-4"
          title={t('navigation:pageHeader.themeThemeClickToSwitch', { theme: themeLabel })}
          aria-label={t('navigation:pageHeader.themeThemeClickToSwitchThemes', {
            theme: themeLabel,
          })}
          onClick={onCycleTheme}
        >
          <ThemeIcon size={16} />
        </Button>
      </div>
    </header>
  )
}
