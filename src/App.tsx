import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Outlet, useLocation, useNavigate, type NavigateOptions } from 'react-router-dom'
import { createPrimaryActionRegistry } from '@/app/primary-action'
import type { AppRouteContext } from '@/app/route-context'
import { STORAGE_KEYS } from '@/app/storage'
import { getNavigation, getPageMeta } from '@/app/navigation'
import { PAGE_IDS, pageFromPath, pagePath } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { BrandLogo } from '@/components/BrandLogo'
import { WebPreviewProvider } from '@/components/WebPreviewProvider'
import { AppSidebar } from '@/components/layout/AppSidebar'
import { CommandPalette, QuickCreate } from '@/components/layout/AppOverlays'
import { PageHeader } from '@/components/layout/PageHeader'
import { StatusBar } from '@/components/layout/StatusBar'
import { AppDialog, Toast, type ToastTone } from '@/components/ui'
import { requestSessionSelection } from '@/features/chat/events'
import { apiJson } from '@/lib/api'
import { showBrowserSystemNotification } from '@/lib/browser-notifications'
import { useAppDialog } from '@/hooks/useAppDialog'
import { useAppUpdate } from '@/features/updates/useAppUpdate'
import { SidebarInset, SidebarProvider } from '@/components/ui/sidebar'
import { useUiStore, type ThemeMode } from '@/stores/ui-store'
import type { ChatAttachment, PendingAsset } from '@/types/chat'
import type { NotificationSettingsData } from '@/types/notifications'
import type { WorkflowActions } from '@/types/workflow'

type ProviderConfig = {
  configured: boolean
  enabled: boolean
  models: Array<{ kind: string }>
}

type AppConfig = {
  providers?: ProviderConfig[]
}

type ToastState = {
  message: string
  tone: ToastTone
}

type PluginStats = {
  enabled: number
  total: number
}

function hasUsableProvider(config: AppConfig) {
  return Boolean(
    config?.providers?.some(
      (provider) =>
        provider.configured &&
        provider.enabled &&
        provider.models.some((model) => model.kind === 'chat'),
    ),
  )
}

function renderNotificationContent(content: string, data: Record<string, unknown>) {
  return String(content || '').replace(
    /\{\{\s*([\w.]+)\s*\}\}/g,
    (_match: string, path: string) => {
      const value = path.split('.').reduce<unknown>((current, key) => {
        if (!current || typeof current !== 'object') return undefined
        return (current as Record<string, unknown>)[key]
      }, data)
      return value == null ? `{{${path}}}` : String(value)
    },
  )
}

function resolveDark(mode: ThemeMode) {
  return (
    mode === 'dark' ||
    (mode === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)
  )
}

function isEditableTarget(target: EventTarget | null) {
  return (
    target instanceof HTMLElement &&
    (target.matches('input, textarea, select') || target.isContentEditable)
  )
}

