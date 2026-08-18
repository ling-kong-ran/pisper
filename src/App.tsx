// 应用外壳：持有全站共享状态（当前页/搜索词/活动会话/Toast/对话框/通知
// 设置/更新控制器/工作流动作），组装侧边栏 + 页头 + 内容 Outlet + 状态栏，
// 并通过 Outlet 上下文向各页面注入公共能力。启动时探测是否已配置可用
// Provider，未配置则引导用户进设置页；同时提供全局快捷键（Cmd+K 命令面板、
// Cmd+N 主操作、` 终端、/ 搜索、Esc 逐层关闭）与浏览器通知轮询。
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Outlet, useLocation, useNavigate, type NavigateOptions } from 'react-router-dom'
import { createPrimaryActionRegistry } from '@/app/primary-action'
import type { AppRouteContext } from '@/app/route-context'
import { STORAGE_KEYS } from '@/app/storage'
import { getNavigation, getPageMeta } from '@/app/navigation'
import { PAGE_IDS, pageFromPath, pagePath } from '@/app/routes'
import {
  CONFIG_SECTIONS,
  SETTINGS_PAGES,
  type SettingsDestination,
} from '@/app/settings-navigation'
import { useI18n } from '@/app/use-i18n'
import { BrandLogo } from '@/components/BrandLogo'
import { WebPreviewProvider } from '@/components/WebPreviewProvider'
import { AppSidebar } from '@/components/layout/AppSidebar'
import { AppDialog } from '@/components/layout/AppDialog'
import { StatusBar } from '@/components/layout/StatusBar'
import { AppToast, ToastProvider, ToastViewport, type ToastTone } from '@/components/ui/toast'
import { chatApi } from '@/features/chat/chat-api'
import {
  ACTIVE_SESSION_CHANGED_EVENT,
  COMMAND_PALETTE_REQUESTED_EVENT,
  requestSessionSelection,
} from '@/features/chat/events'
import { WebDesktopPet } from '@/features/desktop-pet/WebDesktopPet'
import { apiJson } from '@/lib/api'
import { showBrowserSystemNotification } from '@/lib/browser-notifications'
import { useAppDialog } from '@/hooks/useAppDialog'
import { useAppUpdate } from '@/features/updates/useAppUpdate'
import { SidebarInset, SidebarProvider } from '@/components/ui/sidebar'
import { useUiStore, type ThemeMode } from '@/stores/ui-store'
import { readStoredTerminalPanel } from '@/features/terminal/terminal-state'
import type { ChatAttachment, PendingAsset } from '@/types/chat'
import type { NotificationSettingsData } from '@/types/notifications'
import type { WorkflowActions } from '@/types/workflow'

const CommandPalette = lazy(() =>
  import('@/components/layout/AppOverlays').then((module) => ({
    default: module.CommandPalette,
  })),
)
const QuickCreate = lazy(() =>
  import('@/components/layout/AppOverlays').then((module) => ({ default: module.QuickCreate })),
)
const PageHeader = lazy(() =>
  import('@/components/layout/PageHeader').then((module) => ({ default: module.PageHeader })),
)
const TerminalPanel = lazy(() =>
  import('@/features/terminal/TerminalPanel').then((module) => ({
    default: module.TerminalPanel,
  })),
)
type ProviderConfig = {
  configured: boolean
  enabled: boolean
  models: Array<{ kind: string }>
}

type AppConfig = {
  providers?: ProviderConfig[]
}

type ToastState = {
  id: number
  message: string
  tone: ToastTone
}

type PluginStats = {
  enabled: number
  total: number
}

// 是否存在可用 Provider：已配置 + 启用 + 含 chat 类模型的 Provider 至少一个。
// 启动时据此决定是否引导用户先去配置页。
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

// 渲染通知模板：把 {{a.b}} 占位符替换为事件数据中的嵌套字段值，
// 缺字段时保留原占位符不静默消失，便于用户发现模板配置问题。
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

// 按时间判断暗色：18:00–次日 8:00 为暗色，用于 system 主题的时间驱动。
function isAutoDarkByTime() {
  const hour = new Date().getHours()
  return hour >= 18 || hour < 8
}

// 主题模式 → 是否暗色：dark 恒暗；system 交给时间判断。
function resolveDark(mode: ThemeMode) {
  return mode === 'dark' || (mode === 'system' && isAutoDarkByTime())
}

