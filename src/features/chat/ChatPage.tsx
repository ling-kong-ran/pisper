// 聊天主页面：dockview 多会话分屏布局的宿主，持有会话目录与实时
// 同步状态，管理 Dock 的初始化/持久化与多面板交互。
// 移动端 App 不渲染 Dock：轻量标签栏切换活动会话，内容区只挂载一个会话，
// dockview 及其样式经懒加载分包，移动端不下载。
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from 'react'
import type { DockviewGroupPanel } from 'dockview-react'
import {
  Clock,
  Menu,
  MonitorCog,
  Moon,
  PanelRightClose,
  PanelRightOpen,
  RefreshCw,
  Sun,
  TerminalSquare,
  type LucideIcon,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { AppEmptyState } from '@/components/ui/app-primitives'
import { useIsPhoneViewport } from '@/hooks/use-mobile'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { useClientStore } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { useUiStore, type ThemeMode } from '@/stores/ui-store'
import { waitForMobileRuntimeReady } from '@/lib/http'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import type { Notify } from '@/app/route-context'
import type { PendingAsset, SessionSummary } from '@/types/chat'
import { MobileSessionPanel } from './ChatDock'
import { chatApi } from './chat-api'
import { ChatDockContext, type ChatDockContextValue } from './chat-dock-context'
import { useChatDock } from './use-chat-dock'
import { useLiveSessionSync } from './use-live-session-sync'
import { usePromptCommands } from './use-prompt-commands'
import { useSessionCatalog } from './use-session-catalog'
import { useSessionCommands } from './use-session-commands'
import { shouldInheritRecentSessionCwd } from './session-list'
import {
  AUX_CHAT_TOGGLE_EVENT,
  SESSION_CREATE_REQUESTED_EVENT,
  consumeSessionCreationRequest,
  requestAuxChatToggle,
} from './events'

// 中栏头部主题图标：与 PageHeader 的 THEME_META 保持一致顺序。
const CHAT_THEME_META: Record<ThemeMode, LucideIcon> = {
  system: MonitorCog,
  scheduled: Clock,
  light: Sun,
  dark: Moon,
}

// Dock 分屏视图懒加载：只有桌面布局才下载 dockview 分包。
const LazyAuxChatPanel = lazy(() =>
  import('./AuxChatPanel').then((module) => ({ default: module.AuxChatPanel })),
)
const LazyChatDockView = lazy(() =>
  import('./ChatDockView').then((module) => ({ default: module.ChatDockView })),
)

function invokeMobile<T>(command: string): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!invoke) return Promise.reject(new Error('native bridge unavailable'))
  return invoke<T>(command)
}

type MobileRuntimeState = {
  paired?: boolean
  mode?: 'local' | 'remote' | null
}

type ChatPageProps = {
  notify: Notify
  navigate: (page: string, options?: { replace?: boolean }) => void
  browserNotify?: (event: string, data: unknown, options?: { force?: boolean }) => void
  registerPrimaryAction: (action: () => void) => () => void
  pendingAsset: PendingAsset | null
  onAssetConsumed: () => void
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  terminalOpen: boolean
  onToggleTerminal: () => void
}

