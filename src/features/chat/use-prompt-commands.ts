// 输入框斜杠命令：/new、/model 等快捷指令的执行逻辑，
// 插入交互消息并触发相应会话操作。
import { useCallback, useRef } from 'react'
import { APP_NAME } from '@/app/brand'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import { insertInteractiveUserMessage, resolveQueuedInputs } from '@/lib/session-state'
import type { SessionStateUpdate } from '@/lib/session-state'
import {
  createStreamingTextScheduler,
  createToolUpdateScheduler,
  createTypewriterDisplay,
} from '@/lib/streaming-ui'
import type { ChatAttachment, ResourceInvocation, SessionState, SessionSummary } from '@/types/chat'
import { chatApi } from './chat-api'
import { chatErrorMessage, isEndedSessionQueueError } from './chat-errors'
import { pushCurrentActivity, settleToolCalls } from './run-activity'
import { createStreamEventDispatcher, type StreamDispatchState } from './stream-event-dispatch'

const USAGE_UPDATED_EVENT = 'pisper:usage-updated'

type PromptCommandOptions = {
  browserNotify?: (event: string, data: unknown, options?: { force?: boolean }) => void
  notify: Notify
  defaultModel: string
  localStreamSessionsRef: React.MutableRefObject<Set<string>>
  sessionStatesRef: React.MutableRefObject<Record<string, SessionState>>
  setActiveId: (id: string) => void
  setGlobalError: (error: string) => void
  updateSessionState: (id: string, update: SessionStateUpdate) => void
  updateSessions: (
    update: SessionSummary[] | ((current: SessionSummary[]) => SessionSummary[]),
  ) => SessionSummary[]
  createSession: () => Promise<string>
  loadSessionMessages: (id: string, options?: { force?: boolean; limit?: number }) => Promise<void>
  refreshSessions: (preferredId?: string) => Promise<SessionSummary[]>
  syncLiveSession: (id: string) => Promise<void>
}

