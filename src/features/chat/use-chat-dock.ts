// 聊天 Dock hook：基于 dockview 的多会话分屏容器，
// 管理面板开/关/切换/聚焦与会话的映射，并持久化布局到 Runtime 用户目录。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type {
  BuiltInContextMenuItem,
  DockviewApi,
  DockviewGroupPanel,
  DockviewIDisposable,
  DockviewReadyEvent,
  GetTabContextMenuItemsParams,
  ReactContextMenuItemConfig,
} from 'dockview-react'
import { STORAGE_KEYS } from '@/app/storage'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import type { SessionState, SessionSummary } from '@/types/chat'
import {
  SESSION_SELECTED_EVENT,
  consumeSessionSelectionRequest,
  subscribeSessionDeletionUpdates,
} from './events'
import {
  createDockLayoutEnvelope,
  dockPositionForDisposition,
  initialDockSessionIds,
  panelIdForSession,
  classifyDockSnapshot,
  resolveTaskOpen,
  sessionIdFromPanel,
  sessionIdsFromDockLayout,
  type DockLayoutEnvelope,
  type SessionOpenDisposition,
  type SessionOpenRequest,
} from './dock-layout'
import { apiJson } from '@/lib/api'
import { FOCUS_MESSAGE_PAGE_SIZE } from './use-session-catalog'
import {
  WEB_PREVIEW_OPEN_EVENT,
  consumeWebPreviewRequest,
  type WebPreviewOpenRequest,
} from './web-preview-events'
import { useContextPagesStore } from './session-context-pages-store'
import { clearComposerDraft } from './composer-drafts'
import { WEB_PREVIEW_PANEL_ID, webPreviewPanelTitle } from './web-preview-panel'

type DockOptions = {
  sessions: SessionSummary[]
  sessionsRef: React.MutableRefObject<SessionSummary[]>
  sessionStates: Record<string, SessionState>
  activeId: string
  setActiveId: (id: string) => void
  loading: boolean
  localStreamSessionsRef: React.MutableRefObject<Set<string>>
  loadSessionMessages: (id: string, options?: { force?: boolean; limit?: number }) => Promise<void>
  releaseSessionState: (
    id: string,
    options?: { panelOpen?: boolean; localStreamOwned?: boolean },
  ) => boolean
  singleSessionLayout: boolean
  // 会话页由路由决定显示哪个会话：单会话布局下的打开请求改为交给路由，
  // 由 URL 变化再回写活动会话，避免目录与地址栏各自选中不同的会话。
  openRoutedSession?: (sessionId: string) => void
  notify: Notify
}

