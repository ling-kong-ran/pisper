import { BackNavigation } from '@/components/layout/BackNavigation'
// 应用外壳：持有全站共享状态（当前页/搜索词/活动会话/Toast/对话框/通知
// 设置/更新控制器/工作流动作），组装 48px 应用标题栏 + 全宽内容区（页头 + Outlet）
// + 手机底部标签栏，并通过 Outlet 上下文向各页面注入公共能力。桌面没有侧栏与活动栏，
// 一级导航（看板/组件/更多/设置）在标题栏里；设置页的分区导航在设置内容区左侧。
// 启动时探测是否已配置可用 Provider，未配置则展示可跳过的新手引导；同时提供全局快捷键
// （⌘K 命令面板、⌘N 新任务、⌘1/2/3/, 一级标签、⌥A 跳到需要你、⌘[ 返回看板、` 终端、/ 搜索、
// Esc 逐层关闭）与浏览器通知轮询。
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
import { PAGE_IDS, PAGE_PATHS, pageFromPath, pagePath, sessionPath } from '@/app/routes'
import { activeShellTab, SHELL_TAB_PAGES, type ShellTab } from '@/app/shell-tabs'
import { AppTitlebar } from '@/app/AppTitlebar'
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
import { MobileSettingsNavigation } from '@/components/layout/MobileNavigation'
import { AppToast, ToastProvider, ToastViewport, type ToastTone } from '@/components/ui/toast'
import { chatApi } from '@/features/chat/chat-api'
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
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { useAppUpdate } from '@/features/updates/useAppUpdate'
import { shouldShowModelOnboarding, type ModelOnboardingConfig } from '@/features/config/public'
import { useUiStore } from '@/stores/ui-store'
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
const AppShortcuts = lazy(() =>
  import('@/components/layout/AppShortcuts').then((module) => ({ default: module.AppShortcuts })),
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
// 舰队计量与手机标签栏依赖看板数据层（单一轮询者）；按需加载，不进入启动包。
const FleetMeter = lazy(() =>
  import('@/app/FleetMeter').then((module) => ({ default: module.FleetMeter })),
)
const ShellTabBar = lazy(() =>
  import('@/app/ShellTabBar').then((module) => ({ default: module.ShellTabBar })),
)
// 更新提示只在确有可用更新时加载，常驻标题栏不为它增加启动体积。
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

function App() {
  const { t } = useI18n()
  useEffect(() => markStartupPhase('react-app-mounted'), [])
  const location = useLocation()
  const routerNavigate = useNavigate()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const capabilitiesLoaded = useRuntimeCapabilitiesStore((state) => state.loaded)
  const loadRuntimeCapabilities = useRuntimeCapabilitiesStore((state) => state.load)
  const pageMeta = useMemo(() => getPageMeta(t), [t])
  const page = pageFromPath(location.pathname) || 'board'
  // 同时挂在“更多”与设置导航下的页面按进入来源确定唯一归属，壳层各处共用这一结论。
  const settingsOwned = settingsOwnsPage(page, location.state)
  const shellTab = activeShellTab(page, settingsOwned)
  const startupPageRef = useRef(page)
  const [query, setQuery] = useState('')
  const [activeSessionId, setActiveSessionId] = useState(
    () => pageStateStorage.getItem(STORAGE_KEYS.activeSession) || '',
  )
  const mobileApp = useClientStore((state) => state.client === 'mobile-app')
  const clientLoaded = useClientStore((state) => state.loaded)
  const phoneViewport = useIsPhoneViewport()
  const mobileLayout = mobileApp || phoneViewport
  // 底部标签栏只给真实手机端；桌面壳窗口拖窄时仍保留标题栏，避免与 Windows 自绘窗口按钮冲突。
  const phoneShell = mobileLayout && !window.pisperDesktop
  useEffect(() => {
    // 底部表单等经 Portal 挂到 body 的界面按根节点属性适配手机外壳（圆角、安全区）。
    document.documentElement.toggleAttribute('data-phone-shell', phoneShell)
  }, [phoneShell])
  const [paletteOpen, setPaletteOpen] = useState(false)
  const [dispatchOpen, setDispatchOpen] = useState(false)
  // 等待最久的需要你事项：由舰队计量（桌面）或底部标签栏（手机）上报，⌥A 只据此跳转。
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
  // 再 navigate('chat')。会话页已挂载时请求已被它就地处理；否则按待处理意图去 '/board/:id'、
  // '/board/new'（新建），没有意图时回到看板。
  const navigate = useCallback(
    (next: string, options?: NavigateOptions) => {
      if (!PAGE_IDS.has(next) || !runtimePageAvailable(capabilities, next)) return
      if (next === 'chat') {
        if (page !== 'session') {
          const intent = peekSessionNavigationIntent()
          routerNavigate(
            intent?.type === 'create'
              ? sessionPath()
              : intent?.type === 'open'
                ? sessionPath(intent.sessionId)
                : PAGE_PATHS.board,
            options,
          )
        }
      } else routerNavigate(pagePath(next), options)
      setQuery('')
    },
    [capabilities, page, routerNavigate],
  )

  // 打开会话：URL 是会话页的选中依据。会话页已挂载时选择事件由它就地转成路由切换，
  // 外壳不再重复导航，避免同一地址被压入两次历史。
  const openSession = useCallback(
    (id: string, targetEntryId = '') => {
      if (!id) return
      requestSessionSelection(id, 'open', targetEntryId)
      if (page !== 'session') routerNavigate(sessionPath(id))
      setQuery('')
    },
    [page, routerNavigate],
  )

  // 新任务：看板页先请求看板认领（聚焦启动台）；没有视图认领或在其他页面时打开派发浮层。
  const startNewTask = useCallback(
    (source: BoardNewTaskSource) => {
      if (page === 'board' && requestBoardNewTask(source)) return
      setDispatchOpen(true)
    },
    [page],
  )

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
    if (page === 'session') requestSessionCreation('')
    else routerNavigate(sessionPath())
    setQuery('')
  }, [page, routerNavigate])

  useEffect(() => {
    if (capabilitiesLoaded && !runtimePageAvailable(capabilities, page)) {
      routerNavigate(PAGE_PATHS.board, { replace: true })
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

  // 一级标签：设置标签回到模型分区（与旧功能栏的设置键一致）。
  const selectShellTab = useCallback(
    (tab: ShellTab) => {
      if (tab === 'settings') setConfigSection('models')
      else navigate(SHELL_TAB_PAGES[tab])
    },
    [navigate, setConfigSection],
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
  // 会话页回看板、从“更多”进入的功能页回更多：手机返回键与页头“‹ 更多”共用这一层级。
  const moreOwned = shellTab === 'more' && page !== 'more'
  const backToBoard = useCallback(() => routerNavigate(PAGE_PATHS.board), [routerNavigate])
  const backToMore = useCallback(() => routerNavigate(PAGE_PATHS.more), [routerNavigate])
  const routeBack = useCallback(() => {
    if (page === 'session') backToBoard()
    else if (moreOwned) backToMore()
    else return false
    return true
  }, [backToBoard, backToMore, moreOwned, page])
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
  const sharedPageHeader = !['board', 'session', 'more', 'chat'].includes(page)
  const settingsLayout = settingsOwned && !mobileLayout

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
          // 手机外壳的顶部安全区由外壳底色铺满：看板与更多页的页头是页面底色（--bg），其余页面是纸面，
          // 状态栏区域跟随当前页头，避免出现一条白带接灰色页头的两段色。
          className="app-shell flex w-full h-full min-h-0 flex-col overflow-hidden bg-chrome text-content [&[data-phone-shell]]:bg-panel [&[data-phone-shell][data-shell-canvas]]:bg-background pt-[var(--pisper-safe-area-top)] pr-[var(--pisper-safe-area-right)] pb-[var(--pisper-safe-area-bottom)] pl-[var(--pisper-safe-area-left)] [&[data-mobile-app]]:h-[100dvh] [&[data-mobile-app]]:min-h-0 [&[data-mobile-app]]:overflow-hidden [&[data-mobile-keyboard='open']]:pb-0 [&[data-phone-shell]]:pb-0"
          data-mobile-app={mobileLayout || undefined}
          data-phone-shell={phoneShell || undefined}
          data-shell-canvas={page === 'board' || page === 'more' || undefined}
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
          <Suspense fallback={null}>
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
              onOpenBoard={() => selectShellTab('board')}
              onOpenComponents={() => selectShellTab('components')}
              onOpenMore={() => selectShellTab('more')}
              onJumpToAttention={longestWaitingId ? jumpToAttention : undefined}
              onBackToBoard={page === 'session' ? backToBoard : undefined}
            />
          </Suspense>
          {/* 桌面（含手机 App 以外的窄窗口）由标题栏承载一级导航；手机外壳改用底部标签栏。 */}
          {!phoneShell && (
            <AppTitlebar
              activeTab={shellTab}
              onSelectTab={selectShellTab}
              onOpenPalette={() => setPaletteOpen(true)}
              onNewTask={() => startNewTask('titlebar')}
              status={
                appUpdate.status &&
                ['available', 'downloading', 'downloaded'].includes(appUpdate.status.state) ? (
                  <Suspense fallback={null}>
                    {/* 沿用原侧栏的更新提示，收成 32px 图标键放进标题栏。 */}
                    <div className="flex-none [&>button]:size-8 [&>button]:min-h-0">
                      <SidebarUpdateStatus
                        update={appUpdate}
                        collapsed
                        onOpen={openUpdateSettings}
                      />
                    </div>
                  </Suspense>
                ) : undefined
              }
              fleet={
                <Suspense fallback={<span className="h-8 w-24 flex-none" aria-hidden="true" />}>
                  <FleetMeter
                    onOpenSession={openSession}
                    onLongestWaitingChange={setLongestWaitingId}
                  />
                </Suspense>
              }
            />
          )}
          <div
            className="app-body flex min-h-0 flex-1 overflow-hidden"
            data-mobile-app={mobileLayout || undefined}
          >
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
            <main className="main-surface relative flex min-w-0 flex-1 flex-col overflow-hidden bg-panel">
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
                    theme={theme}
                    onCycleTheme={cycleTheme}
                    workflowActions={workflowActions}
                    desktopPlatform={window.pisperDesktop?.platform || ''}
                    mobileApp={mobileApp}
                    terminalOpen={terminalOpen}
                    onToggleTerminal={() => setTerminalOpen((value) => !value)}
                    onBackToMore={moreOwned ? backToMore : undefined}
                  />
                </Suspense>
              )}
              {clientLoaded && mobileLayout && SETTINGS_PAGES.has(page) && settingsOwned && (
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
            <Suspense fallback={<div className="h-14 flex-none" aria-hidden="true" />}>
              <ShellTabBar
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
                      await chatApi.navigateSessionTreeTarget(id, targetEntryId)
                    }
                    openSession(id, targetEntryId)
                  } catch (error) {
                    notify(t('navigation:appOverlays.openLabeledTurnFailed'), 'error')
                    throw error
                  }
                }}
                onNewChat={() => startNewTask('palette')}
              />
            )}
            {modal && <QuickCreate type={modal} close={() => setModal(null)} notify={notify} />}
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