export function usePromptCommands({
  browserNotify,
  notify,
  defaultModel,
  localStreamSessionsRef,
  sessionStatesRef,
  setActiveId,
  setGlobalError,
  updateSessionState,
  updateSessions,
  createSession,
  loadSessionMessages,
  refreshSessions,
  syncLiveSession,
}: PromptCommandOptions) {
  const { t } = useI18n()
  const localStreamSettlersRef = useRef(
    new Map<string, { promise: Promise<void>; resolve: () => void }>(),
  )

  // 发送提示词：无会话先建会话，流式中拒绝；乐观插入用户消息并
  // 注册本地流结算 Promise（供后续 wait），随后发起 SSE 流并分发事件，
  // 结束时结算终态并刷新列表。
  const sendPrompt = useCallback(
    async (
      text: string,
      requestedSessionId: string,
      attachments: ChatAttachment[] = [],
      goalMode = false,
      goalTokenBudget: number | null = null,
      invocation: ResourceInvocation | null = null,
    ) => {
      const prompt =
        text.trim() ||
        (attachments.length ? t('chat:chatPage.pleaseAnalyzeTheseAttachments') : '') ||
        (invocation ? invocation.resourceName : '')
      if (!prompt && !invocation) return
      let sessionId = requestedSessionId
      if (!sessionId) sessionId = await createSession()
      if (!sessionId || sessionStatesRef.current[sessionId]?.streaming) return

      let resolveLocalStream = () => {}
      const localStreamSettled = new Promise<void>((resolve) => {
        resolveLocalStream = resolve
      })
      localStreamSettlersRef.current.set(sessionId, {
        promise: localStreamSettled,
        resolve: resolveLocalStream,
      })
      setActiveId(sessionId)
      setGlobalError('')

      const userMessage = {
        id: `user-${Date.now()}`,
        role: 'user',
        text: prompt,
        invocation,
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
      let streamState: StreamDispatchState = {
        responseText: '',
        responseRenderingStreaming: true,
        thinkingText: '',
        thinkingPrefix: '',
        queuedDuringRun: false,
      }
      const thinkingScheduler = createStreamingTextScheduler(
        (thinkingText, activityAt) => {
          updateSessionState(sessionId, (current) => ({
            ...current,
            thinkingText,
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
      const typewriter = createTypewriterDisplay((responseText, activityAt) => {
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
              item.id === agentId
                ? { ...item, text: responseText, streaming: streamState.responseRenderingStreaming }
                : item,
            ),
          }
        })
      })
      const toolScheduler = createToolUpdateScheduler((batch, activityAt) => {
        updateSessionState(sessionId, (current) => {
          let activityFeed = current.activityFeed || []
          for (const [id, patch] of batch) {
            const existing = activityFeed.find((item) => item.type === 'tool' && item.id === id)
            if (existing) {
              activityFeed = pushCurrentActivity(activityFeed, {
                ...existing,
                ...patch,
                updatedAt: activityAt || existing.updatedAt,
              })
            }
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
      const dispatcher = createStreamEventDispatcher({
        sessionId,
        agentId,
        sessionStatesRef,
        updateSessionState,
        updateSessions,
        typewriter,
        thinkingScheduler,
        toolScheduler,
        t,
      })
      streamState = dispatcher.state

      updateSessionState(sessionId, (current) => {
        const keepPlan = goalMode || current.goal?.status === 'active'
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
          lifecycle: {
            phase: 'starting',
            event: 'prompt_submitted',
            turn: 0,
            updatedAt: runStartedAt,
          },
          thinkingText: '',
          hadQueuedInput: false,
          compaction: null,
          plan: keepPlan ? current.plan : null,
        }
      })
      if (!goalMode) {
        updateSessions((current) =>
          current.map((session) =>
            session.id !== sessionId || session.goal?.status === 'active'
              ? session
              : { ...session, plan: null },
          ),
        )
      }
      localStreamSessionsRef.current.add(sessionId)

      try {
        await chatApi.openStream(
          { sessionId, message: prompt, attachments, goalMode, goalTokenBudget, invocation },
          dispatcher.dispatch,
        )
        typewriter.setTarget(streamState.responseText)
        typewriter.flush()
        thinkingScheduler.flush()
        toolScheduler.flush()
        const fallbackFinishedAt = new Date().toISOString()
        if (sessionStatesRef.current[sessionId]?.streaming) {
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
                  ? { ...item, streaming: false, text: streamState.responseText || item.text }
                  : item,
              ),
            }
          })
        }
        // Reconcile every optimistic SSE bubble with the durable transcript after the run settles.
        await loadSessionMessages(sessionId, { force: true })
        if (
          goalMode ||
          streamState.queuedDuringRun ||
          sessionStatesRef.current[sessionId]?.hadQueuedInput ||
          sessionStatesRef.current[sessionId]?.goal ||
          sessionStatesRef.current[sessionId]?.queuedInputs?.length
        ) {
          updateSessionState(sessionId, { queuedInputs: [], hadQueuedInput: false })
        }
        let completed: SessionSummary | undefined
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
              streamState.responseText.trim().slice(0, 260) ||
              t('chat:chatPage.theAgentHasFinishedResponding'),
            model: sessionStatesRef.current[sessionId]?.model || defaultModel,
          },
        })
      } catch (error) {
        typewriter.cancel()
        thinkingScheduler.flush()
        toolScheduler.cancel()
        const runFinishedAt = new Date().toISOString()
        const message = chatErrorMessage(error)
        updateSessionState(sessionId, (current) => ({
          ...current,
          streaming: false,
          error: message,
          runFinishedAt,
          lastActivityAt: runFinishedAt,
          runNotice: '',
          currentActivity: null,
          activityFeed: [],
          approvals: [],
          tools: settleToolCalls(current.tools, { finishedAt: runFinishedAt, error: message }),
          messages: current.messages.map((item) =>
            item.id === agentId
              ? {
                  ...item,
                  streaming: false,
                  error: message,
                  text: item.text || streamState.responseText || message,
                }
              : item,
          ),
        }))
        if (
          streamState.queuedDuringRun ||
          sessionStatesRef.current[sessionId]?.hadQueuedInput ||
          sessionStatesRef.current[sessionId]?.queuedInputs?.length
        ) {
          try {
            await loadSessionMessages(sessionId, { force: true })
          } catch {}
          updateSessionState(sessionId, { queuedInputs: [], hadQueuedInput: false })
        }
        updateSessions((current) =>
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
        const settler = localStreamSettlersRef.current.get(sessionId)
        localStreamSettlersRef.current.delete(sessionId)
        settler?.resolve()
        window.dispatchEvent(new Event(USAGE_UPDATED_EVENT))
      }
    },
    [
      browserNotify,
      createSession,
      defaultModel,
      loadSessionMessages,
      localStreamSessionsRef,
      refreshSessions,
      sessionStatesRef,
      setActiveId,
      setGlobalError,
      syncLiveSession,
      t,
      updateSessionState,
      updateSessions,
    ],
  )

  // 排队输入：流式中把新提示放入队列（behavior 如 steer），
  // 乐观插入排队中的用户消息，返回是否成功入队。
  const queuePrompt = useCallback(
    async (
      text: string,
      sessionId: string,
      attachments: ChatAttachment[] = [],
      behavior = 'steer',
    ) => {
      const message =
        String(text || '').trim() ||
        (attachments.length ? t('chat:chatPage.pleaseAnalyzeTheseAttachments') : '')
      if (!sessionId || !message) return false
      try {
        const result = await chatApi.queueInput(sessionId, message, attachments, behavior)
        const queuedAt = new Date().toISOString()
        const queuedMessage = {
          id: `interactive-user-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
          role: 'user',
          text: message,
          queuedAt,
          streamingBehavior: result.behavior || behavior,
          attachments: attachments.map(({ id, kind, name, mimeType, size, data }) => ({
            id,
            kind,
            name,
            mimeType,
            size,
            data: kind === 'image' ? data : undefined,
          })),
        }
        updateSessionState(sessionId, (current) => ({
          ...current,
          messages: insertInteractiveUserMessage(current.messages, queuedMessage),
          queuedInputs: resolveQueuedInputs(current.queuedInputs, result.queuedInputs),
          hadQueuedInput: true,
          lastActivityAt: queuedAt,
        }))
        return true
      } catch (error) {
        if (isEndedSessionQueueError(error)) {
          const activeStream = localStreamSettlersRef.current.get(sessionId)
          if (activeStream) await activeStream.promise
          else await syncLiveSession(sessionId)
          updateSessionState(sessionId, {
            streaming: false,
            recovering: false,
            queuedInputs: [],
            hadQueuedInput: false,
          })
          await loadSessionMessages(sessionId, { force: true })
          void sendPrompt(message, sessionId, attachments)
          return true
        }
        notify(chatErrorMessage(error), 'error')
        return false
      }
    },
    [loadSessionMessages, notify, sendPrompt, syncLiveSession, t, updateSessionState],
  )

  // 中止会话运行：调运行时 abort 并本地结算状态（清队列、停流、
  // 中止中的压缩标记为 aborted）。
  const abort = useCallback(
    async (sessionId: string) => {
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
      updateSessions((current) =>
        current.map((session) =>
          session.id === sessionId
            ? { ...session, streaming: false, goal: result.goal ?? session.goal ?? null }
            : session,
        ),
      )
      notify(t('chat:chatPage.currentRunStopped'), 'info')
    },
    [notify, t, updateSessionState, updateSessions],
  )

  return { sendPrompt, queuePrompt, abort }
}
