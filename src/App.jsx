import { Suspense, createContext, lazy, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useContext } from 'react'
import { Navigate, Route, Routes, useLocation, useNavigate } from 'react-router-dom'
import { createPortal } from 'react-dom'
import { DockviewReact } from 'dockview-react'
import {
  AlertTriangle,
  ArrowDown,
  Bot,
  Check,
  ChevronRight,
  Download,
  ExternalLink,
  Eye,
  File,
  FolderOpen,
  Gauge,
  Link2,
  Menu,
  MessageSquare,
  Monitor,
  Moon,
  MoreHorizontal,
  PanelLeft,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRight,
  Paperclip,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Rocket,
  Save,
  Search,
  Send,
  Shield,
  ShieldCheck,
  ShieldOff,
  Square,
  Sun,
  Target,
  X,
} from 'lucide-react'
import { APP_NAME } from './app/brand.js'
import { createPrimaryActionRegistry } from './app/primary-action.js'
import { STORAGE_KEYS } from './app/storage.js'
import { getNavigation, getPageMeta } from './app/navigation.jsx'
import { PAGE_IDS, PAGE_PATHS, pageFromPath, pagePath } from './app/routes.js'
import { useI18n } from './app/use-i18n.js'
import { BrandLogo } from './components/BrandLogo.jsx'
import { WorkspacePicker } from './components/WorkspacePicker.jsx'
import { AppDialog, InputLabel, Panel, SelectLabel, Toast, Toggle } from './components/ui.jsx'
import { useAttachmentSelection } from './features/chat/attachments.js'
import { ChatHistoryPage } from './features/chat/ChatHistoryPage.jsx'
import { ACTIVE_SESSION_CHANGED_EVENT, SESSION_SELECTED_EVENT, SESSIONS_UPDATED_EVENT, announceActiveSession, announceSessionsUpdated, consumeSessionSelectionRequest, requestSessionSelection } from './features/chat/events.js'
import { FocusChatMessage } from './features/chat/ChatMessage.jsx'
import { mergeSessionLists } from './features/chat/session-list.js'
import { createDockLayoutEnvelope, initialDockSessionIds, panelIdForSession, parseDockLayoutEnvelope, sessionIdFromPanel } from './features/chat/dock-layout.js'
import { pushCurrentActivity, settleToolCalls, taskListChanges } from './features/chat/run-activity.js'
import { useAutoScroll } from './hooks/useAutoScroll.js'
import { usePagePrimaryAction } from './hooks/usePagePrimaryAction.js'
import { apiJson, applyTextPatch, consumeEventStream } from './lib/api.js'
import { showBrowserSystemNotification } from './lib/browser-notifications.js'
import { applySessionUpdate, DEFAULT_SESSION_STATE, insertInteractiveUserMessage, resolveQueuedInputs } from './lib/session-state.js'
import { createStreamingTextScheduler, createToolUpdateScheduler, createTypewriterDisplay } from './lib/streaming-ui.js'
import { formatFileSize, formatTokenCount, relativeTime, workspaceName } from './lib/format.js'
import { useAppDialog } from './hooks/useAppDialog.js'
import { useAppUpdate } from './features/updates/useAppUpdate.js'

const PluginsPage = lazy(() => import('./features/plugins/PluginsPage.jsx').then((module) => ({ default: module.PluginsPage })))
const ChannelsPage = lazy(() => import('./features/channels/ChannelsPage.jsx').then((module) => ({ default: module.ChannelsPage })))
const ConfigPage = lazy(() => import('./features/config/ConfigPage.jsx').then((module) => ({ default: module.ConfigPage })))
const MemoryPage = lazy(() => import('./features/memory/MemoryPage.jsx').then((module) => ({ default: module.MemoryPage })))
const SchedulesPage = lazy(() => import('./features/schedules/SchedulesPage.jsx').then((module) => ({ default: module.SchedulesPage })))
const AssetsPage = lazy(() => import('./features/assets/AssetsPage.jsx').then((module) => ({ default: module.AssetsPage })))
const McpPage = lazy(() => import('./features/workflows/PreviewPages.jsx').then((module) => ({ default: module.McpPage })))
const SkillsPage = lazy(() => import('./features/skills/SkillsPage.jsx').then((module) => ({ default: module.SkillsPage })))
const WorkflowsPage = lazy(() => import('./features/workflows/WorkflowsPage.jsx').then((module) => ({ default: module.WorkflowsPage })))
const WorkflowBuilder = lazy(() => import('./features/workflows/WorkflowsPage.jsx').then((module) => ({ default: module.WorkflowBuilder })))
const EMPTY_LIST = []
const MAX_LIVE_THINKING_CHARS = 6_000
const USAGE_UPDATED_EVENT = 'vesper:usage-updated'
const FOCUS_MESSAGE_PAGE_SIZE = 40

function latestPageState(current, data) {
  const incomingStart = Number(data.pageInfo?.start) || 0
  const currentStart = Number.isInteger(current.messageStart) ? current.messageStart : null
  const preservePrefix = currentStart != null && currentStart <= incomingStart
  const prefixLength = preservePrefix ? Math.max(0, incomingStart - currentStart) : 0
  const messageStart = preservePrefix ? currentStart : incomingStart
  return {
    messages: preservePrefix ? [...current.messages.slice(0, prefixLength), ...data.messages] : data.messages,
    messageStart,
    hasOlder: messageStart > 0,
    olderCursor: messageStart > 0 ? String(messageStart) : null,
  }
}

function hasUsableProvider(config) {
  return Boolean(config?.providers?.some((provider) => provider.configured && provider.enabled && provider.models.some((model) => model.kind === 'chat')))
}

function readStoredArray(key) {
  try {
    const value = JSON.parse(localStorage.getItem(key) || '[]')
    return Array.isArray(value) ? value.filter((item) => typeof item === 'string') : []
  } catch {
    return []
  }
}

function renderNotificationContent(content, data) {
  return String(content || '').replace(/\{\{\s*([\w.]+)\s*\}\}/g, (_match, path) => {
    const value = path.split('.').reduce((current, key) => current?.[key], data)
    return value == null ? `{{${path}}}` : String(value)
  })
}

const THEME_SEQUENCE = ['system', 'light', 'dark']
const THEME_META = {
  system: Monitor,
  light: Sun,
  dark: Moon,
}

function resolveDark(mode) {
  return mode === 'dark' || (mode === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)
}

function isEditableTarget(target) {
  return target instanceof HTMLElement && (target.matches('input, textarea, select') || target.isContentEditable)
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
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => localStorage.getItem(STORAGE_KEYS.sidebarCollapsed) === '1')
  const [toast, setToast] = useState(null)
  const [modal, setModal] = useState(null)
  const [configSection, setConfigSection] = useState('models')
  const [pendingAsset, setPendingAsset] = useState(null)
  const [pluginStats, setPluginStats] = useState(null)
  const [startupReady, setStartupReady] = useState(false)
  const [notificationSettings, setNotificationSettings] = useState({ browser: { enabled: false }, templates: [] })
  const [workflowActions, setWorkflowActions] = useState(null)
  const browserEventCursor = useRef('')
  const [primaryActions] = useState(createPrimaryActionRegistry)
  const searchInputRef = useRef(null)
  const toastTimer = useRef(null)
  const appDialog = useAppDialog()
  const appUpdate = useAppUpdate()
  const [theme, setTheme] = useState(() => {
    const stored = localStorage.getItem(STORAGE_KEYS.theme)
    return THEME_SEQUENCE.includes(stored) ? stored : 'system'
  })

  useLayoutEffect(() => {
    document.documentElement.dataset.density = localStorage.getItem(STORAGE_KEYS.density) === 'compact' ? 'compact' : 'comfortable'
  }, [])

  useEffect(() => {
    const apply = () => { document.documentElement.dataset.theme = resolveDark(theme) ? 'dark' : 'light' }
    apply()
    if (theme === 'system') localStorage.removeItem(STORAGE_KEYS.theme)
    else localStorage.setItem(STORAGE_KEYS.theme, theme)
    if (theme !== 'system') return undefined
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [theme])

  const cycleTheme = () => setTheme((current) => THEME_SEQUENCE[(THEME_SEQUENCE.indexOf(current) + 1) % THEME_SEQUENCE.length])

  const toggleSidebarCollapsed = () => setSidebarCollapsed((current) => {
    const next = !current
    localStorage.setItem(STORAGE_KEYS.sidebarCollapsed, next ? '1' : '0')
    return next
  })

  const refreshPluginStats = useCallback(async () => {
    try {
      const data = await apiJson('/api/plugins')
      setPluginStats({
        enabled: data.tools.filter((tool) => tool.enabled).length,
        total: data.tools.length,
      })
    } catch {
      // Keep the rest of the application usable if the plugin catalog is unavailable.
    }
  }, [])

  const notify = useCallback((message, tone = 'success') => {
    setToast({ message, tone })
    window.clearTimeout(toastTimer.current)
    toastTimer.current = window.setTimeout(() => setToast(null), 2800)
  }, [])

  useEffect(() => () => window.clearTimeout(toastTimer.current), [])

  const showSystemNotification = useCallback((title, body, { force = false } = {}) => {
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
  }, [notificationSettings.browser?.enabled])

  const browserNotify = useCallback((event, data, options) => {
    const template = notificationSettings.templates?.find((item) => item.id === event)
    const content = template?.channels?.browser?.content
    if (!template?.enabled || !content) return
    showSystemNotification(template.name, renderNotificationContent(content, data), options)
  }, [notificationSettings.templates, showSystemNotification])

  const registerPrimaryAction = useCallback((action) => {
    return primaryActions.register(action)
  }, [primaryActions])

  const invokePrimaryAction = useCallback(() => {
    primaryActions.invoke()
  }, [primaryActions])

  const registerWorkflowActions = useCallback((actions) => {
    setWorkflowActions(actions)
    return () => setWorkflowActions((current) => current === actions ? null : current)
  }, [])

  const navigate = useCallback((next, options) => {
    if (!PAGE_IDS.has(next)) return
    routerNavigate(pagePath(next), options)
    setQuery('')
    setMobileNav(false)
  }, [routerNavigate])

  const openUpdateSettings = useCallback(() => {
    setConfigSection('updates')
    navigate('config')
  }, [navigate])

  const handlePrimary = useCallback(() => {
    if (page === 'config' && configSection !== 'models') return
    if (['chat', 'config', 'assets', 'plugins', 'channels', 'schedules', 'memory', 'mcp', 'skills', 'workflowCreate'].includes(page)) invokePrimaryAction()
    else if (page === 'chatHistory') { navigate('chat'); invokePrimaryAction() }
    else if (page === 'workflows') navigate('workflowCreate')
    else setModal(page)
  }, [configSection, invokePrimaryAction, navigate, page])

  useEffect(() => {
    const focusSearch = () => {
      if (page === 'chat') {
        navigate('chatHistory')
        requestAnimationFrame(() => requestAnimationFrame(() => searchInputRef.current?.focus()))
      } else searchInputRef.current?.focus()
    }
    const onKeyDown = (event) => {
      const modifier = event.metaKey || event.ctrlKey
      if (modifier && event.key.toLowerCase() === 'k' && !appDialog.dialog && !modal) {
        event.preventDefault()
        setPaletteOpen(true)
      } else if (modifier && event.key.toLowerCase() === 'n' && !isEditableTarget(event.target)) {
        event.preventDefault()
        handlePrimary()
      } else if (event.key === '/' && !modifier && !event.altKey && !isEditableTarget(event.target)) {
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
  }, [appDialog.dialog, handlePrimary, modal, mobileNav, navigate, page, paletteOpen])

  useEffect(() => {
    let active = true
    apiJson('/api/config')
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
    return () => { active = false }
  }, [navigate, primaryActions])

  useEffect(() => {
    refreshPluginStats()
  }, [refreshPluginStats])

  useEffect(() => {
    apiJson('/api/settings/notifications')
      .then(setNotificationSettings)
      .catch(() => {})
  }, [])

  useEffect(() => {
    let active = true
    const poll = async () => {
      try {
        const result = await apiJson(`/api/settings/notifications/browser/events?after=${encodeURIComponent(browserEventCursor.current)}`)
        if (!active) return
        for (const event of result.events || []) showSystemNotification(event.title, event.body, { force: true })
        browserEventCursor.current = result.latestId || browserEventCursor.current
      } catch {}
    }
    poll()
    const timer = window.setInterval(poll, 3000)
    return () => { active = false; window.clearInterval(timer) }
  }, [showSystemNotification])

  const activeMeta = page === 'chat'
    ? [t('会话'), t('拖动标签，或从会话列表拆分到左侧或右侧')]
    : pageMeta[page]

  if (!startupReady) return <div className="app-startup"><BrandLogo size={30} className="startup-logo" /><strong>{t('正在唤醒 Vesper…')}</strong></div>

  return (
    <div className="app-shell">
      <div className="app-body">
        <Sidebar page={page} navigation={navigation} navigate={navigate} open={mobileNav} onClose={() => setMobileNav(false)} collapsed={sidebarCollapsed} onToggleCollapse={toggleSidebarCollapsed} update={appUpdate} onOpenUpdates={openUpdateSettings} />
        <main className="main-surface">
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
            <Routes>
              <Route path={PAGE_PATHS.chat} element={<ChatPage notify={notify} browserNotify={browserNotify} registerPrimaryAction={registerPrimaryAction} pendingAsset={pendingAsset} onAssetConsumed={() => setPendingAsset(null)} requestText={appDialog.prompt} requestConfirm={appDialog.confirm} />} />
              <Route path={PAGE_PATHS.chatHistory} element={<ChatHistoryPage query={query} navigate={navigate} notify={notify} requestConfirm={appDialog.confirm} requestText={appDialog.prompt} />} />
              <Route path={PAGE_PATHS.assets} element={<Suspense fallback={<PageLoader />}><AssetsPage query={query} notify={notify} registerPrimaryAction={registerPrimaryAction} requestConfirm={appDialog.confirm} onUse={(asset) => { const targetSessionId = localStorage.getItem(STORAGE_KEYS.activeSession) || ''; setPendingAsset({ asset, targetSessionId }); if (targetSessionId) requestSessionSelection(targetSessionId); navigate('chat') }} /></Suspense>} />
              <Route path={PAGE_PATHS.channels} element={<Suspense fallback={<PageLoader />}><ChannelsPage notify={notify} registerPrimaryAction={registerPrimaryAction} requestConfirm={appDialog.confirm} /></Suspense>} />
              <Route path={PAGE_PATHS.schedules} element={<Suspense fallback={<PageLoader />}><SchedulesPage notify={notify} registerPrimaryAction={registerPrimaryAction} requestConfirm={appDialog.confirm} openNotificationSettings={() => { setConfigSection('notifications'); navigate('config') }} /></Suspense>} />
              <Route path={PAGE_PATHS.config} element={<Suspense fallback={<PageLoader />}><ConfigPage notify={notify} registerPrimaryAction={registerPrimaryAction} section={configSection} setSection={setConfigSection} onBrowserNotificationChange={setNotificationSettings} requestConfirm={appDialog.confirm} update={appUpdate} /></Suspense>} />
              <Route path={PAGE_PATHS.plugins} element={<Suspense fallback={<PageLoader />}><PluginsPage query={query} notify={notify} registerPrimaryAction={registerPrimaryAction} onStatusChange={setPluginStats} /></Suspense>} />
              <Route path={PAGE_PATHS.memory} element={<Suspense fallback={<PageLoader />}><MemoryPage query={query} notify={notify} registerPrimaryAction={registerPrimaryAction} requestConfirm={appDialog.confirm} /></Suspense>} />
              <Route path={PAGE_PATHS.mcp} element={<Suspense fallback={<PageLoader />}><McpPage query={query} notify={notify} registerPrimaryAction={registerPrimaryAction} requestText={appDialog.prompt} requestConfirm={appDialog.confirm} /></Suspense>} />
              <Route path={PAGE_PATHS.skills} element={<Suspense fallback={<PageLoader />}><SkillsPage query={query} notify={notify} registerPrimaryAction={registerPrimaryAction} requestText={appDialog.prompt} requestConfirm={appDialog.confirm} /></Suspense>} />
              <Route path={PAGE_PATHS.workflows} element={<Suspense fallback={<PageLoader />}><WorkflowsPage query={query} notify={notify} requestConfirm={appDialog.confirm} /></Suspense>} />
              <Route path={PAGE_PATHS.workflowCreate} element={<Suspense fallback={<PageLoader />}><WorkflowBuilder notify={notify} registerPrimaryAction={registerPrimaryAction} registerWorkflowActions={registerWorkflowActions} /></Suspense>} />
              <Route path="/workflows/:workflowId" element={<Suspense fallback={<PageLoader />}><WorkflowBuilder notify={notify} registerPrimaryAction={registerPrimaryAction} registerWorkflowActions={registerWorkflowActions} /></Suspense>} />
              <Route path="*" element={<Navigate to={PAGE_PATHS.chat} replace />} />
            </Routes>
          </div>
        </main>
      </div>
      <StatusBar page={page} pluginStats={pluginStats} />
      {toast && <Toast message={toast.message} tone={toast.tone} />}
      <AppDialog dialog={appDialog.dialog} onClose={appDialog.close} onFinish={appDialog.finish} />
      {paletteOpen && <CommandPalette navigation={navigation} onClose={() => setPaletteOpen(false)} onNavigate={navigate} onOpenSession={(id) => { requestSessionSelection(id); navigate('chat') }} onNewChat={() => { navigate('chat'); requestAnimationFrame(() => requestAnimationFrame(invokePrimaryAction)) }} />}
      {modal && <QuickCreate type={modal} close={() => setModal(null)} notify={notify} />}
    </div>
  )
}

function StatusBar({ page, pluginStats }) {
  const { t, language } = useI18n()
  const [usage, setUsage] = useState(null)
  const [modelLabel, setModelLabel] = useState('')
  const modelRequest = useRef(0)

  const refreshUsage = useCallback(async () => {
    try { setUsage(await apiJson('/api/usage/today')) } catch {}
  }, [])

  useEffect(() => {
    refreshUsage()
    const timer = window.setInterval(refreshUsage, 15_000)
    const refreshWhenVisible = () => { if (document.visibilityState === 'visible') refreshUsage() }
    document.addEventListener('visibilitychange', refreshWhenVisible)
    window.addEventListener(USAGE_UPDATED_EVENT, refreshUsage)
    return () => {
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', refreshWhenVisible)
      window.removeEventListener(USAGE_UPDATED_EVENT, refreshUsage)
    }
  }, [refreshUsage])

  const refreshModel = useCallback(async (sessionId = localStorage.getItem(STORAGE_KEYS.activeSession) || '') => {
    const request = ++modelRequest.current
    try {
      const [config, sessionData] = await Promise.all([apiJson('/api/config'), apiJson('/api/sessions')])
      if (request !== modelRequest.current) return
      const session = sessionData.sessions?.find((item) => item.id === sessionId)
      setModelLabel(session?.model || (config.model ? `${config.provider}/${config.model}` : ''))
    } catch {}
  }, [])

  useEffect(() => {
    const syncModel = (event) => {
      const sessionId = event.detail?.id || localStorage.getItem(STORAGE_KEYS.activeSession) || ''
      if (event.detail?.model) {
        modelRequest.current += 1
        setModelLabel(event.detail.model)
      } else {
        void refreshModel(sessionId)
      }
    }
    const refreshFromSessions = () => { void refreshModel() }
    void refreshModel()
    window.addEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncModel)
    window.addEventListener(SESSIONS_UPDATED_EVENT, refreshFromSessions)
    return () => {
      window.removeEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncModel)
      window.removeEventListener(SESSIONS_UPDATED_EVENT, refreshFromSessions)
    }
  }, [refreshModel])

  const usageTitle = usage
    ? t('输入 {input} · 输出 {output} · 推理 {reasoning} · 缓存读取 {cacheRead}', {
        input: usage.input.toLocaleString(language),
        output: usage.output.toLocaleString(language),
        reasoning: usage.reasoning.toLocaleString(language),
        cacheRead: usage.cacheRead.toLocaleString(language),
      })
    : t('正在统计今日 Token 消耗')

  return <footer className="status-bar">
    <span className="status-model"><Bot size={12} />{modelLabel || t('未配置模型')}</span>
    <span className="status-usage">{['skills', 'mcp', 'workflows', 'workflowCreate'].includes(page)
      ? <>{t('原生运行时')} <em>{t('已接入')}</em></>
      : page === 'plugins'
        ? t('已启用 {enabled} / {total}', { enabled: pluginStats?.enabled ?? '—', total: pluginStats?.total ?? '—' })
        : <>{t('今日 tokens')} <em title={usageTitle}>{usage ? formatTokenCount(usage.totalTokens) : '—'}</em></>}</span>
  </footer>
}

