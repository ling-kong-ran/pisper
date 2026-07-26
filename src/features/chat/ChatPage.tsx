import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  DockviewReact,
  type BuiltInContextMenuItem,
  type DockviewApi,
  type DockviewGroupPanel,
  type DockviewIDisposable,
  type DockviewReadyEvent,
  type GetTabContextMenuItemsParams,
  type ReactContextMenuItemConfig,
} from 'dockview-react'
import { RefreshCw } from 'lucide-react'
import { APP_NAME } from '@/app/brand'
import { STORAGE_KEYS } from '@/app/storage'
import { useI18n } from '@/app/use-i18n'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { Panel } from '@/components/ui'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { applyTextPatch } from '@/lib/api'
import { ApiError } from '@/lib/http'
import { workspaceName } from '@/lib/format'
import {
  applySessionUpdate,
  DEFAULT_SESSION_STATE,
  insertInteractiveUserMessage,
  resolveQueuedInputs,
} from '@/lib/session-state'
import {
  createStreamingTextScheduler,
  createToolUpdateScheduler,
  createTypewriterDisplay,
} from '@/lib/streaming-ui'
import {
  SESSION_SELECTED_EVENT,
  announceActiveSession,
  announceSessionsUpdated,
  consumeSessionSelectionRequest,
} from './events'
import { ChatDockWatermark, SessionDockPanel } from './ChatDock'
import { WebPreviewDockPanel } from './WebPreviewDockPanel'
import { chatApi, type ApiRecord } from './chat-api'
import { ChatDockContext, type ChatDockContextValue } from './chat-dock-context'
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
import { pushCurrentActivity, settleToolCalls, taskListChanges } from './run-activity'
import { shouldPollLiveSession } from './live-session-sync'
import { mergeSessionLists } from './session-list'
import {
  WEB_PREVIEW_OPEN_EVENT,
  consumeWebPreviewRequest,
  type WebPreviewOpenRequest,
} from './web-preview-events'
import { WEB_PREVIEW_PANEL_ID, webPreviewPanelTitle } from './web-preview-panel'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import type { Notify } from '@/app/route-context'
import type {
  ChatAttachment,
  ModelOption,
  PendingAsset,
  SandboxStatus,
  SessionState,
  SessionSummary,
} from '@/types/chat'
import type { SessionStateUpdate } from '@/lib/session-state'

const MAX_LIVE_THINKING_CHARS = 6_000
const USAGE_UPDATED_EVENT = 'vesper:usage-updated'
const FOCUS_MESSAGE_PAGE_SIZE = 40

