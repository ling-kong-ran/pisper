import { BackNavigation } from '@/components/layout/BackNavigation'
// 应用外壳：持有全站共享状态（当前页/搜索词/活动会话/Toast/对话框/通知
// 设置/更新控制器/工作流动作），组装 左侧侧栏（268，⌘B 收起为 60 窄轨）+ 主区（页头 + Outlet）
// + 手机底部标签栏，并通过 Outlet 上下文向各页面注入公共能力。规格 v2：没有标题栏了，
// 一级入口（新任务 / 搜索 / 全部代理 / 功能 / 设置）与按状态分组的会话都在侧栏里；
// 主页 '/' 是会话（上次停留的会话或新任务草稿）。设置页的分区导航暂在设置内容区左侧（A3 移进侧栏）。
// 启动时探测是否已配置可用 Provider，未配置则展示可跳过的新手引导；同时提供全局快捷键
// （⌘K 命令面板、⌘N 新任务、⌘B 侧栏、⌘J 聚焦会话列表、⌘1 全部代理、⌘2 功能、⌘, 设置、
// ⌥A 聚焦等待最久的需要你、⌥↑⌥↓ 切换会话、` 终端、/ 搜索、Esc 逐层关闭）与浏览器通知轮询。
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ComponentProps,
} from 'react'
import { useQuery } from '@tanstack/react-query'
import { Outlet, useLocation, useNavigate, type NavigateOptions } from 'react-router-dom'
import { createPrimaryActionRegistry } from '@/app/primary-action'
import {
  LOCAL_REVEAL_NOTICE_EVENT,
  type AppRouteContext,
  type Notify,
  type NotifyAction,
} from '@/app/route-context'
import { STORAGE_KEYS } from '@/app/storage'
import { pageStateStorage } from '@/lib/page-state-storage'
import { getPageMeta, getWorkflowEditorMeta } from '@/app/navigation'
import { MORE_SEARCH_INPUT_ID } from '@/app/more-pages'
import {
  HOME_ENTRY_STATE,
  HOME_PATH,
  isHomeEntry,
  PAGE_IDS,
  PAGE_PATHS,
  pageFromPath,
  pagePath,
  sessionIdFromPath,
  sessionPath,
} from '@/app/routes'
import {
  activeShellTab,
  requestedPathname,
  SHELL_TAB_PAGES,
  sessionTabNavigation,
  shellBackPath,
  type RouterLocationState,
  type ShellTab,
} from '@/app/shell-tabs'
import { ShellSidebar, SidebarSessionsFallback, type ShellSidebarEntry } from '@/app/ShellSidebar'
import {
  CONFIG_SECTIONS,
  getSettingsNavigation,
  SETTINGS_PAGES,
  settingsOwnsPage,
  type SettingsDestination,
} from '@/app/settings-navigation'
import { useI18n } from '@/app/use-i18n'
import { ensureBoardMessages, ensureCustomUiMessages } from '@/app/i18n'
import { applyUiPreferenceAttributes, resolveDarkTheme } from '@/app/ui-preferences'
import { BrandLogo } from '@/components/BrandLogo'
import { WebPreviewProvider } from '@/app/WebPreviewProvider'
import { TerminalProvider } from '@/features/terminal/public'
// 应用级快捷键随外壳同步加载：它的懒加载块只有约 0.6 kB，但 React 揭示已挂出占位的 Suspense 边界时
// 最多节流 300ms，冷启动外壳画出来之后约 0.3 秒内没有按键监听，按下的 ⌘N / ⌘1–⌘4 会直接丢失。
import { AppShortcuts } from '@/components/layout/AppShortcuts'
import { MobileSettingsNavigation } from '@/components/layout/MobileNavigation'
import { AppToast, ToastProvider, ToastViewport, type ToastTone } from '@/components/ui/toast'
import {
  ACTIVE_SESSION_CHANGED_EVENT,
  COMMAND_PALETTE_REQUESTED_EVENT,
  peekSessionNavigationIntent,
  requestSessionSelection,
  requestSessionCreation,
} from '@/features/chat/events'
import {
  BOARD_FILTER_ID,
  requestBoardNewTask,
  type BoardNewTaskSource,
} from '@/features/board/new-task'
import { requestSidebarCommand } from '@/features/board/sidebar-commands'
import { apiJson } from '@/lib/api'
import {
  fetchStartupQuery,
  invalidateStartupQuery,
  queryClient,
  startupQueryOptions,
} from '@/lib/startup-queries'
import { markStartupPhase } from '@/lib/startup-diagnostics'
import { showBrowserSystemNotification } from '@/lib/browser-notifications'
import { useAppDialog } from '@/hooks/useAppDialog'
import { useIsPhoneViewport, useIsTabletViewport } from '@/hooks/use-mobile'
import { useAppUpdate } from '@/features/updates/useAppUpdate'
import { shouldShowModelOnboarding, type ModelOnboardingConfig } from '@/features/config/public'
import { useUiStore } from '@/stores/ui-store'
import { useSidebarStore } from '@/stores/sidebar-store'
import { useClientStore } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import {
  runtimeConfigSectionAvailable,
  runtimeFeatureAvailable,
  runtimePageAvailable,
} from '@/types/runtime-capabilities'
import { readStoredTerminalPanel } from '@/features/terminal/terminal-state'
import { tauriNotificationAvailable, tauriNotificationNotify } from '@/lib/tauri-notification'
import type { ChatAttachment, PendingAsset } from '@/types/chat'
import type { NotificationSettingsData } from '@/types/notifications'
import type { WorkflowActions } from '@/types/workflow'

