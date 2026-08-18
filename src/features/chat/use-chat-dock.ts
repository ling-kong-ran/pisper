// 聊天 Dock hook：基于 dockview 的多会话分屏容器，
// 管理面板开/关/切换/聚焦与会话的映射，并持久化布局到 localStorage。
import { useCallback, useEffect, useRef, useState } from 'react'
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
import { SESSION_SELECTED_EVENT, consumeSessionSelectionRequest } from './events'
import {
  createDockLayoutEnvelope,
  dockPositionForDisposition,
  initialDockSessionIds,
  panelIdForSession,
  parseDockLayoutEnvelope,
  sessionIdFromPanel,
  type SessionOpenDisposition,
  type SessionOpenRequest,
} from './dock-layout'
import { FOCUS_MESSAGE_PAGE_SIZE } from './use-session-catalog'
import {
  WEB_PREVIEW_OPEN_EVENT,
  consumeWebPreviewRequest,
  type WebPreviewOpenRequest,
} from './web-preview-events'
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
  notify: Notify
}

function readStoredArray(key: string): string[] {
  try {
    const value = JSON.parse(localStorage.getItem(key) || '[]')
    return Array.isArray(value) ? value.filter((item) => typeof item === 'string') : []
  } catch {
    return []
  }
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
  notify,
}: DockOptions) {
  const { t } = useI18n()
  const [dockReady, setDockReady] = useState(0)
  const [compactDock, setCompactDock] = useState(
    () => window.matchMedia('(max-width: 900px)').matches,
  )
  const dockApiRef = useRef<DockviewApi | null>(null)
  const dockInitializedRef = useRef(false)
  const dockDisposablesRef = useRef<DockviewIDisposable[]>([])
  const layoutSaveTimerRef = useRef<number | undefined>(undefined)
  const pendingDockLayoutRef = useRef<string | null>(null)
  const pendingDockRequestRef = useRef<SessionOpenRequest | null>(null)
  const pendingWebPreviewRef = useRef<WebPreviewOpenRequest | null>(null)
  const compactDockRef = useRef(compactDock)
  const legacyTiledSessionIdsRef = useRef(readStoredArray(STORAGE_KEYS.tiledSessions))

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

  // 立即持久化 Dock 布局：序列化当前布局 envelope 写入 localStorage；
  // 序列化失败时保留上一次成功内容，不抛错中断。
  const persistDockLayout = useCallback((api: DockviewApi | null = dockApiRef.current) => {
    if (!dockInitializedRef.current) return false
    window.clearTimeout(layoutSaveTimerRef.current)
    layoutSaveTimerRef.current = undefined
    let serialized = pendingDockLayoutRef.current
    if (api) {
      try {
        const envelope = createDockLayoutEnvelope(api.toJSON(), api.activePanel?.id || '')
        serialized = JSON.stringify(envelope)
      } catch {}
    }
    if (!serialized) return false
    try {
      localStorage.setItem(STORAGE_KEYS.chatDockLayout, serialized)
      pendingDockLayoutRef.current = null
      return true
    } catch {
      return false
    }
  }, [])

  // 调度延迟保存布局：布局变化频繁（拖拽中），统一在空闲后落盘，
  // 避免每次 drag/resize 事件都触发一次序列化与写入。
  const scheduleDockLayoutSave = useCallback((api: DockviewApi | null = dockApiRef.current) => {
    if (!api || !dockInitializedRef.current) return
    let serialized: string
    try {
      serialized = JSON.stringify(createDockLayoutEnvelope(api.toJSON(), api.activePanel?.id || ''))
    } catch {
      return
    }
    pendingDockLayoutRef.current = serialized
    window.clearTimeout(layoutSaveTimerRef.current)
    layoutSaveTimerRef.current = window.setTimeout(() => {
      layoutSaveTimerRef.current = undefined
      if (!dockInitializedRef.current || dockApiRef.current !== api) return
      try {
        localStorage.setItem(STORAGE_KEYS.chatDockLayout, serialized)
        if (pendingDockLayoutRef.current === serialized) pendingDockLayoutRef.current = null
      } catch {}
    }, 180)
  }, [])

  // 在 Dock 打开会话：不存在则新建面板，存在则聚焦并（可选）加载消息；
  // 返回是否新开了面板，供调用方决定是否移动分组。
  const openSessionInDock = useCallback(
    (sessionId: string, disposition: SessionOpenDisposition = 'open') => {
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
          if (referencePanel && existing.id !== referencePanel.id) {
            existing.api.moveTo({
              group: referencePanel.group,
              position: dockPositionForDisposition(effectiveDisposition),
            })
          } else if (existing.group.size > 1) {
            existing.api.moveTo({
              group: existing.group,
              position: dockPositionForDisposition(effectiveDisposition),
            })
          }
        }
        existing.api.setActive()
        setActiveId(sessionId)
        return true
      }
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
      setActiveId(sessionId)
      void loadSessionMessages(sessionId, { limit: FOCUS_MESSAGE_PAGE_SIZE })
      return true
    },
    [loadSessionMessages, sessionsRef, setActiveId, t],
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
    if (!dockReady || loading || dockInitializedRef.current || !dockApiRef.current) return
    const api = dockApiRef.current
    const validIds = new Set(sessions.map((session) => session.id))
    const storedLayout = parseDockLayoutEnvelope(localStorage.getItem(STORAGE_KEYS.chatDockLayout))
    if (storedLayout) {
      try {
        api.fromJSON(storedLayout.layout)
        for (const panel of [...api.panels]) {
          if (panel.api.component !== 'session') continue
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
  }, [
    activeId,
    dockReady,
    loadSessionMessages,
    loading,
    openSessionInDock,
    openWebPreviewInDock,
    scheduleDockLayoutSave,
    sessions,
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
    > => [
      {
        label: t('chat:chatPage.splitToLeft'),
        action: () => splitDockPanel(panel.id, 'left'),
        disabled: compactDock || panel.group.size <= 1,
      },
      {
        label: t('chat:chatPage.splitToRight'),
        action: () => splitDockPanel(panel.id, 'right'),
        disabled: compactDock || panel.group.size <= 1,
      },
      {
        label: t('chat:chatPage.splitToTop'),
        action: () => splitDockPanel(panel.id, 'above'),
        disabled: compactDock || panel.group.size <= 1,
      },
      {
        label: t('chat:chatPage.splitToBottom'),
        action: () => splitDockPanel(panel.id, 'below'),
        disabled: compactDock || panel.group.size <= 1,
      },
      'separator',
      'close',
    ],
    [compactDock, splitDockPanel, t],
  )

  return {
    compactDock,
    openSessionInDock,
    splitDockPanel,
    closeDockPanel,
    moveSessionToGroup,
    onDockReady,
    getTabContextMenuItems,
  }
}