function PageLoader() {
  const { t } = useI18n()
  return <Panel className="empty-state"><RefreshCw className="spin" size={24} /><h2>{t('正在点亮此页')}</h2><p>{t('所需能力正在依次就位…')}</p></Panel>
}

function Sidebar({ page, navigation, navigate, open, onClose, collapsed, onToggleCollapse, update, onOpenUpdates }) {
  const { t, language } = useI18n()
  const [sessions, setSessions] = useState([])
  const [historyExpanded, setHistoryExpanded] = useState(true)
  const [activeSessionId, setActiveSessionId] = useState(() => localStorage.getItem(STORAGE_KEYS.activeSession) || '')
  const active = page === 'workflowCreate' ? 'workflows' : page === 'chatHistory' ? 'chat' : page

  const refreshSessions = useCallback(async () => {
    try {
      const data = await apiJson('/api/sessions')
      setSessions([...(data.sessions || [])].sort((a, b) => Date.parse(b.modified) - Date.parse(a.modified)))
    } catch {}
  }, [])

  useEffect(() => {
    refreshSessions()
    const timer = window.setInterval(refreshSessions, 20_000)
    const refreshWhenVisible = () => { if (document.visibilityState === 'visible') refreshSessions() }
    const syncActive = (event) => setActiveSessionId(event.detail?.id || localStorage.getItem(STORAGE_KEYS.activeSession) || '')
    document.addEventListener('visibilitychange', refreshWhenVisible)
    window.addEventListener(SESSIONS_UPDATED_EVENT, refreshSessions)
    window.addEventListener(SESSION_SELECTED_EVENT, syncActive)
    window.addEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActive)
    return () => {
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', refreshWhenVisible)
      window.removeEventListener(SESSIONS_UPDATED_EVENT, refreshSessions)
      window.removeEventListener(SESSION_SELECTED_EVENT, syncActive)
      window.removeEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActive)
    }
  }, [refreshSessions])

  const openRecentSession = (id) => {
    setActiveSessionId(id)
    requestSessionSelection(id)
    navigate('chat')
  }

  return (
    <>
      {open && <button className="nav-scrim" aria-label={t('关闭导航')} onClick={onClose} />}
      <aside className={`sidebar ${open ? 'is-open' : ''} ${collapsed ? 'collapsed' : ''}`}>
        <div className="brand"><BrandLogo size={22} /><strong>{APP_NAME}</strong><button className="mobile-close" onClick={onClose}><X size={18} /></button></div>
        <div className="nav-list">
          <nav className="nav-primary" aria-label={t('主导航')}>
            {navigation.map(([group, items]) => (
              <div className="nav-group" key={group}>
                <span className="nav-group-label">{group}</span>
                {items.map(([id, label, Icon]) => <button className={`nav-main ${active === id ? 'active' : ''}`} key={id} title={label} onClick={() => navigate(id)}><Icon size={16} /><span>{label}</span></button>)}
              </div>
            ))}
          </nav>
          <section className={`nav-history-section ${historyExpanded ? 'is-expanded' : ''}`} aria-label={t('最近会话')}>
            <div className="nav-history-section-head">
              <button className="nav-history-heading" aria-controls="sidebar-recent-sessions" aria-expanded={historyExpanded} onClick={() => setHistoryExpanded((value) => !value)}><span>{t('最近会话')}</span><ChevronRight className={historyExpanded ? 'is-open' : ''} size={14} /></button>
              <button className="nav-history-view-all" aria-label={t('查看全部历史会话，共 {count} 个', { count: sessions.length })} onClick={() => navigate('chatHistory')}>{t('查看全部')}</button>
            </div>
            {historyExpanded && <div className="nav-history-list" id="sidebar-recent-sessions">
              {sessions.slice(0, 4).map((session) => <button className={`nav-history-item ${session.id === activeSessionId ? 'active-session' : ''}`} aria-current={session.id === activeSessionId ? 'page' : undefined} title={`${session.name || t('未命名会话')} · ${relativeTime(session.modified, language)}`} onClick={() => openRecentSession(session.id)} key={session.id}><span>{session.name || t('未命名会话')}</span></button>)}
              {!sessions.length && <span className="nav-history-empty">{t('暂无历史会话')}</span>}
            </div>}
          </section>
        </div>
        <div className="mt-auto grid gap-2">
          <SidebarUpdateStatus update={update} collapsed={collapsed} onOpen={onOpenUpdates} />
          <button className="sidebar-collapse !mt-0" title={t(collapsed ? '展开侧栏' : '收起侧栏')} aria-label={t(collapsed ? '展开侧栏' : '收起侧栏')} onClick={onToggleCollapse}>{collapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}<span>{t(collapsed ? '展开侧栏' : '收起侧栏')}</span></button>
        </div>
      </aside>
    </>
  )
}

function SidebarUpdateStatus({ update, collapsed, onOpen }) {
  const { t } = useI18n()
  const status = update?.status || { state: 'idle' }
  const desktop = Boolean(update?.info?.desktop)
  if (!['available', 'downloading', 'downloaded'].includes(status.state)) return null
  const downloading = status.state === 'downloading'
  const downloaded = status.state === 'downloaded'
  const label = t(downloaded ? '等待重启安装' : downloading ? '正在下载' : desktop ? '发现新版本' : '发现代码更新')
  const detail = downloading
    ? `${Math.round(status.percent || 0)}%`
    : desktop && status.availableVersion
      ? `v${status.availableVersion}`
      : status.behindBy
        ? t('落后 {branch} {count} 个提交', { branch: status.branch || 'main', count: status.behindBy })
        : status.availableCommit
          ? status.availableCommit.slice(0, 7)
          : t('点击查看更新')
  const Icon = downloaded ? Rocket : downloading ? RefreshCw : desktop ? Download : ExternalLink
  return <button type="button" className={`flex min-h-11 w-full items-center rounded-[var(--r-sm)] border border-[var(--stroke)] bg-[var(--accent-soft)] text-[var(--text)] transition-colors hover:bg-[var(--surface-hover)] ${collapsed ? 'justify-center px-0' : 'gap-2.5 px-3 text-left'}`} title={`${label} · ${detail}`} aria-label={`${label} · ${detail}`} onClick={onOpen}>
    <Icon className={downloading ? 'spin shrink-0' : 'shrink-0'} size={16} />
    {!collapsed && <span className="min-w-0"><strong className="block truncate text-[12px]">{label}</strong><small className="mt-0.5 block truncate text-[11px] text-[var(--muted)]">{detail}</small></span>}
  </button>
}

function PageHeader({ meta, page, query, setQuery, configSection, onMenu, onPrimary, theme, onCycleTheme, searchInputRef, workflowActions, desktopPlatform }) {
  const { t } = useI18n()
  const primary = page === 'config' && configSection !== 'models' ? null : ({
    chat: [t('新会话'), Plus], chatHistory: [t('新会话'), Plus], assets: [t('添加链接'), Link2], channels: [t('连接渠道'), Plus], schedules: [t('新建任务'), Plus],
    config: [t('添加 Provider'), Plus], plugins: [t('保存策略'), Save], memory: [t('点亮星辰'), Plus], mcp: [t('添加服务'), Plus],
    skills: [t('安装技能'), Plus], workflows: [t('新建工作流'), Plus], workflowCreate: [t('发布'), Rocket],
  }[page])
  const PrimaryIcon = primary?.[1]
  const ThemeIcon = THEME_META[theme] || THEME_META.system
  const themeLabel = t(theme === 'light' ? '浅色' : theme === 'dark' ? '深色' : '跟随系统')
  const desktop = Boolean(desktopPlatform)
  return (
    <header className={`page-header ${desktop ? '[-webkit-app-region:drag]' : ''} ${desktopPlatform === 'darwin' ? 'pl-[74px]' : ''}`}>
      <button className={`mobile-menu ${desktop ? '[-webkit-app-region:no-drag]' : ''}`} onClick={onMenu}><Menu size={19} /></button>
      <div className="title-block"><h1>{meta[0]}</h1><p>{meta[1]}</p></div>
      <div className={`header-actions ${desktop ? '[-webkit-app-region:no-drag]' : ''} ${desktopPlatform && desktopPlatform !== 'darwin' ? 'pr-[138px]' : ''}`}>
        {page === 'workflowCreate' ? (
          <>
            <button className="button secondary" disabled={!workflowActions || workflowActions.busy || workflowActions.running} onClick={() => workflowActions?.save()}><Save size={15} />{t('保存草稿')}</button>
            <button className="button dark" disabled={!workflowActions || workflowActions.busy} onClick={() => workflowActions?.run()}>{workflowActions?.running ? <Square size={15} /> : <Play size={15} />}{t(workflowActions?.running ? '停止' : '试运行')}</button>
          </>
        ) : page === 'chat' ? null : (
          <label className="search-box" title={t('搜索（/）')}><Search size={15} /><input ref={searchInputRef} value={query} onChange={(e) => setQuery(e.target.value)} placeholder={page === 'mcp' ? t('搜索服务或工具') : page === 'memory' ? t('搜索星辰或文件') : t('搜索{page}', { page: meta[0] })} /></label>
        )}
        {primary && <button className="button primary" title={t('{action}（Ctrl/⌘ N）', { action: primary[0] })} onClick={onPrimary}><PrimaryIcon size={15} />{primary[0]}</button>}
        <button className="icon-button theme-toggle" title={t('主题：{theme}（点击切换）', { theme: themeLabel })} aria-label={t('主题：{theme}，点击切换主题', { theme: themeLabel })} onClick={onCycleTheme}><ThemeIcon size={16} /></button>
      </div>
    </header>
  )
}