const DesktopWindowControls = lazy(() =>
  import('@/components/layout/DesktopWindowControls').then((m) => ({
    default: m.DesktopWindowControls,
  })),
)
const FloatingWidgets = lazy(async () => {
  const [{ FloatingWidgets }] = await Promise.all([
    import('@/app/FloatingWidgets'),
    ensureCustomUiMessages(),
  ])
  return { default: FloatingWidgets }
})
// 命令面板的页面分组带一组图标，与面板一起按需加载；分组随语言与 Runtime 能力即时计算。
const CommandPalette = lazy(async () => {
  const [{ CommandPalette }, { getNavigation }] = await Promise.all([
    import('@/components/layout/AppOverlays'),
    import('@/app/palette-navigation'),
  ])
  function AppCommandPalette(props: Omit<ComponentProps<typeof CommandPalette>, 'navigation'>) {
    const { t } = useI18n()
    const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
    const navigation = useMemo(() => getNavigation(t, capabilities), [capabilities, t])
    return <CommandPalette {...props} navigation={navigation} />
  }
  return { default: AppCommandPalette }
})
const AppDialog = lazy(() =>
  import('@/components/layout/AppDialog').then((module) => ({ default: module.AppDialog })),
)
const QuickCreate = lazy(() =>
  import('@/components/layout/AppOverlays').then((module) => ({ default: module.QuickCreate })),
)
const WebDesktopPet = lazy(() =>
  import('@/features/desktop-pet/WebDesktopPet').then((module) => ({
    default: module.WebDesktopPet,
  })),
)
const ConfigSearchBox = lazy(() =>
  import('@/features/config/public-components').then((module) => ({
    default: module.ConfigSearchBox,
  })),
)
const ModelOnboardingDialog = lazy(() =>
  import('@/features/config/public-components').then((module) => ({
    default: module.ModelOnboardingDialog,
  })),
)
// 侧栏的会话分组、窄轨计数与命令处理依赖看板数据层（单一轮询者）；文案在看板命名空间里，
// 与分组块一起按需加载，不进入启动包。外壳先画骨架行。
const loadSidebarSessions = () =>
  Promise.all([import('@/features/board/sidebar'), ensureBoardMessages()]).then(
    ([module]) => module,
  )
const SidebarSessions = lazy(() =>
  loadSidebarSessions().then((module) => ({ default: module.SidebarSessions })),
)
const SidebarRailCounts = lazy(() =>
  loadSidebarSessions().then((module) => ({ default: module.SidebarRailCounts })),
)
const SidebarCommands = lazy(() =>
  loadSidebarSessions().then((module) => ({ default: module.SidebarCommands })),
)
// 手机标签栏依赖看板数据层（单一轮询者）；按需加载，不进入启动包。
const ShellTabBar = lazy(() =>
  import('@/app/ShellTabBar').then((module) => ({ default: module.ShellTabBar })),
)
// 更新提示只在确有可用更新时加载，常驻侧栏不为它增加启动体积。
const SidebarUpdateStatus = lazy(() => import('@/components/layout/SidebarUpdateStatus'))
const SettingsSectionNav = lazy(() =>
  import('@/components/layout/SettingsSectionNav').then((module) => ({
    default: module.SettingsSectionNav,
  })),
)
// 派发浮层的文案在看板命名空间里，随浮层一起按需注册。
const DispatchOverlay = lazy(async () => {
  const [{ DispatchOverlay }] = await Promise.all([
    import('@/features/board/DispatchOverlay'),
    ensureBoardMessages(),
  ])
  return { default: DispatchOverlay }
})
// ⌘/ 快捷键速查：文案同样在看板命名空间里，随对话框按需注册。
const ShortcutHelpDialog = lazy(async () => {
  const [{ ShortcutHelpDialog }] = await Promise.all([
    import('@/features/config/ShortcutHelpDialog'),
    ensureBoardMessages(),
  ])
  return { default: ShortcutHelpDialog }
})

const PageHeader = lazy(() =>
  import('@/components/layout/PageHeader').then((module) => ({ default: module.PageHeader })),
)
const TerminalPanel = lazy(() =>
  import('@/features/terminal/TerminalPanel').then((module) => ({
    default: module.TerminalPanel,
  })),
)
const MobileViewportStabilizer = lazy(() =>
  import('@/components/layout/MobileViewportStabilizer').then((module) => ({
    default: module.MobileViewportStabilizer,
  })),
)
type ToastState = {
  id: number
  message: string
  tone: ToastTone
  action?: NotifyAction
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

// 安全解码路径段；非法编码返回空串而非抛错中断渲染。
function decodePathSegment(value: string) {
  try {
    return decodeURIComponent(value)
  } catch {
    return ''
  }
}

function invokeMobile<T>(command: string, args?: unknown): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!invoke) return Promise.reject(new Error('native bridge unavailable'))
  return invoke<T>(command, args)
}