// 是否可编辑目标（输入/文本域/选择/可编辑区域）：
// 全局快捷键在这些元素上按下时不应被拦截（保留原生输入体验）。
function isEditableTarget(target: EventTarget | null) {
  return (
    target instanceof HTMLElement &&
    (target.matches('input, textarea, select') || target.isContentEditable)
  )
}

// 安全解码路径段；非法编码返回空串而非抛错中断渲染。
function decodePathSegment(value: string) {
  try {
    return decodeURIComponent(value)
  } catch {
    return ''
  }
}

function App() {
  const { t } = useI18n()
  const location = useLocation()
  const routerNavigate = useNavigate()
  const navigation = useMemo(() => getNavigation(t), [t])
  const pageMeta = useMemo(() => getPageMeta(t), [t])
  const page = pageFromPath(location.pathname) || 'chat'
  const startupPageRef = useRef(page)
  const lastWorkbenchPathRef = useRef(SETTINGS_PAGES.has(page) ? '/chat' : location.pathname)
  const [query, setQuery] = useState('')
  const [activeSessionId, setActiveSessionId] = useState(
    () => localStorage.getItem(STORAGE_KEYS.activeSession) || '',
  )
  const [mobileNav, setMobileNav] = useState(false)
  const [paletteOpen, setPaletteOpen] = useState(false)
  const sidebarCollapsed = useUiStore((state) => state.sidebarCollapsed)
  const setSidebarCollapsed = useUiStore((state) => state.setSidebarCollapsed)
  const theme = useUiStore((state) => state.theme)
  const cycleTheme = useUiStore((state) => state.cycleTheme)
  const density = useUiStore((state) => state.density)
  const [toast, setToast] = useState<ToastState | null>(null)
  const [modal, setModal] = useState<string | null>(null)
  const requestedConfigSection =
    page === 'config' ? decodePathSegment(location.pathname.split('/')[2] || 'models') : 'models'
  const configSection = CONFIG_SECTIONS.has(requestedConfigSection)
    ? requestedConfigSection
    : 'models'
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
  const terminalLabels = useMemo(
    () => ({
      terminal: t('terminal:terminalPanel.terminal'),
      resizeTerminal: t('terminal:terminalPanel.resizeTerminal'),
      hideTerminal: t('terminal:terminalPanel.hideTerminal'),
      showTerminal: t('terminal:terminalPanel.showTerminal'),
      newTerminal: t('terminal:terminalPanel.newTerminal'),
      closeTerminal: t('terminal:terminalPanel.closeTerminal'),
      maximizeTerminal: t('terminal:terminalPanel.maximizeTerminal'),
      openTerminal: t('terminal:terminalPanel.openTerminal'),
      usesActiveSessionWorkspace: t('terminal:terminalPanel.usesActiveSessionWorkspace'),
      orphanedTerminal: t('terminal:terminalPanel.orphanedTerminal'),
      starting: t('terminal:terminalPanel.starting'),
      processExited: (code: number | null) =>
        t('terminal:terminalPanel.processExited', { code: code ?? '-' }),
    }),
    [t],
  )
  const [terminalOpen, setTerminalOpen] = useState(() => Boolean(readStoredTerminalPanel().open))
  const [terminalHeight, setTerminalHeight] = useState(() =>
    Math.max(180, Math.min(640, Number(readStoredTerminalPanel().height) || 300)),
  )
  const browserEventCursor = useRef('')
  const [primaryActions] = useState(createPrimaryActionRegistry)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const toastSequence = useRef(0)
  const appDialog = useAppDialog()
  const appUpdate = useAppUpdate()
  useEffect(() => {
    document.documentElement.dataset.density = density
  }, [density])

  useEffect(() => {
    localStorage.setItem(
      STORAGE_KEYS.terminalPanel,
      JSON.stringify({ open: terminalOpen, height: terminalHeight }),
    )
  }, [terminalHeight, terminalOpen])

  useEffect(() => {
    const apply = () => {
      const dark = resolveDark(theme)
      document.documentElement.dataset.theme = dark ? 'dark' : 'light'
      document.documentElement.classList.toggle('dark', dark)
    }
    apply()
    if (theme !== 'system') return undefined
    // 时间驱动：每分钟重新评估，以在 18:00 / 8:00 边界自动切换主题
    const timer = window.setInterval(apply, 60_000)
    return () => window.clearInterval(timer)
  }, [theme])

  const toggleSidebarCollapsed = () => {
    setSidebarCollapsed(!sidebarCollapsed)
  }

  // 刷新插件启用统计：拉取插件工具列表，统计启用数/总数供状态栏展示；
  // 目录不可用时静默失败，不影响其余功能。
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

  // 应用内 Toast：每次递增 id 保证连续提示正确触发切换动画。
  const notify = useCallback((message: string, tone: ToastTone = 'success') => {
    toastSequence.current += 1
    setToast({ id: toastSequence.current, message, tone })
  }, [])

  useEffect(() => {
    const syncActiveSession = (event: Event) => {
      const id = (event as CustomEvent<{ id?: string }>).detail?.id
      setActiveSessionId(id ?? localStorage.getItem(STORAGE_KEYS.activeSession) ?? '')
    }
    window.addEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActiveSession)
    return () => window.removeEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActiveSession)
  }, [])

  // 系统通知（桌面/浏览器）：已开启浏览器通知开关才发；
  // 前台聚焦时静默（不打扰），force 强制忽略该规则；桌面桥接优先。
  const showSystemNotification = useCallback(
    (title: string, body: string, { force = false }: { force?: boolean } = {}) => {
      if (!notificationSettings.browser?.enabled) return
      if (!force && document.visibilityState === 'visible' && document.hasFocus()) return
      if (window.pisperDesktop?.showNotification) {
        void window.pisperDesktop.showNotification({ title, body }).catch(() => {})
        return
      }
      void showBrowserSystemNotification({
        title,
        body,
        tag: `pisper-${title}`,
        url: window.location.href,
      }).catch(() => {})
    },
    [notificationSettings.browser?.enabled],
  )

  // 按事件模板发浏览器通知：模板启用且有内容才发，
  // 渲染时替换 {{变量}} 占位符。
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

  // 页面注册主操作（透传注册表）。
  const registerPrimaryAction = useCallback(
    (action: () => void) => {
      return primaryActions.register(action)
    },
    [primaryActions],
  )

  // 触发页面主操作（如 Cmd+N 快捷键）。
  const invokePrimaryAction = useCallback(() => {
    primaryActions.invoke()
  }, [primaryActions])

  // 注册工作流编辑器的动作（保存/运行），供页头按钮触发；
  // 返回注销函数，新动作注册时若仍指向旧值则清除。
  const registerWorkflowActions = useCallback((actions: WorkflowActions) => {
    setWorkflowActions(actions)
    return () => setWorkflowActions((current) => (current === actions ? null : current))
  }, [])

  // 页面导航：只接受已知页面 id（防硬编码跳转），跳转同时清空搜索词。
  const navigate = useCallback(
    (next: string, options?: NavigateOptions) => {
      if (!PAGE_IDS.has(next)) return
      routerNavigate(pagePath(next), options)
      setQuery('')
    },
    [routerNavigate],
  )

  useEffect(() => {
    if (page === 'config' && requestedConfigSection !== configSection) {
      routerNavigate(`/config/${configSection}`, { replace: true })
    }
  }, [configSection, page, requestedConfigSection, routerNavigate])

  // 切换配置分区：未知分区回退到 models，同步路由并清空搜索词。
  const setConfigSection = useCallback(
    (section: string) => {
      const nextSection = CONFIG_SECTIONS.has(section) ? section : 'models'
      routerNavigate(`/config/${nextSection}`)
      setQuery('')
    },
    [routerNavigate],
  )

  const openUpdateSettings = useCallback(() => setConfigSection('updates'), [setConfigSection])

  const openNotificationSettings = useCallback(
    () => setConfigSection('notifications'),
    [setConfigSection],
  )

  // 设置导航统一入口：配置分区 vs 独立设置页两种跳转。
  const navigateSettings = useCallback(
    (destination: SettingsDestination) => {
      if (destination.type === 'config') setConfigSection(destination.id)
      else navigate(destination.id)
    },
    [navigate, setConfigSection],
  )

  useEffect(() => {
    if (!SETTINGS_PAGES.has(page)) lastWorkbenchPathRef.current = location.pathname
  }, [location.pathname, page])

  // 退出设置页：回到进入设置前的最后一个工作台页面。
  const exitSettings = useCallback(() => {
    routerNavigate(lastWorkbenchPathRef.current)
    setQuery('')
  }, [routerNavigate])

  const providerScanStarted = useRef(false)
  useEffect(() => {
    if (!startupReady || providerScanStarted.current) return
    providerScanStarted.current = true
    void (async () => {
      try {
        const data = await apiJson<{
          providers?: Array<{
            importable?: boolean
            imported?: boolean
            conflict?: boolean
          }>
        }>('/api/providers/discovery')
        const count = (data.providers || []).filter(
          (provider) => provider.importable && !provider.imported && !provider.conflict,
        ).length
        if (count <= 0) return
        const approved = await appDialog.confirm({
          title: t('common:app.importableProvidersTitle'),
          message: t('common:app.importableProvidersMessage', { count }),
          confirmLabel: t('common:app.openSettings'),
          tone: 'primary',
        })
        if (approved) {
          setConfigSection('models')
        }
      } catch {
        // 本地配置扫描失败时静默忽略，不影响启动
      }
    })()
  }, [appDialog, setConfigSection, startupReady, t])

  // 解析会话工作目录：从会话列表查 cwd（供终端绑定工作区）。
  const resolveSessionCwd = useCallback(async (sessionId: string) => {
    if (!sessionId) return ''
    const data = await apiJson<{ sessions?: Array<{ id: string; cwd?: string }> }>('/api/sessions')
    return data.sessions?.find((session) => session.id === sessionId)?.cwd || ''
  }, [])

  // 使用资源：把资源（附件）标记为待投递到活动会话，跳转聊天页，
  // 若已存在活动会话则触发选中事件让输入框接收。
  const useAsset = useCallback(
    (asset: ChatAttachment) => {
      const targetSessionId = localStorage.getItem(STORAGE_KEYS.activeSession) || ''
      setPendingAsset({ asset, targetSessionId })
      if (targetSessionId) requestSessionSelection(targetSessionId)
      navigate('chat')
    },
    [navigate],
  )

  // 主操作分发：按当前页面决定触发哪个动作（新建会话/新建工作流/快捷创建）；
  // 配置页非 models 分区时不响应，避免误触发。
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
    const openCommandPalette = () => {
      if (!appDialog.dialog && !modal) setPaletteOpen(true)
    }
    const onKeyDown = (event: KeyboardEvent) => {
      const modifier = event.metaKey || event.ctrlKey
      if (modifier && event.key.toLowerCase() === 'k' && !appDialog.dialog && !modal) {
        event.preventDefault()
        openCommandPalette()
      } else if (
        modifier &&
        event.key === '`' &&
        window.pisperDesktop?.terminalProfiles &&
        !(event.target instanceof HTMLElement && event.target.closest('.terminal-panel'))
      ) {
        event.preventDefault()
        setTerminalOpen((value) => !value)
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
    window.addEventListener(COMMAND_PALETTE_REQUESTED_EVENT, openCommandPalette)
    return () => {
      window.removeEventListener('keydown', onKeyDown)
      window.removeEventListener(COMMAND_PALETTE_REQUESTED_EVENT, openCommandPalette)
    }
  }, [appDialog.dialog, handlePrimary, mobileNav, modal, navigate, page, paletteOpen])

  useEffect(() => {
    let active = true
    apiJson<AppConfig>('/api/config')
      .then((config) => {
        if (!active) return
        if (!hasUsableProvider(config)) {
          primaryActions.clear()
          primaryActions.invoke()
          if (!SETTINGS_PAGES.has(startupPageRef.current)) navigate('config', { replace: true })
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
          t('common:app.dragTabsOrSplitChatsInAnyDirectionFromTheSessionList'),
        ]
      : pageMeta[page] || [t('common:app.sessions'), '']

  const routeContext: AppRouteContext = {
    query,
    activeSessionId,
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
      <div className="app-startup dark:bg-[var(--bg)] dark:text-[var(--text)] flex w-full min-h-[100vh] items-center justify-center gap-[10px] bg-[var(--bg)] text-[var(--text-muted)] text-[13px]">
        <BrandLogo size={30} className="startup-logo [.app-startup_&]:mr-[2px]" />
        <strong>{t('common:app.wakingPisper')}</strong>
      </div>
    )

  return (
    <ToastProvider duration={2800} swipeDirection="right">
      <div className="app-shell dark:bg-[var(--bg)] dark:text-[var(--text)] max-[900px]:min-h-[100dvh] max-[900px]:h-auto max-[900px]:overflow-visible flex w-full h-full min-h-[600px] flex-col overflow-hidden bg-[var(--bg)]">
        <WebPreviewProvider />
        <WebDesktopPet />
        <SidebarProvider
          className="app-body max-[900px]:h-[100dvh] max-[900px]:min-h-[620px] max-[900px]:flex-none max-[650px]:h-[100dvh] max-[650px]:min-h-0 max-[650px]:flex-none flex min-h-0 flex-1"
          open={!sidebarCollapsed}
          onOpenChange={(open) => setSidebarCollapsed(!open)}
          openMobile={mobileNav}
          onOpenMobileChange={setMobileNav}
        >
          <AppSidebar
            page={page}
            configSection={configSection}
            navigation={navigation}
            navigate={navigate}
            navigateSettings={navigateSettings}
            onExitSettings={exitSettings}
            collapsed={sidebarCollapsed}
            onToggleCollapse={toggleSidebarCollapsed}
            update={appUpdate}
            onOpenUpdates={openUpdateSettings}
          />
          <SidebarInset className="main-surface before:[content:''] before:absolute before:z-[-1] before:inset-[0_0_auto] before:h-[220px] before:bg-[linear-gradient(180deg,var(--main-glow-start)_0%,var(--main-glow-end)_100%)] before:pointer-events-none dark:bg-[var(--main-surface-bg)] dark:before:bg-[linear-gradient(180deg,var(--main-glow-start)_0%,var(--main-glow-end)_100%)] dark:[background-image:radial-gradient(rgba(255,_255,_255,_.05)_1px,_transparent_1.3px),_radial-gradient(rgba(255,_255,_255,_.025)_1px,_transparent_1.3px)] dark:[background-size:26px_26px,_41px_41px] dark:[background-position:0_0,_13px_20px] relative flex min-w-0 flex-1 h-full flex-col overflow-hidden [border-left:0] bg-[var(--main-surface-bg)] shadow-[inset_0_1px_0_var(--main-surface-inset),_0_20px_60px_-28px_var(--main-surface-shadow)]">
            <Suspense fallback={null}>
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
                desktopPlatform={window.pisperDesktop?.platform || ''}
                terminalOpen={terminalOpen}
                onToggleTerminal={() => setTerminalOpen((value) => !value)}
              />
            </Suspense>
            <div
              className={`page-content [&.page-chat]:flex [&.page-chat]:overflow-hidden [&.page-chat]:p-[0_18px_14px] [&.page-workflowCreate]:[padding-inline:24px] min-[651px]:[[data-density='compact']_&]:pb-[14px] max-[900px]:p-[0_16px_18px] max-[650px]:overflow-x-hidden max-[650px]:[&.page-chat]:p-[0_8px_8px] max-[650px]:[&.page-workflowCreate]:overflow-auto flex-1 min-h-0 overflow-auto [padding:0_max(24px,_calc((100%_-_1320px)_/_2))_24px] [scrollbar-color:var(--control-muted)_transparent] [animation:page-in_var(--d2)_var(--ease-out)] page-${page}`}
              key={page}
            >
              <Outlet context={routeContext} />
            </div>
            {window.pisperDesktop?.terminalProfiles && (
              <Suspense fallback={null}>
                <TerminalPanel
                  open={terminalOpen}
                  height={terminalHeight}
                  labels={terminalLabels}
                  activeSessionId={activeSessionId}
                  resolveSessionCwd={resolveSessionCwd}
                  onOpenChange={setTerminalOpen}
                  onHeightChange={setTerminalHeight}
                />
              </Suspense>
            )}
          </SidebarInset>
        </SidebarProvider>
        <StatusBar page={page} pluginStats={pluginStats} />
        {toast && (
          <AppToast
            key={toast.id}
            open
            message={toast.message}
            tone={toast.tone}
            closeLabel={t('common:ui.closeDialog')}
            onOpenChange={(open) => !open && setToast(null)}
          />
        )}
        <ToastViewport />
        <AppDialog
          dialog={appDialog.dialog}
          onClose={appDialog.close}
          onFinish={appDialog.finish}
        />
        <Suspense fallback={null}>
          {paletteOpen && (
            <CommandPalette
              navigation={navigation}
              onClose={() => setPaletteOpen(false)}
              onNavigate={navigate}
              onOpenSession={async (id, targetEntryId, targetActive) => {
                try {
                  if (targetEntryId && !targetActive) {
                    await chatApi.navigateSessionTreeTarget(id, targetEntryId)
                  }
                  requestSessionSelection(id, 'open', targetEntryId)
                  navigate('chat')
                } catch (error) {
                  notify(t('navigation:appOverlays.openLabeledTurnFailed'), 'error')
                  throw error
                }
              }}
              onNewChat={() => {
                navigate('chat')
                requestAnimationFrame(() => requestAnimationFrame(invokePrimaryAction))
              }}
            />
          )}
          {modal && <QuickCreate type={modal} close={() => setModal(null)} notify={notify} />}
        </Suspense>
      </div>
    </ToastProvider>
  )
}

export default App