const ChatDockContext = createContext(null)

function ChatPage({ notify, browserNotify, registerPrimaryAction, pendingAsset, onAssetConsumed, requestText, requestConfirm }) {
  const { t } = useI18n()
  const [remoteSessions, setRemoteSessions] = useState([])
  const [activeId, setActiveId] = useState(() => localStorage.getItem(STORAGE_KEYS.activeSession) || '')
  const [sessionStates, setSessionStates] = useState({})
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [model, setModel] = useState(() => t('等待配置'))
  const [availableModels, setAvailableModels] = useState([])
  const [sandboxStatus, setSandboxStatus] = useState({ state: 'checking', supported: true, platform: '' })
  const [workspaceSession, setWorkspaceSession] = useState(null)
  const [dockReady, setDockReady] = useState(0)
  const [compactDock, setCompactDock] = useState(() => window.matchMedia('(max-width: 900px)').matches)
  const [railOpen, setRailOpenState] = useState(() => localStorage.getItem(STORAGE_KEYS.sessionRail) !== '0')
  const setRailOpen = useCallback((open) => {
    setRailOpenState(open)
    localStorage.setItem(STORAGE_KEYS.sessionRail, open ? '1' : '0')
  }, [])
  const creatingSessionRef = useRef(null)
  const sessionStatesRef = useRef(sessionStates)
  const sessionsRef = useRef(remoteSessions)
  const dockApiRef = useRef(null)
  const dockInitializedRef = useRef(false)
  const dockDisposablesRef = useRef([])
  const layoutSaveTimerRef = useRef(null)
  const pendingDockRequestRef = useRef(null)
  const compactDockRef = useRef(compactDock)
  const legacyTiledSessionIdsRef = useRef(readStoredArray(STORAGE_KEYS.tiledSessions))

  useEffect(() => {
    sessionStatesRef.current = sessionStates
  }, [sessionStates])

  useEffect(() => {
    sessionsRef.current = remoteSessions
  }, [remoteSessions])

  useEffect(() => {
    const media = window.matchMedia('(max-width: 900px)')
    const update = () => { compactDockRef.current = media.matches; setCompactDock(media.matches) }
    update()
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [])

  const updateSessionState = useCallback((id, update) => {
    if (!id) return
    const current = sessionStatesRef.current
    const previous = current[id] || DEFAULT_SESSION_STATE
    const next = applySessionUpdate(previous, update)
    if (next === previous) return
    const states = { ...current, [id]: next }
    sessionStatesRef.current = states
    setSessionStates(states)
  }, [])

  const syncLiveSession = useCallback(async (id) => {
    if (!id) return
    try {
      const data = await apiJson(`/api/sessions/${encodeURIComponent(id)}/live`)
      updateSessionState(id, (current) => {
        const finishedAt = data.finishedAt || current.runFinishedAt || new Date().toISOString()
        return {
          ...current,
          ...latestPageState(current, data),
          tools: data.streaming ? (data.tools || []) : settleToolCalls(data.tools || [], { finishedAt, error: data.error || '' }),
          streaming: data.streaming,
          recovering: data.streaming,
          runStartedAt: data.startedAt || current.runStartedAt || null,
          lastActivityAt: data.lastActivityAt || current.lastActivityAt || data.startedAt || null,
          runFinishedAt: data.streaming ? null : finishedAt,
          runNotice: data.streaming ? current.runNotice || '' : '',
          loaded: true,
          loading: false,
          error: data.error || '',
          model: data.model || current.model,
          cwd: data.cwd || current.cwd,
          permissionMode: data.permissionMode || current.permissionMode,
          executionMode: data.executionMode || current.executionMode,
          goal: data.goal ?? current.goal ?? null,
          taskList: data.taskList ?? current.taskList ?? null,
          contextUsage: data.contextUsage ?? current.contextUsage ?? null,
          compaction: data.compaction ?? current.compaction ?? null,
          approvals: data.approvals || [],
          agents: data.agents || [],
          currentActivity: data.streaming ? (data.currentActivity || current.currentActivity || null) : null,
          activityFeed: data.streaming ? (data.activityFeed || current.activityFeed || []) : [],
          thinkingText: data.streaming ? (data.thinkingText ?? current.thinkingText ?? '') : '',
          queuedInputs: resolveQueuedInputs(current.queuedInputs, data.queuedInputs),
          hadQueuedInput: data.streaming ? Boolean(current.hadQueuedInput || data.queuedInputs?.length) : false,
        }
      })
      setRemoteSessions((current) => current.map((session) => session.id === id ? { ...session, streaming: data.streaming, model: data.model || session.model, cwd: data.cwd || session.cwd, permissionMode: data.permissionMode || session.permissionMode, executionMode: data.executionMode || session.executionMode, goal: data.goal ?? session.goal ?? null, taskList: data.taskList ?? session.taskList ?? null } : session))
    } catch (caught) {
      updateSessionState(id, { recovering: false, loading: false, error: caught.message })
    }
  }, [updateSessionState])

  const loadSessionMessages = useCallback(async (id, { force = false, limit = FOCUS_MESSAGE_PAGE_SIZE } = {}) => {
    if (!id) return
    const current = sessionStatesRef.current[id]
    if (current?.recovering) { await syncLiveSession(id); return }
    if (!force && (current?.loading || current?.streaming || (current?.loaded && (current.pageSize || 0) >= limit))) return
    updateSessionState(id, { loading: true })
    try {
      const data = await apiJson(`/api/sessions/${encodeURIComponent(id)}/messages?limit=${limit}`)
      updateSessionState(id, (latest) => latest.streaming
        ? { ...latest, loaded: true, loading: false, pageSize: Math.max(latest.pageSize || 0, limit) }
        : { ...latest, ...latestPageState(latest, data), contextUsage: data.contextUsage ?? latest.contextUsage ?? null, loaded: true, loading: false, pageSize: Math.max(latest.pageSize || 0, limit), error: '', olderError: '' })
    } catch (caught) {
      updateSessionState(id, { loading: false, error: caught.message })
    }
  }, [syncLiveSession, updateSessionState])

  const loadOlderMessages = useCallback(async (id) => {
    const current = sessionStatesRef.current[id]
    if (!id || !current?.hasOlder || !current.olderCursor || current.loadingOlder) return false
    updateSessionState(id, { loadingOlder: true, olderError: '' })
    try {
      const data = await apiJson(`/api/sessions/${encodeURIComponent(id)}/messages?limit=${FOCUS_MESSAGE_PAGE_SIZE}&before=${encodeURIComponent(current.olderCursor)}`)
      updateSessionState(id, (latest) => {
        const existingIds = new Set(latest.messages.map((message) => message.id))
        const older = data.messages.filter((message) => !existingIds.has(message.id))
        return {
          ...latest,
          messages: [...older, ...latest.messages],
          messageStart: data.pageInfo.start,
          hasOlder: data.pageInfo.hasMore,
          olderCursor: data.pageInfo.nextCursor,
          loadingOlder: false,
          olderError: '',
        }
      })
      return data.messages.length > 0
    } catch (caught) {
      updateSessionState(id, { loadingOlder: false, olderError: caught.message })
      return false
    }
  }, [updateSessionState])

  const scheduleDockLayoutSave = useCallback((api = dockApiRef.current) => {
    if (!api || !dockInitializedRef.current) return
    window.clearTimeout(layoutSaveTimerRef.current)
    layoutSaveTimerRef.current = window.setTimeout(() => {
      const envelope = createDockLayoutEnvelope(api.toJSON(), api.activePanel?.id || '')
      localStorage.setItem(STORAGE_KEYS.chatDockLayout, JSON.stringify(envelope))
    }, 180)
  }, [])

  const openSessionInDock = useCallback((sessionId, disposition = 'open') => {
    const session = sessionsRef.current.find((item) => item.id === sessionId)
    const api = dockApiRef.current
    if (!session || !api || !dockInitializedRef.current) {
      pendingDockRequestRef.current = { sessionId, disposition }
      return false
    }
    const effectiveDisposition = compactDockRef.current ? 'open' : disposition
    const panelId = panelIdForSession(sessionId)
    const existing = api.getPanel(panelId)
    const referencePanel = api.activePanel
    if (existing) {
      if (effectiveDisposition !== 'open') {
        if (referencePanel && existing.id !== referencePanel.id) existing.api.moveTo({ group: referencePanel.group, position: effectiveDisposition })
        else if (existing.group.size > 1) existing.api.moveTo({ group: existing.group, position: effectiveDisposition })
      }
      existing.api.setActive()
      setActiveId(sessionId)
      return true
    }
    const position = effectiveDisposition === 'open'
      ? (api.activeGroup ? { referenceGroup: api.activeGroup } : undefined)
      : (referencePanel ? { referencePanel, direction: effectiveDisposition } : { direction: effectiveDisposition })
    api.addPanel({
      id: panelId,
      component: 'session',
      title: session.name || t('未命名会话'),
      params: { sessionId },
      renderer: 'always',
      minimumWidth: 300,
      ...(position ? { position } : {}),
    })
    setActiveId(sessionId)
    void loadSessionMessages(sessionId, { limit: FOCUS_MESSAGE_PAGE_SIZE })
    return true
  }, [loadSessionMessages, t])

  const splitDockPanel = useCallback((panelId, direction) => {
    const panel = dockApiRef.current?.getPanel(panelId)
    if (!panel) return
    if (compactDockRef.current) {
      notify(t('窄屏模式下会话将保留在同一标签组中。'), 'info')
      return
    }
    if (panel.group.size <= 1) {
      notify(t('当前分组只有一个会话，请从左侧会话列表选择其他会话进行拆分。'), 'info')
      return
    }
    panel.api.moveTo({ group: panel.group, position: direction })
    panel.api.setActive()
  }, [notify, t])

  const closeDockPanel = useCallback((panelId) => {
    dockApiRef.current?.getPanel(panelId)?.api.close()
  }, [])

  const onDockReady = useCallback(({ api }) => {
    if (dockApiRef.current && dockApiRef.current !== api) dockInitializedRef.current = false
    for (const disposable of dockDisposablesRef.current) disposable.dispose()
    dockDisposablesRef.current = []
    dockApiRef.current = api
    dockDisposablesRef.current.push(
      api.onDidActivePanelChange(({ panel }) => {
        const sessionId = sessionIdFromPanel(panel)
        setActiveId(sessionId)
      }),
      api.onDidLayoutChange(() => scheduleDockLayoutSave(api)),
      api.onDidAddPanel((panel) => {
        const sessionId = sessionIdFromPanel(panel)
        if (sessionId) void loadSessionMessages(sessionId, { limit: FOCUS_MESSAGE_PAGE_SIZE })
      }),
    )
    setDockReady((generation) => generation + 1)
  }, [loadSessionMessages, scheduleDockLayoutSave])

  useEffect(() => {
    const selectSession = (event) => {
      const sessionId = event.detail?.sessionId || event.detail?.id
      if (!sessionId) return
      localStorage.removeItem(STORAGE_KEYS.sessionOpenRequest)
      openSessionInDock(sessionId, event.detail?.disposition || 'open')
    }
    window.addEventListener(SESSION_SELECTED_EVENT, selectSession)
    return () => window.removeEventListener(SESSION_SELECTED_EVENT, selectSession)
  }, [openSessionInDock])

  useEffect(() => () => {
    window.clearTimeout(layoutSaveTimerRef.current)
    for (const disposable of dockDisposablesRef.current) disposable.dispose()
    dockDisposablesRef.current = []
    dockApiRef.current = null
  }, [])

  useEffect(() => {
    if (activeId) localStorage.setItem(STORAGE_KEYS.activeSession, activeId)
  }, [activeId])

  const refreshSessions = async (preferredId) => {
    const data = await apiJson('/api/sessions')
    sessionsRef.current = data.sessions
    setRemoteSessions(data.sessions)
    if (preferredId) setActiveId(preferredId)
    else setActiveId((current) => data.sessions.some((session) => session.id === current) ? current : (data.sessions[0]?.id || ''))
    announceSessionsUpdated()
    return data.sessions
  }

  const createSession = () => {
    if (creatingSessionRef.current) return creatingSessionRef.current
    const request = (async () => {
      try {
        setError('')
        const created = await apiJson('/api/sessions', { method: 'POST', body: JSON.stringify({ name: t('新会话') }) })
        setActiveId(created.id)
        setRemoteSessions((current) => {
          const next = mergeSessionLists(current, [created])
          sessionsRef.current = next
          return next
        })
        updateSessionState(created.id, { messages: [], tools: [], approvals: [], queuedInputs: [], permissionMode: created.permissionMode || 'auto', executionMode: created.executionMode || 'workspace', goal: created.goal || null, taskList: created.taskList || null, contextUsage: created.contextUsage || null, compaction: null, streaming: false, error: '', loaded: true, pageSize: FOCUS_MESSAGE_PAGE_SIZE, messageStart: 0, hasOlder: false, olderCursor: null, runStartedAt: null, lastActivityAt: null, runFinishedAt: null, runStopped: false, runNotice: '' })
        try {
          await refreshSessions(created.id)
        } catch (caught) {
          setError(t('会话已创建，但刷新列表失败：{error}', { error: caught.message }))
        }
        openSessionInDock(created.id)
        notify(t('新会话已创建'))
        return created.id
      } catch (caught) {
        setError(caught.message)
        return ''
      }
    })()
    creatingSessionRef.current = request
    void request.finally(() => {
      if (creatingSessionRef.current === request) creatingSessionRef.current = null
    })
    return request
  }
  usePagePrimaryAction(registerPrimaryAction, createSession)

  useEffect(() => {
    let active = true
    Promise.all([
      apiJson('/api/sessions'),
      apiJson('/api/config'),
      apiJson('/api/sandbox/status'),
    ])
      .then(async ([sessionData, configData, sandboxData]) => {
        setSandboxStatus(sandboxData)
        if (!active) return
        setModel(configData.model ? `${configData.provider}/${configData.model}` : t('未配置模型'))
        setAvailableModels(configData.providers.flatMap((provider) => provider.configured && provider.enabled
          ? provider.models.filter((item) => item.kind === 'chat').map((item) => ({
              key: `${provider.id}/${item.id}`,
              provider: provider.id,
              modelId: item.id,
              label: item.name || item.id,
              providerName: provider.name || provider.id,
            }))
          : []))
        let list = sessionData.sessions
        if (!list.length && (creatingSessionRef.current || Object.keys(sessionStatesRef.current).length)) {
          await creatingSessionRef.current
          if (!active) return
          list = (await apiJson('/api/sessions')).sessions
        }
        if (!list.length) {
          const created = await apiJson('/api/sessions', { method: 'POST', body: JSON.stringify({ name: t('新会话') }) })
          list = [created]
        }
        if (!active) return
        setRemoteSessions((current) => {
          const next = mergeSessionLists(current, list)
          sessionsRef.current = next
          return next
        })
        announceSessionsUpdated()
        for (const session of list) {
          updateSessionState(session.id, {
            agents: session.agents || [],
            ...(session.streaming ? { streaming: true, recovering: true, loaded: false, error: '' } : {}),
          })
        }
        const storedId = localStorage.getItem(STORAGE_KEYS.activeSession)
        const knownIds = new Set([...list.map((session) => session.id), ...Object.keys(sessionStatesRef.current)])
        setActiveId((current) => knownIds.has(current) ? current : (knownIds.has(storedId) ? storedId : (list[0]?.id || '')))
      })
      .catch((caught) => active && setError(caught.message))
      .finally(() => active && setLoading(false))
    return () => { active = false }
  }, [t, updateSessionState])

  useEffect(() => {
    if (!dockReady || loading || dockInitializedRef.current || !dockApiRef.current || !remoteSessions.length) return
    const api = dockApiRef.current
    const validIds = new Set(remoteSessions.map((session) => session.id))
    const storedLayout = parseDockLayoutEnvelope(localStorage.getItem(STORAGE_KEYS.chatDockLayout))
    if (storedLayout) {
      try {
        api.fromJSON(storedLayout.layout)
        for (const panel of [...api.panels]) {
          const sessionId = sessionIdFromPanel(panel)
          if (!validIds.has(sessionId)) api.removePanel(panel)
        }
      } catch {
        api.clear()
        localStorage.removeItem(STORAGE_KEYS.chatDockLayout)
      }
    }
    if (!api.panels.length) {
      const initialIds = initialDockSessionIds({
        activeSessionId: activeId || localStorage.getItem(STORAGE_KEYS.activeSession) || '',
        legacyTiledSessionIds: legacyTiledSessionIdsRef.current.slice(0, 4),
        validSessionIds: remoteSessions.map((session) => session.id),
      })
      let referenceGroup
      for (const sessionId of initialIds) {
        const session = remoteSessions.find((item) => item.id === sessionId)
        if (!session) continue
        const panel = api.addPanel({
          id: panelIdForSession(sessionId),
          component: 'session',
          title: session.name || t('未命名会话'),
          params: { sessionId },
          renderer: 'always',
          minimumWidth: 300,
          ...(referenceGroup ? { position: { referenceGroup } } : {}),
          inactive: Boolean(referenceGroup),
        })
        referenceGroup ||= panel.group
      }
    }
    for (const panel of api.panels) {
      const session = remoteSessions.find((item) => item.id === sessionIdFromPanel(panel))
      if (session) panel.api.setTitle(session.name || t('未命名会话'))
    }
    dockInitializedRef.current = true
    localStorage.removeItem(STORAGE_KEYS.chatMode)
    const request = pendingDockRequestRef.current || consumeSessionSelectionRequest()
    pendingDockRequestRef.current = null
    if (request) openSessionInDock(request.sessionId, request.disposition)
    else {
      const preferredPanel = api.getPanel(storedLayout?.activePanelId || panelIdForSession(activeId)) || api.panels[0]
      preferredPanel?.api.setActive()
    }
    scheduleDockLayoutSave(api)
  }, [activeId, dockReady, loading, openSessionInDock, remoteSessions, scheduleDockLayoutSave, t])

  useEffect(() => {
    if (!dockInitializedRef.current || !dockApiRef.current) return
    for (const session of remoteSessions) dockApiRef.current.getPanel(panelIdForSession(session.id))?.api.setTitle(session.name || t('未命名会话'))
  }, [remoteSessions, t])

  useEffect(() => {
    let active = true
    const poll = () => {
      if (!active) return
      for (const [id, state] of Object.entries(sessionStatesRef.current)) {
        const hasActiveAgents = state.agents?.some((agent) => ['queued', 'starting', 'running'].includes(agent.status))
        if (state.recovering || state.approvals?.length || hasActiveAgents) void syncLiveSession(id)
      }
    }
    poll()
    const timer = window.setInterval(poll, 800)
    return () => { active = false; window.clearInterval(timer) }
  }, [syncLiveSession])

  const sendPrompt = async (text, requestedSessionId, attachments = [], goalMode = false) => {
    const prompt = text.trim() || (attachments.length ? t('请分析这些附件。') : '')
    if (!prompt) return
    let sessionId = requestedSessionId
    if (!sessionId) sessionId = await createSession()
    if (!sessionId) return
    if (sessionStatesRef.current[sessionId]?.streaming) return
    setActiveId(sessionId)
    setError('')
    const userMessage = { id: `user-${Date.now()}`, role: 'user', text: prompt, attachments: attachments.map(({ id, kind, name, mimeType, size, data }) => ({ id, kind, name, mimeType, size, data: kind === 'image' ? data : undefined })) }
    const agentId = `agent-${Date.now()}`
    const runStartedAt = new Date().toISOString()
    let responseText = ''
    let thinkingText = ''
    let queuedDuringRun = false
    const thinkingScheduler = createStreamingTextScheduler((text, activityAt) => {
      updateSessionState(sessionId, (current) => ({
        ...current,
        thinkingText: text,
        lastActivityAt: activityAt || current.lastActivityAt,
        currentActivity: { type: 'model', stage: 'thinking', updatedAt: activityAt || current.lastActivityAt },
      }))
    }, { intervalMs: 80 })
    const typewriter = createTypewriterDisplay((text, activityAt) => {
      updateSessionState(sessionId, (current) => {
        const activity = { type: 'model', stage: 'responding', updatedAt: activityAt || current.lastActivityAt }
        return {
          ...current,
          lastActivityAt: activityAt || current.lastActivityAt,
          runNotice: '',
          currentActivity: activity,
          activityFeed: pushCurrentActivity(current.activityFeed, activity),
          messages: current.messages.map((item) => item.id === agentId ? { ...item, text } : item),
        }
      })
    })
    const toolScheduler = createToolUpdateScheduler((batch, activityAt) => {
      updateSessionState(sessionId, (current) => {
        let activityFeed = current.activityFeed || []
        for (const [id, patch] of batch) {
          const existing = activityFeed.find((item) => item.type === 'tool' && item.id === id)
          if (existing) activityFeed = pushCurrentActivity(activityFeed, { ...existing, ...patch, updatedAt: activityAt || existing.updatedAt })
        }
        const currentActivity = current.currentActivity?.type === 'tool' && batch.has(current.currentActivity.id)
          ? { ...current.currentActivity, ...batch.get(current.currentActivity.id), updatedAt: activityAt || current.currentActivity.updatedAt }
          : current.currentActivity
        return {
          ...current,
          lastActivityAt: activityAt || current.lastActivityAt,
          tools: current.tools.map((item) => {
            const patch = batch.get(item.id)
            return patch ? { ...item, ...patch } : item
          }),
          currentActivity,
          activityFeed,
        }
      })
    })
    updateSessionState(sessionId, (current) => {
      const keepTaskList = goalMode || current.goal?.status === 'active'
      return {
        ...current,
        messages: [...current.messages, userMessage, { id: agentId, role: 'agent', text: '', streaming: true }],
        tools: [],
        approvals: [],
        error: '',
        streaming: true,
        loaded: true,
        runStartedAt,
        lastActivityAt: runStartedAt,
        runFinishedAt: null,
        runStopped: false,
        runNotice: '',
        currentActivity: { type: 'model', stage: 'thinking', updatedAt: runStartedAt },
        activityFeed: [],
        thinkingText: '',
        hadQueuedInput: false,
        compaction: null,
        // Explicit null means cleared; do not keep a previous turn's plan hanging around.
        taskList: keepTaskList ? current.taskList : null,
      }
    })
    if (!goalMode) {
      setRemoteSessions((current) => current.map((session) => {
        if (session.id !== sessionId) return session
        if (session.goal?.status === 'active') return session
        return { ...session, taskList: null }
      }))
    }
    try {
      const response = await fetch('/api/chat', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ sessionId, message: prompt, attachments, goalMode }),
      })
      await consumeEventStream(response, (event, data) => {
        const eventAt = new Date().toISOString()
        if (event === 'meta') {
          updateSessionState(sessionId, (current) => ({
            ...current,
            model: data.model,
            cwd: data.cwd,
            permissionMode: data.permissionMode,
            executionMode: data.executionMode,
            goal: data.goal ?? null,
            // Prefer explicit meta.taskList (including empty) over leftover client state.
            taskList: data.taskList !== undefined ? data.taskList : current.taskList,
            agents: data.agents || current.agents || [],
            currentActivity: data.currentActivity || current.currentActivity || null,
            activityFeed: data.activityFeed || current.activityFeed || [],
            thinkingText: data.thinkingText ?? current.thinkingText ?? '',
            queuedInputs: resolveQueuedInputs(current.queuedInputs, data.queuedInputs),
            hadQueuedInput: Boolean(current.hadQueuedInput || data.queuedInputs?.length),
            contextUsage: data.contextUsage ?? current.contextUsage ?? null,
            runStartedAt: data.startedAt || current.runStartedAt,
            lastActivityAt: data.lastActivityAt || eventAt,
          }))
          if (data.cwd || data.permissionMode || data.executionMode || data.goal !== undefined || data.taskList !== undefined) {
            setRemoteSessions((current) => current.map((session) => session.id === sessionId ? {
              ...session,
              cwd: data.cwd || session.cwd,
              permissionMode: data.permissionMode || session.permissionMode,
              executionMode: data.executionMode || session.executionMode,
              goal: data.goal ?? session.goal ?? null,
              taskList: data.taskList !== undefined ? data.taskList : session.taskList,
            } : session))
          }
        } else if (event === 'queue_update') {
          if (data.queuedInputs?.length) queuedDuringRun = true
          updateSessionState(sessionId, (current) => ({
            ...current,
            queuedInputs: data.queuedInputs || [],
            hadQueuedInput: Boolean(current.hadQueuedInput || data.queuedInputs?.length),
          }))
        } else if (event === 'agent_update') {
          updateSessionState(sessionId, (current) => {
            const activity = data.currentActivity || (data.agent ? { type: 'agent', agent: data.agent, updatedAt: data.agent.lastActivityAt || eventAt } : null)
            return {
              ...current,
              agents: data.agents || [],
              currentActivity: current.currentActivity?.type === 'tool' ? current.currentActivity : activity || current.currentActivity,
              activityFeed: activity ? pushCurrentActivity(current.activityFeed, activity) : current.activityFeed,
              lastActivityAt: data.agent?.lastActivityAt || eventAt,
            }
          })
        } else if (event === 'context_usage') {
          updateSessionState(sessionId, { contextUsage: data })
        } else if (event === 'compaction_start') {
          typewriter.flush()
          toolScheduler.flush()
          updateSessionState(sessionId, (current) => {
            const activity = { type: 'compaction', compaction: data, updatedAt: data.startedAt || eventAt }
            return { ...current, compaction: data, currentActivity: activity, activityFeed: pushCurrentActivity(current.activityFeed, activity), lastActivityAt: data.startedAt || eventAt }
          })
        } else if (event === 'compaction_end') {
          updateSessionState(sessionId, (current) => {
            const activity = { type: 'model', stage: 'processing_result', updatedAt: data.finishedAt || eventAt }
            return { ...current, compaction: data, currentActivity: activity, activityFeed: pushCurrentActivity(current.activityFeed, activity), lastActivityAt: data.finishedAt || eventAt }
          })
        } else if (event === 'text_patch') {
          responseText = applyTextPatch(responseText, data)
          typewriter.setTarget(responseText, eventAt)
        } else if (event === 'text_delta') {
          responseText += data.delta || ''
          typewriter.setTarget(responseText, eventAt)
        } else if (event === 'thinking_reset') {
          thinkingText = ''
          thinkingScheduler.cancel()
          updateSessionState(sessionId, {
            thinkingText: '',
            currentActivity: { type: 'model', stage: 'thinking', updatedAt: data.updatedAt || eventAt },
            lastActivityAt: data.updatedAt || eventAt,
          })
        } else if (event === 'thinking_patch') {
          thinkingText = applyTextPatch(thinkingText, data)
          thinkingScheduler.push(thinkingText.slice(-MAX_LIVE_THINKING_CHARS), eventAt)
        } else if (event === 'tool_start') {
          thinkingScheduler.flush()
          typewriter.flush()
          toolScheduler.flush()
          updateSessionState(sessionId, (current) => {
            const activity = { type: 'tool', id: data.id, name: data.name, args: data.args, status: 'running', startedAt: data.startedAt || eventAt, updatedAt: eventAt }
            return {
              ...current,
              lastActivityAt: eventAt,
              runNotice: '',
              tools: [...current.tools, activity],
              currentActivity: activity,
              activityFeed: pushCurrentActivity(current.activityFeed, activity),
            // Freeze the pre-tool preamble once, so the UI can render: preamble → tools → post-tool text.
            messages: current.messages.map((item) => item.id === agentId
              ? {
                  ...item,
                  text: responseText || item.text,
                  streamPreamble: item.streamPreamble ?? responseText ?? item.text ?? '',
                }
              : item),
            }
          })
        } else if (event === 'tool_update') {
          toolScheduler.push(data.id, {
            message: data.message || '',
            updatedAt: data.updatedAt || eventAt,
            ...(data.agent ? { agent: data.agent } : {}),
          }, data.updatedAt || eventAt)
        } else if (event === 'tool_end') {
          toolScheduler.flush()
          updateSessionState(sessionId, (current) => {
            const completedTool = current.tools.find((item) => item.id === data.id)
            const finishedAt = data.finishedAt || eventAt
            const preserveEvent = ['update_task_list', 'spawn_agent', 'list_agents', 'send_message', 'followup_task', 'wait_agent', 'interrupt_agent'].includes(completedTool?.name)
              && ['plan', 'agent'].includes(current.currentActivity?.type)
            const toolActivity = { ...(completedTool || {}), type: 'tool', status: data.error ? 'error' : 'done', message: data.message || completedTool?.message || '', updatedAt: finishedAt, finishedAt }
            const agentActivity = data.agent ? { type: 'agent', agent: data.agent, updatedAt: data.agent.lastActivityAt || finishedAt } : null
            let activityFeed = current.activityFeed || []
            if (data.error || (!preserveEvent && !agentActivity)) activityFeed = pushCurrentActivity(activityFeed, toolActivity)
            if (agentActivity) activityFeed = pushCurrentActivity(activityFeed, agentActivity)
            return {
              ...current,
              lastActivityAt: finishedAt,
              tools: current.tools.map((item) => item.id === data.id ? { ...item, status: data.error ? 'error' : 'done', message: data.message || '', updatedAt: finishedAt, finishedAt } : item),
              currentActivity: data.error ? toolActivity : agentActivity || (preserveEvent ? current.currentActivity : toolActivity),
              activityFeed,
            }
          })
        } else if (event === 'permission_request') {
          typewriter.flush()
          toolScheduler.flush()
          updateSessionState(sessionId, (current) => ({ ...current, lastActivityAt: eventAt, approvals: [...(current.approvals || []).filter((item) => item.id !== data.id), data] }))
        } else if (event === 'permission_resolved') {
          updateSessionState(sessionId, (current) => ({ ...current, lastActivityAt: eventAt, approvals: (current.approvals || []).filter((item) => item.id !== data.id) }))
        } else if (event === 'generated_asset') {
          updateSessionState(sessionId, (current) => ({
            ...current,
            lastActivityAt: eventAt,
            messages: current.messages.map((item) => item.id === agentId
              ? { ...item, attachments: [...(item.attachments || []).filter((attachment) => attachment.id !== data.id), data] }
              : item),
          }))
        } else if (event === 'goal_update') {
          updateSessionState(sessionId, { goal: data.goal ?? null })
          setRemoteSessions((current) => current.map((session) => session.id === sessionId ? { ...session, goal: data.goal ?? null } : session))
        } else if (event === 'task_list_update') {
          typewriter.flush()
          toolScheduler.flush()
          updateSessionState(sessionId, (current) => {
            const nextTaskList = data.taskList !== undefined ? data.taskList : current.taskList
            const activity = data.currentActivity || { type: 'plan', taskList: nextTaskList, changes: taskListChanges(current.taskList, nextTaskList), updatedAt: nextTaskList?.updatedAt || eventAt }
            return {
              ...current,
              lastActivityAt: eventAt,
              taskList: nextTaskList,
              currentActivity: activity,
              activityFeed: pushCurrentActivity(current.activityFeed, activity),
            }
          })
          setRemoteSessions((current) => current.map((session) => session.id === sessionId ? {
            ...session,
            taskList: data.taskList !== undefined ? data.taskList : session.taskList,
          } : session))
        } else if (event === 'session_title') {
          setRemoteSessions((current) => current.map((session) => session.id === sessionId ? { ...session, name: data.name } : session))
        } else if (event === 'retry') {
          const retryNotice = t('正在重试 {attempt}/{maxAttempts}：{message}', { attempt: data.attempt, maxAttempts: data.maxAttempts, message: data.message })
          updateSessionState(sessionId, (current) => {
            const activity = { type: 'retry', message: retryNotice, updatedAt: eventAt }
            return { ...current, runNotice: retryNotice, currentActivity: activity, activityFeed: pushCurrentActivity(current.activityFeed, activity), lastActivityAt: eventAt }
          })
        } else if (event === 'done') {
          queuedDuringRun ||= Boolean(sessionStatesRef.current[sessionId]?.hadQueuedInput || sessionStatesRef.current[sessionId]?.queuedInputs?.length)
          const finishedAt = data.finishedAt || eventAt
          if (typeof data.text === 'string') responseText = data.text
          typewriter.setTarget(responseText, finishedAt)
          typewriter.flush()
          thinkingScheduler.cancel()
          toolScheduler.cancel()
          updateSessionState(sessionId, (current) => ({
            ...current,
            streaming: false,
            thinkingText: '',
            runFinishedAt: finishedAt,
            lastActivityAt: finishedAt,
            runNotice: '',
            currentActivity: null,
            activityFeed: [],
            goal: data.goal ?? current.goal ?? null,
            taskList: data.taskList !== undefined ? data.taskList : current.taskList,
            agents: data.agents || current.agents || [],
            contextUsage: data.contextUsage ?? current.contextUsage ?? null,
            compaction: data.compaction ?? current.compaction ?? null,
            approvals: data.approvals || [],
            tools: settleToolCalls(data.tools || current.tools, { finishedAt }),
            messages: current.messages.map((item) => item.id === agentId ? {
              ...item,
              text: typeof data.text === 'string' ? data.text : responseText || item.text,
              streaming: false,
              streamPreamble: undefined,
              ...(data.assets?.length ? { attachments: data.assets } : {}),
            } : item),
          }))
          setRemoteSessions((current) => current.map((session) => session.id === sessionId ? {
            ...session,
            streaming: false,
            goal: data.goal ?? session.goal ?? null,
            taskList: data.taskList !== undefined ? data.taskList : session.taskList,
          } : session))
          return false
        } else if (event === 'error') {
          queuedDuringRun ||= Boolean(sessionStatesRef.current[sessionId]?.hadQueuedInput || sessionStatesRef.current[sessionId]?.queuedInputs?.length)
          const finishedAt = data.finishedAt || eventAt
          if (typeof data.text === 'string') responseText = data.text
          typewriter.setTarget(responseText, finishedAt)
          typewriter.flush()
          thinkingScheduler.cancel()
          toolScheduler.cancel()
          updateSessionState(sessionId, (current) => ({
            ...current,
            streaming: false,
            thinkingText: '',
            runFinishedAt: finishedAt,
            lastActivityAt: finishedAt,
            currentActivity: null,
            activityFeed: [],
            agents: data.agents || current.agents || [],
            contextUsage: data.contextUsage ?? current.contextUsage ?? null,
            compaction: data.compaction ?? current.compaction ?? null,
            approvals: [],
            tools: settleToolCalls(data.tools || current.tools, { finishedAt, error: data.message }),
            messages: current.messages.map((item) => item.id === agentId ? {
              ...item,
              text: typeof data.text === 'string' ? data.text : responseText || item.text,
              streaming: false,
              streamPreamble: undefined,
            } : item),
          }))
          throw new Error(data.message)
        }
      })
      typewriter.setTarget(responseText)
      typewriter.flush()
      thinkingScheduler.flush()
      toolScheduler.flush()
      const fallbackFinishedAt = new Date().toISOString()
      const stillStreaming = Boolean(sessionStatesRef.current[sessionId]?.streaming)
      if (stillStreaming) {
        updateSessionState(sessionId, (current) => {
          const runFinishedAt = current.runFinishedAt || fallbackFinishedAt
          return { ...current, streaming: false, thinkingText: '', runFinishedAt, lastActivityAt: runFinishedAt, runNotice: '', currentActivity: null, activityFeed: [], approvals: [], tools: settleToolCalls(current.tools, { finishedAt: runFinishedAt }), messages: current.messages.map((item) => item.id === agentId ? { ...item, streaming: false, text: responseText || item.text } : item) }
        })
      }
      // Steering and follow-up inputs can create multiple user/assistant turns inside one SSE run.
      // Reload the persisted transcript once the run settles so those turns render as separate bubbles.
      if (goalMode || queuedDuringRun || sessionStatesRef.current[sessionId]?.hadQueuedInput || sessionStatesRef.current[sessionId]?.goal || sessionStatesRef.current[sessionId]?.queuedInputs?.length) {
        await loadSessionMessages(sessionId, { force: true })
        updateSessionState(sessionId, { queuedInputs: [], hadQueuedInput: false })
      }
      let completed
      try {
        const sessions = await refreshSessions()
        completed = sessions.find((session) => session.id === sessionId)
      } catch {
        void syncLiveSession(sessionId)
      }
      browserNotify?.('chat.completed', { chat: { title: completed?.name || t('{app} 对话', { app: APP_NAME }), summary: responseText.trim().slice(0, 260) || t('Agent 已完成回复。'), model: sessionStatesRef.current[sessionId]?.model || model } })
    } catch (caught) {
      typewriter.cancel()
      thinkingScheduler.cancel()
      toolScheduler.cancel()
      const runFinishedAt = new Date().toISOString()
      updateSessionState(sessionId, (current) => ({ ...current, streaming: false, thinkingText: '', error: caught.message, runFinishedAt, lastActivityAt: runFinishedAt, runNotice: '', currentActivity: null, activityFeed: [], approvals: [], tools: settleToolCalls(current.tools, { finishedAt: runFinishedAt, error: caught.message }), messages: current.messages.map((item) => item.id === agentId ? { ...item, streaming: false, error: caught.message, text: item.text || responseText || caught.message } : item) }))
      if (queuedDuringRun || sessionStatesRef.current[sessionId]?.hadQueuedInput || sessionStatesRef.current[sessionId]?.queuedInputs?.length) {
        try { await loadSessionMessages(sessionId, { force: true }) } catch {}
        updateSessionState(sessionId, { queuedInputs: [], hadQueuedInput: false })
      }
      setRemoteSessions((current) => current.map((session) => session.id === sessionId ? { ...session, streaming: false } : session))
    } finally {
      typewriter.cancel()
      thinkingScheduler.cancel()
      toolScheduler.cancel()
      updateSessionState(sessionId, { streaming: false, thinkingText: '', hadQueuedInput: false })
      window.dispatchEvent(new Event(USAGE_UPDATED_EVENT))
    }
  }

  const queuePrompt = async (text, sessionId, behavior = 'steer') => {
    const message = String(text || '').trim()
    if (!sessionId || !message) return false
    try {
      const result = await apiJson(`/api/sessions/${encodeURIComponent(sessionId)}/input`, {
        method: 'POST',
        body: JSON.stringify({ message, behavior }),
      })
      const queuedAt = new Date().toISOString()
      const queuedMessage = {
        id: `interactive-user-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
        role: 'user',
        text: message,
        queuedAt,
        streamingBehavior: result.behavior || behavior,
      }
      updateSessionState(sessionId, (current) => ({
        ...current,
        messages: insertInteractiveUserMessage(current.messages, queuedMessage),
        queuedInputs: resolveQueuedInputs(current.queuedInputs, result.queuedInputs),
        hadQueuedInput: true,
        lastActivityAt: queuedAt,
      }))
      return true
    } catch (caught) {
      notify(caught.message, 'error')
      return false
    }
  }

  const abort = async (sessionId) => {
    if (!sessionId) return
    const result = await apiJson(`/api/sessions/${encodeURIComponent(sessionId)}/abort`, { method: 'POST', body: '{}' })
    const runFinishedAt = new Date().toISOString()
    updateSessionState(sessionId, (current) => ({ ...current, streaming: false, thinkingText: '', queuedInputs: [], hadQueuedInput: false, goal: result.goal ?? null, compaction: current.compaction?.active ? { ...current.compaction, active: false, status: 'aborted', aborted: true, finishedAt: runFinishedAt } : current.compaction, runFinishedAt, lastActivityAt: runFinishedAt, runStopped: true, runNotice: '', approvals: [], tools: settleToolCalls(current.tools, { finishedAt: runFinishedAt, error: t('已停止') }), messages: current.messages.map((item) => item.streaming ? { ...item, streaming: false } : item) }))
    setRemoteSessions((current) => current.map((session) => session.id === sessionId ? { ...session, streaming: false, goal: result.goal ?? session.goal ?? null } : session))
    notify(t('已停止当前运行'), 'info')
  }

  const pauseGoal = async (sessionId) => {
    if (!sessionId) return
    try {
      const result = await apiJson(`/api/sessions/${encodeURIComponent(sessionId)}/goal`, {
        method: 'PATCH', body: JSON.stringify({ action: 'pause' }),
      })
      updateSessionState(sessionId, { goal: result.goal || null })
      setRemoteSessions((current) => current.map((session) => session.id === sessionId ? { ...session, goal: result.goal || null } : session))
      notify(t('Goal 已暂停'), 'info')
    } catch (caught) {
      updateSessionState(sessionId, { error: caught.message })
    }
  }

  const switchSessionModel = async (sessionId, nextModel) => {
    const selected = availableModels.find((item) => item.key === nextModel)
    if (!sessionId || !selected || sessionStatesRef.current[sessionId]?.streaming) return
    updateSessionState(sessionId, { switchingModel: true, error: '' })
    try {
      const updated = await apiJson(`/api/sessions/${encodeURIComponent(sessionId)}/model`, {
        method: 'PUT',
        body: JSON.stringify({ provider: selected.provider, model: selected.modelId }),
      })
      updateSessionState(sessionId, { model: updated.model, contextUsage: updated.contextUsage ?? null, switchingModel: false })
      setRemoteSessions((current) => current.map((session) => session.id === sessionId ? { ...session, model: updated.model } : session))
      notify(t('已切换至 {model}', { model: selected.label }))
    } catch (caught) {
      updateSessionState(sessionId, { switchingModel: false, error: caught.message })
    }
  }

  const refreshSandboxStatus = useCallback(async () => {
    const next = await apiJson('/api/sandbox/status')
    setSandboxStatus(next)
    return next
  }, [])

  const ensureWorkspaceSandbox = async () => {
    const status = await refreshSandboxStatus()
    if (status.state === 'ready' || status.state === 'active') return true
    if (status.platform !== 'win32' || status.state !== 'not-installed') {
      notify(t('当前设备无法启用工作区沙箱：{reason}', { reason: status.message || status.errors?.join('、') || t('环境不受支持') }), 'error')
      return false
    }
    const confirmed = await requestConfirm({
      title: t('首次启用安全执行'),
      message: t('Vesper 将创建低权限沙箱账户并配置网络隔离。Windows 会显示一次管理员权限确认，Agent 不会获得管理员权限。'),
      confirmLabel: t('继续安装'),
      tone: 'primary',
    })
    if (!confirmed) return false
    setSandboxStatus((current) => ({ ...current, state: 'installing' }))
    const installed = await apiJson('/api/sandbox/install', { method: 'POST', body: '{}' })
    setSandboxStatus(installed)
    if (installed.cancelled) { notify(t('沙箱安装已取消'), 'info'); return false }
    if (installed.state !== 'ready' && installed.state !== 'active') {
      notify(t('本地沙箱安装失败：{reason}', { reason: installed.message || t('请检查系统设置后重试') }), 'error')
      return false
    }
    notify(t('本地沙箱已启用'))
    return true
  }

  const switchSessionExecutionMode = async (sessionId, executionMode) => {
    if (!sessionId) return false
    updateSessionState(sessionId, { switchingPermission: true, error: '' })
    try {
      if (executionMode === 'workspace' && !(await ensureWorkspaceSandbox())) {
        updateSessionState(sessionId, { switchingPermission: false })
        return false
      }
      if (executionMode === 'full-access') {
        const confirmed = await requestConfirm({
          title: t('启用完全访问'),
          message: t('完全访问允许 Agent 操作工作区之外的文件和网络服务，并且 Shell 命令不再使用本地沙箱。'),
          confirmLabel: t('启用完全访问'),
        })
        if (!confirmed) {
          updateSessionState(sessionId, { switchingPermission: false })
          return false
        }
      }
      const updated = await apiJson(`/api/sessions/${encodeURIComponent(sessionId)}/execution-mode`, {
        method: 'PUT', body: JSON.stringify({ mode: executionMode }),
      })
      updateSessionState(sessionId, { executionMode: updated.executionMode, permissionMode: updated.permissionMode, switchingPermission: false })
      setRemoteSessions((current) => current.map((session) => session.id === sessionId ? { ...session, executionMode: updated.executionMode, permissionMode: updated.permissionMode } : session))
      notify(t('执行模式已切换为{mode}', { mode: t(updated.executionMode === 'read-only' ? '只读' : updated.executionMode === 'full-access' ? '完全访问' : '工作区') }))
      return true
    } catch (caught) {
      updateSessionState(sessionId, { switchingPermission: false, error: caught.message })
      return false
    }
  }

  const resolveToolApproval = async (sessionId, approvalId, approved) => {
    updateSessionState(sessionId, (current) => ({ ...current, approvals: (current.approvals || []).filter((item) => item.id !== approvalId), error: '' }))
    try {
      const resolution = await apiJson(`/api/sessions/${encodeURIComponent(sessionId)}/approvals/${encodeURIComponent(approvalId)}`, {
        method: 'POST', body: JSON.stringify({ approved }),
      })
      if (resolution.alreadyResolved) void syncLiveSession(sessionId)
    } catch (caught) {
      await syncLiveSession(sessionId)
      if (caught.status === 404) {
        notify(t('授权状态已更新'), 'info')
        return
      }
      updateSessionState(sessionId, { error: caught.message })
      throw caught
    }
  }

  const switchSessionCwd = async (session, cwd) => {
    if (!session?.id || sessionStatesRef.current[session.id]?.streaming) return
    updateSessionState(session.id, { switchingCwd: true, error: '' })
    try {
      const updated = await apiJson(`/api/sessions/${encodeURIComponent(session.id)}/cwd`, {
        method: 'PUT',
        body: JSON.stringify({ cwd }),
      })
      updateSessionState(session.id, { cwd: updated.cwd, switchingCwd: false })
      setRemoteSessions((current) => current.map((item) => item.id === session.id ? { ...item, cwd: updated.cwd } : item))
      setWorkspaceSession(null)
      notify(t('工作目录已切换至 {workspace}', { workspace: workspaceName(updated.cwd) }))
    } catch (caught) {
      updateSessionState(session.id, { switchingCwd: false, error: caught.message })
      throw caught
    }
  }

  const renameSession = async (session) => {
    const name = await requestText({ title: t('重命名会话'), inputLabel: t('会话标题'), value: session.name, confirmLabel: t('保存') })
    if (name === null || name === session.name) return
    try {
      const updated = await apiJson(`/api/sessions/${encodeURIComponent(session.id)}`, {
        method: 'PATCH',
        body: JSON.stringify({ name }),
      })
      setRemoteSessions((current) => current.map((item) => item.id === session.id ? { ...item, name: updated.name } : item))
      announceSessionsUpdated()
      notify(t('会话标题已更新'))
    } catch (caught) {
      setError(caught.message)
    }
  }

  const dockComponents = useMemo(() => ({ session: SessionDockPanel }), [])
  const getTabContextMenuItems = useCallback(({ panel }) => [
    { label: t('拆分到左侧'), action: () => splitDockPanel(panel.id, 'left'), disabled: compactDock || panel.group.size <= 1 },
    { label: t('拆分到右侧'), action: () => splitDockPanel(panel.id, 'right'), disabled: compactDock || panel.group.size <= 1 },
    'separator',
    'close',
  ], [compactDock, splitDockPanel, t])
  const activeSession = remoteSessions.find((session) => session.id === activeId)
  const activeState = sessionStates[activeId] || { messages: [], tools: [], approvals: [], taskList: null, streaming: false, error: '', loading: false, switchingModel: false, switchingCwd: false, switchingPermission: false, messageStart: null, hasOlder: false, olderCursor: null }
  const announcedModel = activeState.model || activeSession?.model || ''

  useEffect(() => {
    announceActiveSession(activeId, announcedModel)
  }, [activeId, announcedModel])

  useEffect(() => {
    document.title = activeSession?.name ? `${activeSession.name} · ${APP_NAME}` : APP_NAME
    return () => { document.title = APP_NAME }
  }, [activeSession?.name])

  const dockContextValue = {
    sessions: remoteSessions,
    sessionStates,
    defaultModel: model,
    availableModels,
    globalError: error,
    activeId,
    compactDock,
    pendingAsset,
    onAssetConsumed,
    loadSessionMessages,
    loadOlderMessages,
    sendPrompt,
    queuePrompt,
    abort,
    pauseGoal,
    switchSessionModel,
    switchSessionExecutionMode,
    sandboxStatus,
    resolveToolApproval,
    setWorkspaceSession,
    renameSession,
    splitDockPanel,
    closeDockPanel,
    openRail: railOpen ? null : () => setRailOpen(true),
  }

  return (
    <>
    <div className={`chat-layout dock-layout ${railOpen ? 'rail-open' : ''}`}>
      {loading ? <Panel className="empty-state"><RefreshCw className="spin" size={24} /><h2>{t('正在唤醒 Agent')}</h2><p>{t('模型、会话与上下文正在依次归位…')}</p></Panel> : <>
        {railOpen && <SessionRail sessions={remoteSessions} states={sessionStates} activeId={activeId} splitEnabled={!compactDock} onSelect={(id) => openSessionInDock(id, 'open')} onSplit={(id, direction) => openSessionInDock(id, direction)} onCreate={createSession} onClose={() => setRailOpen(false)} />}
        <div className="chat-dock-workspace">
          <ChatDockContext.Provider value={dockContextValue}>
            <DockviewReact
              className="dockview-theme-light dockview-theme-vesper"
              components={dockComponents}
              watermarkComponent={ChatDockWatermark}
              onReady={onDockReady}
              getTabContextMenuItems={getTabContextMenuItems}
              disableFloatingGroups
              disableDnd={compactDock}
              noPanelsOverlay="watermark"
            />
          </ChatDockContext.Provider>
        </div>
      </>}
    </div>
    {workspaceSession && <WorkspacePicker session={workspaceSession} onClose={() => setWorkspaceSession(null)} onSelect={(cwd) => switchSessionCwd(workspaceSession, cwd)} />}
    </>
  )
}

function SessionDockPanel({ params, api }) {
  const context = useContext(ChatDockContext)
  const sessionId = params?.sessionId || sessionIdFromPanel(api?.id)
  const session = context?.sessions.find((item) => item.id === sessionId)
  const state = context?.sessionStates[sessionId] || DEFAULT_SESSION_STATE
  const loadMessages = context?.loadSessionMessages

  useEffect(() => {
    if (sessionId) void loadMessages?.(sessionId, { limit: FOCUS_MESSAGE_PAGE_SIZE })
  }, [loadMessages, sessionId])

  if (!context || !session) return <div className="session-dock-missing"><AlertTriangle size={16} />{context ? 'Session unavailable' : 'Loading session'}</div>
  const pending = context.pendingAsset?.asset && (!context.pendingAsset.targetSessionId || context.pendingAsset.targetSessionId === sessionId)
    ? context.pendingAsset.asset
    : null
  return <div className="session-dock-panel" onFocusCapture={() => api.setActive()}>
    <FocusSession
      session={session}
      messages={state.messages || EMPTY_LIST}
      messageStart={state.messageStart}
      hasOlder={state.hasOlder}
      loadingOlder={state.loadingOlder}
      olderError={state.olderError}
      model={state.model || session.model || context.defaultModel}
      executionMode={state.executionMode || session.executionMode || 'workspace'}
      sandboxStatus={context.sandboxStatus}
      goal={state.goal ?? session.goal ?? null}
      currentActivity={state.currentActivity}
      activityFeed={state.activityFeed || EMPTY_LIST}
      thinkingText={state.thinkingText || ''}
      queuedInputs={state.queuedInputs || EMPTY_LIST}
      compaction={state.compaction}
      contextUsage={state.contextUsage}
      cwd={state.cwd || session.cwd}
      availableModels={context.availableModels}
      switchingModel={state.switchingModel}
      switchingCwd={state.switchingCwd}
      switchingPermission={state.switchingPermission}
      streaming={state.streaming}
      runStartedAt={state.runStartedAt}
      lastActivityAt={state.lastActivityAt}
      runFinishedAt={state.runFinishedAt}
      runStopped={state.runStopped}
      runNotice={state.runNotice}
      approvals={state.approvals || EMPTY_LIST}
      error={state.error || (context.activeId === sessionId ? context.globalError : '')}
      pendingAsset={pending}
      onAssetConsumed={context.onAssetConsumed}
      onLoadOlder={() => context.loadOlderMessages(sessionId)}
      onModelChange={(nextModel) => context.switchSessionModel(sessionId, nextModel)}
      onExecutionModeChange={(nextMode) => context.switchSessionExecutionMode(sessionId, nextMode)}
      onGoalPause={() => context.pauseGoal(sessionId)}
      onApproval={(approvalId, approved) => context.resolveToolApproval(sessionId, approvalId, approved)}
      onWorkspace={() => context.setWorkspaceSession(session)}
      onRename={() => context.renameSession(session)}
      onSplitLeft={() => context.splitDockPanel(api.id, 'left')}
      onSplitRight={() => context.splitDockPanel(api.id, 'right')}
      onClosePanel={() => context.closeDockPanel(api.id)}
      canSplit={!context.compactDock && api.group.size > 1}
      onSend={(value, attachments, goalMode) => context.sendPrompt(value, sessionId, attachments, goalMode)}
      onQueue={(value, behavior) => context.queuePrompt(value, sessionId, behavior)}
      onAbort={() => context.abort(sessionId)}
      onOpenRail={context.openRail}
    />
  </div>
}

function ChatDockWatermark() {
  const context = useContext(ChatDockContext)
  const { t } = useI18n()
  return <div className="chat-dock-watermark"><MessageSquare size={34} /><strong>{t('打开一个会话开始工作')}</strong><span>{t('从左侧会话列表打开，或拆分到左侧或右侧。')}</span>{context?.openRail && <button type="button" className="button secondary" onClick={context.openRail}><PanelLeftOpen size={14} />{t('展开会话列表')}</button>}</div>
}

function SessionRail({ sessions, states, activeId, splitEnabled, onSelect, onSplit, onCreate, onClose }) {
  const { t, language } = useI18n()
  const [railQuery, setRailQuery] = useState('')
  const filtered = useMemo(() => {
    const keyword = railQuery.trim().toLowerCase()
    return [...sessions]
      .sort((a, b) => Date.parse(b.modified) - Date.parse(a.modified))
      .filter((session) => !keyword || `${session.name} ${session.firstMessage || ''}`.toLowerCase().includes(keyword))
  }, [sessions, railQuery])
  return (
    <aside className="session-rail" aria-label={t('会话列表')}>
      <div className="session-rail-head">
        <strong>{t('会话')}</strong>
        <button className="icon-button" title={t('新会话')} aria-label={t('新会话')} onClick={onCreate}><Plus size={15} /></button>
        <button className="icon-button" title={t('收起会话列表')} aria-label={t('收起会话列表')} onClick={onClose}><PanelLeftClose size={15} /></button>
      </div>
      <label className="session-rail-search"><Search size={13} /><input value={railQuery} onChange={(event) => setRailQuery(event.target.value)} placeholder={t('搜索会话')} /></label>
      <div className="session-rail-list">
        {filtered.map((session) => {
          const state = states[session.id]
          const streaming = Boolean(state?.streaming)
          const compacting = Boolean(state?.compaction?.active)
          return <div className={`session-rail-row ${session.id === activeId ? 'active' : ''}`} key={session.id}>
            <button className="session-rail-item" onClick={() => onSelect(session.id)}>
              <span className="session-rail-item-name">{streaming && <i className="session-rail-live" />}{session.name || t('未命名会话')}</span>
              <span className="session-rail-item-meta">{compacting ? t('正在压缩上下文') : streaming ? t('Agent 运行中') : t('{count} 条消息', { count: session.messageCount || 0 })} · {relativeTime(session.modified, language)}</span>
            </button>
            {splitEnabled && <div className="session-rail-split-actions">
              <button type="button" title={t('拆分到左侧')} aria-label={t('将 {name} 拆分到左侧', { name: session.name })} onClick={() => onSplit(session.id, 'left')}><PanelLeft size={13} /></button>
              <button type="button" title={t('拆分到右侧')} aria-label={t('将 {name} 拆分到右侧', { name: session.name })} onClick={() => onSplit(session.id, 'right')}><PanelRight size={13} /></button>
            </div>}
          </div>
        })}
        {!filtered.length && <span className="session-rail-empty">{t(railQuery.trim() ? '没有匹配的会话' : '暂无历史会话')}</span>}
      </div>
    </aside>
  )
}

function ContextUsageIndicator({ usage }) {
  const { t } = useI18n()
  const contextWindow = Number(usage?.contextWindow) || 0
  if (!contextWindow) return null
  const known = usage?.percent != null && Number.isFinite(Number(usage.percent))
  const percent = known ? Math.max(0, Number(usage.percent)) : null
  const roundedPercent = percent == null ? null : Math.round(percent)
  const compactAtPercent = usage?.compactAtPercent == null ? null : Number(usage.compactAtPercent)
  const warningAt = compactAtPercent == null ? 75 : Math.max(50, compactAtPercent - 15)
  const tone = percent == null ? 'unknown' : compactAtPercent != null && percent >= compactAtPercent ? 'danger' : percent >= warningAt ? 'warning' : 'normal'
  const usageText = known
    ? t(usage.estimated ? '上下文占用（估算）：{tokens} / {limit} tokens（{percent}%）' : '上下文占用：{tokens} / {limit} tokens（{percent}%）', {
        tokens: formatTokenCount(usage.tokens),
        limit: formatTokenCount(contextWindow),
        percent: roundedPercent,
      })
    : t('上下文占用将在下一次模型响应后更新；上限 {limit} tokens', { limit: formatTokenCount(contextWindow) })
  const thresholdText = usage.autoCompactEnabled && compactAtPercent != null
    ? t('自动压缩阈值约 {percent}%', { percent: Math.round(compactAtPercent) })
    : t('自动上下文压缩未启用')
  const label = `${usageText} · ${thresholdText}`
  const tokenLabel = `${usage?.tokens == null ? '—' : formatTokenCount(usage.tokens)} / ${formatTokenCount(contextWindow)}`
  return <div className={`context-usage-chip ${tone}`} role="status" aria-label={label} title={label}>
    <Gauge size={12} />
    <span><strong>{tokenLabel}</strong><small>{roundedPercent == null ? '—' : `${roundedPercent}%`}</small></span>
    <i aria-hidden="true"><b style={{ width: `${Math.min(100, percent || 0)}%` }} /></i>
  </div>
}

function SessionModelSelect({ value, models, onChange, disabled, compact = false }) {
  const { t } = useI18n()
  const currentModel = models.find((model) => model.key === value)
  const hasCurrentModel = Boolean(currentModel)
  const currentLabel = currentModel ? `${currentModel.providerName} · ${currentModel.label}` : value.split('/').at(-1)
  return (
    <label className={`session-model-select icon-only ${compact ? 'compact' : ''}`} title={t(disabled ? '当前模型：{model}（运行期间不可切换）' : '当前模型：{model}，点击切换', { model: currentLabel })}>
      <Bot size={compact ? 11 : 14} />
      <select value={value} onChange={(event) => onChange(event.target.value)} disabled={disabled || models.length === 0} aria-label={t('当前会话模型')}>
        {!hasCurrentModel && <option value={value}>{value.split('/').at(-1)}</option>}
        {models.map((model) => <option key={model.key} value={model.key}>{model.providerName} · {model.label}</option>)}
      </select>
    </label>
  )
}

function welcomeChips(t) {
  return [
    { label: t('解释代码'), prompt: t('解释这段代码的工作原理：') },
    { label: t('写单测'), prompt: t('为以下代码编写单元测试：') },
    { label: t('重构'), prompt: t('重构这段代码并说明改进点：') },
    { label: t('查 bug'), prompt: t('帮我定位并修复这个 bug：') },
  ]
}

function executionModeOptions(t, sandboxStatus) {
  const workspaceDescription = sandboxStatus?.state === 'not-installed'
    ? t('可以修改当前项目；首次使用需要安装本地沙箱')
    : sandboxStatus?.supported === false || ['unsupported', 'unavailable', 'error'].includes(sandboxStatus?.state)
      ? t('当前设备无法使用本地沙箱')
      : t('可以修改当前项目；越界操作需要确认')
  return [
    ['read-only', t('只读'), t('只能查看和分析代码'), Eye],
    ['workspace', t('工作区'), workspaceDescription, Shield],
    ['full-access', t('完全访问'), t('不使用沙箱，可以访问本机文件和网络'), ShieldOff],
  ]
}

function ExecutionModeSelect({ value, sandboxStatus, onChange, disabled, compact = false }) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [menuPosition, setMenuPosition] = useState({ left: 0, top: 0, width: 270 })
  const rootRef = useRef(null)
  const menuRef = useRef(null)
  const options = executionModeOptions(t, sandboxStatus)
  const current = options.find((item) => item[0] === value) || options[1]
  const CurrentIcon = current[3]
  const sandboxTone = sandboxStatus?.state === 'installing' || sandboxStatus?.state === 'checking'
    ? 'checking'
    : sandboxStatus?.state === 'not-installed' || sandboxStatus?.state === 'unavailable' || sandboxStatus?.state === 'error'
      ? 'warning'
      : sandboxStatus?.supported === false
        ? 'danger'
        : 'ready'
  const positionMenu = useCallback(() => {
    const trigger = rootRef.current?.querySelector('button')
    if (!trigger) return
    const rect = trigger.getBoundingClientRect()
    const edge = 8
    const gap = 8
    const width = Math.min(270, window.innerWidth - edge * 2)
    const height = menuRef.current?.offsetHeight || 190
    const left = Math.max(edge, Math.min(rect.right - width, window.innerWidth - width - edge))
    const top = rect.top >= height + gap + edge
      ? rect.top - height - gap
      : Math.min(rect.bottom + gap, window.innerHeight - height - edge)
    setMenuPosition({ left, top: Math.max(edge, top), width })
  }, [])
  useLayoutEffect(() => {
    if (!open) return undefined
    positionMenu()
    window.addEventListener('resize', positionMenu)
    window.addEventListener('scroll', positionMenu, true)
    return () => {
      window.removeEventListener('resize', positionMenu)
      window.removeEventListener('scroll', positionMenu, true)
    }
  }, [open, positionMenu])
  useEffect(() => {
    if (!open) return undefined
    const close = (event) => {
      if (!rootRef.current?.contains(event.target) && !menuRef.current?.contains(event.target)) setOpen(false)
    }
    const escape = (event) => { if (event.key === 'Escape') setOpen(false) }
    document.addEventListener('mousedown', close)
    document.addEventListener('keydown', escape)
    return () => { document.removeEventListener('mousedown', close); document.removeEventListener('keydown', escape) }
  }, [open])
  const menu = open && createPortal(<div ref={menuRef} className="permission-mode-menu execution-mode-menu !fixed !right-auto !bottom-auto z-[80]" style={menuPosition} role="menu"><div className="execution-mode-menu-title">{t('执行模式')}</div>{options.map(([mode, label, description, Icon]) => <button type="button" role="menuitemradio" aria-checked={mode === current[0]} className={mode === current[0] ? 'active' : ''} onClick={() => { onChange(mode); setOpen(false) }} key={mode}><span className={`permission-level level-${mode}`}><Icon size={13} /></span><span><strong>{label}{mode === 'workspace' && <small className="recommended-mode">{t('推荐')}</small>}</strong><small>{description}</small></span>{mode === current[0] && <Check size={13} />}</button>)}</div>, document.body)
  return <><div ref={rootRef} className={`permission-mode-select execution-mode-select icon-only ${compact ? 'compact' : ''} ${open ? 'open' : ''}`}><button type="button" className={`permission-mode-trigger icon-only mode-${current[0]}`} title={t('执行模式：{mode}——{description}', { mode: current[1], description: current[2] })} disabled={disabled} aria-haspopup="menu" aria-expanded={open} aria-label={t('执行模式：{mode}', { mode: current[1] })} onClick={() => setOpen((visible) => !visible)}><CurrentIcon size={compact ? 11 : 14} />{value === 'workspace' && <i className={`sandbox-status-dot ${sandboxTone}`} />}</button></div>{menu}</>
}

function ToolApproval({ approvals, onResolve, compact = false }) {
  const { t } = useI18n()
  const [resolving, setResolving] = useState(false)
  const resolvingRef = useRef(false)
  const approval = approvals[0]
  if (!approval) return null
  const escalated = approval.toolName === 'bash' && approval.args?.sandbox_permissions === 'require_escalated'
  const resolve = async (approved) => {
    if (resolvingRef.current) return
    resolvingRef.current = true
    setResolving(true)
    try { await onResolve(approval.id, approved) } finally { resolvingRef.current = false; setResolving(false) }
  }
  return <div className={`tool-approval ${compact ? 'compact' : ''}`}><div><ShieldCheck size={compact ? 12 : 15} /><span><strong>{t(escalated ? '{tool} 请求在沙箱外执行' : '{tool} 请求授权', { tool: approval.toolName })}</strong><small>{approval.reason}{approvals.length > 1 ? ` · ${t('另有 {count} 项等待', { count: approvals.length - 1 })}` : ''}</small></span></div>{!compact && <details><summary>{t('查看调用参数')}</summary><pre>{JSON.stringify(approval.args, null, 2)}</pre></details>}<div className="tool-approval-actions"><button type="button" className="button secondary" disabled={resolving} onClick={() => resolve(false)}>{t('拒绝')}</button><button type="button" className="button primary" disabled={resolving} onClick={() => resolve(true)}>{resolving ? <RefreshCw className="spin" size={12} /> : <Check size={12} />}{t(escalated ? '本次允许' : '允许')}</button></div></div>
}

function FocusSession({ session, messages, messageStart, hasOlder, loadingOlder, olderError, model, executionMode, sandboxStatus, goal, currentActivity, activityFeed, thinkingText, queuedInputs, compaction, contextUsage, cwd, availableModels, switchingModel, switchingCwd, switchingPermission, streaming, runStartedAt, lastActivityAt, runFinishedAt, runStopped, runNotice, approvals, error, pendingAsset, canSplit, onAssetConsumed, onLoadOlder, onModelChange, onExecutionModeChange, onGoalPause, onApproval, onWorkspace, onRename, onSplitLeft, onSplitRight, onClosePanel, onSend, onQueue, onAbort, onOpenRail }) {
  const { t, language } = useI18n()
  const [value, setValue] = useState('')
  const [goalArmed, setGoalArmed] = useState(false)
  const [queueing, setQueueing] = useState(false)
  const selection = useAttachmentSelection()
  const addSelectedAttachments = selection.addAttachments
  const promptRef = useRef(null)
  const prependSnapshot = useRef(null)
  const lastMessage = messages[messages.length - 1]
  // Bucket streaming text length so auto-scroll does not fire on every token.
  const textScrollBucket = Math.floor((lastMessage?.text?.length || 0) / 64)
  const activityVersion = activityFeed.map((activity) => `${activity.type}:${activity.id || activity.agent?.id || ''}:${activity.status || activity.stage || activity.agent?.status || ''}:${activity.updatedAt || ''}`).join('|')
  const transcriptVersion = `${session?.id || ''}:${lastMessage?.id || ''}:${textScrollBucket}:${lastMessage?.attachments?.length || 0}:${activityVersion}:${goal?.status || ''}:${goal?.tokensUsed || 0}:${compaction?.status || ''}:${compaction?.finishedAt || ''}:${error || ''}:${streaming ? '1' : '0'}`
  const { scrollRef: transcriptRef, onScroll: onTranscriptScroll, hasUnread, scrollToBottom } = useAutoScroll(transcriptVersion)
  const latestRunProps = useMemo(() => ({
    streaming,
    text: lastMessage?.role === 'agent' ? lastMessage.text : '',
    currentActivity,
    activityFeed,
    thinkingText,
    compaction,
    error: error || (lastMessage?.role === 'agent' ? lastMessage.error : ''),
    stopped: runStopped,
    notice: runNotice,
    startedAt: runStartedAt,
    lastActivityAt,
    finishedAt: runFinishedAt,
  }), [streaming, lastMessage, currentActivity, activityFeed, thinkingText, compaction, error, runStopped, runNotice, runStartedAt, lastActivityAt, runFinishedAt])
  const loadOlder = useCallback(async () => {
    const node = transcriptRef.current
    if (!node || !hasOlder || loadingOlder || prependSnapshot.current) return
    prependSnapshot.current = { scrollHeight: node.scrollHeight, scrollTop: node.scrollTop }
    const loaded = await onLoadOlder?.()
    if (!loaded) prependSnapshot.current = null
  }, [hasOlder, loadingOlder, onLoadOlder, transcriptRef])
  const handleTranscriptScroll = useCallback((event) => {
    onTranscriptScroll(event)
    if (event.currentTarget.scrollTop <= 96) void loadOlder()
  }, [loadOlder, onTranscriptScroll])
  useLayoutEffect(() => {
    const snapshot = prependSnapshot.current
    const node = transcriptRef.current
    if (!snapshot || !node) return
    node.scrollTop = snapshot.scrollTop + node.scrollHeight - snapshot.scrollHeight
    prependSnapshot.current = null
  }, [messageStart, transcriptRef])
  useEffect(() => {
    setGoalArmed(false)
    setQueueing(false)
  }, [session?.id])
  useEffect(() => {
    if (!pendingAsset) return
    addSelectedAttachments([pendingAsset])
    onAssetConsumed?.()
  }, [pendingAsset, onAssetConsumed, addSelectedAttachments])
  const applyWelcomeChip = (prompt) => {
    setValue(prompt)
    requestAnimationFrame(() => {
      const el = promptRef.current
      if (!el) return
      el.focus()
      el.style.height = 'auto'
      el.style.height = `${Math.min(el.scrollHeight, 220)}px`
    })
  }
  const submit = async (event) => {
    event.preventDefault()
    if (!value.trim() && !selection.attachments.length) return
    if (streaming) {
      if (!value.trim() || queueing) return
      setQueueing(true)
      const queued = await onQueue?.(value, 'steer')
      setQueueing(false)
      if (!queued) return
      setValue('')
      setGoalArmed(false)
      requestAnimationFrame(() => scrollToBottom('smooth'))
      if (promptRef.current) promptRef.current.style.height = 'auto'
      return
    }
    if (executionMode === 'workspace' && !['ready', 'active'].includes(sandboxStatus?.state)) {
      const ready = await onExecutionModeChange('workspace')
      if (!ready) return
    }
    onSend(value, selection.attachments, goalArmed)
    scrollToBottom('smooth')
    setValue('')
    setGoalArmed(false)
    if (promptRef.current) promptRef.current.style.height = 'auto'
    selection.clearAttachments()
  }
  return (
    <Panel className="focus-session">
      <div className="card-head"><div className="session-runtime-meta">{onOpenRail && <button className="icon-button session-rail-open-btn" title={t('展开会话列表')} aria-label={t('展开会话列表')} onClick={onOpenRail}><PanelLeftOpen size={15} /></button>}<span className={streaming ? 'success' : ''}>{t(compaction?.active ? '正在压缩上下文' : streaming ? 'Agent 运行中' : '等待输入')}</span><button className="workspace-chip" title={cwd} onClick={onWorkspace} disabled={streaming || switchingCwd}><FolderOpen size={11} />{workspaceName(cwd, language)}</button></div><div className="focus-session-head-actions">{streaming && <button className="button danger tiny" onClick={onAbort}><Square size={12} />{t('停止')}</button>}<SessionActionsMenu session={session} canSplit={canSplit} streaming={streaming} switchingCwd={switchingCwd} onSplitLeft={onSplitLeft} onSplitRight={onSplitRight} onClosePanel={onClosePanel} onWorkspace={onWorkspace} onRename={onRename} /></div></div>
      <div className="transcript" ref={transcriptRef} onScroll={handleTranscriptScroll}>
        {(hasOlder || loadingOlder || olderError) && <div className="history-page-loader">{olderError ? <button type="button" className="button secondary" onClick={loadOlder}><RefreshCw size={13} />{t('重试加载更早消息')}</button> : loadingOlder ? <><RefreshCw className="spin" size={14} />{t('正在加载更早消息…')}</> : <button type="button" className="button secondary" onClick={loadOlder}><ArrowDown className="history-up-arrow" size={14} />{t('加载更早消息')}</button>}</div>}
        {!messages.length && <div className="agent-welcome"><BrandLogo size={44} className="welcome-logo" /><h2>{t('让我们从一束想法开始')}</h2><p>{t('Vesper 已准备好读取当前工作区、搜索代码，并陪你把任务推进到完成。默认在工作区沙箱中运行。')}</p><div className="welcome-chips">{welcomeChips(t).map((chip) => <button type="button" key={chip.label} onClick={() => applyWelcomeChip(chip.prompt)}>{chip.label}</button>)}</div></div>}
        {messages.map((message, index) => {
          const isLatestAgent = message.role === 'agent' && index === messages.length - 1
          const agentState = message.streaming || (isLatestAgent && streaming) ? 'thinking' : isLatestAgent && !message.error ? 'waiting' : 'idle'
          const showRunActivity = isLatestAgent && streaming
          return <FocusChatMessage
            key={message.id}
            message={message}
            agentState={agentState}
            showRunActivity={showRunActivity}
            runProps={showRunActivity ? latestRunProps : null}
          />
        })}
        {error && <div className="chat-error"><AlertTriangle size={14} />{error}</div>}
      </div>
      {hasUnread && <button type="button" className="button secondary jump-to-latest" onClick={() => scrollToBottom('smooth')}><ArrowDown size={14} />{t('有新内容')}</button>}
      <form className="focus-composer-shell" onSubmit={submit}><ToolApproval approvals={approvals} onResolve={onApproval} />{streaming && queuedInputs.length > 0 && <div className="queued-input-tray"><span>{t('已发送给运行中的 Agent')}</span>{queuedInputs.slice(-3).map((item, index) => <small key={item.id || `${item.behavior}-${index}`} title={item.text}>{item.text}</small>)}{queuedInputs.length > 3 && <em>{t('另有 {count} 条', { count: queuedInputs.length - 3 })}</em>}</div>}<AttachmentTray attachments={selection.attachments} onRemove={selection.removeAttachment} />{selection.attachmentError && <span className="attachment-error">{selection.attachmentError}</span>}<div className="focus-composer"><button type="button" className="attach-trigger" title={t('添加附件')} aria-label={t('添加附件')} onClick={() => selection.inputRef.current?.click()} disabled={streaming}><Paperclip size={17} />{selection.attachments.length > 0 && <i>{selection.attachments.length}</i>}</button><input ref={selection.inputRef} className="sr-only" type="file" multiple accept="image/*,.txt,.md,.json,.js,.jsx,.ts,.tsx,.css,.html,.xml,.yaml,.yml,.csv,.log,.py,.java,.go,.rs,.sh,.ps1,.toml,.sql,.pdf,.docx,.pptx,.xlsx,.odt,.odp,.ods,.rtf,.epub" onChange={selection.chooseFiles} /><SessionModelSelect value={model} models={availableModels} onChange={onModelChange} disabled={streaming || switchingModel} /><ExecutionModeSelect value={executionMode} sandboxStatus={sandboxStatus} onChange={onExecutionModeChange} disabled={streaming || switchingPermission} /><div className="focus-composer-secondary"><ContextUsageIndicator usage={contextUsage} /><GoalModeControl goal={goal} armed={goalArmed} onChange={(enabled) => { if (!enabled && goal?.status === 'active') void onGoalPause?.(); else setGoalArmed(enabled) }} /></div><textarea ref={promptRef} rows="1" value={value} onChange={(event) => { setValue(event.target.value); event.currentTarget.style.height = 'auto'; event.currentTarget.style.height = `${Math.min(event.currentTarget.scrollHeight, 220)}px` }} onPaste={streaming ? undefined : selection.pasteImages} onKeyDown={(event) => { if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); event.currentTarget.form?.requestSubmit() } }} placeholder={t(streaming ? '向正在运行的 Agent 追加说明，Shift + Enter 换行' : '写下你想完成的事，Shift + Enter 换行')} /><button className="send-button" title={t(streaming ? '发送给正在运行的 Agent' : '发送消息')} aria-label={t(streaming ? '发送给正在运行的 Agent' : '发送消息')} disabled={queueing || (streaming ? !value.trim() : (!value.trim() && !selection.attachments.length))}>{queueing ? <RefreshCw className="spin" size={17} /> : <Send size={18} />}</button></div></form>
    </Panel>
  )
}

function GoalModeControl({ goal, armed, onChange }) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const rootRef = useRef(null)
  const active = goal?.status === 'active'
  const enabled = active || armed
  const status = active
    ? t('正在自动执行')
    : armed
      ? t(goal?.status === 'paused' ? '下一条消息将继续 Goal' : '下一条消息将启动 Goal')
      : goal?.status === 'complete'
        ? t('Goal 已完成')
        : goal?.status === 'budget_limited'
          ? t('Goal 已达到预算')
          : goal?.status === 'paused'
            ? t('Goal 已暂停')
            : t('仅对下一条消息启用')
  const objective = String(goal?.objective || '').replace(/\s+/g, ' ').trim()
  const detail = armed ? status : objective || status
  const usage = active ? t('已用 {used}/{budget} tokens', { used: goal.tokensUsed || 0, budget: goal.tokenBudget || 0 }) : ''
  const label = [t('Goal 模式'), detail, usage].filter(Boolean).join(' · ')

  useEffect(() => {
    if (!open) return undefined
    const close = (event) => { if (!rootRef.current?.contains(event.target)) setOpen(false) }
    const escape = (event) => { if (event.key === 'Escape') setOpen(false) }
    document.addEventListener('mousedown', close)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('keydown', escape)
    }
  }, [open])

  const change = (next) => {
    onChange(next)
    setOpen(false)
  }

  return <div ref={rootRef} className={`goal-mode-select ${open ? 'open' : ''} ${active || armed ? 'active' : ''}`}><button type="button" className="goal-mode-trigger" title={label} aria-label={label} aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen((visible) => !visible)}><Target size={14} /></button>{open && <div className="goal-mode-menu" role="dialog" aria-label={t('Goal 模式')}><div className="goal-mode-menu-row"><span className="goal-mode-menu-icon"><Target size={15} /></span><span><strong>{t('Goal 模式')}</strong><small title={detail}>{detail}</small></span><Toggle value={enabled} onChange={change} ariaLabel={label} title={label} /></div>{active && <p>{usage}</p>}</div>}</div>
}

function SessionActionsMenu({ session, canSplit, streaming, switchingCwd, onSplitLeft, onSplitRight, onClosePanel, onWorkspace, onRename }) {
  const { t, language } = useI18n()
  const [open, setOpen] = useState(false)
  const rootRef = useRef(null)

  useEffect(() => {
    if (!open) return undefined
    const close = (event) => { if (!rootRef.current?.contains(event.target)) setOpen(false) }
    const escape = (event) => { if (event.key === 'Escape') setOpen(false) }
    document.addEventListener('mousedown', close)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('keydown', escape)
    }
  }, [open])

  const run = (action) => {
    setOpen(false)
    action?.()
  }

  return <div ref={rootRef} className="session-actions-menu-root"><button type="button" className="icon-button" title={t('会话操作')} aria-label={t('打开会话操作菜单')} aria-haspopup="menu" aria-expanded={open} disabled={!session} onClick={() => setOpen((visible) => !visible)}><MoreHorizontal size={17} /></button>{open && <div className="permission-mode-menu session-actions-menu" role="menu"><button type="button" role="menuitem" disabled={!canSplit} onClick={() => run(onSplitLeft)}><PanelLeft size={15} /><span><strong>{t('拆分到左侧')}</strong><small>{t(canSplit ? '将当前标签移动到左侧新分组' : '当前分组只有一个会话')}</small></span></button><button type="button" role="menuitem" disabled={!canSplit} onClick={() => run(onSplitRight)}><PanelRight size={15} /><span><strong>{t('拆分到右侧')}</strong><small>{t(canSplit ? '将当前标签移动到右侧新分组' : '当前分组只有一个会话')}</small></span></button><button type="button" role="menuitem" disabled={streaming || switchingCwd} onClick={() => run(onWorkspace)}><FolderOpen size={15} /><span><strong>{t('设置工作目录')}</strong><small>{streaming ? t('Agent 运行期间不能切换') : workspaceName(session?.cwd, language)}</small></span></button><button type="button" role="menuitem" onClick={() => run(onRename)}><Pencil size={15} /><span><strong>{t('重命名会话')}</strong><small>{session?.name || t('新会话')}</small></span></button><button type="button" role="menuitem" onClick={() => run(onClosePanel)}><X size={15} /><span><strong>{t('关闭标签')}</strong><small>{t('保留历史记录，仅关闭当前视图')}</small></span></button></div>}</div>
}

function CommandPalette({ navigation, onClose, onNavigate, onOpenSession, onNewChat }) {
  const { t, language } = useI18n()
  const [query, setQuery] = useState('')
  const [sessions, setSessions] = useState([])
  const [activeIndex, setActiveIndex] = useState(0)

  useEffect(() => {
    let active = true
    apiJson('/api/sessions')
      .then((data) => { if (active) setSessions(data.sessions || []) })
      .catch(() => {})
    return () => { active = false }
  }, [])

  const entries = useMemo(() => {
    const keyword = query.trim().toLocaleLowerCase(language)
    const matches = (...values) => !keyword || values.some((value) => String(value || '').toLocaleLowerCase(language).includes(keyword))
    const result = []
    const newChatLabel = t('新会话')
    if (matches(newChatLabel, t('操作'))) result.push({ id: 'action:new-chat', Icon: Plus, label: newChatLabel, hint: t('操作'), run: onNewChat })
    for (const [group, items] of navigation) {
      for (const [id, label, Icon] of items) {
        if (matches(label, group, id)) result.push({ id: `page:${id}`, Icon, label, hint: group, run: () => onNavigate(id) })
      }
    }
    for (const session of [...sessions].sort((a, b) => Date.parse(b.modified) - Date.parse(a.modified))) {
      if (!matches(session.name, session.firstMessage)) continue
      result.push({ id: `session:${session.id}`, Icon: MessageSquare, label: session.name || t('未命名会话'), hint: relativeTime(session.modified, language), run: () => onOpenSession(session.id) })
      if (result.filter((entry) => entry.id.startsWith('session:')).length >= 8) break
    }
    return result
  }, [language, navigation, onNavigate, onNewChat, onOpenSession, query, sessions, t])

  useEffect(() => { setActiveIndex(0) }, [query])
  const selectedIndex = Math.min(activeIndex, Math.max(0, entries.length - 1))
  const runEntry = (entry) => {
    if (!entry) return
    onClose()
    entry.run()
  }
  const onKeyDown = (event) => {
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      setActiveIndex((current) => entries.length ? (current + 1) % entries.length : 0)
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      setActiveIndex((current) => entries.length ? (current - 1 + entries.length) % entries.length : 0)
    } else if (event.key === 'Enter') {
      event.preventDefault()
      runEntry(entries[selectedIndex])
    } else if (event.key === 'Escape') {
      event.preventDefault()
      onClose()
    }
  }

  return <div className="modal-backdrop palette-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
    <section className="command-palette" role="dialog" aria-modal="true" aria-label={t('命令面板')}>
      <label className="palette-input"><Search size={16} /><input autoFocus value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={onKeyDown} placeholder={t('搜索页面、会话或操作')} /><kbd>Esc</kbd></label>
      <div className="palette-list" role="listbox" aria-label={t('命令面板')}>
        {entries.map((entry, index) => <button type="button" className={`palette-item ${index === selectedIndex ? 'active' : ''}`} role="option" aria-selected={index === selectedIndex} onMouseEnter={() => setActiveIndex(index)} onClick={() => runEntry(entry)} key={entry.id}><entry.Icon size={15} /><span className="palette-item-label">{entry.label}</span><span className="palette-item-hint">{entry.hint}</span></button>)}
        {!entries.length && <div className="palette-empty">{t('无匹配结果')}</div>}
      </div>
    </section>
  </div>
}

function QuickCreate({ type, close, notify }) {
  const { t } = useI18n()
  const titles = { chat: t('新建会话'), assets: t('导出资产'), channels: t('连接渠道'), schedules: t('新建定时任务'), config: t('添加 Provider'), plugins: t('保存插件策略'), memory: t('点亮星辰'), mcp: t('添加 MCP 服务'), skills: t('安装技能') }
  return <div className="modal-backdrop" onMouseDown={close}><form className="modal" onMouseDown={(e) => e.stopPropagation()} onSubmit={(e) => { e.preventDefault(); notify(t('{action}成功', { action: titles[type] })); close() }}><div className="card-head"><div><h2>{titles[type]}</h2><p>{t('填写基本信息后即可继续配置。')}</p></div><button type="button" className="icon-button" aria-label={t('关闭对话框')} onClick={close}><X size={17} /></button></div><InputLabel label={t('名称')} value="" placeholder={t('输入名称')} /><InputLabel label={t('描述')} value="" placeholder={t('补充简短描述')} /><SelectLabel label={t('类型')} options={[t('默认'), t('自定义'), t('从模板创建')]} /><div className="modal-actions"><button type="button" className="button secondary" onClick={close}>{t('取消')}</button><button className="button primary"><Plus size={14} />{t('确认创建')}</button></div></form></div>
}

function AttachmentTray({ attachments, onRemove, compact = false }) {
  const { t } = useI18n()
  if (!attachments.length) return null
  return <div className={`attachment-tray ${compact ? 'compact' : ''}`}>{attachments.map((attachment) => <div className="attachment-chip" key={attachment.id}>{attachment.kind === 'image' ? <img src={`data:${attachment.mimeType};base64,${attachment.data}`} alt="" /> : <span className="attachment-icon"><File size={13} /></span>}<span><strong>{attachment.name}</strong><small>{t(attachment.kind === 'image' ? '图片' : attachment.kind === 'document' ? '文档' : '文本')} · {formatFileSize(attachment.size)}{attachment.truncated ? ` · ${t('已截断')}` : ''}</small></span><button type="button" aria-label={t('移除 {name}', { name: attachment.name })} onClick={() => onRemove(attachment.id)}><X size={12} /></button></div>)}</div>
}

export default App