// routerState 读数据路由的实时状态（router.tsx 注入）：导航在 transition 里提交，渲染出的 location 会落后。
function App({ routerState }: { routerState?: () => RouterLocationState }) {
  const { t } = useI18n()
  useEffect(() => markStartupPhase('react-app-mounted'), [])
  const location = useLocation()
  const routerNavigate = useNavigate()
  // “现在在哪 / 要去哪”以路由器为准；没有注入时（测试等）退回已渲染的位置，与没有进行中的导航时一致。
  const readRouter = useCallback(
    (): RouterLocationState => routerState?.() ?? { location, navigation: { state: 'idle' } },
    [location, routerState],
  )
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const capabilitiesLoaded = useRuntimeCapabilitiesStore((state) => state.loaded)
  const loadRuntimeCapabilities = useRuntimeCapabilitiesStore((state) => state.load)
  const pageMeta = useMemo(() => getPageMeta(t), [t])
  const page = pageFromPath(location.pathname) || 'board'
  // 主页（'/'）的页面 id 也是 'session'，但挂的是主页落地（草稿 / 加载 / 错误态），不是会话路由：
  // 打开或新建会话的请求在主页上没人就地接收，外壳必须自己导航。只有 /board/:id（含 /board/new）算已挂载。
  const sessionRouteMounted = Boolean(sessionIdFromPath(location.pathname))
  // 同时挂在“更多”与设置导航下的页面按进入来源确定唯一归属，壳层各处共用这一结论。
  const settingsOwned = settingsOwnsPage(page, location.state)
  const startupPageRef = useRef(page)
  const [query, setQuery] = useState('')
  const [activeSessionId, setActiveSessionId] = useState(
    () => pageStateStorage.getItem(STORAGE_KEYS.activeSession) || '',
  )
  const mobileApp = useClientStore((state) => state.client === 'mobile-app')
  const clientLoaded = useClientStore((state) => state.loaded)
  const phoneViewport = useIsPhoneViewport()
  const mobileLayout = mobileApp || phoneViewport
  // 平板（手机 App，≥960×640）走桌面布局：顶部 48px 栏、设置分区导航；返回键与软键盘仍按移动端处理。
  const tabletApp = useIsTabletViewport() && mobileApp
  const phoneLayout = mobileLayout && !tabletApp
  // 底部标签栏只给真实手机端；桌面壳窗口拖窄时仍是侧栏外壳，避免与 Windows 自绘窗口按钮冲突。
  const phoneShell = mobileLayout && !window.pisperDesktop && !tabletApp
  // 当前页归哪个一级入口：组件工作室与其余功能页归“更多”（侧栏的“功能”）。
  const shellTab = activeShellTab(page, settingsOwned)
  const sidebarActive: ShellSidebarEntry | null =
    shellTab === 'board'
      ? 'board'
      : shellTab === 'more'
        ? 'features'
        : shellTab === 'settings'
          ? 'settings'
          : null
  const toggleSidebar = useSidebarStore((state) => state.toggleCollapsed)
  useEffect(() => {
    // 底部表单等经 Portal 挂到 body 的界面按根节点属性适配手机外壳（圆角、安全区）。
    document.documentElement.toggleAttribute('data-phone-shell', phoneShell)
  }, [phoneShell])
  const [paletteOpen, setPaletteOpen] = useState(false)
  const [dispatchOpen, setDispatchOpen] = useState(false)
  const [shortcutHelpOpen, setShortcutHelpOpen] = useState(false)
  // 等待最久的需要你事项：手机标签栏上报，手机外壳的 ⌥A（外接键盘）据此跳转；
  // 桌面的 ⌥A 交给侧栏聚焦那一行（规格 2.8），不再直接打开会话。
  const [longestWaitingId, setLongestWaitingId] = useState('')
  const theme = useUiStore((state) => state.theme)
  const cycleTheme = useUiStore((state) => state.cycleTheme)
  const density = useUiStore((state) => state.density)
  const accent = useUiStore((state) => state.accent)
  const customAccent = useUiStore((state) => state.customAccent)
  const fontScale = useUiStore((state) => state.fontScale)
  const uiFontSize = useUiStore((state) => state.uiFontSize)
  const codeFontSize = useUiStore((state) => state.codeFontSize)
  const radius = useUiStore((state) => state.radius)
  const motion = useUiStore((state) => state.motion)
  const [toastQueue, setToastQueue] = useState<ToastState[]>([])
  const toast = toastQueue[0]
  const [modal, setModal] = useState<string | null>(null)
  const [modelOnboardingOpen, setModelOnboardingOpen] = useState(false)
  const requestedConfigSection =
    page === 'config' ? decodePathSegment(location.pathname.split('/')[2] || 'models') : 'models'
  const configSection =
    CONFIG_SECTIONS.has(requestedConfigSection) &&
    runtimeConfigSectionAvailable(capabilities, requestedConfigSection)
      ? requestedConfigSection
      : 'models'
  const [pendingAsset, setPendingAsset] = useState<PendingAsset | null>(null)
  const {
    data: configData,
    isPending: configPending,
    isSuccess: configSucceeded,
  } = useQuery(startupQueryOptions<ModelOnboardingConfig>('config'))
  const { data: notificationData } = useQuery(
    startupQueryOptions<NotificationSettingsData>('notification-settings'),
  )
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
  useEffect(() => {
    const toggle = () => {
      if (
        window.pisperDesktop?.terminalProfiles &&
        runtimeFeatureAvailable(capabilities, 'terminal')
      )
        setTerminalOpen((open) => !open)
    }
    window.addEventListener('pisper:toggle-terminal', toggle)
    return () => window.removeEventListener('pisper:toggle-terminal', toggle)
  }, [capabilities])
  const browserEventCursor = useRef('')
  const [primaryActions] = useState(createPrimaryActionRegistry)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const appShellRef = useRef<HTMLDivElement>(null)
  const pageHeaderRef = useRef<HTMLElement>(null)
  const toastSequence = useRef(0)
  const appDialog = useAppDialog()
  const appUpdate = useAppUpdate()
  useEffect(() => {
    applyUiPreferenceAttributes({
      accent,
      density,
      fontScale,
      motion,
      radius,
      customAccent,
      uiFontSize,
      codeFontSize,
    })
  }, [accent, density, fontScale, motion, radius, customAccent, uiFontSize, codeFontSize])

  useEffect(() => {
    pageStateStorage.setItem(
      STORAGE_KEYS.terminalPanel,
      JSON.stringify({ open: terminalOpen, height: terminalHeight }),
    )
  }, [terminalHeight, terminalOpen])

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const apply = () => {
      const dark = resolveDarkTheme(theme, media.matches)
      document.documentElement.dataset.theme = dark ? 'dark' : 'light'
      document.documentElement.classList.toggle('dark', dark)
    }
    apply()
    if (theme === 'system') {
      media.addEventListener('change', apply)
      return () => media.removeEventListener('change', apply)
    }
    if (theme === 'scheduled') {
      // 时间边界按分钟重新评估，避免长期打开应用时主题停留在旧状态。
      const timer = window.setInterval(apply, 60_000)
      return () => window.clearInterval(timer)
    }
    return undefined
  }, [theme])

  // 每条通知独立完成展示，避免连续成功/失败反馈互相覆盖。
  const notify = useCallback<Notify>((message, tone = 'success', options) => {
    toastSequence.current += 1
    const next = { id: toastSequence.current, message, tone, action: options?.action }
    setToastQueue((queue) => [...queue, next])
  }, [])

  useEffect(() => {
    const syncActiveSession = (event: Event) => {
      const id = (event as CustomEvent<{ id?: string }>).detail?.id
      setActiveSessionId(id ?? pageStateStorage.getItem(STORAGE_KEYS.activeSession) ?? '')
    }
    window.addEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActiveSession)
    // Markdown 本地路径链接的 reveal 结果统一走全站 Toast：
    // 组件只发事件不持有 Toast，避免在 Markdown 静态闭包引入 radix Toast 图。
    const onLocalRevealNotice = (event: Event) => {
      const detail = (event as CustomEvent<{ message?: string; tone?: string }>).detail
      if (detail?.message) notify(detail.message, detail.tone === 'error' ? 'error' : 'info')
    }
    window.addEventListener(LOCAL_REVEAL_NOTICE_EVENT, onLocalRevealNotice)
    // 识别客户端形态与 Runtime 能力后，壳层统一裁剪不可用入口。
    void useClientStore
      .getState()
      .load({ refresh: false })
      .then(() => markStartupPhase('client-info-loaded'))
    void loadRuntimeCapabilities({ refresh: false }).then(() =>
      markStartupPhase('capabilities-loaded'),
    )
    return () => {
      window.removeEventListener(ACTIVE_SESSION_CHANGED_EVENT, syncActiveSession)
      window.removeEventListener(LOCAL_REVEAL_NOTICE_EVENT, onLocalRevealNotice)
    }
  }, [loadRuntimeCapabilities, notify])

  const mobileStartupPermissionsRequested = useRef(false)
  useEffect(() => {
    if (!clientLoaded || !mobileApp || mobileStartupPermissionsRequested.current) return
    mobileStartupPermissionsRequested.current = true
    void (async () => {
      // 局域网发现需要系统授权；相机权限只在用户主动开始扫码时申请。
      await invokeMobile('mobile_ensure_local_network_permission').catch(() => undefined)
    })()
  }, [clientLoaded, mobileApp])

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
      // 移动端壳：走 Tauri 本地通知插件（Android WebView 不支持 Notification Web API）。
      if (tauriNotificationAvailable()) {
        void tauriNotificationNotify(title, body).catch(() => {})
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
      if (!template?.enabled || template.channels?.browser?.enabled === false || !content) return
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
  // navigate('chat') 是旧调用方的兼容垫片：先 requestSessionSelection(id) / requestSessionCreation()
  // 再 navigate('chat')。会话路由已挂载时请求已被它就地处理；否则（含主页）按待处理意图去 '/board/:id'、
  // '/board/new'（新建），没有意图时回到主页（会话）。
  const navigate = useCallback(
    (next: string, options?: NavigateOptions) => {
      if (!PAGE_IDS.has(next) || !runtimePageAvailable(capabilities, next)) return
      if (next === 'chat') {
        if (!sessionRouteMounted) {
          const intent = peekSessionNavigationIntent()
          routerNavigate(
            intent?.type === 'create'
              ? sessionPath()
              : intent?.type === 'open'
                ? sessionPath(intent.sessionId)
                : HOME_PATH,
            options,
          )
        }
      } else routerNavigate(pagePath(next), options)
      setQuery('')
    },
    [capabilities, routerNavigate, sessionRouteMounted],
  )

  // 打开会话：URL 是会话页的选中依据。会话路由已挂载时选择事件由它就地转成路由切换，
  // 外壳不再重复导航，避免同一地址被压入两次历史；主页与其他页面由外壳导航过去。
  const openSession = useCallback(
    (id: string, targetEntryId = '') => {
      if (!id) return
      requestSessionSelection(id, 'open', targetEntryId)
      if (!sessionRouteMounted) routerNavigate(sessionPath(id))
      setQuery('')
    },
    [routerNavigate, sessionRouteMounted],
  )

  // 新任务：自带新任务输入框的页面先认领——看板聚焦启动台；主页的新任务草稿认领 ⌘N 与侧栏“新任务”，
  // 聚焦草稿输入框（会话页的页面 id 也是 session，但那里没有认领者）。没有视图认领时打开派发浮层。
  // 路由已请求去别处、界面还没换过去时，离开中的看板不该认领，要显示的草稿也还没挂上：
  // 先记下请求，等目标页提交后（下方 effect）再问它，只执行一次。
  const pendingNewTask = useRef<BoardNewTaskSource | null>(null)
  const startNewTask = useCallback(
    (source: BoardNewTaskSource) => {
      if (requestedPathname(readRouter()) !== location.pathname) {
        pendingNewTask.current = source
        return
      }
      if ((page === 'board' || page === 'session') && requestBoardNewTask(source)) return
      setDispatchOpen(true)
    },
    [location.pathname, page, readRouter],
  )
  useEffect(() => {
    const source = pendingNewTask.current
    if (!source || requestedPathname(readRouter()) !== location.pathname) return
    pendingNewTask.current = null
    startNewTask(source)
  }, [location, readRouter, startNewTask])

  // 派发浮层受理后：⌘↵ 打开该会话；↵ 留在当前页，只提示已派发（新卡片出现在看板与舰队计量里）。
  const taskDispatched = useCallback(
    (sessionId: string, { open }: { open: boolean }) => {
      setDispatchOpen(false)
      if (open) openSession(sessionId)
      else notify(t('board:launch.dispatched'))
    },
    [notify, openSession, t],
  )

  // 浮层在发送途中被关闭、随后派发失败：草稿已放回，用全局提示报错并给出重新打开的入口。
  const dispatchFailedAfterClose = useCallback(
    (message: string) =>
      notify(message, 'error', {
        action: { label: t('board:dispatch.reopen'), onAction: () => setDispatchOpen(true) },
      }),
    [notify, t],
  )

  // 新会话（命令面板、历史页等旧入口）：直接进入新建会话页。
  const startNewChat = useCallback(() => {
    if (sessionRouteMounted) requestSessionCreation('')
    else routerNavigate(sessionPath())
    setQuery('')
  }, [routerNavigate, sessionRouteMounted])

  useEffect(() => {
    if (capabilitiesLoaded && !runtimePageAvailable(capabilities, page)) {
      routerNavigate(HOME_PATH, { replace: true })
      return
    }
    if (page === 'config' && requestedConfigSection !== configSection) {
      routerNavigate(`/config/${configSection}`, { replace: true })
    }
  }, [
    capabilities,
    capabilitiesLoaded,
    configSection,
    page,
    requestedConfigSection,
    routerNavigate,
  ])

  // 切换配置分区：未知分区回退到 models，同步路由并清空搜索词。
  const setConfigSection = useCallback(
    (section: string, view?: 'appearance' | 'widgets') => {
      const nextSection =
        CONFIG_SECTIONS.has(section) && runtimeConfigSectionAvailable(capabilities, section)
          ? section
          : 'models'
      // 独立组件已迁到“组件”标签，旧调用方传 widgets 时直接去组件页。
      if (nextSection === 'interface' && view === 'widgets') routerNavigate(PAGE_PATHS.components)
      else routerNavigate(`/config/${nextSection}`)
      setQuery('')
    },
    [capabilities, routerNavigate],
  )

  // 一级入口（侧栏入口、手机标签栏与快捷键共用）：设置回到模型分区（与旧功能栏的设置键一致）。
  // “会话”走与 '/' 相同的主页逻辑（上次停留的会话或新任务草稿）；已在（或正要去）会话页或主页时不再压入历史。
  // 按路由器的最新去向判断：刚按 ⌘1 / ⌘2 离开主页、界面还没换过去时，已渲染的 page 仍是 session，
  // 回主页的入口不能因此什么都不做（61458a38 的竞态修复，原样保留给侧栏入口和快捷键）。
  const selectShellTab = useCallback(
    (tab: ShellTab) => {
      if (tab === 'session') {
        const next = sessionTabNavigation(readRouter())
        if (next) routerNavigate(next.to, { replace: next.replace, state: next.state })
        setQuery('')
      } else if (tab === 'settings') setConfigSection('models')
      else navigate(SHELL_TAB_PAGES[tab])
    },
    [navigate, readRouter, routerNavigate, setConfigSection],
  )

  // 侧栏会话行：会话页（含主页）之间切换只换路由（replace，保留历史状态，规格 4.6）；
  // 从其他页面打开时 push，浏览器后退回到来处。当前会话按路由器的最新去向判断，连点或连按 ⌥↓
  // 时界面还没换过去也不会重复导航（沿用 effectiveStripCurrent 的教训：不能只看渲染状态）。
  const currentSessionTarget = useCallback(
    () => sessionIdFromPath(requestedPathname(readRouter())),
    [readRouter],
  )
  const openSidebarSession = useCallback(
    (id: string) => {
      if (!id) return
      const router = readRouter()
      const target = requestedPathname(router)
      if (sessionIdFromPath(target) === id) return
      setQuery('')
      if (pageFromPath(target) !== 'session') {
        routerNavigate(sessionPath(id))
        return
      }
      // 主页（草稿）上打开的会话与从主页落地的一样是根；会话之间切换沿用当前条目的状态。
      const state = target === HOME_PATH ? HOME_ENTRY_STATE : router.location.state
      routerNavigate(sessionPath(id), { replace: true, state })
    },
    [readRouter, routerNavigate],
  )
  // 侧栏归档了正在看的会话：回到主页（新任务草稿），与会话页标题菜单里归档的去向一致。
  const sidebarArchived = useCallback(
    (id: string) => {
      if (currentSessionTarget() === id) routerNavigate(HOME_PATH, { replace: true })
    },
    [currentSessionTarget, routerNavigate],
  )

  const dismissModelOnboarding = useCallback(() => {
    try {
      pageStateStorage.setItem(STORAGE_KEYS.modelOnboardingDismissed, '1')
    } catch {
      // 存储受限时仍允许关闭；本次应用会话内不会再次打开。
    }
    setModelOnboardingOpen(false)
  }, [])

  const openModelSettingsFromOnboarding = useCallback(() => {
    dismissModelOnboarding()
    // 用户主动选择设置时，把模型页主操作排队；页面挂载后直接打开快速设置。
    primaryActions.clear()
    primaryActions.invoke()
    setConfigSection('models')
  }, [dismissModelOnboarding, primaryActions, setConfigSection])

  const openImportableSettingsFromOnboarding = useCallback(() => {
    dismissModelOnboarding()
    // 导入页保持连接管理区展开；无需经过新建连接向导。
    routerNavigate('/config/models?import=1')
    setQuery('')
  }, [dismissModelOnboarding, routerNavigate])

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

  // 解析会话工作目录：从会话列表查 cwd（供终端绑定工作区）。
  const resolveSessionCwd = useCallback(async (sessionId: string) => {
    if (!sessionId) return ''
    const data = await fetchStartupQuery<{ sessions?: Array<{ id: string; cwd?: string }> }>(
      'sessions',
    )
    return data.sessions?.find((session) => session.id === sessionId)?.cwd || ''
  }, [])

  // 使用资源：把资源（附件）标记为待投递到活动会话并打开该会话；
  // 没有活动会话时新建一个，由新会话的输入框接收。
  const useAsset = useCallback(
    (asset: ChatAttachment) => {
      const targetSessionId = pageStateStorage.getItem(STORAGE_KEYS.activeSession) || ''
      setPendingAsset({ asset, targetSessionId })
      if (targetSessionId) openSession(targetSessionId)
      else routerNavigate(sessionPath())
    },
    [openSession, routerNavigate],
  )

  // 主操作分发：按当前页面决定触发哪个动作（新建会话/新建工作流/快捷创建）；
  // 配置页非 models 分区时不响应，避免误触发。
  const handlePrimary = useCallback(() => {
    if (page === 'config' && configSection !== 'models') return
    if (
      [
        'session',
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
    else if (page === 'chatHistory') startNewChat()
    else if (page === 'workflows') navigate('workflowCreate')
    else setModal(page)
  }, [configSection, invokePrimaryAction, navigate, page, startNewChat])

  useEffect(() => {
    const openCommandPalette = () => {
      if (!appDialog.dialog && !modal && !modelOnboardingOpen) setPaletteOpen(true)
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing || event.keyCode === 229) return
      if (event.key === 'Escape' && !appDialog.dialog) {
        if (paletteOpen) {
          setPaletteOpen(false)
          event.preventDefault()
        } else if (modal) {
          setModal(null)
          event.preventDefault()
        }

        return
      }
    }
    window.addEventListener('keydown', onKeyDown)
    window.addEventListener(COMMAND_PALETTE_REQUESTED_EVENT, openCommandPalette)
    return () => {
      window.removeEventListener('keydown', onKeyDown)
      window.removeEventListener(COMMAND_PALETTE_REQUESTED_EVENT, openCommandPalette)
    }
  }, [appDialog.dialog, modal, modelOnboardingOpen, paletteOpen])

  const startupConfigHandled = useRef(false)
  useEffect(() => {
    if (configPending || startupConfigHandled.current) return
    startupConfigHandled.current = true
    setStartupReady(true)
    // 配置失败只结束等待，不把网络故障当作未配置，也不因后续刷新再次重定向。
    if (!configSucceeded || !configData) return
    markStartupPhase('config-loaded')
    let dismissed = false
    try {
      dismissed = pageStateStorage.getItem(STORAGE_KEYS.modelOnboardingDismissed) === '1'
    } catch {
      // 存储不可读时按未关闭处理，仍允许用户在界面上跳过。
    }
    if (startupPageRef.current !== 'config' && shouldShowModelOnboarding(configData, dismissed)) {
      setModelOnboardingOpen(true)
    }
  }, [configData, configPending, configSucceeded])

  useEffect(() => {
    if (startupReady && capabilitiesLoaded && (page === 'board' || page === 'session')) {
      markStartupPhase('chat-shell-ready')
    }
  }, [capabilitiesLoaded, page, startupReady])

  const previousPage = useRef(page)
  useLayoutEffect(() => {
    // 设置页仍由各表单保存；离开时失效共享快照，避免聊天页继续使用旧模型配置。
    if (previousPage.current === 'config' && page !== 'config') {
      void invalidateStartupQuery('config')
      void invalidateStartupQuery('notification-settings')
    }
    previousPage.current = page
  }, [page])

  useEffect(() => {
    if (notificationData) setNotificationSettings(notificationData)
  }, [notificationData])

  useEffect(() => {
    let active = true
    const poll = async () => {
      try {
        const cursor = browserEventCursor.current
        const result = await queryClient.fetchQuery({
          queryKey: ['browser-notification-events', cursor],
          queryFn: () =>
            apiJson<{
              events?: Array<{ title: string; body: string }>
              latestId?: string
            }>(`/api/settings/notifications/browser/events?after=${encodeURIComponent(cursor)}`),
          staleTime: 0,
          retry: false,
        })
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
          mobileLayout ? '' : t('common:app.dragTabsOrSplitChatsInAnyDirectionFromTheSessionList'),
        ]
      : page === 'config'
        ? [
            getSettingsNavigation(t, { mobileApp, capabilities })
              .flatMap((group) => group.items)
              .find((item) => item.key === `config:${configSection}`)?.label || pageMeta.config[0],
            '',
          ]
        : page === 'workflowCreate'
          ? // 编辑器挂载前（懒加载）按路由段推断；挂载后以编辑器判断为准（不存在的 id 打开的是空白新建草稿）。
            getWorkflowEditorMeta(
              t,
              workflowActions
                ? workflowActions.editing
                : decodePathSegment(location.pathname.split('/')[2] || 'new') !== 'new',
            )
          : pageMeta[page] || [t('common:app.sessions'), '']

  const updateNotificationSettings = useCallback((settings: NotificationSettingsData) => {
    // 回调须稳定，否则通知表单会因依赖变化重复加载；保存值不能被较早的请求覆盖。
    void queryClient.cancelQueries({ queryKey: ['notification-settings'], exact: true })
    queryClient.setQueryData(['notification-settings'], settings)
    setNotificationSettings(settings)
  }, [])

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
    setNotificationSettings: updateNotificationSettings,
    appUpdate,
    registerWorkflowActions,
    startNewTask: () => startNewTask('board'),
  }
  // 从“更多”进入的功能页：页头显示“‹ 更多”。
  const moreOwned = shellTab === 'more' && page !== 'more'
  const backToMore = useCallback(() => routerNavigate(PAGE_PATHS.more), [routerNavigate])
  // 手机原生返回的路由层级（shellBackPath）：会话主页是根（主页与从主页落地的会话交还平台），
  // 从看板等页面点进来的会话走历史回到来处，一级标签页回主页，更多的子页回更多，组件详情回列表。
  // 路由历史下标（React Router 写在 history.state.idx）判断当前条目之前还有没有应用内条目。
  const routeBack = useCallback(() => {
    const index: unknown = window.history.state?.idx
    const parent = shellBackPath(page, location.pathname, settingsOwned, {
      phone: phoneShell,
      homeEntry: isHomeEntry(location.state),
      canGoBack: typeof index === 'number' && index > 0,
    })
    if (parent === -1) routerNavigate(-1)
    else if (parent) routerNavigate(parent)
    return parent !== null
  }, [location.pathname, location.state, page, phoneShell, routerNavigate, settingsOwned])
  const jumpToAttention = useCallback(() => {
    if (longestWaitingId) openSession(longestWaitingId)
  }, [longestWaitingId, openSession])
  const focusPageSearch = useCallback(() => {
    if (page === 'more') document.getElementById(MORE_SEARCH_INPUT_ID)?.focus()
    // 看板页的 / 聚焦筛选（项目筛选控件；没有项目时是状态筛选）。
    else if (page === 'board') document.getElementById(BOARD_FILTER_ID)?.focus()
    else searchInputRef.current?.focus()
  }, [page])
  // 看板、会话页与更多页自带页头；其余功能页沿用共享页头（面包屑、搜索、主操作）。
  // 会话页与主页隐藏底部标签栏（规格 7.3）：看板入口在顶栏左侧，返回走系统返回键；组件仍挂着（不渲染），
  // ⌥A 的等待最久事项照常更新。懒加载期间的占位也随之省掉。
  const tabBarFallback =
    page === 'session' ? null : <div className="h-14 flex-none" aria-hidden="true" />
  const sharedPageHeader = !['board', 'session', 'more', 'chat'].includes(page)
  const settingsLayout = settingsOwned && !phoneLayout
  // 手机用横滑分区条；平板（手机 App）与桌面一样用左侧分区导航。
  const mobileSettingsNav =
    clientLoaded && mobileLayout && SETTINGS_PAGES.has(page) && settingsOwned && !tabletApp

  if (!startupReady || !capabilitiesLoaded)
    return (
      <div
        role="status"
        aria-live="polite"
        aria-busy="true"
        className="app-startup flex min-h-[100vh] w-full items-center justify-center gap-2.5 bg-chrome text-[13px] text-content-muted"
      >
        <BrandLogo size={30} className="startup-logo" />
        <strong className="font-medium">{t('common:app.wakingPisper')}</strong>
      </div>
    )

  const configSearch = (placement?: 'sidebar') => (
    <Suspense fallback={null}>
      <ConfigSearchBox
        query={query}
        onQueryChange={setQuery}
        onSelect={setConfigSection}
        inputRef={searchInputRef}
        placement={placement}
      />
    </Suspense>
  )

  return (
    <ToastProvider duration={2800} swipeDirection="right">
      <TerminalProvider bridge={window.pisperDesktop}>
        <div
          ref={appShellRef}
          // 手机外壳的顶部安全区由外壳底色铺满：看板与更多页的页头是页面底色（--bg），会话页是外壳灰，
          // 其余页面是纸面；状态栏区域跟随当前页头，避免出现一条白带接灰色页头的两段色。
          className="app-shell flex w-full h-full min-h-0 flex-col overflow-hidden bg-chrome text-content [&[data-phone-shell]]:bg-panel [&[data-phone-shell][data-shell-canvas]]:bg-background [&[data-phone-shell][data-shell-chrome]]:bg-chrome pt-[var(--pisper-safe-area-top)] pr-[var(--pisper-safe-area-right)] pb-[var(--pisper-safe-area-bottom)] pl-[var(--pisper-safe-area-left)] [&[data-mobile-app]]:h-[100dvh] [&[data-mobile-app]]:min-h-0 [&[data-mobile-app]]:overflow-hidden [&[data-mobile-keyboard='open']]:pb-0 [&[data-phone-shell]]:pb-0"
          data-mobile-app={mobileLayout || undefined}
          data-phone-shell={phoneShell || undefined}
          data-shell-canvas={page === 'board' || page === 'more' || undefined}
          // 手机会话页的顶栏与代理条是外壳灰（规格 7.3），状态栏区域跟着它。
          data-shell-chrome={page === 'session' || undefined}
          data-custom-titlebar={window.pisperDesktop?.customTitlebar || undefined}
        >
          {window.pisperDesktop?.customTitlebar && (
            <Suspense fallback={null}>
              <DesktopWindowControls />
            </Suspense>
          )}
          <WebPreviewProvider />
          {mobileLayout && (
            <Suspense fallback={null}>
              <MobileViewportStabilizer
                enabled={clientLoaded && startupReady}
                shellRef={appShellRef}
              />
            </Suspense>
          )}
          {runtimeFeatureAvailable(capabilities, 'desktopPet') && (
            <Suspense fallback={null}>
              <WebDesktopPet />
            </Suspense>
          )}
          <BackNavigation onRouteBack={mobileLayout ? routeBack : undefined} />
          <AppShortcuts
            blocked={Boolean(
              appDialog.dialog || modal || modelOnboardingOpen || paletteOpen || dispatchOpen,
            )}
            onCommandPalette={() => setPaletteOpen(true)}
            onPrimary={() => startNewTask('shortcut')}
            onSearch={focusPageSearch}
            onToggleTerminal={
              window.pisperDesktop?.terminalProfiles &&
              runtimeFeatureAvailable(capabilities, 'terminal')
                ? () => setTerminalOpen((value) => !value)
                : undefined
            }
            onSettings={() => selectShellTab('settings')}
            onOpenSession={() => selectShellTab('session')}
            onOpenBoard={() => selectShellTab('board')}
            onOpenComponents={() => navigate('components')}
            onOpenMore={() => selectShellTab('more')}
            // 侧栏只在桌面外壳（含平板）里：⌘B / ⌘J / ⌥A / ⌥↑⌥↓ 都交给它；手机外壳的 ⌥A 仍直接打开会话（A4 改为抽屉）。
            onToggleSidebar={phoneShell ? undefined : toggleSidebar}
            onAgentPanel={
              phoneShell ? undefined : () => requestSidebarCommand({ kind: 'focus-list' })
            }
            onPreviousAgent={
              phoneShell
                ? undefined
                : () => requestSidebarCommand({ kind: 'adjacent', direction: -1 })
            }
            onNextAgent={
              phoneShell
                ? undefined
                : () => requestSidebarCommand({ kind: 'adjacent', direction: 1 })
            }
            onJumpToAttention={
              phoneShell
                ? longestWaitingId
                  ? jumpToAttention
                  : undefined
                : () => requestSidebarCommand({ kind: 'focus-attention' })
            }
            // F6：侧栏 → 顶栏 → 对话 → 输入框（规格 2.8）。区域逻辑按需加载，不占入口包。
            onCycleFocusRegion={() =>
              void import('@/app/focus-regions').then((module) => module.cycleFocusRegion())
            }
            onShortcutHelp={() => setShortcutHelpOpen(true)}
          />
          <div
            className="app-body flex min-h-0 flex-1 overflow-hidden"
            data-mobile-app={mobileLayout || undefined}
          >
            {!phoneShell && (
              <ShellSidebar
                active={sidebarActive}
                onNewTask={() => startNewTask('sidebar')}
                onSearch={() => setPaletteOpen(true)}
                onOpenBoard={() => selectShellTab('board')}
                onOpenFeatures={() => selectShellTab('more')}
                onOpenSettings={() => selectShellTab('settings')}
                content={
                  <Suspense fallback={<SidebarSessionsFallback />}>
                    <SidebarSessions
                      currentId={sessionIdFromPath(location.pathname)}
                      notify={notify}
                      requestText={appDialog.prompt}
                      onOpenSession={openSidebarSession}
                      onOpenBoard={() => selectShellTab('board')}
                      onOpenHistory={() => navigate('chatHistory')}
                      onArchived={sidebarArchived}
                    />
                  </Suspense>
                }
                railCounts={
                  <Suspense fallback={<SidebarSessionsFallback collapsed />}>
                    <SidebarRailCounts onOpenBoard={() => selectShellTab('board')} />
                  </Suspense>
                }
                update={
                  appUpdate.status &&
                  ['available', 'downloading', 'downloaded'].includes(appUpdate.status.state) ? (
                    <Suspense fallback={null}>
                      {/* 沿用原侧栏的更新提示，收成 32px 图标键放在设置旁；触屏抬到 44px。 */}
                      <div className="flex-none [&>button]:size-8 [&>button]:min-h-0 pointer-coarse:[&>button]:size-11">
                        <SidebarUpdateStatus
                          update={appUpdate}
                          collapsed
                          onOpen={openUpdateSettings}
                        />
                      </div>
                    </Suspense>
                  ) : undefined
                }
              />
            )}
            {!phoneShell && (
              // 命令处理（⌘J / ⌥A / ⌥↑⌥↓）展开与收起时都挂着，也让看板同步在侧栏收起时保持存活。
              <Suspense fallback={null}>
                <SidebarCommands
                  resolveCurrentId={currentSessionTarget}
                  renderedSessionId={
                    page === 'session' ? sessionIdFromPath(location.pathname) : null
                  }
                  onOpenSession={openSidebarSession}
                />
              </Suspense>
            )}
            {settingsLayout && (
              <Suspense
                fallback={
                  <nav
                    aria-busy="true"
                    aria-label={t('config:settingsShell.settingsNavigation')}
                    className="w-[232px] flex-none border-r border-stroke-soft bg-panel"
                  />
                }
              >
                <SettingsSectionNav
                  page={page}
                  configSection={configSection}
                  mobileApp={mobileApp}
                  onNavigate={navigateSettings}
                  search={page === 'config' ? configSearch('sidebar') : undefined}
                  version={appUpdate.info?.version}
                />
              </Suspense>
            )}
            <main
              data-focus-region="main"
              className="main-surface relative flex min-w-0 flex-1 flex-col overflow-hidden bg-panel"
            >
              {sharedPageHeader && (
                <Suspense fallback={null}>
                  <PageHeader
                    elementRef={pageHeaderRef}
                    meta={activeMeta}
                    page={page}
                    settingsOwned={settingsOwned}
                    query={query}
                    setQuery={setQuery}
                    configSection={configSection}
                    onMenu={() => setPaletteOpen(true)}
                    onPrimary={handlePrimary}
                    searchSlot={page === 'config' && !settingsLayout ? configSearch() : undefined}
                    searchInputRef={searchInputRef}
                    workflowActions={workflowActions}
                    desktopPlatform={window.pisperDesktop?.platform || ''}
                    mobileApp={mobileApp}
                    terminalOpen={terminalOpen}
                    onToggleTerminal={() => setTerminalOpen((value) => !value)}
                    onBackToMore={moreOwned ? backToMore : undefined}
                  />
                </Suspense>
              )}
              {mobileSettingsNav && (
                <MobileSettingsNavigation
                  page={page}
                  configSection={configSection}
                  mobileApp={mobileApp}
                  onNavigate={navigateSettings}
                />
              )}
              <div
                // 会话页沿用聊天页的内容区布局：可收缩的 flex 容器，下方终端面板展开时对话区随之让高。
                className={`page-content flex-1 min-h-0 overflow-auto ${page === 'session' || page === 'chat' ? 'page-chat flex overflow-hidden p-0' : ['board', 'more', 'config', 'schedules', 'mcp', 'skills', 'channels', 'memory', 'assets', 'plugins', 'workflows', 'workflowCreate', 'components', 'customComponentTool'].includes(page) ? `page-${page} overflow-hidden p-0` : `page-${page} px-6 pt-5 pb-5 max-[650px]:px-3 max-[650px]:pt-0`}`}
                key={page}
              >
                <Outlet context={routeContext} />
              </div>
              {window.pisperDesktop?.terminalProfiles &&
                runtimeFeatureAvailable(capabilities, 'terminal') && (
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
            </main>
          </div>
          {phoneShell && (
            <Suspense fallback={tabBarFallback}>
              <ShellTabBar
                hidden={page === 'session'}
                activeTab={shellTab}
                onSelectTab={selectShellTab}
                onNewTask={() => startNewTask('tabbar')}
                onLongestWaitingChange={setLongestWaitingId}
              />
            </Suspense>
          )}
          <Suspense fallback={null}>
            <FloatingWidgets anchorRef={pageHeaderRef} notify={notify} />
          </Suspense>
          {toast && (
            <AppToast
              key={toast.id}
              open
              message={toast.message}
              tone={toast.tone}
              action={toast.action}
              duration={toast.action ? 10_000 : undefined}
              closeLabel={t('common:ui.closeDialog')}
              onOpenChange={(open) =>
                !open && setToastQueue((queue) => queue.filter((entry) => entry.id !== toast.id))
              }
            />
          )}
          <ToastViewport hotkey={[]} label={t('common:ui.notifications')} />
          {appDialog.dialog && (
            <Suspense
              fallback={
                <span role="status" className="sr-only">
                  {t('common:pageTemplate.loading')}
                </span>
              }
            >
              <AppDialog
                dialog={appDialog.dialog}
                onClose={appDialog.close}
                onFinish={appDialog.finish}
              />
            </Suspense>
          )}
          <Suspense fallback={null}>
            {modelOnboardingOpen && (
              <ModelOnboardingDialog
                open
                onDismiss={dismissModelOnboarding}
                onOpenImport={openImportableSettingsFromOnboarding}
                onOpenSettings={openModelSettingsFromOnboarding}
              />
            )}
          </Suspense>
          <Suspense fallback={null}>
            {paletteOpen && (
              <CommandPalette
                onClose={() => setPaletteOpen(false)}
                onNavigate={navigate}
                onOpenSession={async (id, targetEntryId, targetActive) => {
                  try {
                    if (targetEntryId && !targetActive) {
                      // 只有在面板里选中带标签的轮次才会用到；面板块自己已经加载了 chat-api，
                      // 这里按需取用即可，不为它把 chat-api 放进启动包。
                      const { chatApi } = await import('@/features/chat/chat-api')
                      await chatApi.navigateSessionTreeTarget(id, targetEntryId)
                    }
                    openSession(id, targetEntryId)
                  } catch (error) {
                    notify(t('navigation:appOverlays.openLabeledTurnFailed'), 'error')
                    throw error
                  }
                }}
                onNewChat={() => startNewTask('palette')}
                onToggleTheme={cycleTheme}
              />
            )}
            {modal && <QuickCreate type={modal} close={() => setModal(null)} notify={notify} />}
            {shortcutHelpOpen && (
              <ShortcutHelpDialog
                open
                onOpenChange={setShortcutHelpOpen}
                onCustomize={() => routerNavigate('/config/shortcuts')}
              />
            )}
          </Suspense>
          <Suspense fallback={null}>
            {dispatchOpen && (
              <DispatchOverlay
                open
                phone={phoneShell}
                onOpenChange={setDispatchOpen}
                onDispatched={taskDispatched}
                onDetachedFailure={dispatchFailedAfterClose}
                onConfigureModels={() => routerNavigate('/config/models')}
              />
            )}
          </Suspense>
        </div>
      </TerminalProvider>
    </ToastProvider>
  )
}

export default App