function App() {
  const { t } = useI18n()
  const location = useLocation()
  const routerNavigate = useNavigate()
  const navigation = useMemo(() => getNavigation(t), [t])
  const pageMeta = useMemo(() => getPageMeta(t), [t])
  const page = pageFromPath(location.pathname) || 'chat'
  const [query, setQuery] = useState('')
  const [mobileNav, setMobileNav] = useState(false)
  const [paletteOpen, setPaletteOpen] = useState(false)
  const sidebarCollapsed = useUiStore((state) => state.sidebarCollapsed)
  const setSidebarCollapsed = useUiStore((state) => state.setSidebarCollapsed)
  const theme = useUiStore((state) => state.theme)
  const cycleTheme = useUiStore((state) => state.cycleTheme)
  const density = useUiStore((state) => state.density)
  const [toast, setToast] = useState<ToastState | null>(null)
  const [modal, setModal] = useState<string | null>(null)
  const [configSection, setConfigSection] = useState('models')
  const [pendingAsset, setPendingAsset] = useState<PendingAsset | null>(null)
  const [pluginStats, setPluginStats] = useState<PluginStats | null>(null)
  const [startupReady, setStartupReady] = useState(false)
  const [notificationSettings, setNotificationSettings] = useState<NotificationSettingsData>({
    browser: { enabled: false },
    connections: {},
    scopes: [],
    templates: [],
  })
  const [workflowActions, setWorkflowActions] = useState<WorkflowActions | null>(null)
  const browserEventCursor = useRef('')
  const [primaryActions] = useState(createPrimaryActionRegistry)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const toastTimer = useRef<number | undefined>(undefined)
  const appDialog = useAppDialog()
  const appUpdate = useAppUpdate()
  useEffect(() => {
    document.documentElement.dataset.density = density
  }, [density])

  useEffect(() => {
    const apply = () => {
      const dark = resolveDark(theme)
      document.documentElement.dataset.theme = dark ? 'dark' : 'light'
      document.documentElement.classList.toggle('dark', dark)
    }
    apply()
    if (theme !== 'system') return undefined
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [theme])

  const toggleSidebarCollapsed = () => {
    setSidebarCollapsed(!sidebarCollapsed)
  }

  const refreshPluginStats = useCallback(async () => {
    try {
      const data = await apiJson<{
        tools: Array<{ enabled: boolean }>
      }>('/api/plugins')
      setPluginStats({
        enabled: data.tools.filter((tool) => tool.enabled).length,
        total: data.tools.length,
      })
    } catch {
      // Keep the rest of the application usable if the plugin catalog is unavailable.
    }
  }, [])

  const notify = useCallback((message: string, tone: ToastTone = 'success') => {
    setToast({ message, tone })
    window.clearTimeout(toastTimer.current)
    toastTimer.current = window.setTimeout(() => setToast(null), 2800)
  }, [])

  useEffect(() => () => window.clearTimeout(toastTimer.current), [])

  const showSystemNotification = useCallback(
    (title: string, body: string, { force = false }: { force?: boolean } = {}) => {
      if (!notificationSettings.browser?.enabled) return
      if (!force && document.visibilityState === 'visible' && document.hasFocus()) return
      if (window.vesperDesktop?.showNotification) {
        void window.vesperDesktop.showNotification({ title, body }).catch(() => {})
        return
      }
      void showBrowserSystemNotification({
        title,
        body,
        tag: `vesper-${title}`,
        url: window.location.href,
      }).catch(() => {})
    },
    [notificationSettings.browser?.enabled],
  )

  const browserNotify = useCallback(
    (event: string, data: unknown, options?: { force?: boolean }) => {
      const template = notificationSettings.templates?.find((item) => item.id === event)
      const content = template?.channels?.browser?.content
      if (!template?.enabled || !content) return
      showSystemNotification(
        template.name,
        renderNotificationContent(content, (data || {}) as Record<string, unknown>),
        options,
      )
    },
    [notificationSettings.templates, showSystemNotification],
  )

  const registerPrimaryAction = useCallback(
    (action: () => void) => {
      return primaryActions.register(action)
    },
    [primaryActions],
  )

  const invokePrimaryAction = useCallback(() => {
    primaryActions.invoke()
  }, [primaryActions])

  const registerWorkflowActions = useCallback((actions: WorkflowActions) => {
    setWorkflowActions(actions)
    return () => setWorkflowActions((current) => (current === actions ? null : current))
  }, [])

  const navigate = useCallback(
    (next: string, options?: NavigateOptions) => {
      if (!PAGE_IDS.has(next)) return
      routerNavigate(pagePath(next), options)
      setQuery('')
    },
    [routerNavigate],
  )

  const openUpdateSettings = useCallback(() => {
    setConfigSection('updates')
    navigate('config')
  }, [navigate])

  const openNotificationSettings = useCallback(() => {
    setConfigSection('notifications')
    navigate('config')
  }, [navigate])

  const useAsset = useCallback(
    (asset: ChatAttachment) => {
      const targetSessionId = localStorage.getItem(STORAGE_KEYS.activeSession) || ''
      setPendingAsset({ asset, targetSessionId })
      if (targetSessionId) requestSessionSelection(targetSessionId)
      navigate('chat')
    },
    [navigate],
  )

  const handlePrimary = useCallback(() => {
    if (page === 'config' && configSection !== 'models') return
    if (
      [
        'chat',
        'config',
        'assets',
        'plugins',
        'channels',
        'schedules',
        'memory',
        'mcp',
        'skills',
        'workflowCreate',
      ].includes(page)
    )
      invokePrimaryAction()
    else if (page === 'chatHistory') {
      navigate('chat')
      invokePrimaryAction()
    } else if (page === 'workflows') navigate('workflowCreate')
    else setModal(page)
  }, [configSection, invokePrimaryAction, navigate, page])

  useEffect(() => {
    const focusSearch = () => {
      if (page === 'chat') {
        navigate('chatHistory')
        requestAnimationFrame(() => requestAnimationFrame(() => searchInputRef.current?.focus()))
      } else searchInputRef.current?.focus()
    }
    const onKeyDown = (event: KeyboardEvent) => {
      const modifier = event.metaKey || event.ctrlKey
      if (modifier && event.key.toLowerCase() === 'k' && !appDialog.dialog && !modal) {
        event.preventDefault()
        setPaletteOpen(true)
      } else if (modifier && event.key.toLowerCase() === 'n' && !isEditableTarget(event.target)) {
        event.preventDefault()
        handlePrimary()
      } else if (
        event.key === '/' &&
        !modifier &&
        !event.altKey &&
        !isEditableTarget(event.target)
      ) {
        event.preventDefault()
        focusSearch()
      } else if (event.key === 'Escape' && !appDialog.dialog) {
        if (paletteOpen) setPaletteOpen(false)
        else if (modal) setModal(null)
        else if (mobileNav) setMobileNav(false)
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [appDialog.dialog, handlePrimary, mobileNav, modal, navigate, page, paletteOpen])

  useEffect(() => {
    let active = true
    apiJson<AppConfig>('/api/config')
      .then((config) => {
        if (!active) return
        if (!hasUsableProvider(config)) {
          primaryActions.clear()
          primaryActions.invoke()
          navigate('config', { replace: true })
        }
      })
      .catch(() => {})
      .finally(() => active && setStartupReady(true))
    return () => {
      active = false
    }
  }, [navigate, primaryActions])

  useEffect(() => {
    refreshPluginStats()
  }, [refreshPluginStats])

  useEffect(() => {
    apiJson<NotificationSettingsData>('/api/settings/notifications')
      .then(setNotificationSettings)
      .catch(() => {})
  }, [])

  useEffect(() => {
    let active = true
    const poll = async () => {
      try {
        const result = await apiJson<{
          events?: Array<{ title: string; body: string }>
          latestId?: string
        }>(
          `/api/settings/notifications/browser/events?after=${encodeURIComponent(browserEventCursor.current)}`,
        )
        if (!active) return
        for (const event of result.events || [])
          showSystemNotification(event.title, event.body, { force: true })
        browserEventCursor.current = result.latestId || browserEventCursor.current
      } catch {}
    }
    poll()
    const timer = window.setInterval(poll, 3000)
    return () => {
      active = false
      window.clearInterval(timer)
    }
  }, [showSystemNotification])

  const activeMeta: readonly [string, string] =
    page === 'chat'
      ? [
          t('common:app.sessions'),
          t('common:app.dragTabsOrSplitChatsToTheLeftOrRightFromTheSessionList'),
        ]
      : pageMeta[page] || [t('common:app.sessions'), '']

  const routeContext: AppRouteContext = {
    query,
    navigate,
    notify,
    browserNotify,
    registerPrimaryAction,
    pendingAsset,
    onAssetConsumed: () => setPendingAsset(null),
    onUseAsset: useAsset,
    requestText: appDialog.prompt,
    requestConfirm: appDialog.confirm,
    openNotificationSettings,
    configSection,
    setConfigSection,
    setNotificationSettings,
    appUpdate,
    setPluginStats,
    registerWorkflowActions,
  }

  if (!startupReady)
    return (
      <div className="app-startup">
        <BrandLogo size={30} className="startup-logo" />
        <strong>{t('common:app.wakingVesper')}</strong>
      </div>
    )

  return (
    <div className="app-shell">
      <WebPreviewProvider />
      <SidebarProvider
        className="app-body"
        open={!sidebarCollapsed}
        onOpenChange={(open) => setSidebarCollapsed(!open)}
        openMobile={mobileNav}
        onOpenMobileChange={setMobileNav}
      >
        <AppSidebar
          page={page}
          navigation={navigation}
          navigate={navigate}
          collapsed={sidebarCollapsed}
          onToggleCollapse={toggleSidebarCollapsed}
          update={appUpdate}
          onOpenUpdates={openUpdateSettings}
        />
        <SidebarInset className="main-surface">
          <PageHeader
            meta={activeMeta}
            page={page}
            query={query}
            setQuery={setQuery}
            configSection={configSection}
            onMenu={() => setMobileNav(true)}
            onPrimary={handlePrimary}
            searchInputRef={searchInputRef}
            theme={theme}
            onCycleTheme={cycleTheme}
            workflowActions={workflowActions}
            desktopPlatform={window.vesperDesktop?.platform || ''}
          />
          <div className={`page-content page-${page}`} key={page}>
            <Outlet context={routeContext} />
          </div>
        </SidebarInset>
      </SidebarProvider>
      <StatusBar page={page} pluginStats={pluginStats} />
      {toast && <Toast message={toast.message} tone={toast.tone} />}
      <AppDialog dialog={appDialog.dialog} onClose={appDialog.close} onFinish={appDialog.finish} />
      {paletteOpen && (
        <CommandPalette
          navigation={navigation}
          onClose={() => setPaletteOpen(false)}
          onNavigate={navigate}
          onOpenSession={(id) => {
            requestSessionSelection(id)
            navigate('chat')
          }}
          onNewChat={() => {
            navigate('chat')
            requestAnimationFrame(() => requestAnimationFrame(invokePrimaryAction))
          }}
        />
      )}
      {modal && <QuickCreate type={modal} close={() => setModal(null)} notify={notify} />}
    </div>
  )
}

export default App
