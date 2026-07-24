import type { RefObject } from 'react'
import {
  Link2,
  Menu,
  Monitor,
  Moon,
  Plus,
  Rocket,
  Save,
  Search,
  Square,
  Sun,
  Play,
  type LucideIcon,
} from 'lucide-react'
import { useI18n } from '../../app/use-i18n'
import type { ThemeMode } from '../../stores/ui-store'
import { SidebarTrigger } from '../ui/sidebar'

const THEME_META: Record<ThemeMode, LucideIcon> = {
  system: Monitor,
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
}: PageHeaderProps) {
  const { t } = useI18n()
  const primaryActions: Partial<Record<string, [string, LucideIcon]>> = {
    chat: [t('navigation:pageHeader.newChat'), Plus],
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
      className={`page-header ${desktop ? '[-webkit-app-region:drag]' : ''} ${desktopPlatform === 'darwin' ? 'pl-[74px]' : ''}`}
    >
      <SidebarTrigger
        className={`mobile-menu ${desktop ? '[-webkit-app-region:no-drag]' : ''}`}
        onClick={onMenu}
      >
        <Menu size={19} />
      </SidebarTrigger>
      <div className="title-block">
        <h1>{meta[0]}</h1>
        <p>{meta[1]}</p>
      </div>
      <div
        className={`header-actions ${desktop ? '[-webkit-app-region:no-drag]' : ''} ${desktopPlatform && desktopPlatform !== 'darwin' ? 'pr-[138px]' : ''}`}
      >
        {page === 'workflowCreate' ? (
          <>
            <button
              className="button secondary"
              disabled={!workflowActions || workflowActions.busy || workflowActions.running}
              onClick={() => workflowActions?.save()}
            >
              <Save size={15} />
              {t('navigation:pageHeader.saveDraft')}
            </button>
            <button
              className="button dark"
              disabled={!workflowActions || workflowActions.busy}
              onClick={() => workflowActions?.run()}
            >
              {workflowActions?.running ? <Square size={15} /> : <Play size={15} />}
              {workflowActions?.running
                ? t('navigation:pageHeader.stop')
                : t('navigation:pageHeader.testRun')}
            </button>
          </>
        ) : page === 'chat' ? null : (
          <label className="search-box" title={t('navigation:pageHeader.search')}>
            <Search size={15} />
            <input
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
          <button
            className="button primary"
            title={t('navigation:pageHeader.primaryActionShortcut', { action: primary[0] })}
            onClick={onPrimary}
          >
            {(() => {
              const PrimaryIcon = primary[1]
              return <PrimaryIcon size={15} />
            })()}
            {primary[0]}
          </button>
        )}
        <button
          className="icon-button theme-toggle"
          title={t('navigation:pageHeader.themeThemeClickToSwitch', { theme: themeLabel })}
          aria-label={t('navigation:pageHeader.themeThemeClickToSwitchThemes', {
            theme: themeLabel,
          })}
          onClick={onCycleTheme}
        >
          <ThemeIcon size={16} />
        </button>
      </div>
    </header>
  )
}
