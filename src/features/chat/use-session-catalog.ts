// 会话目录 hook：拉取/刷新会话列表，维护会话状态缓存（含实时流），
// 处理会话树的展开与定位，并负责新会话的创建（含工作区分组）。
// 会话列表不再由本 hook 持有：sessions 是权威缓存 ['sessions','board'] 的镜像（看板同步负责轮询），
// 本地写入经 setQueryData 回到同一份缓存；sessionStates（转录与流状态）仍按会话由聊天视图持有。
import { hashKey, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import { APP_NAME } from '@/app/brand'
import { STORAGE_KEYS } from '@/app/storage'
import { pageStateStorage } from '@/lib/page-state-storage'
import { useI18n } from '@/app/use-i18n'
import {
  applySessionUpdate,
  DEFAULT_SESSION_STATE,
  shouldRetainClosedSessionState,
} from '@/lib/session-state'
import type { SessionStateUpdate } from '@/lib/session-state'
import { planFromPayloadOr } from '@/lib/plan-protocol'
import type { Notify } from '@/app/route-context'
import type { ModelOption, SessionState, SessionSummary } from '@/types/chat'
import { chatApi, SESSIONS_QUERY_KEY } from './chat-api'
import { chatErrorMessage } from './chat-errors'
import {
  announceActiveSession,
  announceSessionsUpdated,
  subscribeSessionDeletionUpdates,
  subscribeSessionOrganizationUpdates,
  subscribeSessionTitleUpdates,
} from './events'
import {
  applySessionOrganizationUpdate,
  applySessionTitleUpdate,
  createSessionTitleReconciler,
  mergeSessionLists,
  sessionCwdForCreate,
} from './session-list'

export const FOCUS_MESSAGE_PAGE_SIZE = 40
// 前台恢复时本机 Runtime 可能短暂返回空目录；之后这段时间内镜像不接受空快照。
const RECOVERY_EMPTY_GRACE_MS = 10_000

type SessionsUpdate = SessionSummary[] | ((current: SessionSummary[]) => SessionSummary[])
type SessionCatalogData = { sessions: SessionSummary[] }
type ChatConfigData = Awaited<ReturnType<typeof chatApi.getConfig>>

// 可用模型与默认模型的口径：只列已配置且启用的服务商里的对话模型。
function configModels(
  configData: ChatConfigData,
  t: ReturnType<typeof useI18n>['t'],
): { defaultModel: string; availableModels: ModelOption[] } {
  return {
    defaultModel: configData.model
      ? `${configData.provider}/${configData.model}`
      : t('chat:chatPage.noModelConfigured'),
    availableModels: configData.providers.flatMap((provider) =>
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
  }
}

// 会话页离开再进入（看板 ↔ 会话）时，目录与配置都已在共享缓存里：直接用缓存起步，
// 不再显示“正在唤醒”、也不重新拉整个目录。目录由看板 store 持续保鲜（事件流或轮询）。
function cachedCatalog(queryClient: ReturnType<typeof useQueryClient>) {
  const sessions = queryClient.getQueryData<SessionCatalogData>(SESSIONS_QUERY_KEY)?.sessions
  const config = queryClient.getQueryData<ChatConfigData>(['config'])
  return Array.isArray(sessions) && sessions.length && config ? { sessions, config } : null
}

type SessionCatalogOptions = {
  notify: Notify
}

type CreateSessionOptions = {
  inheritRecentCwd?: boolean
}

export function useSessionCatalog({ notify }: SessionCatalogOptions) {
  const { t } = useI18n()
  const queryClient = useQueryClient()
  const [seed] = useState(() => cachedCatalog(queryClient))
  const [sessions, setSessions] = useState<SessionSummary[]>(() => seed?.sessions ?? [])
  const [activeId, setActiveId] = useState(
    () => pageStateStorage.getItem(STORAGE_KEYS.activeSession) || '',
  )
  const [sessionStates, setSessionStates] = useState<Record<string, SessionState>>({})
  const [loading, setLoading] = useState(!seed)
  const [globalError, setGlobalError] = useState('')
  const [defaultModel, setDefaultModel] = useState(() =>
    seed ? configModels(seed.config, t).defaultModel : t('chat:chatPage.waitingForConfiguration'),
  )
  const [availableModels, setAvailableModels] = useState<ModelOption[]>(() =>
    seed ? configModels(seed.config, t).availableModels : [],
  )
  const sessionsRef = useRef(sessions)
  const sessionStatesRef = useRef(sessionStates)
  // 面板级订阅：每个 Dock 面板只关心自己会话的 state，
  // 流式帧只通知引用发生变化的会话，避免扇出到所有面板。
  const sessionStateListenersRef = useRef(new Map<string, Set<() => void>>())
  const creatingSessionRef = useRef<Promise<string> | null>(null)
  const retainedSessionStatesRef = useRef(new Map<string, number>())
  const discardedSessionStatesRef = useRef(new Set<string>())
  // 恢复期刷新时暂停“缓存 → 镜像”，先用刷新前的镜像判断服务端是否只是暂时返回空目录。
  const mirrorHoldsRef = useRef(0)
  const preserveNonEmptyUntilRef = useRef(0)
  const [titleReconciler] = useState(createSessionTitleReconciler)

  // 更新会话列表（函数式或替换）：先写回权威缓存让侧栏、看板看到同一份目录，再同步镜像。
  // 缓存还没有快照时不凭空创建，避免把局部列表当成新鲜的完整目录交给其他读者；
  // 结构共享在内容未变时保留旧引用，镜像据此跳过无谓的重渲染。
  const updateSessions = useCallback(
    (update: SessionsUpdate) => {
      const current = sessionsRef.current
      const next = typeof update === 'function' ? update(current) : update
      if (next === current) return next
      const stored = queryClient.setQueryData<SessionCatalogData>(SESSIONS_QUERY_KEY, (data) =>
        data ? { ...data, sessions: next } : undefined,
      )
      const mirrored = stored?.sessions ?? next
      if (mirrored !== sessionsRef.current) {
        sessionsRef.current = mirrored
        setSessions(mirrored)
      }
      return mirrored
    },
    [queryClient],
  )

  // 缓存 → 镜像：订阅同步触发，保证 sessionsRef 与缓存一致后再被函数式更新读取。
  const syncSessionsMirror = useCallback(() => {
    if (mirrorHoldsRef.current) return
    const cached = queryClient.getQueryData<SessionCatalogData>(SESSIONS_QUERY_KEY)?.sessions
    if (!Array.isArray(cached) || cached === sessionsRef.current) return
    if (cached.length) preserveNonEmptyUntilRef.current = 0
    else if (sessionsRef.current.length && Date.now() < preserveNonEmptyUntilRef.current) return
    sessionsRef.current = cached
    setSessions(cached)
  }, [queryClient])

  useEffect(() => {
    syncSessionsMirror()
    const sessionsHash = hashKey(SESSIONS_QUERY_KEY)
    return queryClient.getQueryCache().subscribe((event) => {
      if (event.type === 'updated' && event.query.queryHash === sessionsHash) syncSessionsMirror()
    })
  }, [queryClient, syncSessionsMirror])

  useEffect(() => {
    return subscribeSessionTitleUpdates(window, (titleUpdate) => {
      titleReconciler.record(titleUpdate)
      updateSessions((current) => applySessionTitleUpdate(current, titleUpdate))
    })
  }, [titleReconciler, updateSessions])

  useEffect(
    () =>
      subscribeSessionOrganizationUpdates(window, (update) => {
        updateSessions((current) => applySessionOrganizationUpdate(current, update))
      }),
    [updateSessions],
  )

  // 整体替换会话状态表（供批量恢复/清空）。
  const replaceSessionStates = useCallback(
    (incoming: Record<string, SessionState>) => {
      const states = discardedSessionStatesRef.current.size
        ? Object.fromEntries(
            Object.entries(incoming).filter(([id]) => !discardedSessionStatesRef.current.has(id)),
          )
        : incoming
      const previous = sessionStatesRef.current
      sessionStatesRef.current = states
      setSessionStates(states)
      const changedActivity = Object.keys(states).filter(
        (id) => states[id].streaming !== previous[id]?.streaming,
      )
      if (changedActivity.length) {
        const changed = new Set(changedActivity)
        queryClient.setQueryData<SessionCatalogData>(SESSIONS_QUERY_KEY, (catalog) =>
          catalog
            ? {
                ...catalog,
                sessions: catalog.sessions.map((session) =>
                  changed.has(session.id)
                    ? { ...session, streaming: states[session.id].streaming }
                    : session,
                ),
              }
            : catalog,
        )
      }
      // 只通知 state 引用真正变化的会话订阅者。
      const keys = new Set([...Object.keys(previous), ...Object.keys(states)])
      for (const key of keys) {
        if (previous[key] === states[key]) continue
        const listeners = sessionStateListenersRef.current.get(key)
        if (listeners) for (const listener of [...listeners]) listener()
      }
    },
    [queryClient],
  )

  useEffect(() => {
    return subscribeSessionDeletionUpdates(window, ({ deletedIds }) => {
      titleReconciler.remove(deletedIds)
      const deleted = new Set(deletedIds)
      const remaining = updateSessions((current) =>
        current.filter((session) => !deleted.has(session.id)),
      )
      setActiveId((current) => (deleted.has(current) ? remaining[0]?.id || '' : current))
      const states = sessionStatesRef.current
      if (deletedIds.some((id) => id in states)) {
        replaceSessionStates(
          Object.fromEntries(Object.entries(states).filter(([id]) => !deleted.has(id))),
        )
      }
    })
  }, [replaceSessionStates, titleReconciler, updateSessions])

  // 订阅单个会话状态（配 useSyncExternalStore）：返回退订函数。
  const subscribeSessionState = useCallback((id: string, listener: () => void) => {
    const listeners = sessionStateListenersRef.current.get(id) || new Set()
    listeners.add(listener)
    sessionStateListenersRef.current.set(id, listeners)
    return () => {
      listeners.delete(listener)
      if (!listeners.size) sessionStateListenersRef.current.delete(id)
    }
  }, [])

  // 读取单个会话状态快照（引用稳定：applySessionUpdate 无变化时保留原引用）。
  const getSessionState = useCallback(
    (id: string): SessionState => sessionStatesRef.current[id] || DEFAULT_SESSION_STATE,
    [],
  )

  // 更新单个会话状态：经 applySessionUpdate 去重，无变化不触发渲染。
  const updateSessionState = useCallback(
    (id: string, update: SessionStateUpdate) => {
      if (!id || discardedSessionStatesRef.current.has(id)) return
      const current = sessionStatesRef.current
      const previous = current[id] || DEFAULT_SESSION_STATE
      const next = applySessionUpdate(previous, update)
      if (next === previous) return
      replaceSessionStates({ ...current, [id]: next })
    },
    [replaceSessionStates],
  )

  // 释放会话状态：面板已关且未持有本地流、且无运行中活动时才真正丢弃，
  // 否则保留状态供重开无缝续显（返回是否释放）。
  const releaseSessionState = useCallback(
    (id: string, { panelOpen = false, localStreamOwned = false } = {}) => {
      if (!id || panelOpen || localStreamOwned || retainedSessionStatesRef.current.has(id))
        return false
      const current = sessionStatesRef.current
      const state = current[id]
      if (!state || shouldRetainClosedSessionState(state)) return false
      const states = { ...current }
      delete states[id]
      replaceSessionStates(states)
      return true
    },
    [replaceSessionStates],
  )

  // 仅用于服务端已确认不存在的临时 ID。区别于关闭面板：旧流、在途快照
  // 均不得恢复已清理正文；新建临时会话使用新的 ID，不会复用此墓碑。
  const discardSessionState = useCallback(
    (id: string) => {
      if (!id) return
      discardedSessionStatesRef.current.add(id)
      const states = { ...sessionStatesRef.current }
      delete states[id]
      replaceSessionStates(states)
    },
    [replaceSessionStates],
  )

  // 非 Dock 展示面板持有窄租约，避免目录回收仍被临时侧聊使用的正文。
  const retainSessionState = useCallback(
    (id: string) => {
      if (discardedSessionStatesRef.current.has(id)) return () => {}
      const owners = retainedSessionStatesRef.current
      owners.set(id, (owners.get(id) ?? 0) + 1)
      let released = false
      return () => {
        if (released) return
        released = true
        const count = owners.get(id) ?? 0
        if (count > 1) owners.set(id, count - 1)
        else {
          owners.delete(id)
          releaseSessionState(id)
        }
      }
    },
    [releaseSessionState],
  )

  // 刷新会话列表：拉取后更新活动 id，并广播列表更新事件；preferredId 优先。
  // 恢复阶段若服务端暂时返回空列表，保留已有目录，避免活动页签短暂失去会话摘要。
  const refreshSessions = useCallback(
    async (preferredId?: string, { preserveExistingOnEmpty = false } = {}) => {
      const titleRevision = titleReconciler.getRevision()
      // 请求落地时缓存会同步收到服务端原始快照；恢复期先冻结镜像，空目录不能抢先覆盖已有会话。
      if (preserveExistingOnEmpty) mirrorHoldsRef.current += 1
      let nextSessions: SessionSummary[]
      try {
        const data = await chatApi.listSessions()
        const preserve = Boolean(
          preserveExistingOnEmpty && !data.sessions.length && sessionsRef.current.length,
        )
        // 刷新结束后的失效重读和轮询可能仍拿到空目录，短暂窗口内继续保留已有会话。
        if (preserve) preserveNonEmptyUntilRef.current = Date.now() + RECOVERY_EMPTY_GRACE_MS
        nextSessions = preserve
          ? sessionsRef.current
          : titleReconciler.reconcile(data.sessions, titleRevision)
      } finally {
        if (preserveExistingOnEmpty) {
          mirrorHoldsRef.current -= 1
          // 冻结期间被忽略的其他更新在这里补齐；成功路径随后还会写回最终目录。
          syncSessionsMirror()
        }
      }
      // 保留旧目录时镜像未变，不写缓存：其他读者照常显示服务端快照，只有会话页保留页签。
      updateSessions(nextSessions)
      if (preferredId) setActiveId(preferredId)
      else
        setActiveId((current) =>
          nextSessions.some((session) => session.id === current)
            ? current
            : nextSessions[0]?.id || '',
        )
      announceSessionsUpdated()
      return nextSessions
    },
    [syncSessionsMirror, titleReconciler, updateSessions],
  )

  // 创建会话记录：按调用方决定是否继承最近会话的工作目录；
  // 用 ref 去重并发创建，成功后初始化会话状态并合并进列表。
  const createSessionRecord = useCallback(
    (cwd = '', { inheritRecentCwd = true }: CreateSessionOptions = {}) => {
      if (creatingSessionRef.current) return creatingSessionRef.current
      const request = (async () => {
        try {
          setGlobalError('')
          const created = await chatApi.createSession(
            t('chat:chatPage.newChat'),
            sessionCwdForCreate(cwd, sessionsRef.current, inheritRecentCwd),
          )
          setActiveId(created.id)
          updateSessions((current) => mergeSessionLists(current, [created]))
          updateSessionState(created.id, {
            messages: [],
            tools: [],
            approvals: [],
            queuedInputs: [],
            permissionMode: created.permissionMode || 'ask',
            executionMode: created.executionMode || 'approval-required',
            runMode: created.runMode || 'plan',
            goal: created.goal || null,
            plan: planFromPayloadOr(created, null),
            contextUsage: created.contextUsage || null,
            sessionUsage: created.sessionUsage || null,
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
          } catch (error) {
            setGlobalError(
              t('chat:chatPage.theChatWasCreatedButTheListCouldNotBeRefreshedError', {
                error: chatErrorMessage(error),
              }),
            )
          }
          notify(t('chat:chatPage.newChatCreated'))
          return created.id
        } catch (error) {
          setGlobalError(chatErrorMessage(error))
          return ''
        }
      })()
      creatingSessionRef.current = request
      void request.finally(() => {
        if (creatingSessionRef.current === request) creatingSessionRef.current = null
      })
      return request
    },
    [notify, refreshSessions, t, updateSessionState, updateSessions],
  )

  useEffect(() => {
    let active = true
    let titleRevision = titleReconciler.getRevision()
    // 懒加载聊天页可能晚于壳层请求完成，首次读取也须复用尚未失效的共享快照。
    const initial: Promise<[SessionCatalogData, ChatConfigData]> = seed
      ? Promise.resolve([{ sessions: seed.sessions }, seed.config])
      : Promise.all([chatApi.listSessions({ refresh: false }), chatApi.getConfig()])
    initial
      .then(async ([sessionData, configData]) => {
        if (!active) return
        const models = configModels(configData, t)
        setDefaultModel(models.defaultModel)
        setAvailableModels(models.availableModels)
        let list = sessionData.sessions
        if (
          !list.length &&
          (creatingSessionRef.current || Object.keys(sessionStatesRef.current).length)
        ) {
          await creatingSessionRef.current
          if (!active) return
          titleRevision = titleReconciler.getRevision()
          list = (await chatApi.listSessions()).sessions
        }
        if (!list.length) {
          const created = await chatApi.createSession(t('chat:chatPage.newChat'))
          list = [created]
        }
        if (!active) return
        list = titleReconciler.reconcile(list, titleRevision)
        updateSessions((current) => mergeSessionLists(current, list))
        for (const session of list) {
          updateSessionState(session.id, {
            team: session.team || null,
            agents: session.agents || [],
            plan: planFromPayloadOr(session, null),
            ...(session.streaming
              ? { streaming: true, recovering: true, loaded: false, error: '' }
              : {}),
          })
        }
        const storedId = pageStateStorage.getItem(STORAGE_KEYS.activeSession)
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
        // 远程回落重载后的首次成功加载：告知用户为何回到本机模式。
        if (typeof window !== 'undefined' && window.__PISPER_MOBILE_APP__) {
          try {
            const { consumeRemoteFallbackNotice } = await import('@/lib/mobile-remote-fallback')
            if (consumeRemoteFallbackNotice())
              notify(t('chat:chatPage.remoteUnavailableSwitchedToLocal'))
          } catch {
            // 提示模块加载失败不应把已经成功的会话目录变成失败状态。
          }
        }
      })
      .catch(async (error) => {
        if (!active) return
        // 远程桌面链路超时/不可达时自动回落本机模式，而不是停在「正在唤醒 Agent」。
        if (typeof window !== 'undefined' && window.__PISPER_MOBILE_APP__) {
          try {
            const { fallbackRemoteToLocalAfterFailure } =
              await import('@/lib/mobile-remote-fallback')
            if (await fallbackRemoteToLocalAfterFailure(error)) return
          } catch {
            // 回落模块加载失败时继续展示原始错误，避免初始化 Promise 变成未处理拒绝。
          }
        }
        if (!active) return
        setGlobalError(chatErrorMessage(error))
      })
      .finally(() => active && setLoading(false))
    return () => {
      active = false
    }
  }, [notify, seed, t, titleReconciler, updateSessionState, updateSessions])

  useEffect(() => {
    if (activeId) pageStateStorage.setItem(STORAGE_KEYS.activeSession, activeId)
    else pageStateStorage.removeItem(STORAGE_KEYS.activeSession)
  }, [activeId])

  const activeSession = sessions.find((session) => session.id === activeId)
  const activeState = sessionStates[activeId] || DEFAULT_SESSION_STATE
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

  return {
    sessions,
    sessionsRef,
    activeId,
    setActiveId,
    sessionStates,
    sessionStatesRef,
    loading,
    globalError,
    setGlobalError,
    defaultModel,
    availableModels,
    updateSessions,
    replaceSessionStates,
    updateSessionState,
    releaseSessionState,
    retainSessionState,
    discardSessionState,
    subscribeSessionState,
    getSessionState,
    refreshSessions,
    createSessionRecord,
  }
}