export function ChatPage({
  notify,
  navigate,
  browserNotify,
  registerPrimaryAction,
  pendingAsset,
  onAssetConsumed,
  requestText,
  requestConfirm,
  terminalOpen,
  onToggleTerminal,
}: ChatPageProps) {
  const { t } = useI18n()
  const mobileApp = useClientStore((state) => state.client === 'mobile-app')
  const clientLoaded = useClientStore((state) => state.loaded)
  const phoneViewport = useIsPhoneViewport()
  const mobileLayout = mobileApp || phoneViewport
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  // 终端依赖桌面壳 PTY：网页端（浏览器/dev）不可用，按钮随之隐藏。
  const terminalAvailable =
    runtimeFeatureAvailable(capabilities, 'terminal') &&
    Boolean(window.pisperDesktop?.terminalProfiles)
  const localStreamSessionsRef = useRef(new Set<string>())
  const streamGenerationRef = useRef(new Map<string, number>())
  const resumeSyncRef = useRef<Promise<void> | null>(null)
  const catalog = useSessionCatalog({ notify })
  const liveSync = useLiveSessionSync({
    sessionStates: catalog.sessionStates,
    sessionStatesRef: catalog.sessionStatesRef,
    localStreamSessionsRef,
    streamGenerationRef,
    updateSessionState: catalog.updateSessionState,
    updateSessions: catalog.updateSessions,
  })
  const dock = useChatDock({
    sessions: catalog.sessions,
    sessionsRef: catalog.sessionsRef,
    sessionStates: catalog.sessionStates,
    activeId: catalog.activeId,
    setActiveId: catalog.setActiveId,
    loading: catalog.loading,
    localStreamSessionsRef,
    loadSessionMessages: liveSync.loadSessionMessages,
    releaseSessionState: catalog.releaseSessionState,
    singleSessionLayout: mobileLayout,
    notify,
  })

  const createSessionRecord = catalog.createSessionRecord
  const loadSessionMessages = liveSync.loadSessionMessages
  const refreshSessions = catalog.refreshSessions
  const sessionStatesRef = catalog.sessionStatesRef
  const syncLiveSession = liveSync.syncLiveSession
  const setGlobalError = catalog.setGlobalError
  const openSessionInDock = dock.openSessionInDock
  const moveSessionToGroup = dock.moveSessionToGroup
  const [recallPulse, setRecallPulse] = useState({ sessionId: '', token: 0 })
  // 新建会话：先创建记录（带可选 cwd），再在 Dock 打开，
  // 若指定了目标分组则把面板移动到该组。
  const createSession = useCallback(
    async (targetGroup?: DockviewGroupPanel, cwd = '') => {
      let mobileState: MobileRuntimeState | null = null
      if (mobileApp) {
        // 手机本机模式不能使用桌面会话路径；只有已配对且明确处于远程模式才继承它。
        mobileState = await invokeMobile<MobileRuntimeState>('mobile_state').catch(() => null)
      }
      const inheritRecentCwd = shouldInheritRecentSessionCwd(mobileApp, mobileState)
      const sessionId = await createSessionRecord(cwd, { inheritRecentCwd })
      if (!sessionId) return ''
      const opened = openSessionInDock(sessionId)
      if (opened) moveSessionToGroup(sessionId, targetGroup)
      return sessionId
    },
    [createSessionRecord, mobileApp, moveSessionToGroup, openSessionInDock],
  )
  usePagePrimaryAction(registerPrimaryAction, createSession)

  const promptCommands = usePromptCommands({
    browserNotify,
    notify,
    defaultModel: catalog.defaultModel,
    localStreamSessionsRef,
    streamGenerationRef,
    sessionStatesRef: catalog.sessionStatesRef,
    setActiveId: catalog.setActiveId,
    setGlobalError: catalog.setGlobalError,
    updateSessionState: catalog.updateSessionState,
    updateSessions: catalog.updateSessions,
    createSession,
    loadSessionMessages: liveSync.loadSessionMessages,
    refreshSessions: catalog.refreshSessions,
    syncLiveSession: liveSync.syncLiveSession,
  })
  const { sendPrompt } = promptCommands

  // 处理会话创建请求（可携带自动发送的提示词，如视觉生成卡片的「试试示例」）：
  // 事件监听 + localStorage 持久化，跨页面跳转/重启后挂载时也会补建。
  useEffect(() => {
    const createRequested = () => {
      const request = consumeSessionCreationRequest()
      if (!request) return
      void (async () => {
        const sessionId = await createSession(undefined, request.cwd)
        if (sessionId && request.prompt) void sendPrompt(request.prompt, sessionId)
      })()
    }
    window.addEventListener(SESSION_CREATE_REQUESTED_EVENT, createRequested)
    createRequested()
    return () => window.removeEventListener(SESSION_CREATE_REQUESTED_EVENT, createRequested)
  }, [createSession, sendPrompt])
  const sessionCommands = useSessionCommands({
    notify,
    requestText,
    requestConfirm,
    availableModels: catalog.availableModels,
    sessionStatesRef: catalog.sessionStatesRef,
    updateSessionState: catalog.updateSessionState,
    updateSessions: catalog.updateSessions,
    replaceSessionStates: catalog.replaceSessionStates,
    setGlobalError: catalog.setGlobalError,
    syncLiveSession: liveSync.syncLiveSession,
  })

  // 移动 WebView 从后台恢复时，SSE 可能既不报错也不再产生活动；
  // 先刷新目录，再强制用服务端快照校准所有已缓存/仍运行的会话。
  const syncAfterForeground = useCallback(() => {
    if (document.visibilityState !== 'visible' || resumeSyncRef.current) return
    const request = (async () => {
      if (mobileApp) {
        // 与 API 共用有界恢复闸门，避免独立原生命令挂起后永久锁住 resumeSyncRef。
        await waitForMobileRuntimeReady().catch(() => undefined)
      }
      let sessions: SessionSummary[] = []
      try {
        sessions = await refreshSessions(undefined, { preserveExistingOnEmpty: true })
      } catch {
        // 实时快照仍可使用已缓存的会话状态，目录请求失败不阻断恢复。
      }
      const ids = new Set([
        ...Object.keys(sessionStatesRef.current),
        ...(catalog.activeId ? [catalog.activeId] : []),
        ...sessions.filter((session) => session.streaming).map((session) => session.id),
      ])
      await Promise.allSettled(Array.from(ids).map((id) => syncLiveSession(id, { force: true })))
    })().finally(() => {
      resumeSyncRef.current = null
    })
    resumeSyncRef.current = request
  }, [catalog.activeId, mobileApp, refreshSessions, sessionStatesRef, syncLiveSession])

  useEffect(() => {
    const recover = () => syncAfterForeground()
    document.addEventListener('visibilitychange', recover)
    window.addEventListener('pageshow', recover)
    window.addEventListener('online', recover)
    return () => {
      document.removeEventListener('visibilitychange', recover)
      window.removeEventListener('pageshow', recover)
      window.removeEventListener('online', recover)
    }
  }, [syncAfterForeground])

  // 强制重载会话分支：刷新消息 + 列表，供恢复/切换后同步。
  const reloadSessionBranch = useCallback(
    async (sessionId: string) => {
      await loadSessionMessages(sessionId, { force: true })
      await refreshSessions(sessionId)
    },
    [loadSessionMessages, refreshSessions],
  )

  // 从边界条目派生新分支：调用运行时导航到历史条目处分支，
  // 成功后刷新列表并在 Dock 打开新会话。
  const branchFromEntry = useCallback(
    async (session: SessionSummary, boundaryEntryId: string) => {
      if (!session?.id || !boundaryEntryId) return
      try {
        setGlobalError('')
        const result = await chatApi.navigateSessionTree(session.id, boundaryEntryId, false)
        if (result.cancelled) return
        await reloadSessionBranch(session.id)
        setRecallPulse((current) => ({ sessionId: session.id, token: current.token + 1 }))
        notify(t('chat:chatPage.branchedFromNode'))
      } catch (error) {
        setGlobalError(error instanceof Error ? error.message : String(error))
      }
    },
    [notify, reloadSessionBranch, setGlobalError, t],
  )

  // 从已完成回复创建独立对话：命名后调用运行时 derive 接口，
  // 新会话拥有独立上下文，创建成功后立即打开对应 Dock 面板。
  const createChildSession = useCallback(
    async (session: SessionSummary, boundaryEntryId: string) => {
      if (!session?.id || !boundaryEntryId) return
      const name = await requestText({
        title: t('chat:chatPage.createChildChat'),
        inputLabel: t('chat:chatPage.chatTitle'),
        value: `${t('chat:chatPage.separateChatSuffix')} · ${session.name || t('chat:chatPage.newChat')}`,
        confirmLabel: t('chat:chatPage.create'),
      })
      if (name === null) return
      try {
        setGlobalError('')
        const created = await chatApi.deriveSession(session.id, boundaryEntryId, name)
        await refreshSessions(created.id)
        setRecallPulse((current) => ({ sessionId: session.id, token: current.token + 1 }))
        await new Promise<void>((resolve) => window.setTimeout(resolve, 550))
        openSessionInDock(created.id)
        notify(t('chat:chatPage.childChatCreated'))
      } catch (error) {
        setGlobalError(error instanceof Error ? error.message : String(error))
      }
    },
    [notify, openSessionInDock, refreshSessions, requestText, setGlobalError, t],
  )

  // 右栏辅助对话：新建默认打开；开合状态持久化。
  const [auxOpen, setAuxOpen] = useState(() => localStorage.getItem('pisper-aux-open') !== '0')
  const updateAuxOpen = useCallback((open: boolean) => {
    setAuxOpen(open)
    localStorage.setItem('pisper-aux-open', open ? '1' : '0')
  }, [])
  // 中右卡片比例（右栏占容器百分比）：默认 0.618 黄金分割，可拖拽调整，持久化。
  const [auxRatio, setAuxRatio] = useState(() => {
    const saved = Number(localStorage.getItem('pisper-aux-ratio'))
    return saved >= 0.2 && saved <= 0.75 ? saved : 0.382
  })
  const auxRatioRef = useRef(auxRatio)
  auxRatioRef.current = auxRatio
  useEffect(() => {
    localStorage.setItem('pisper-aux-ratio', String(Math.round(auxRatio * 1000) / 1000))
  }, [auxRatio])
  const auxRatioDrag = useRef<{ startX: number; startRatio: number; total: number } | null>(null)
  const startAuxRatioDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    const layout = event.currentTarget.parentElement
    auxRatioDrag.current = {
      startX: event.clientX,
      startRatio: auxRatio,
      total: layout ? layout.getBoundingClientRect().width : window.innerWidth,
    }
  }
  const moveAuxRatioDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!auxRatioDrag.current || auxRatioDrag.current.total <= 0) return
    // 向左拖 = 右栏变宽（clientX 减小 → 差值为正）。
    const delta = (auxRatioDrag.current.startX - event.clientX) / auxRatioDrag.current.total
    setAuxRatio(Math.min(0.75, Math.max(0.2, auxRatioDrag.current.startRatio + delta)))
  }
  const endAuxRatioDrag = () => {
    auxRatioDrag.current = null
  }
  // 中栏头部三件套的本地状态：侧栏开合（ui-store）与主题循环。
  const sidebarCollapsed = useUiStore((state) => state.sidebarCollapsed)
  const setSidebarCollapsed = useUiStore((state) => state.setSidebarCollapsed)
  const toggleSidebar = useCallback(
    () => setSidebarCollapsed(!sidebarCollapsed),
    [setSidebarCollapsed, sidebarCollapsed],
  )
  const theme = useUiStore((state) => state.theme)
  const cycleTheme = useUiStore((state) => state.cycleTheme)
  const ThemeIcon = CHAT_THEME_META[theme]
  const themeLabel =
    theme === 'light'
      ? t('navigation:pageHeader.light')
      : theme === 'dark'
        ? t('navigation:pageHeader.dark')
        : theme === 'scheduled'
          ? t('navigation:pageHeader.scheduled')
          : t('navigation:pageHeader.system')
  // 页头图标簇的辅助对话按钮经事件总线联动（PageHeader 与 ChatPage 跨层）。
  const auxOpenRef = useRef(auxOpen)
  auxOpenRef.current = auxOpen
  useEffect(() => {
    const toggle = () => updateAuxOpen(!auxOpenRef.current)
    window.addEventListener(AUX_CHAT_TOGGLE_EVENT, toggle)
    return () => window.removeEventListener(AUX_CHAT_TOGGLE_EVENT, toggle)
  }, [updateAuxOpen])

  const openModelSettings = useCallback(() => navigate('config'), [navigate])
  // 会话状态不进 context：流式期间 sessionStates 每帧变化，
  // 若随 context 广播会让所有 Dock 面板每帧重渲染。面板改为按会话订阅，
  // context 只携带低频数据与稳定回调，配合 useMemo 保持引用稳定。
  const dockContextValue: ChatDockContextValue = useMemo(
    () => ({
      sessions: catalog.sessions,
      subscribeSessionState: catalog.subscribeSessionState,
      getSessionState: catalog.getSessionState,
      defaultModel: catalog.defaultModel,
      availableModels: catalog.availableModels,
      globalError: catalog.globalError,
      activeId: catalog.activeId,
      compactDock: dock.compactDock,
      sessionTreePulseSessionId: recallPulse.sessionId,
      sessionTreePulseToken: recallPulse.token,
      pendingAsset,
      onAssetConsumed,
      notify,
      openModelSettings,
      loadSessionMessages: liveSync.loadSessionMessages,
      loadOlderMessages: liveSync.loadOlderMessages,
      sendPrompt: promptCommands.sendPrompt,
      retryLastTurn: promptCommands.retryLastTurn,
      queuePrompt: promptCommands.queuePrompt,
      withdrawQueuedInput: promptCommands.withdrawQueuedInput,
      abort: promptCommands.abort,
      pauseGoal: sessionCommands.pauseGoal,
      setGoalBudget: sessionCommands.setGoalBudget,
      compactSession: sessionCommands.compactSession,
      setCompactionThreshold: sessionCommands.setCompactionThreshold,
      switchSessionModel: sessionCommands.switchSessionModel,
      loadSessionThinkingLevel: sessionCommands.loadSessionThinkingLevel,
      switchSessionThinkingLevel: sessionCommands.switchSessionThinkingLevel,
      switchSessionExecutionMode: sessionCommands.switchSessionExecutionMode,
      switchSessionRunMode: sessionCommands.switchSessionRunMode,
      resolveToolApproval: sessionCommands.resolveToolApproval,
      selectSessionWorkspace: sessionCommands.selectSessionWorkspace,
      renameSession: sessionCommands.renameSession,
      branchFromEntry,
      createChildSession,
      reloadSessionBranch,
      splitDockPanel: dock.splitDockPanel,
      closeDockPanel: dock.closeDockPanel,
    }),
    [
      catalog.sessions,
      catalog.subscribeSessionState,
      catalog.getSessionState,
      catalog.defaultModel,
      catalog.availableModels,
      catalog.globalError,
      catalog.activeId,
      dock.compactDock,
      dock.splitDockPanel,
      dock.closeDockPanel,
      recallPulse.sessionId,
      recallPulse.token,
      pendingAsset,
      onAssetConsumed,
      notify,
      openModelSettings,
      liveSync.loadSessionMessages,
      liveSync.loadOlderMessages,
      promptCommands.sendPrompt,
      promptCommands.retryLastTurn,
      promptCommands.queuePrompt,
      promptCommands.withdrawQueuedInput,
      promptCommands.abort,
      sessionCommands.pauseGoal,
      sessionCommands.setGoalBudget,
      sessionCommands.compactSession,
      sessionCommands.setCompactionThreshold,
      sessionCommands.switchSessionModel,
      sessionCommands.loadSessionThinkingLevel,
      sessionCommands.switchSessionThinkingLevel,
      sessionCommands.switchSessionExecutionMode,
      sessionCommands.switchSessionRunMode,
      sessionCommands.resolveToolApproval,
      sessionCommands.selectSessionWorkspace,
      sessionCommands.renameSession,
      branchFromEntry,
      createChildSession,
      reloadSessionBranch,
    ],
  )

  return (
    <>
      <div
        className={`chat-layout relative grid w-full min-w-0 min-h-0 flex-1 gap-[6px] dock-layout max-[650px]:flex max-[650px]:flex-col max-[650px]:min-h-0 max-[650px]:gap-[0] -m-[6px] ${auxOpen && !mobileLayout ? 'grid-cols-[minmax(0,1fr)_auto]' : 'grid-cols-[minmax(0,1fr)]'}`}
      >
        {catalog.loading ? (
          <AppEmptyState>
            <RefreshCw className="animate-spin" size={24} />
            <h2>{t('chat:chatPage.wakingTheAgent')}</h2>
            <p>{t('chat:chatPage.modelsSessionsAndContextAreSettlingIntoPlace')}</p>
          </AppEmptyState>
        ) : (
          <>
            <div className="chat-dock-workspace max-[650px]:[flex:1_1_0] max-[650px]:min-h-0 relative flex min-w-0 min-h-0 flex-col [isolation:isolate] overflow-hidden [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-md)] bg-[var(--surface-muted)]">
              {/* 中栏头部：桌面聊天页的应用页头三件套移到这里（汉堡/标题/工具簇）。 */}
              {!mobileLayout && (
                <div className="chat-column-header flex h-[40px] flex-none items-center gap-[4px] [border-bottom:1px_solid_var(--stroke-soft)] [padding:0_8px]">
                  <button
                    type="button"
                    className="grid h-[28px] w-[28px] flex-none place-items-center border-0 rounded-[var(--r-xs)] bg-transparent text-[var(--text-muted)] cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--text)]"
                    title={t('navigation:appSidebar.expandSidebar')}
                    aria-label={t('navigation:appSidebar.expandSidebar')}
                    onClick={toggleSidebar}
                  >
                    <Menu size={17} />
                  </button>
                  <span className="ml-[2px] text-[13px] font-[650] text-[var(--text)]">
                    {t('common:app.sessions')}
                  </span>
                  <div className="flex-1" />
                  {terminalAvailable && (
                    <button
                      type="button"
                      className={`grid h-[28px] w-[28px] flex-none place-items-center border-0 rounded-[var(--r-xs)] bg-transparent cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--text)] max-[1200px]:hidden ${terminalOpen ? 'bg-[var(--surface-hover)] text-[var(--brand-blue)]' : 'text-[var(--text-muted)]'}`}
                      title={t('navigation:pageHeader.toggleTerminal')}
                      aria-label={t('navigation:pageHeader.toggleTerminal')}
                      aria-pressed={terminalOpen}
                      onClick={onToggleTerminal}
                    >
                      <TerminalSquare size={16} />
                    </button>
                  )}
                  <button
                    type="button"
                    className="grid h-[28px] w-[28px] flex-none place-items-center border-0 rounded-[var(--r-xs)] bg-transparent text-[var(--text-muted)] cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--text)] max-[1200px]:hidden"
                    title={t('navigation:pageHeader.auxChat')}
                    aria-label={t('navigation:pageHeader.auxChat')}
                    onClick={requestAuxChatToggle}
                  >
                    {auxOpen ? <PanelRightClose size={16} /> : <PanelRightOpen size={16} />}
                  </button>
                  <button
                    type="button"
                    className="grid h-[28px] w-[28px] flex-none place-items-center border-0 rounded-[var(--r-xs)] bg-transparent text-[var(--text-muted)] cursor-pointer hover:bg-[var(--surface-hover)] hover:text-[var(--text)]"
                    title={t('navigation:pageHeader.themeThemeClickToSwitch', {
                      theme: themeLabel,
                    })}
                    aria-label={t('navigation:pageHeader.themeThemeClickToSwitchThemes', {
                      theme: themeLabel,
                    })}
                    onClick={cycleTheme}
                  >
                    <ThemeIcon size={16} />
                  </button>
                </div>
              )}
              <div className="relative flex min-h-0 flex-1 flex-col">
                <ChatDockContext.Provider value={dockContextValue}>
                  {clientLoaded && mobileLayout ? (
                    <MobileSessionPanel
                      sessionIds={dock.mobileSessionIds}
                      onSelectSession={openSessionInDock}
                      onCreateSession={createSession}
                    />
                  ) : clientLoaded ? (
                    <Suspense fallback={null}>
                      <LazyChatDockView
                        compactDock={dock.compactDock}
                        onDockReady={dock.onDockReady}
                        getTabContextMenuItems={dock.getTabContextMenuItems}
                        createSession={createSession}
                      />
                    </Suspense>
                  ) : null}
                </ChatDockContext.Provider>
              </div>
            </div>
            {/* 中右之间的比例拖拽手柄：拖动调整右栏宽度占比（持久化）。 */}
            {!mobileLayout && auxOpen && (
              <div
                className="relative z-[10] w-[6px] flex-none cursor-col-resize bg-transparent after:absolute after:inset-y-0 after:left-[2px] after:w-px after:bg-transparent hover:after:bg-[var(--stroke-hover)]"
                onPointerDown={startAuxRatioDrag}
                onPointerMove={moveAuxRatioDrag}
                onPointerUp={endAuxRatioDrag}
                aria-hidden="true"
              />
            )}
            {/* 右栏辅助对话：与中栏完全同款卡片（同边框/圆角/底色），栅格 6px 缝分隔；窄屏隐藏。 */}
            {!mobileLayout && auxOpen && (
              <div
                className="relative min-h-0 overflow-hidden max-[1200px]:hidden [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-md)] bg-[var(--surface-subtle)]"
                style={{ width: `${Math.round(auxRatio * 100)}%` }}
              >
                <Suspense fallback={null}>
                  <LazyAuxChatPanel
                    cwd={catalog.sessions.find((item) => item.id === catalog.activeId)?.cwd || ''}
                    onClose={() => updateAuxOpen(false)}
                  />
                </Suspense>
              </div>
            )}
          </>
        )}
      </div>
      {sessionCommands.workspaceSession && (
        <WorkspacePicker
          open
          initialPath={sessionCommands.workspaceSession.cwd}
          description={t('common:workspacePicker.selectWorkspaceForChat', {
            name: sessionCommands.workspaceSession.name,
          })}
          onOpenChange={(open) => !open && sessionCommands.setWorkspaceSession(null)}
          onSelect={(cwd) =>
            sessionCommands.switchSessionCwd(sessionCommands.workspaceSession!, cwd)
          }
        />
      )}
    </>
  )
}
