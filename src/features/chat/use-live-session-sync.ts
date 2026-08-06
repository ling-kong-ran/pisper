import { useCallback, useEffect, useRef } from 'react'
import { planFromPayloadOr } from '@/lib/plan-protocol'
import { resolveQueuedInputs } from '@/lib/session-state'
import type { SessionStateUpdate } from '@/lib/session-state'
import type { EntityRecord, SessionState, SessionSummary } from '@/types/chat'
import { chatApi, type ApiRecord } from './chat-api'
import { chatErrorMessage } from './chat-errors'
import { shouldPollLiveSession } from './live-session-sync'
import { settleToolCalls } from './run-activity'
import { FOCUS_MESSAGE_PAGE_SIZE } from './use-session-catalog'

type SessionSyncOptions = {
  sessionStates: Record<string, SessionState>
  sessionStatesRef: React.MutableRefObject<Record<string, SessionState>>
  localStreamSessionsRef: React.MutableRefObject<Set<string>>
  updateSessionState: (id: string, update: SessionStateUpdate) => void
  updateSessions: (
    update: SessionSummary[] | ((current: SessionSummary[]) => SessionSummary[]),
  ) => SessionSummary[]
}

export function reconcileMessagePage(current: SessionState, data: ApiRecord) {
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

export function reconcileLiveSnapshot(
  current: SessionState,
  data: ApiRecord,
  fallbackFinishedAt = new Date().toISOString(),
): SessionState {
  const finishedAt = data.finishedAt || current.runFinishedAt || fallbackFinishedAt
  return {
    ...current,
    ...reconcileMessagePage(current, data),
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
    plan: planFromPayloadOr(data, current.plan ?? null),
    contextUsage: data.contextUsage ?? current.contextUsage ?? null,
    compaction: data.compaction ?? current.compaction ?? null,
    approvals: data.approvals || [],
    agents: data.agents || [],
    currentActivity: data.streaming
      ? data.currentActivity || current.currentActivity || null
      : data.currentActivity?.type === 'agent'
        ? data.currentActivity
        : null,
    activityFeed: data.streaming
      ? data.activityFeed || current.activityFeed || []
      : (data.activityFeed || []).filter((activity: EntityRecord) => activity.type === 'agent'),
    thinkingText: data.streaming ? (data.thinkingText ?? current.thinkingText ?? '') : '',
    queuedInputs: resolveQueuedInputs(current.queuedInputs, data.queuedInputs),
    hadQueuedInput: data.streaming
      ? Boolean(current.hadQueuedInput || data.queuedInputs?.length)
      : false,
  }
}

export function useLiveSessionSync({
  sessionStates,
  sessionStatesRef,
  localStreamSessionsRef,
  updateSessionState,
  updateSessions,
}: SessionSyncOptions) {
  const liveSyncInFlightRef = useRef(new Set<string>())
  const pollingRef = useRef<{ timer: number | null; activeSessions: Set<string> }>({
    timer: null,
    activeSessions: new Set(),
  })

  const syncLiveSession = useCallback(
    async (id: string) => {
      if (!id || localStreamSessionsRef.current.has(id) || liveSyncInFlightRef.current.has(id))
        return
      liveSyncInFlightRef.current.add(id)
      try {
        const data = await chatApi.getLiveSession(id)
        // A local stream can start while the request is in flight. Its optimistic message owns state.
        if (localStreamSessionsRef.current.has(id)) return
        updateSessionState(id, (current) => reconcileLiveSnapshot(current, data))
        updateSessions((current) =>
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
                  plan: planFromPayloadOr(data, session.plan ?? null),
                }
              : session,
          ),
        )
      } catch (error) {
        if (!localStreamSessionsRef.current.has(id)) {
          updateSessionState(id, {
            recovering: false,
            loading: false,
            error: chatErrorMessage(error),
          })
        }
      } finally {
        liveSyncInFlightRef.current.delete(id)
      }
    },
    [localStreamSessionsRef, updateSessionState, updateSessions],
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
                ...reconcileMessagePage(latest, data),
                // Prefer an in-memory selection (including optimistic switches) over a stale page read.
                model: latest.model || data.model || undefined,
                contextUsage: data.contextUsage ?? latest.contextUsage ?? null,
                loaded: true,
                loading: false,
                pageSize: Math.max(latest.pageSize || 0, limit),
                error: '',
                olderError: '',
              },
        )
        const resolvedModel =
          sessionStatesRef.current[id]?.model || (typeof data.model === 'string' ? data.model : '')
        if (resolvedModel) {
          updateSessions((current) =>
            current.map((session) =>
              session.id === id && session.model !== resolvedModel
                ? { ...session, model: resolvedModel }
                : session,
            ),
          )
        }
        if (!sessionStatesRef.current[id]?.streaming) {
          void chatApi
            .getThinkingLevel(id)
            .then((thinking) => {
              if (sessionStatesRef.current[id]?.streaming) return
              const availableThinkingLevels = Array.isArray(thinking.availableLevels)
                ? thinking.availableLevels.map((level) => String(level))
                : Array.isArray(thinking.availableThinkingLevels)
                  ? thinking.availableThinkingLevels.map((level) => String(level))
                  : []
              const thinkingLevel = String(thinking.thinkingLevel || '')
              updateSessionState(id, {
                thinkingLevel: thinkingLevel || undefined,
                availableThinkingLevels,
                thinkingStatus: String(thinking.status || thinking.thinkingStatus || ''),
                thinkingMessage: String(thinking.message || thinking.thinkingMessage || ''),
              })
              if (thinkingLevel) {
                updateSessions((current) =>
                  current.map((session) =>
                    session.id === id && session.thinkingLevel !== thinkingLevel
                      ? { ...session, thinkingLevel }
                      : session,
                  ),
                )
              }
            })
            .catch(() => {})
        }
      } catch (error) {
        updateSessionState(id, { loading: false, error: chatErrorMessage(error) })
      }
    },
    [sessionStatesRef, syncLiveSession, updateSessionState, updateSessions],
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
      } catch (error) {
        updateSessionState(id, { loadingOlder: false, olderError: chatErrorMessage(error) })
        return false
      }
    },
    [sessionStatesRef, updateSessionState],
  )

  const stopPolling = useCallback(() => {
    if (pollingRef.current.timer) window.clearInterval(pollingRef.current.timer)
    pollingRef.current.timer = null
    pollingRef.current.activeSessions.clear()
  }, [])

  const ensurePolling = useCallback(() => {
    if (pollingRef.current.timer || !pollingRef.current.activeSessions.size) return
    pollingRef.current.timer = window.setInterval(() => {
      const requests = Array.from(pollingRef.current.activeSessions).map(async (id) => {
        const state = sessionStatesRef.current[id]
        if (
          !shouldPollLiveSession(state, {
            localStreamOwned: localStreamSessionsRef.current.has(id),
          })
        ) {
          pollingRef.current.activeSessions.delete(id)
          return
        }
        await syncLiveSession(id)
      })
      void Promise.allSettled(requests).then(() => {
        if (!pollingRef.current.activeSessions.size) stopPolling()
      })
    }, 2_000)
  }, [localStreamSessionsRef, sessionStatesRef, stopPolling, syncLiveSession])

  useEffect(() => {
    const knownIds = new Set(Object.keys(sessionStates))
    for (const id of pollingRef.current.activeSessions) {
      if (!knownIds.has(id)) pollingRef.current.activeSessions.delete(id)
    }
    for (const [id, state] of Object.entries(sessionStates)) {
      const shouldPoll = shouldPollLiveSession(state, {
        localStreamOwned: localStreamSessionsRef.current.has(id),
      })
      if (shouldPoll) pollingRef.current.activeSessions.add(id)
      else pollingRef.current.activeSessions.delete(id)
    }
    if (pollingRef.current.activeSessions.size) ensurePolling()
    else stopPolling()
  }, [ensurePolling, localStreamSessionsRef, sessionStates, stopPolling])

  useEffect(() => stopPolling, [stopPolling])

  return { syncLiveSession, loadSessionMessages, loadOlderMessages, liveSyncInFlightRef }
}