export function useChatDock({
  sessions,
  sessionsRef,
  sessionStates,
  activeId,
  setActiveId,
  loading,
  localStreamSessionsRef,
  loadSessionMessages,
  releaseSessionState,
  singleSessionLayout,
  openRoutedSession,
  notify,
}: DockOptions) {
  const { t } = useI18n()
  const noticeRef = useRef({ notify, t })
  noticeRef.current = { notify, t }
  const [dockReady, setDockReady] = useState(0)
  // 手机布局不渲染 Dock：打开会话时只切换活动 id。
  const singleSessionLayoutRef = useRef(singleSessionLayout)
  singleSessionLayoutRef.current = singleSessionLayout
  const openRoutedSessionRef = useRef(openRoutedSession)
  openRoutedSessionRef.current = openRoutedSession
  const [compactDock, setCompactDock] = useState(
    () => window.matchMedia('(max-width: 900px)').matches,
  )
  const dockApiRef = useRef<DockviewApi | null>(null)
  const dockInitializedRef = useRef(false)
  const layoutWritableRef = useRef(true)
  const latestLayoutRef = useRef<DockLayoutEnvelope | null>(null)
  const dockDisposablesRef = useRef<DockviewIDisposable[]>([])
  const layoutSaveTimerRef = useRef<number | undefined>(undefined)
  const pendingDockLayoutRef = useRef<string | null>(null)
  const pendingDockRequestRef = useRef<SessionOpenRequest | null>(null)
  const pendingWebPreviewRef = useRef<WebPreviewOpenRequest | null>(null)
  const compactDockRef = useRef(compactDock)
  const pendingLayoutWriteRef = useRef<string | null>(null)
  const layoutWriteRunningRef = useRef(false)
  const [storedDockLayout, setStoredDockLayout] = useState<DockLayoutEnvelope | null>(null)
  const [dockLayoutLoaded, setDockLayoutLoaded] = useState(false)
  const [mobileOpenedSessionIds, setMobileOpenedSessionIds] = useState<string[]>([])
  useEffect(() => {
    if (!singleSessionLayout || !activeId) return
    setMobileOpenedSessionIds((current) =>
      current.includes(activeId) ? current : [...current, activeId],
    )
  }, [activeId, singleSessionLayout])
  // 移动端把桌面布局和本次打开过的会话压平为页签，分屏关系仍留在桌面端维护。
  const mobileSessionIds = useMemo(() => {
    const validIds = new Set(sessions.map((session) => session.id))
    return [
      ...mobileOpenedSessionIds,
      ...sessionIdsFromDockLayout(
        singleSessionLayout
          ? latestLayoutRef.current?.layout || storedDockLayout?.layout
          : storedDockLayout?.layout,
      ),
      activeId,
    ].filter((id, index, ids) => id && validIds.has(id) && ids.indexOf(id) === index)
  }, [activeId, mobileOpenedSessionIds, sessions, storedDockLayout, singleSessionLayout])

  useEffect(() => {
    let mounted = true
    void apiJson<unknown>('/api/settings/chat-dock-layout')
      .then((value) => {
        if (!mounted) return
        const snapshot = classifyDockSnapshot(value)
        layoutWritableRef.current = snapshot.writable
        latestLayoutRef.current = snapshot.layout
        setStoredDockLayout(snapshot.layout)
        if (!snapshot.writable)
          noticeRef.current.notify(noticeRef.current.t('chat:workbench.layoutPreserved'), 'info')
        setDockLayoutLoaded(true)
      })
      .catch(() => {
        if (mounted) {
          layoutWritableRef.current = false
          noticeRef.current.notify(noticeRef.current.t('chat:workbench.layoutPreserved'), 'info')
          setDockLayoutLoaded(true)
        }
      })
    return () => {
      mounted = false
    }
  }, [])

  useEffect(() => {
    const media = window.matchMedia('(max-width: 900px)')
    const update = () => {
      compactDockRef.current = media.matches
      setCompactDock(media.matches)
    }
    update()
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [])

  // 只保留最新布局快照，由后台消费者异步写入，避免保存请求阻塞交互。
  const consumeDockLayoutWrites = useCallback(() => {
    if (layoutWriteRunningRef.current) return
    layoutWriteRunningRef.current = true
    void (async () => {
      do {
        while (pendingLayoutWriteRef.current) {
          const serialized = pendingLayoutWriteRef.current
          pendingLayoutWriteRef.current = null
          try {
            await apiJson('/api/settings/chat-dock-layout', {
              method: 'PUT',
              body: serialized,
            })
          } catch {}
        }
        layoutWriteRunningRef.current = false
        if (pendingLayoutWriteRef.current) layoutWriteRunningRef.current = true
      } while (layoutWriteRunningRef.current)
    })()
  }, [])

  const queueDockLayoutWrite = useCallback(
    (serialized: string) => {
      if (!layoutWritableRef.current) return
      pendingLayoutWriteRef.current = serialized
      consumeDockLayoutWrites()
    },
    [consumeDockLayoutWrites],
  )

  // 立即持久化 Dock 布局：序列化当前布局 envelope 写入 Runtime 用户目录；
  // 序列化失败时保留上一次成功内容，不抛错中断。
  const persistDockLayout = useCallback(
    (api: DockviewApi | null = dockApiRef.current) => {
      if (!dockInitializedRef.current) return false
      window.clearTimeout(layoutSaveTimerRef.current)
      layoutSaveTimerRef.current = undefined
      let serialized = pendingDockLayoutRef.current
      if (api) {
        try {
          const envelope = createDockLayoutEnvelope(api.toJSON(), api.activePanel?.id || '')
          latestLayoutRef.current = envelope
          serialized = JSON.stringify(envelope)
        } catch {}
      }
      if (!serialized) return false
      queueDockLayoutWrite(serialized)
      pendingDockLayoutRef.current = null
      return true
    },
    [queueDockLayoutWrite],
  )

  // 调度延迟保存布局：布局变化频繁（拖拽中），统一在空闲后落盘，
  // 避免每次 drag/resize 事件都触发一次序列化与写入。
  const scheduleDockLayoutSave = useCallback(
    (api: DockviewApi | null = dockApiRef.current) => {
      if (!api || !dockInitializedRef.current) return
      let serialized: string
      try {
        const envelope = createDockLayoutEnvelope(api.toJSON(), api.activePanel?.id || '')
        latestLayoutRef.current = envelope
        serialized = JSON.stringify(envelope)
      } catch {
        return
      }
      pendingDockLayoutRef.current = serialized
      window.clearTimeout(layoutSaveTimerRef.current)
      layoutSaveTimerRef.current = window.setTimeout(() => {
        layoutSaveTimerRef.current = undefined
        if (!dockInitializedRef.current || dockApiRef.current !== api) return
        queueDockLayoutWrite(serialized)
        if (pendingDockLayoutRef.current === serialized) pendingDockLayoutRef.current = null
      }, 180)
    },
    [queueDockLayoutWrite],
  )

  // 在 Dock 打开会话：不存在则新建面板，存在则聚焦并（可选）加载消息；
  // 返回是否新开了面板，供调用方决定是否移动分组。
  const openSessionInDock = useCallback(
    (sessionId: string, disposition: SessionOpenDisposition = 'open') => {
      // 单会话布局无 Dock：记录打开顺序并直接切换活动会话。
      if (singleSessionLayoutRef.current) {
        if (sessionId && openRoutedSessionRef.current) {
          openRoutedSessionRef.current(sessionId)
          return true
        }
        pendingDockRequestRef.current = { sessionId, disposition: 'open' }
        if (sessionId) {
          setMobileOpenedSessionIds((current) =>
            current.includes(sessionId) ? current : [...current, sessionId],
          )
        }
        setActiveId(sessionId)
        return true
      }
      const session = sessionsRef.current.find((item) => item.id === sessionId)
      const api = dockApiRef.current
      if (!session || !api || !dockInitializedRef.current) {
        pendingDockRequestRef.current = { sessionId, disposition }
        return false
      }
      const panelId = panelIdForSession(sessionId)
      const existing = api.getPanel(panelId)
      const referencePanel = api.activePanel
      const action = resolveTaskOpen({
        sessionId,
        visibleIds: api.panels.filter((panel) => panel.api.isVisible).map(sessionIdFromPanel),
        focusedId: sessionIdFromPanel(referencePanel),
        disposition,
        width: compactDockRef.current ? 0 : referencePanel?.api.width || api.width,
        height: compactDockRef.current ? 0 : referencePanel?.api.height || api.height,
      })
      if (existing) {
        existing.api.setActive()
        setActiveId(sessionId)
        return true
      }
      if (action.kind === 'unavailable') {
        notify(t('chat:workbench.splitUnavailable'), 'info')
        return false
      }
      const effectiveDisposition = disposition
      const position =
        effectiveDisposition === 'open'
          ? api.activeGroup
            ? { referenceGroup: api.activeGroup }
            : undefined
          : referencePanel
            ? { referencePanel, direction: effectiveDisposition }
            : { direction: effectiveDisposition }
      api.addPanel({
        id: panelId,
        component: 'session',
        title: session.name || t('chat:chatPage.untitledChat'),
        params: { sessionId },
        renderer: 'always',
        minimumWidth: 300,
        minimumHeight: 240,
        ...(position ? { position } : {}),
      })
      // 先在原分组加入新任务，再关闭被替换的视图，保留其分屏位置。
      if (action.kind === 'replace' && referencePanel) referencePanel.api.close()
      setActiveId(sessionId)
      void loadSessionMessages(sessionId, { limit: FOCUS_MESSAGE_PAGE_SIZE })
      return true
    },
    [loadSessionMessages, sessionsRef, setActiveId, notify, t],
  )

  // 在 Dock 打开 Web 预览面板：已存在则更新 URL 并聚焦，
  // 否则在活动面板右侧新建；Dock 未就绪时暂存请求待就绪后补开。
  const openWebPreviewInDock = useCallback(
    (request: WebPreviewOpenRequest) => {
      const api = dockApiRef.current
      if (!api || !dockInitializedRef.current) {
        pendingWebPreviewRef.current = request
        return false
      }
      const existing = api.getPanel(WEB_PREVIEW_PANEL_ID)
      const title = webPreviewPanelTitle(request.url, t('common:webPreview.title'))
      if (existing) {
        existing.api.updateParameters({ url: request.url })
        existing.api.setTitle(title)
        existing.api.setActive()
        return true
      }
      const referencePanel = api.activePanel
      const position = compactDockRef.current
        ? api.activeGroup
          ? { referenceGroup: api.activeGroup }
          : undefined
        : referencePanel
          ? { referencePanel, direction: 'right' as const }
          : { direction: 'right' as const }
      api.addPanel({
        id: WEB_PREVIEW_PANEL_ID,
        component: 'webPreview',
        title,
        params: { url: request.url },
        renderer: 'always',
        minimumWidth: 360,
        ...(position ? { position } : {}),
      })
      return true
    },
    [t],
  )

  const splitDockPanel = useCallback(
    (panelId: string, direction: Exclude<SessionOpenDisposition, 'open'>) => {
      const panel = dockApiRef.current?.getPanel(panelId)
      if (!panel) return
      if (compactDockRef.current) {
        notify(t('chat:chatPage.onNarrowScreensChatsStayInASingleTabGroup'), 'info')
        return
      }
      if (panel.group.size <= 1) {
        notify(
          t(
            'chat:chatPage.thisGroupHasOnlyOneChatChooseAnotherChatFromTheSessionListToCreateASplit',
          ),
          'info',
        )
        return
      }
      panel.api.moveTo({ group: panel.group, position: dockPositionForDisposition(direction) })
      panel.api.setActive()
    },
    [notify, t],
  )

  // 关闭 Dock 面板。
  const closeDockPanel = useCallback((panelId: string) => {
    dockApiRef.current?.getPanel(panelId)?.api.close()
  }, [])

  // 把会话面板移动到指定分组（无目标或已在同组时忽略）。
  const moveSessionToGroup = useCallback((sessionId: string, group?: DockviewGroupPanel) => {
    if (!group) return
    const panel = dockApiRef.current?.getPanel(panelIdForSession(sessionId))
    if (panel && panel.group !== group) panel.api.moveTo({ group })
  }, [])

  // 面板关闭时释放会话状态：告知目录该面板已关（且未持有本地流），
  // 决定是否保留运行中的状态供重开续显。
  const releaseIfClosed = useCallback(
    (sessionId: string) => {
      const panelOpen = Boolean(dockApiRef.current?.getPanel(panelIdForSession(sessionId)))
      return releaseSessionState(sessionId, {
        panelOpen,
        localStreamOwned: localStreamSessionsRef.current.has(sessionId),
      })
    },
    [localStreamSessionsRef, releaseSessionState],
  )

  // Dock 就绪回调：绑定布局/面板事件（活动变更、拖拽、添加/移除），
  // 恢复上次持久化布局，并补开暂存的预览/会话请求。
  const onDockReady = useCallback(
    ({ api }: DockviewReadyEvent) => {
      if (dockApiRef.current && dockApiRef.current !== api) dockInitializedRef.current = false
      for (const disposable of dockDisposablesRef.current) disposable.dispose()
      dockDisposablesRef.current = []
      dockApiRef.current = api
      dockDisposablesRef.current.push(
        api.onDidActivePanelChange(({ panel }) => {
          const sessionId = sessionIdFromPanel(panel)
          if (sessionId) setActiveId(sessionId)
          scheduleDockLayoutSave(api)
        }),
        api.onDidLayoutChange(() => scheduleDockLayoutSave(api)),
        api.onDidAddPanel((panel) => {
          panel.group.header.hidden = panel.group.panels.every(
            (item) => item.api.component === 'session',
          )
          const sessionId = sessionIdFromPanel(panel)
          if (sessionId) void loadSessionMessages(sessionId, { limit: FOCUS_MESSAGE_PAGE_SIZE })
        }),
        api.onDidRemovePanel((panel) => {
          const sessionId = sessionIdFromPanel(panel)
          if (sessionId) queueMicrotask(() => releaseIfClosed(sessionId))
        }),
      )
      setDockReady((generation) => generation + 1)
    },
    [loadSessionMessages, releaseIfClosed, scheduleDockLayoutSave, setActiveId],
  )

  useEffect(() => {
    if (!dockInitializedRef.current) return
    for (const id of Object.keys(sessionStates)) releaseIfClosed(id)
  }, [releaseIfClosed, sessionStates])

  useEffect(() => {
    const selectSession = (event: Event) => {
      const detail = (event as CustomEvent<SessionOpenRequest & { id?: string }>).detail
      const sessionId = detail?.sessionId || detail?.id
      if (!sessionId) return
      localStorage.removeItem(STORAGE_KEYS.sessionOpenRequest)
      openSessionInDock(sessionId, detail?.disposition || 'open')
      if (detail?.targetEntryId) {
        void loadSessionMessages(sessionId, { force: true, limit: FOCUS_MESSAGE_PAGE_SIZE })
      }
    }
    window.addEventListener(SESSION_SELECTED_EVENT, selectSession)
    return () => window.removeEventListener(SESSION_SELECTED_EVENT, selectSession)
  }, [loadSessionMessages, openSessionInDock])

  useEffect(() => {
    return subscribeSessionDeletionUpdates(window, ({ deletedIds }) => {
      const deleted = new Set(deletedIds)
      for (const id of deletedIds) {
        useContextPagesStore.getState().forget(id)
        clearComposerDraft(id)
      }
      setMobileOpenedSessionIds((current) => current.filter((id) => !deleted.has(id)))
      if (pendingDockRequestRef.current && deleted.has(pendingDockRequestRef.current.sessionId)) {
        pendingDockRequestRef.current = null
      }
      const api = dockApiRef.current
      if (!api || !dockInitializedRef.current) return
      const panels = deletedIds.flatMap((id) => {
        const panel = api.getPanel(panelIdForSession(id))
        return panel ? [panel] : []
      })
      for (const panel of panels) panel.api.close()
      if (panels.length && !api.panels.some((panel) => sessionIdFromPanel(panel))) {
        const fallback = sessionsRef.current.find((session) => !deleted.has(session.id))
        if (fallback) openSessionInDock(fallback.id)
        else setActiveId('')
      }
    })
  }, [openSessionInDock, sessionsRef, setActiveId])

  useEffect(() => {
    const openPreview = (event: Event) => {
      const request = (event as CustomEvent<WebPreviewOpenRequest>).detail
      if (!request?.url) return
      localStorage.removeItem(STORAGE_KEYS.webPreviewRequest)
      openWebPreviewInDock(request)
    }
    window.addEventListener(WEB_PREVIEW_OPEN_EVENT, openPreview)
    return () => window.removeEventListener(WEB_PREVIEW_OPEN_EVENT, openPreview)
  }, [openWebPreviewInDock])

  useEffect(() => {
    const flushWhenHidden = () => {
      if (document.visibilityState === 'hidden') persistDockLayout()
    }
    const flushBeforePageHide = () => persistDockLayout()
    document.addEventListener('visibilitychange', flushWhenHidden)
    window.addEventListener('pagehide', flushBeforePageHide)
    return () => {
      document.removeEventListener('visibilitychange', flushWhenHidden)
      window.removeEventListener('pagehide', flushBeforePageHide)
    }
  }, [persistDockLayout])

  useEffect(
    () => () => {
      const api = dockApiRef.current
      persistDockLayout(api)
      dockInitializedRef.current = false
      window.clearTimeout(layoutSaveTimerRef.current)
      layoutSaveTimerRef.current = undefined
      for (const disposable of dockDisposablesRef.current) disposable.dispose()
      dockDisposablesRef.current = []
      dockApiRef.current = null
    },
    [persistDockLayout],
  )

  useEffect(() => {
    if (
      !dockReady ||
      !dockLayoutLoaded ||
      loading ||
      dockInitializedRef.current ||
      !dockApiRef.current
    )
      return
    const api = dockApiRef.current
    let cancelled = false
    queueMicrotask(() => {
      if (cancelled || dockApiRef.current !== api || dockInitializedRef.current) return
      const validIds = new Set(sessions.map((session) => session.id))
      const storedLayout = latestLayoutRef.current || storedDockLayout
      if (storedLayout) {
        try {
          api.fromJSON(storedLayout.layout)
          for (const panel of [...api.panels]) {
            if (panel.api.component !== 'session') continue
            const sessionId = sessionIdFromPanel(panel)
            if (!validIds.has(sessionId)) api.removePanel(panel)
          }
        } catch {
          layoutWritableRef.current = false
          notify(t('chat:workbench.layoutPreserved'), 'info')
          api.clear()
        }
      }
      if (!api.panels.length) {
        const initialIds = initialDockSessionIds({
          activeSessionId: activeId,
          validSessionIds: sessions.map((session) => session.id),
        })
        let referenceGroup: DockviewGroupPanel | undefined
        for (const sessionId of initialIds) {
          const session = sessions.find((item) => item.id === sessionId)
          if (!session) continue
          const panel = api.addPanel({
            id: panelIdForSession(sessionId),
            component: 'session',
            title: session.name || t('chat:chatPage.untitledChat'),
            params: { sessionId },
            renderer: 'always',
            minimumWidth: 300,
            ...(referenceGroup ? { position: { referenceGroup } } : {}),
            inactive: Boolean(referenceGroup),
          })
          referenceGroup ||= panel.group
        }
      }
      for (const group of api.groups) {
        for (const panel of [...group.panels]) {
          if (panel !== group.activePanel && panel.api.component === 'session')
            api.removePanel(panel)
        }
        group.header.hidden = group.panels.every((panel) => panel.api.component === 'session')
      }
      for (const panel of api.panels) {
        const session = sessions.find((item) => item.id === sessionIdFromPanel(panel))
        if (session) panel.api.setTitle(session.name || t('chat:chatPage.untitledChat'))
      }
      dockInitializedRef.current = true
      localStorage.removeItem(STORAGE_KEYS.chatMode)
      const request = pendingDockRequestRef.current || consumeSessionSelectionRequest()
      pendingDockRequestRef.current = null
      if (request) {
        openSessionInDock(request.sessionId, request.disposition)
        if (request.targetEntryId) {
          void loadSessionMessages(request.sessionId, {
            force: true,
            limit: FOCUS_MESSAGE_PAGE_SIZE,
          })
        }
      } else {
        const preferredPanel =
          api.getPanel(storedLayout?.activePanelId || panelIdForSession(activeId)) || api.panels[0]
        preferredPanel?.api.setActive()
      }
      const previewRequest = pendingWebPreviewRef.current || consumeWebPreviewRequest()
      pendingWebPreviewRef.current = null
      if (previewRequest) openWebPreviewInDock(previewRequest)
      scheduleDockLayoutSave(api)
    })
    return () => {
      cancelled = true
    }
  }, [
    activeId,
    dockLayoutLoaded,
    dockReady,
    loadSessionMessages,
    loading,
    openSessionInDock,
    openWebPreviewInDock,
    scheduleDockLayoutSave,
    sessions,
    storedDockLayout,
    notify,
    t,
  ])

  useEffect(() => {
    if (!dockInitializedRef.current || !dockApiRef.current) return
    for (const session of sessions) {
      dockApiRef.current
        .getPanel(panelIdForSession(session.id))
        ?.api.setTitle(session.name || t('chat:chatPage.untitledChat'))
    }
  }, [sessions, t])

  const getTabContextMenuItems = useCallback(
    ({
      panel,
    }: GetTabContextMenuItemsParams): Array<
      BuiltInContextMenuItem | ReactContextMenuItemConfig
    > => {
      const closeItem: ReactContextMenuItemConfig = {
        label: t('chat:focusSession.closeTab'),
        action: () => closeDockPanel(panel.id),
      }
      if (compactDock || panel.group.size <= 1) return [closeItem]
      return [
        {
          label: t('chat:chatPage.splitToLeft'),
          action: () => splitDockPanel(panel.id, 'left'),
        },
        {
          label: t('chat:chatPage.splitToRight'),
          action: () => splitDockPanel(panel.id, 'right'),
        },
        {
          label: t('chat:chatPage.splitToTop'),
          action: () => splitDockPanel(panel.id, 'above'),
        },
        {
          label: t('chat:chatPage.splitToBottom'),
          action: () => splitDockPanel(panel.id, 'below'),
        },
        'separator',
        closeItem,
      ]
    },
    [closeDockPanel, compactDock, splitDockPanel, t],
  )

  const onDockDispose = useCallback(
    (api: DockviewApi) => {
      if (dockApiRef.current !== api) return
      persistDockLayout(api)
      dockInitializedRef.current = false
      for (const disposable of dockDisposablesRef.current) disposable.dispose()
      dockDisposablesRef.current = []
      dockApiRef.current = null
    },
    [persistDockLayout],
  )

  return {
    compactDock,
    mobileSessionIds,
    openSessionInDock,
    splitDockPanel,
    closeDockPanel,
    moveSessionToGroup,
    onDockReady,
    onDockDispose,
    getTabContextMenuItems,
  }
}