type ChatPageProps = {
  notify: Notify
  browserNotify?: (event: string, data: unknown, options?: { force?: boolean }) => void
  registerPrimaryAction: (action: () => void) => () => void
  pendingAsset: PendingAsset | null
  onAssetConsumed: () => void
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

function latestPageState(current: SessionState, data: ApiRecord) {
  const incomingStart = Number(data.pageInfo?.start) || 0
  const currentStart = Number.isInteger(current.messageStart) ? current.messageStart : null
  const preservePrefix = currentStart != null && currentStart <= incomingStart
  const prefixLength = preservePrefix ? Math.max(0, incomingStart - currentStart) : 0
  const messageStart = preservePrefix ? currentStart : incomingStart
  return {
    messages: preservePrefix
      ? [...current.messages.slice(0, prefixLength), ...data.messages]
      : data.messages,
    messageStart,
    hasOlder: messageStart > 0,
    olderCursor: messageStart > 0 ? String(messageStart) : null,
  }
}

function readStoredArray(key: string): string[] {
  try {
    const value = JSON.parse(localStorage.getItem(key) || '[]')
    return Array.isArray(value) ? value.filter((item) => typeof item === 'string') : []
  } catch {
    return []
  }
}

export function ChatPage({
  notify,
  browserNotify,
  registerPrimaryAction,
  pendingAsset,
  onAssetConsumed,
  requestText,
  requestConfirm,
}: ChatPageProps) {
  const { t } = useI18n()
  const [remoteSessions, setRemoteSessions] = useState<SessionSummary[]>([])
  const [activeId, setActiveId] = useState(
    () => localStorage.getItem(STORAGE_KEYS.activeSession) || '',
  )
  const [sessionStates, setSessionStates] = useState<Record<string, SessionState>>({})
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [model, setModel] = useState(() => t('chat:chatPage.waitingForConfiguration'))
  const [availableModels, setAvailableModels] = useState<ModelOption[]>([])
  const [sandboxStatus, setSandboxStatus] = useState<SandboxStatus>({
    state: 'checking',
    supported: true,
    platform: '',
  })
  const [workspaceSession, setWorkspaceSession] = useState<SessionSummary | null>(null)
  const [dockReady, setDockReady] = useState(0)
  const [compactDock, setCompactDock] = useState(
    () => window.matchMedia('(max-width: 900px)').matches,
  )
  const creatingSessionRef = useRef<Promise<string> | null>(null)
  const sessionStatesRef = useRef(sessionStates)
  const sessionsRef = useRef(remoteSessions)
  const dockApiRef = useRef<DockviewApi | null>(null)
  const dockInitializedRef = useRef(false)
  const dockDisposablesRef = useRef<DockviewIDisposable[]>([])
  const layoutSaveTimerRef = useRef<number | undefined>(undefined)
  const pendingDockRequestRef = useRef<SessionOpenRequest | null>(null)
  const pendingWebPreviewRef = useRef<WebPreviewOpenRequest | null>(null)
  const compactDockRef = useRef(compactDock)
  const legacyTiledSessionIdsRef = useRef(readStoredArray(STORAGE_KEYS.tiledSessions))
  const localStreamSessionsRef = useRef(new Set<string>())
  const liveSyncInFlightRef = useRef(new Set<string>())

  useEffect(() => {
    sessionStatesRef.current = sessionStates
  }, [sessionStates])

  useEffect(() => {
    sessionsRef.current = remoteSessions
  }, [remoteSessions])

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

  const updateSessionState = useCallback((id: string, update: SessionStateUpdate) => {
    if (!id) return
    const current = sessionStatesRef.current
    const previous = current[id] || DEFAULT_SESSION_STATE
    const next = applySessionUpdate(previous, update)
    if (next === previous) return
    const states = { ...current, [id]: next }
    sessionStatesRef.current = states
    setSessionStates(states)
  }, [])

  const syncLiveSession = useCallback(
    async (id: string) => {
      if (!id || localStreamSessionsRef.current.has(id) || liveSyncInFlightRef.current.has(id))
        return
      liveSyncInFlightRef.current.add(id)
      try {
        const data = await chatApi.getLiveSession(id)
        // A local SSE stream may have started while the snapshot request was in flight.
        // Never let that stale snapshot replace the optimistic assistant message and its stable key.
        if (localStreamSessionsRef.current.has(id)) return
        updateSessionState(id, (current) => {
          const finishedAt = data.finishedAt || current.runFinishedAt || new Date().toISOString()
          return {
            ...current,
            ...latestPageState(current, data),
            tools: data.streaming
              ? data.tools || []
              : settleToolCalls(data.tools || [], { finishedAt, error: data.error || '' }),
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
            currentActivity: data.streaming
              ? data.currentActivity || current.currentActivity || null
              : null,
            activityFeed: data.streaming ? data.activityFeed || current.activityFeed || [] : [],
            thinkingText: data.streaming ? (data.thinkingText ?? current.thinkingText ?? '') : '',
            queuedInputs: resolveQueuedInputs(current.queuedInputs, data.queuedInputs),
            hadQueuedInput: data.streaming
              ? Boolean(current.hadQueuedInput || data.queuedInputs?.length)
              : false,
          }
        })
        setRemoteSessions((current) =>
          current.map((session) =>
            session.id === id
              ? {
                  ...session,
                  streaming: data.streaming,
                  model: data.model || session.model,
                  cwd: data.cwd || session.cwd,
                  permissionMode: data.permissionMode || session.permissionMode,
                  executionMode: data.executionMode || session.executionMode,
                  goal: data.goal ?? session.goal ?? null,
                  taskList: data.taskList ?? session.taskList ?? null,
                }
              : session,
          ),
        )
      } catch (caught) {
        if (!localStreamSessionsRef.current.has(id))
          updateSessionState(id, { recovering: false, loading: false, error: errorMessage(caught) })
      } finally {
        liveSyncInFlightRef.current.delete(id)
      }
    },
    [updateSessionState],
  )

  const loadSessionMessages = useCallback(
    async (
      id: string,
      { force = false, limit = FOCUS_MESSAGE_PAGE_SIZE }: { force?: boolean; limit?: number } = {},
    ) => {
      if (!id) return
      const current = sessionStatesRef.current[id]
      if (current?.recovering) {
        await syncLiveSession(id)
        return
      }
      if (
        !force &&
        (current?.loading ||
          current?.streaming ||
          (current?.loaded && (current.pageSize || 0) >= limit))
      )
        return
      updateSessionState(id, { loading: true })
      try {
        const data = await chatApi.getMessages(id, { limit })
        updateSessionState(id, (latest) =>
          latest.streaming
            ? {
                ...latest,
                loaded: true,
                loading: false,
                pageSize: Math.max(latest.pageSize || 0, limit),
              }
            : {
                ...latest,
                ...latestPageState(latest, data),
                contextUsage: data.contextUsage ?? latest.contextUsage ?? null,
                loaded: true,
                loading: false,
                pageSize: Math.max(latest.pageSize || 0, limit),
                error: '',
                olderError: '',
              },
        )
      } catch (caught) {
        updateSessionState(id, { loading: false, error: errorMessage(caught) })
      }
    },
    [syncLiveSession, updateSessionState],
  )

  const loadOlderMessages = useCallback(
    async (id: string) => {
      const current = sessionStatesRef.current[id]
      if (!id || !current?.hasOlder || !current.olderCursor || current.loadingOlder) return false
      updateSessionState(id, { loadingOlder: true, olderError: '' })
      try {
        const data = await chatApi.getMessages(id, {
          limit: FOCUS_MESSAGE_PAGE_SIZE,
          before: current.olderCursor,
        })
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
        updateSessionState(id, { loadingOlder: false, olderError: errorMessage(caught) })
        return false
      }
    },
    [updateSessionState],
  )

  const scheduleDockLayoutSave = useCallback((api: DockviewApi | null = dockApiRef.current) => {
    if (!api || !dockInitializedRef.current) return
    window.clearTimeout(layoutSaveTimerRef.current)
    layoutSaveTimerRef.current = window.setTimeout(() => {
      const envelope = createDockLayoutEnvelope(api.toJSON(), api.activePanel?.id || '')
      localStorage.setItem(STORAGE_KEYS.chatDockLayout, JSON.stringify(envelope))
    }, 180)
  }, [])

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
          if (referencePanel && existing.id !== referencePanel.id)
            existing.api.moveTo({
              group: referencePanel.group,
              position: dockPositionForDisposition(effectiveDisposition),
            })
          else if (existing.group.size > 1)
            existing.api.moveTo({
              group: existing.group,
              position: dockPositionForDisposition(effectiveDisposition),
            })
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
    [loadSessionMessages, t],
  )

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
      panel.api.moveTo({
        group: panel.group,
        position: dockPositionForDisposition(direction),
      })
      panel.api.setActive()
    },
    [notify, t],
  )

  const closeDockPanel = useCallback((panelId: string) => {
    dockApiRef.current?.getPanel(panelId)?.api.close()
  }, [])

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
        }),
        api.onDidLayoutChange(() => scheduleDockLayoutSave(api)),
        api.onDidAddPanel((panel) => {
          const sessionId = sessionIdFromPanel(panel)
          if (sessionId) void loadSessionMessages(sessionId, { limit: FOCUS_MESSAGE_PAGE_SIZE })
        }),
      )
      setDockReady((generation) => generation + 1)
    },
    [loadSessionMessages, scheduleDockLayoutSave],
  )

  useEffect(() => {
    const selectSession = (event: Event) => {
      const detail = (event as CustomEvent<SessionOpenRequest & { id?: string }>).detail
      const sessionId = detail?.sessionId || detail?.id
      if (!sessionId) return
      localStorage.removeItem(STORAGE_KEYS.sessionOpenRequest)
      openSessionInDock(sessionId, detail?.disposition || 'open')
    }
    window.addEventListener(SESSION_SELECTED_EVENT, selectSession)
    return () => window.removeEventListener(SESSION_SELECTED_EVENT, selectSession)
  }, [openSessionInDock])

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

  useEffect(
    () => () => {
      window.clearTimeout(layoutSaveTimerRef.current)
      for (const disposable of dockDisposablesRef.current) disposable.dispose()
      dockDisposablesRef.current = []
      dockApiRef.current = null
    },
    [],
  )

  useEffect(() => {
    if (activeId) localStorage.setItem(STORAGE_KEYS.activeSession, activeId)
  }, [activeId])

  const refreshSessions = async (preferredId?: string) => {
    const data = await chatApi.listSessions()
    sessionsRef.current = data.sessions
    setRemoteSessions(data.sessions)
    if (preferredId) setActiveId(preferredId)
    else
      setActiveId((current) =>
        data.sessions.some((session) => session.id === current)
          ? current
          : data.sessions[0]?.id || '',
      )
    announceSessionsUpdated()
    return data.sessions
  }

  const createSession = () => {
    if (creatingSessionRef.current) return creatingSessionRef.current
    const request = (async () => {
      try {
        setError('')
        const created = await chatApi.createSession(t('chat:chatPage.newChat'))
        setActiveId(created.id)
        setRemoteSessions((current) => {
          const next = mergeSessionLists(current, [created])
          sessionsRef.current = next
          return next
        })
        updateSessionState(created.id, {
          messages: [],
          tools: [],
          approvals: [],
          queuedInputs: [],
          permissionMode: created.permissionMode || 'auto',
          executionMode: created.executionMode || 'workspace',
          goal: created.goal || null,
          taskList: created.taskList || null,
          contextUsage: created.contextUsage || null,
          compaction: null,
          streaming: false,
          error: '',
          loaded: true,
          pageSize: FOCUS_MESSAGE_PAGE_SIZE,
          messageStart: 0,
          hasOlder: false,
          olderCursor: null,
          runStartedAt: null,
          lastActivityAt: null,
          runFinishedAt: null,
          runStopped: false,
          runNotice: '',
        })
        try {
          await refreshSessions(created.id)
        } catch (caught) {
          setError(
            t('chat:chatPage.theChatWasCreatedButTheListCouldNotBeRefreshedError', {
              error: errorMessage(caught),
            }),
          )
        }
        openSessionInDock(created.id)
        notify(t('chat:chatPage.newChatCreated'))
        return created.id
      } catch (caught) {
        setError(errorMessage(caught))
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
    Promise.all([chatApi.listSessions(), chatApi.getConfig(), chatApi.getSandboxStatus()])
      .then(async ([sessionData, configData, sandboxData]) => {
        setSandboxStatus(sandboxData)
        if (!active) return
        setModel(
          configData.model
            ? `${configData.provider}/${configData.model}`
            : t('chat:chatPage.noModelConfigured'),
        )
        setAvailableModels(
          configData.providers.flatMap((provider) =>
            provider.configured && provider.enabled
              ? provider.models
                  .filter((item) => item.kind === 'chat')
                  .map((item) => ({
                    key: `${provider.id}/${item.id}`,
                    provider: provider.id,
                    modelId: item.id,
                    label: item.name || item.id,
                    providerName: provider.name || provider.id,
                  }))
              : [],
          ),
        )
        let list = sessionData.sessions
        if (
          !list.length &&
          (creatingSessionRef.current || Object.keys(sessionStatesRef.current).length)
        ) {
          await creatingSessionRef.current
          if (!active) return
          list = (await chatApi.listSessions()).sessions
        }
        if (!list.length) {
          const created = await chatApi.createSession(t('chat:chatPage.newChat'))
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
            ...(session.streaming
              ? { streaming: true, recovering: true, loaded: false, error: '' }
              : {}),
          })
        }
        const storedId = localStorage.getItem(STORAGE_KEYS.activeSession)
        const knownIds = new Set([
          ...list.map((session) => session.id),
          ...Object.keys(sessionStatesRef.current),
        ])
        setActiveId((current) =>
          knownIds.has(current)
            ? current
            : storedId && knownIds.has(storedId)
              ? storedId
              : list[0]?.id || '',
        )
      })
      .catch((caught) => active && setError(errorMessage(caught)))
      .finally(() => active && setLoading(false))
    return () => {
      active = false
    }
  }, [t, updateSessionState])

  useEffect(() => {
    if (!dockReady || loading || dockInitializedRef.current || !dockApiRef.current) return
    const api = dockApiRef.current
    const validIds = new Set(remoteSessions.map((session) => session.id))
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
        validSessionIds: remoteSessions.map((session) => session.id),
      })
      let referenceGroup: DockviewGroupPanel | undefined
      for (const sessionId of initialIds) {
        const session = remoteSessions.find((item) => item.id === sessionId)
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
      const session = remoteSessions.find((item) => item.id === sessionIdFromPanel(panel))
      if (session) panel.api.setTitle(session.name || t('chat:chatPage.untitledChat'))
    }
    dockInitializedRef.current = true
    localStorage.removeItem(STORAGE_KEYS.chatMode)
    const request = pendingDockRequestRef.current || consumeSessionSelectionRequest()
    pendingDockRequestRef.current = null
    if (request) openSessionInDock(request.sessionId, request.disposition)
    else {
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
    loading,
    openSessionInDock,
    openWebPreviewInDock,
    remoteSessions,
    scheduleDockLayoutSave,
    t,
  ])

  useEffect(() => {
    if (!dockInitializedRef.current || !dockApiRef.current) return
    for (const session of remoteSessions)
      dockApiRef.current
        .getPanel(panelIdForSession(session.id))
        ?.api.setTitle(session.name || t('chat:chatPage.untitledChat'))
  }, [remoteSessions, t])

  useEffect(() => {
    let active = true
    const poll = () => {
      if (!active) return
      for (const [id, state] of Object.entries(sessionStatesRef.current)) {
        if (
          shouldPollLiveSession(state, {
            localStreamOwned: localStreamSessionsRef.current.has(id),
          })
        )
          void syncLiveSession(id)
      }
    }
    poll()
    const timer = window.setInterval(poll, 800)
    return () => {
      active = false
      window.clearInterval(timer)
    }
  }, [syncLiveSession])

  const sendPrompt = async (
    text: string,
    requestedSessionId: string,
    attachments: ChatAttachment[] = [],
    goalMode = false,
    goalTokenBudget: number | null = null,
  ) => {
    const prompt =
      text.trim() || (attachments.length ? t('chat:chatPage.pleaseAnalyzeTheseAttachments') : '')
    if (!prompt) return
    let sessionId = requestedSessionId
    if (!sessionId) sessionId = await createSession()
    if (!sessionId) return
    if (sessionStatesRef.current[sessionId]?.streaming) return
    setActiveId(sessionId)
    setError('')
    const userMessage = {
      id: `user-${Date.now()}`,
      role: 'user',
      text: prompt,
      attachments: attachments.map(({ id, kind, name, mimeType, size, data }) => ({
        id,
        kind,
        name,
        mimeType,
        size,
        data: kind === 'image' ? data : undefined,
      })),
    }
    const agentId = `agent-${Date.now()}`
    const runStartedAt = new Date().toISOString()
    let responseText = ''
    let thinkingText = ''
    let queuedDuringRun = false
    const thinkingScheduler = createStreamingTextScheduler(
      (text, activityAt) => {
        updateSessionState(sessionId, (current) => ({
          ...current,
          thinkingText: text,
          lastActivityAt: activityAt || current.lastActivityAt,
          currentActivity: {
            type: 'model',
            stage: 'thinking',
            updatedAt: activityAt || current.lastActivityAt,
          },
        }))
      },
      { intervalMs: 80 },
    )
    const typewriter = createTypewriterDisplay((text, activityAt) => {
      updateSessionState(sessionId, (current) => {
        const activity = {
          type: 'model',
          stage: 'responding',
          updatedAt: activityAt || current.lastActivityAt,
        }
        return {
          ...current,
          lastActivityAt: activityAt || current.lastActivityAt,
          runNotice: '',
          currentActivity: activity,
          activityFeed: pushCurrentActivity(current.activityFeed, activity),
          messages: current.messages.map((item) =>
            item.id === agentId ? { ...item, text } : item,
          ),
        }
      })
    })
    const toolScheduler = createToolUpdateScheduler((batch, activityAt) => {
      updateSessionState(sessionId, (current) => {
        let activityFeed = current.activityFeed || []
        for (const [id, patch] of batch) {
          const existing = activityFeed.find((item) => item.type === 'tool' && item.id === id)
          if (existing)
            activityFeed = pushCurrentActivity(activityFeed, {
              ...existing,
              ...patch,
              updatedAt: activityAt || existing.updatedAt,
            })
        }
        const currentActivity =
          current.currentActivity?.type === 'tool' && batch.has(current.currentActivity.id)
            ? {
                ...current.currentActivity,
                ...batch.get(current.currentActivity.id),
                updatedAt: activityAt || current.currentActivity.updatedAt,
              }
            : current.currentActivity
        return {
          ...current,
          lastActivityAt: activityAt || current.lastActivityAt,
          tools: current.tools.map((item) => {
            const patch = batch.get(String(item.id || ''))
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
        messages: [
          ...current.messages,
          userMessage,
          { id: agentId, role: 'agent', text: '', streaming: true },
        ],
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
      setRemoteSessions((current) =>
        current.map((session) => {
          if (session.id !== sessionId) return session
          if (session.goal?.status === 'active') return session
          return { ...session, taskList: null }
        }),
      )
    }
    localStreamSessionsRef.current.add(sessionId)
    try {
      await chatApi.openStream(
        { sessionId, message: prompt, attachments, goalMode, goalTokenBudget },
        (event, data) => {
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
            if (
              data.cwd ||
              data.permissionMode ||
              data.executionMode ||
              data.goal !== undefined ||
              data.taskList !== undefined
            ) {
              setRemoteSessions((current) =>
                current.map((session) =>
                  session.id === sessionId
                    ? {
                        ...session,
                        cwd: data.cwd || session.cwd,
                        permissionMode: data.permissionMode || session.permissionMode,
                        executionMode: data.executionMode || session.executionMode,
                        goal: data.goal ?? session.goal ?? null,
                        taskList: data.taskList !== undefined ? data.taskList : session.taskList,
                      }
                    : session,
                ),
              )
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
              const activity =
                data.currentActivity ||
                (data.agent
                  ? {
                      type: 'agent',
                      agent: data.agent,
                      updatedAt: data.agent.lastActivityAt || eventAt,
                    }
                  : null)
              return {
                ...current,
                agents: data.agents || [],
                currentActivity:
                  current.currentActivity?.type === 'tool'
                    ? current.currentActivity
                    : activity || current.currentActivity,
                activityFeed: activity
                  ? pushCurrentActivity(current.activityFeed, activity)
                  : current.activityFeed,
                lastActivityAt: data.agent?.lastActivityAt || eventAt,
              }
            })
          } else if (event === 'context_usage') {
            updateSessionState(sessionId, { contextUsage: data })
          } else if (event === 'compaction_start') {
            typewriter.flush()
            toolScheduler.flush()
            updateSessionState(sessionId, (current) => {
              const activity = {
                type: 'compaction',
                compaction: data,
                updatedAt: data.startedAt || eventAt,
              }
              return {
                ...current,
                compaction: data,
                currentActivity: activity,
                activityFeed: pushCurrentActivity(current.activityFeed, activity),
                lastActivityAt: data.startedAt || eventAt,
              }
            })
          } else if (event === 'compaction_end') {
            updateSessionState(sessionId, (current) => {
              const activity = {
                type: 'model',
                stage: 'processing_result',
                updatedAt: data.finishedAt || eventAt,
              }
              return {
                ...current,
                compaction: data,
                currentActivity: activity,
                activityFeed: pushCurrentActivity(current.activityFeed, activity),
                lastActivityAt: data.finishedAt || eventAt,
              }
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
              currentActivity: {
                type: 'model',
                stage: 'thinking',
                updatedAt: data.updatedAt || eventAt,
              },
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
              const activity = {
                type: 'tool',
                id: data.id,
                name: data.name,
                args: data.args,
                status: 'running',
                startedAt: data.startedAt || eventAt,
                updatedAt: eventAt,
                ...(data.output !== undefined ? { output: data.output } : {}),
              }
              return {
                ...current,
                lastActivityAt: eventAt,
                runNotice: '',
                tools: [...current.tools, activity],
                currentActivity: activity,
                activityFeed: pushCurrentActivity(current.activityFeed, activity),
                messages: current.messages.map((item) =>
                  item.id === agentId
                    ? {
                        ...item,
                        text: responseText || item.text,
                      }
                    : item,
                ),
              }
            })
          } else if (event === 'tool_update') {
            toolScheduler.push(
              data.id,
              {
                message: data.message || '',
                updatedAt: data.updatedAt || eventAt,
                ...(data.output !== undefined ? { output: data.output } : {}),
                ...(data.agent ? { agent: data.agent } : {}),
              },
              data.updatedAt || eventAt,
            )
          } else if (event === 'tool_end') {
            toolScheduler.flush()
            updateSessionState(sessionId, (current) => {
              const completedTool = current.tools.find((item) => item.id === data.id)
              const finishedAt = data.finishedAt || eventAt
              const preserveEvent =
                [
                  'update_task_list',
                  'spawn_agent',
                  'list_agents',
                  'send_message',
                  'followup_task',
                  'wait_agent',
                  'interrupt_agent',
                ].includes(String(completedTool?.name || '')) &&
                ['plan', 'agent'].includes(current.currentActivity?.type)
              const toolActivity = {
                ...(completedTool || {}),
                type: 'tool',
                status: data.error ? 'error' : 'done',
                message: data.message || completedTool?.message || '',
                updatedAt: finishedAt,
                finishedAt,
                ...(data.output !== undefined ? { output: data.output } : {}),
              }
              const agentActivity = data.agent
                ? {
                    type: 'agent',
                    agent: data.agent,
                    updatedAt: data.agent.lastActivityAt || finishedAt,
                  }
                : null
              let activityFeed = current.activityFeed || []
              if (data.error || (!preserveEvent && !agentActivity))
                activityFeed = pushCurrentActivity(activityFeed, toolActivity)
              if (agentActivity) activityFeed = pushCurrentActivity(activityFeed, agentActivity)
              return {
                ...current,
                lastActivityAt: finishedAt,
                tools: current.tools.map((item) =>
                  item.id === data.id
                    ? {
                        ...item,
                        status: data.error ? 'error' : 'done',
                        message: data.message || '',
                        updatedAt: finishedAt,
                        finishedAt,
                        ...(data.output !== undefined ? { output: data.output } : {}),
                      }
                    : item,
                ),
                currentActivity: data.error
                  ? toolActivity
                  : agentActivity || (preserveEvent ? current.currentActivity : toolActivity),
                activityFeed,
              }
            })
          } else if (event === 'permission_request') {
            typewriter.flush()
            toolScheduler.flush()
            updateSessionState(sessionId, (current) => ({
              ...current,
              lastActivityAt: eventAt,
              approvals: [...(current.approvals || []).filter((item) => item.id !== data.id), data],
            }))
          } else if (event === 'permission_resolved') {
            updateSessionState(sessionId, (current) => ({
              ...current,
              lastActivityAt: eventAt,
              approvals: (current.approvals || []).filter((item) => item.id !== data.id),
            }))
          } else if (event === 'generated_asset') {
            updateSessionState(sessionId, (current) => ({
              ...current,
              lastActivityAt: eventAt,
              messages: current.messages.map((item) =>
                item.id === agentId
                  ? {
                      ...item,
                      attachments: [
                        ...(item.attachments || []).filter(
                          (attachment) => attachment.id !== data.id,
                        ),
                        data,
                      ],
                    }
                  : item,
              ),
            }))
          } else if (event === 'goal_update') {
            updateSessionState(sessionId, { goal: data.goal ?? null })
            setRemoteSessions((current) =>
              current.map((session) =>
                session.id === sessionId ? { ...session, goal: data.goal ?? null } : session,
              ),
            )
          } else if (event === 'task_list_update') {
            typewriter.flush()
            toolScheduler.flush()
            updateSessionState(sessionId, (current) => {
              const nextTaskList = data.taskList !== undefined ? data.taskList : current.taskList
              const activity = data.currentActivity || {
                type: 'plan',
                taskList: nextTaskList,
                changes: taskListChanges(current.taskList, nextTaskList),
                updatedAt: nextTaskList?.updatedAt || eventAt,
              }
              return {
                ...current,
                lastActivityAt: eventAt,
                taskList: nextTaskList,
                currentActivity: activity,
                activityFeed: pushCurrentActivity(current.activityFeed, activity),
              }
            })
            setRemoteSessions((current) =>
              current.map((session) =>
                session.id === sessionId
                  ? {
                      ...session,
                      taskList: data.taskList !== undefined ? data.taskList : session.taskList,
                    }
                  : session,
              ),
            )
          } else if (event === 'session_title') {
            setRemoteSessions((current) =>
              current.map((session) =>
                session.id === sessionId ? { ...session, name: data.name } : session,
              ),
            )
          } else if (event === 'retry') {
            const retryNotice = t('chat:chatPage.retryingAttemptMaxAttemptsMessage', {
              attempt: data.attempt,
              maxAttempts: data.maxAttempts,
              message: data.message,
            })
            updateSessionState(sessionId, (current) => {
              const activity = { type: 'retry', message: retryNotice, updatedAt: eventAt }
              return {
                ...current,
                runNotice: retryNotice,
                currentActivity: activity,
                activityFeed: pushCurrentActivity(current.activityFeed, activity),
                lastActivityAt: eventAt,
              }
            })
          } else if (event === 'done') {
            queuedDuringRun ||= Boolean(
              sessionStatesRef.current[sessionId]?.hadQueuedInput ||
              sessionStatesRef.current[sessionId]?.queuedInputs?.length,
            )
            const finishedAt = data.finishedAt || eventAt
            if (typeof data.text === 'string') responseText = data.text
            typewriter.setTarget(responseText, finishedAt)
            typewriter.flush()
            thinkingScheduler.flush()
            toolScheduler.cancel()
            updateSessionState(sessionId, (current) => ({
              ...current,
              streaming: false,
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
              messages: current.messages.map((item) =>
                item.id === agentId
                  ? {
                      ...item,
                      text: typeof data.text === 'string' ? data.text : responseText || item.text,
                      streaming: false,
                      ...(data.assets?.length ? { attachments: data.assets } : {}),
                    }
                  : item,
              ),
            }))
            setRemoteSessions((current) =>
              current.map((session) =>
                session.id === sessionId
                  ? {
                      ...session,
                      streaming: false,
                      goal: data.goal ?? session.goal ?? null,
                      taskList: data.taskList !== undefined ? data.taskList : session.taskList,
                    }
                  : session,
              ),
            )
            return false
          } else if (event === 'error') {
            queuedDuringRun ||= Boolean(
              sessionStatesRef.current[sessionId]?.hadQueuedInput ||
              sessionStatesRef.current[sessionId]?.queuedInputs?.length,
            )
            const finishedAt = data.finishedAt || eventAt
            if (typeof data.text === 'string') responseText = data.text
            typewriter.setTarget(responseText, finishedAt)
            typewriter.flush()
            thinkingScheduler.flush()
            toolScheduler.cancel()
            updateSessionState(sessionId, (current) => ({
              ...current,
              streaming: false,
              runFinishedAt: finishedAt,
              lastActivityAt: finishedAt,
              currentActivity: null,
              activityFeed: [],
              agents: data.agents || current.agents || [],
              contextUsage: data.contextUsage ?? current.contextUsage ?? null,
              compaction: data.compaction ?? current.compaction ?? null,
              approvals: [],
              tools: settleToolCalls(data.tools || current.tools, {
                finishedAt,
                error: data.message,
              }),
              messages: current.messages.map((item) =>
                item.id === agentId
                  ? {
                      ...item,
                      text: typeof data.text === 'string' ? data.text : responseText || item.text,
                      streaming: false,
                    }
                  : item,
              ),
            }))
            throw new Error(data.message)
          }
        },
      )
      typewriter.setTarget(responseText)
      typewriter.flush()
      thinkingScheduler.flush()
      toolScheduler.flush()
      const fallbackFinishedAt = new Date().toISOString()
      const stillStreaming = Boolean(sessionStatesRef.current[sessionId]?.streaming)
      if (stillStreaming) {
        updateSessionState(sessionId, (current) => {
          const runFinishedAt = current.runFinishedAt || fallbackFinishedAt
          return {
            ...current,
            streaming: false,
            runFinishedAt,
            lastActivityAt: runFinishedAt,
            runNotice: '',
            currentActivity: null,
            activityFeed: [],
            approvals: [],
            tools: settleToolCalls(current.tools, { finishedAt: runFinishedAt }),
            messages: current.messages.map((item) =>
              item.id === agentId
                ? { ...item, streaming: false, text: responseText || item.text }
                : item,
            ),
          }
        })
      }
      // Steering and follow-up inputs can create multiple user/assistant turns inside one SSE run.
      // Reload the persisted transcript once the run settles so those turns render as separate bubbles.
      if (
        goalMode ||
        queuedDuringRun ||
        sessionStatesRef.current[sessionId]?.hadQueuedInput ||
        sessionStatesRef.current[sessionId]?.goal ||
        sessionStatesRef.current[sessionId]?.queuedInputs?.length
      ) {
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
      browserNotify?.('chat.completed', {
        chat: {
          title: completed?.name || t('chat:chatPage.appChat', { app: APP_NAME }),
          summary:
            responseText.trim().slice(0, 260) || t('chat:chatPage.theAgentHasFinishedResponding'),
          model: sessionStatesRef.current[sessionId]?.model || model,
        },
      })
    } catch (caught) {
      typewriter.cancel()
      thinkingScheduler.flush()
      toolScheduler.cancel()
      const runFinishedAt = new Date().toISOString()
      const caughtMessage = errorMessage(caught)
      updateSessionState(sessionId, (current) => ({
        ...current,
        streaming: false,
        error: caughtMessage,
        runFinishedAt,
        lastActivityAt: runFinishedAt,
        runNotice: '',
        currentActivity: null,
        activityFeed: [],
        approvals: [],
        tools: settleToolCalls(current.tools, { finishedAt: runFinishedAt, error: caughtMessage }),
        messages: current.messages.map((item) =>
          item.id === agentId
            ? {
                ...item,
                streaming: false,
                error: caughtMessage,
                text: item.text || responseText || caughtMessage,
              }
            : item,
        ),
      }))
      if (
        queuedDuringRun ||
        sessionStatesRef.current[sessionId]?.hadQueuedInput ||
        sessionStatesRef.current[sessionId]?.queuedInputs?.length
      ) {
        try {
          await loadSessionMessages(sessionId, { force: true })
        } catch {}
        updateSessionState(sessionId, { queuedInputs: [], hadQueuedInput: false })
      }
      setRemoteSessions((current) =>
        current.map((session) =>
          session.id === sessionId ? { ...session, streaming: false } : session,
        ),
      )
    } finally {
      localStreamSessionsRef.current.delete(sessionId)
      typewriter.cancel()
      thinkingScheduler.cancel()
      toolScheduler.cancel()
      updateSessionState(sessionId, { streaming: false, hadQueuedInput: false })
      window.dispatchEvent(new Event(USAGE_UPDATED_EVENT))
    }
  }

  const queuePrompt = async (text: string, sessionId: string, behavior = 'steer') => {
    const message = String(text || '').trim()
    if (!sessionId || !message) return false
    try {
      const result = await chatApi.queueInput(sessionId, message, behavior)
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
      notify(errorMessage(caught), 'error')
      return false
    }
  }

  const abort = async (sessionId: string) => {
    if (!sessionId) return
    const result = await chatApi.abort(sessionId)
    const runFinishedAt = new Date().toISOString()
    updateSessionState(sessionId, (current) => ({
      ...current,
      streaming: false,
      queuedInputs: [],
      hadQueuedInput: false,
      goal: result.goal ?? null,
      compaction: current.compaction?.active
        ? {
            ...current.compaction,
            active: false,
            status: 'aborted',
            aborted: true,
            finishedAt: runFinishedAt,
          }
        : current.compaction,
      runFinishedAt,
      lastActivityAt: runFinishedAt,
      runStopped: true,
      runNotice: '',
      approvals: [],
      tools: settleToolCalls(current.tools, {
        finishedAt: runFinishedAt,
        error: t('chat:chatPage.stopped'),
      }),
      messages: current.messages.map((item) =>
        item.streaming ? { ...item, streaming: false } : item,
      ),
    }))
    setRemoteSessions((current) =>
      current.map((session) =>
        session.id === sessionId
          ? { ...session, streaming: false, goal: result.goal ?? session.goal ?? null }
          : session,
      ),
    )
    notify(t('chat:chatPage.currentRunStopped'), 'info')
  }

  const pauseGoal = async (sessionId: string) => {
    if (!sessionId) return
    try {
      const result = await chatApi.pauseGoal(sessionId)
      updateSessionState(sessionId, { goal: result.goal || null })
      setRemoteSessions((current) =>
        current.map((session) =>
          session.id === sessionId ? { ...session, goal: result.goal || null } : session,
        ),
      )
      notify(t('chat:chatPage.goalPaused'), 'info')
    } catch (caught) {
      updateSessionState(sessionId, { error: errorMessage(caught) })
    }
  }

  const setGoalBudget = async (sessionId: string, tokenBudget: number) => {
    if (!sessionId) return
    try {
      const result = await chatApi.setGoalBudget(sessionId, tokenBudget)
      updateSessionState(sessionId, { goal: result.goal || null })
      setRemoteSessions((current) =>
        current.map((session) =>
          session.id === sessionId ? { ...session, goal: result.goal || null } : session,
        ),
      )
      notify(t('chat:chatPage.goalTokenBudgetUpdated'), 'info')
    } catch (caught) {
      updateSessionState(sessionId, { error: errorMessage(caught) })
    }
  }

  const switchSessionModel = async (sessionId: string, nextModel: string) => {
    const selected = availableModels.find((item) => item.key === nextModel)
    if (!sessionId || !selected || sessionStatesRef.current[sessionId]?.streaming) return
    updateSessionState(sessionId, { switchingModel: true, error: '' })
    try {
      const updated = await chatApi.updateModel(sessionId, selected.provider, selected.modelId)
      updateSessionState(sessionId, {
        model: updated.model,
        contextUsage: updated.contextUsage ?? null,
        switchingModel: false,
      })
      setRemoteSessions((current) =>
        current.map((session) =>
          session.id === sessionId ? { ...session, model: updated.model } : session,
        ),
      )
      notify(t('chat:chatPage.switchedToModel', { model: selected.label }))
    } catch (caught) {
      updateSessionState(sessionId, { switchingModel: false, error: errorMessage(caught) })
    }
  }

  const refreshSandboxStatus = useCallback(async () => {
    const next = await chatApi.getSandboxStatus()
    setSandboxStatus(next)
    return next
  }, [])

  const ensureWorkspaceSandbox = async () => {
    const status = await refreshSandboxStatus()
    if (status.state === 'ready' || status.state === 'active') return true
    if (status.platform !== 'win32' || status.state !== 'not-installed') {
      notify(
        t('chat:chatPage.theWorkspaceSandboxCannotBeEnabledOnThisDeviceReason', {
          reason:
            status.message ||
            status.errors?.join('、') ||
            t('chat:chatPage.unsupportedEnvironment'),
        }),
        'error',
      )
      return false
    }
    const confirmed = await requestConfirm({
      title: t('chat:chatPage.enableSecureExecution'),
      message: t(
        'chat:chatPage.vesperWillCreateALowPrivilegeSandboxAccountAndConfigureNetworkIsolationWindowsWillShowOneAdminis',
      ),
      confirmLabel: t('chat:chatPage.continueSetup'),
      tone: 'primary',
    })
    if (!confirmed) return false
    setSandboxStatus((current) => ({ ...current, state: 'installing' }))
    const installed = await chatApi.installSandbox()
    setSandboxStatus(installed)
    if (installed.cancelled) {
      notify(t('chat:chatPage.sandboxSetupCancelled'), 'info')
      return false
    }
    if (installed.state !== 'ready' && installed.state !== 'active') {
      notify(
        t('chat:chatPage.localSandboxSetupFailedReason', {
          reason: installed.message || t('chat:chatPage.checkTheSystemSettingsAndTryAgain'),
        }),
        'error',
      )
      return false
    }
    notify(t('chat:chatPage.localSandboxEnabled'))
    return true
  }

  const switchSessionExecutionMode = async (sessionId: string, executionMode: string) => {
    if (!sessionId) return false
    updateSessionState(sessionId, { switchingPermission: true, error: '' })
    try {
      if (executionMode === 'workspace' && !(await ensureWorkspaceSandbox())) {
        updateSessionState(sessionId, { switchingPermission: false })
        return false
      }
      if (executionMode === 'full-access') {
        const confirmed = await requestConfirm({
          title: t('chat:chatPage.enableFullAccess'),
          message: t(
            'chat:chatPage.fullAccessAllowsTheAgentToUseFilesAndNetworkServicesOutsideTheWorkspaceAndShellCommandsWillNoLon',
          ),
          confirmLabel: t('chat:chatPage.enableFullAccess'),
        })
        if (!confirmed) {
          updateSessionState(sessionId, { switchingPermission: false })
          return false
        }
      }
      const updated = await chatApi.updateExecutionMode(sessionId, executionMode)
      updateSessionState(sessionId, {
        executionMode: updated.executionMode,
        permissionMode: updated.permissionMode,
        switchingPermission: false,
      })
      setRemoteSessions((current) =>
        current.map((session) =>
          session.id === sessionId
            ? {
                ...session,
                executionMode: updated.executionMode,
                permissionMode: updated.permissionMode,
              }
            : session,
        ),
      )
      notify(
        t('chat:chatPage.executionModeChangedToMode', {
          mode:
            updated.executionMode === 'read-only'
              ? t('chat:chatPage.readOnly')
              : updated.executionMode === 'full-access'
                ? t('chat:chatPage.fullAccess')
                : t('chat:chatPage.workspace'),
        }),
      )
      return true
    } catch (caught) {
      updateSessionState(sessionId, { switchingPermission: false, error: errorMessage(caught) })
      return false
    }
  }

  const resolveToolApproval = async (sessionId: string, approvalId: string, approved: boolean) => {
    updateSessionState(sessionId, (current) => ({
      ...current,
      approvals: (current.approvals || []).filter((item) => item.id !== approvalId),
      error: '',
    }))
    try {
      const resolution = await chatApi.resolveApproval(sessionId, approvalId, approved)
      if (resolution.alreadyResolved) void syncLiveSession(sessionId)
    } catch (caught) {
      await syncLiveSession(sessionId)
      if (caught instanceof ApiError && caught.status === 404) {
        notify(t('chat:chatPage.approvalStatusUpdated'), 'info')
        return
      }
      updateSessionState(sessionId, { error: errorMessage(caught) })
      throw caught
    }
  }

  const switchSessionCwd = async (session: SessionSummary, cwd: string) => {
    if (!session?.id || sessionStatesRef.current[session.id]?.streaming) return
    updateSessionState(session.id, { switchingCwd: true, error: '' })
    try {
      const updated = await chatApi.updateCwd(session.id, cwd)
      updateSessionState(session.id, { cwd: updated.cwd, switchingCwd: false })
      setRemoteSessions((current) =>
        current.map((item) => (item.id === session.id ? { ...item, cwd: updated.cwd } : item)),
      )
      setWorkspaceSession(null)
      notify(
        t('chat:chatPage.workingDirectoryChangedToWorkspace', {
          workspace: workspaceName(updated.cwd),
        }),
      )
    } catch (caught) {
      updateSessionState(session.id, { switchingCwd: false, error: errorMessage(caught) })
      throw caught
    }
  }

  const renameSession = async (session: SessionSummary) => {
    const name = await requestText({
      title: t('chat:chatPage.renameChat'),
      inputLabel: t('chat:chatPage.chatTitle'),
      value: session.name,
      confirmLabel: t('chat:chatPage.save'),
    })
    if (name === null || name === session.name) return
    try {
      const updated = await chatApi.renameSession(session.id, name)
      setRemoteSessions((current) =>
        current.map((item) => (item.id === session.id ? { ...item, name: updated.name } : item)),
      )
      announceSessionsUpdated()
      notify(t('chat:chatPage.chatTitleUpdated'))
    } catch (caught) {
      setError(errorMessage(caught))
    }
  }

  const dockComponents = useMemo(
    () => ({ session: SessionDockPanel, webPreview: WebPreviewDockPanel }),
    [],
  )
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
  const activeSession = remoteSessions.find((session) => session.id === activeId)
  const activeState = sessionStates[activeId] || {
    messages: [],
    tools: [],
    approvals: [],
    taskList: null,
    streaming: false,
    error: '',
    loading: false,
    switchingModel: false,
    switchingCwd: false,
    switchingPermission: false,
    messageStart: null,
    hasOlder: false,
    olderCursor: null,
  }
  const announcedModel = activeState.model || activeSession?.model || ''

  useEffect(() => {
    announceActiveSession(activeId, announcedModel)
  }, [activeId, announcedModel])

  useEffect(() => {
    document.title = activeSession?.name ? `${activeSession.name} · ${APP_NAME}` : APP_NAME
    return () => {
      document.title = APP_NAME
    }
  }, [activeSession?.name])

  const dockContextValue: ChatDockContextValue = {
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
    setGoalBudget,
    switchSessionModel,
    switchSessionExecutionMode,
    sandboxStatus,
    resolveToolApproval,
    setWorkspaceSession,
    renameSession,
    splitDockPanel,
    closeDockPanel,
  }

  return (
    <>
      <div className="chat-layout dock-layout">
        {loading ? (
          <Panel className="empty-state">
            <RefreshCw className="spin" size={24} />
            <h2>{t('chat:chatPage.wakingTheAgent')}</h2>
            <p>{t('chat:chatPage.modelsSessionsAndContextAreSettlingIntoPlace')}</p>
          </Panel>
        ) : (
          <>
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
          </>
        )}
      </div>
      {workspaceSession && (
        <WorkspacePicker
          session={workspaceSession}
          onClose={() => setWorkspaceSession(null)}
          onSelect={(cwd) => switchSessionCwd(workspaceSession, cwd)}
        />
      )}
    </>
  )
}
