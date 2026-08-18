// 会话目录 hook：拉取/刷新会话列表，维护会话状态缓存（含实时流），
// 处理会话树的展开与定位，并负责新会话的创建（含工作区分组）。
import { useCallback, useEffect, useRef, useState } from 'react'
import { APP_NAME } from '@/app/brand'
import { STORAGE_KEYS } from '@/app/storage'
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
import { chatApi } from './chat-api'
import { chatErrorMessage } from './chat-errors'
import { announceActiveSession, announceSessionsUpdated } from './events'
import { mergeSessionLists, recentSessionCwd } from './session-list'

export const FOCUS_MESSAGE_PAGE_SIZE = 40

type SessionsUpdate = SessionSummary[] | ((current: SessionSummary[]) => SessionSummary[])

type SessionCatalogOptions = {
  notify: Notify
}

export function useSessionCatalog({ notify }: SessionCatalogOptions) {
  const { t } = useI18n()
  const [sessions, setSessions] = useState<SessionSummary[]>([])
  const [activeId, setActiveId] = useState(
    () => localStorage.getItem(STORAGE_KEYS.activeSession) || '',
  )
  const [sessionStates, setSessionStates] = useState<Record<string, SessionState>>({})
  const [loading, setLoading] = useState(true)
  const [globalError, setGlobalError] = useState('')
  const [defaultModel, setDefaultModel] = useState(() => t('chat:chatPage.waitingForConfiguration'))
  const [availableModels, setAvailableModels] = useState<ModelOption[]>([])
  const sessionsRef = useRef(sessions)
  const sessionStatesRef = useRef(sessionStates)
  const creatingSessionRef = useRef<Promise<string> | null>(null)

  // 更新会话列表（函数式或替换），同步 ref 与 state。
  const updateSessions = useCallback((update: SessionsUpdate) => {
    const current = sessionsRef.current
    const next = typeof update === 'function' ? update(current) : update
    sessionsRef.current = next
    setSessions(next)
    return next
  }, [])

  // 整体替换会话状态表（供批量恢复/清空）。
  const replaceSessionStates = useCallback((states: Record<string, SessionState>) => {
    sessionStatesRef.current = states
    setSessionStates(states)
  }, [])

  // 更新单个会话状态：经 applySessionUpdate 去重，无变化不触发渲染。
  const updateSessionState = useCallback(
    (id: string, update: SessionStateUpdate) => {
      if (!id) return
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
      if (!id || panelOpen || localStreamOwned) return false
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

  // 刷新会话列表：拉取后合并（保留本地乐观项），更新活动 id
  // 并广播列表更新事件；preferredId 优先。
  const refreshSessions = useCallback(
    async (preferredId?: string) => {
      const data = await chatApi.listSessions()
      updateSessions(data.sessions)
      if (preferredId) setActiveId(preferredId)
      else
        setActiveId((current) =>
          data.sessions.some((session) => session.id === current)
            ? current
            : data.sessions[0]?.id || '',
        )
      announceSessionsUpdated()
      return data.sessions
    },
    [updateSessions],
  )

  // 创建会话记录：cwd 缺省时继承最近会话的工作目录；
  // 用 ref 去重并发创建，成功后初始化会话状态并合并进列表。
  const createSessionRecord = useCallback(
    (cwd = '') => {
      if (creatingSessionRef.current) return creatingSessionRef.current
      const request = (async () => {
        try {
          setGlobalError('')
          const created = await chatApi.createSession(
            t('chat:chatPage.newChat'),
            cwd || recentSessionCwd(sessionsRef.current),
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
    Promise.all([chatApi.listSessions(), chatApi.getConfig()])
      .then(async ([sessionData, configData]) => {
        if (!active) return
        setDefaultModel(
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
        updateSessions((current) => mergeSessionLists(current, list))
        announceSessionsUpdated()
        for (const session of list) {
          updateSessionState(session.id, {
            agents: session.agents || [],
            plan: planFromPayloadOr(session, null),
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
      .catch((error) => active && setGlobalError(chatErrorMessage(error)))
      .finally(() => active && setLoading(false))
    return () => {
      active = false
    }
  }, [t, updateSessionState, updateSessions])

  useEffect(() => {
    if (activeId) localStorage.setItem(STORAGE_KEYS.activeSession, activeId)
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
    refreshSessions,
    createSessionRecord,
  }
}
