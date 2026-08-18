// SSE 事件分发核心：把每个流式记录转换为会话状态更新。
// 文本/思考走打字机与合并调度器，工具事件归并到活动，plan 事件更新
// 计划面板；在事件间保留会话“正在运行”的语义。
import { applyTextPatch } from '@/lib/api'
import type { I18nValues } from '@/app/i18n'
import {
  isPlanUpdateEvent,
  isPlanWriteTool,
  planFromPayload,
  planFromPayloadOr,
} from '@/lib/plan-protocol'
import { resolveQueuedInputs } from '@/lib/session-state'
import type { SessionStateUpdate } from '@/lib/session-state'
import type {
  createStreamingTextScheduler,
  createToolUpdateScheduler,
  createTypewriterDisplay,
} from '@/lib/streaming-ui'
import type { EntityRecord, SessionState, SessionSummary } from '@/types/chat'
import type { ApiRecord } from './chat-api'
import { planChanges, pushCurrentActivity, settleToolCalls } from './run-activity'

const MAX_LIVE_THINKING_CHARS = 6_000

type TextScheduler = ReturnType<typeof createStreamingTextScheduler>
type ToolScheduler = ReturnType<typeof createToolUpdateScheduler>
type Typewriter = ReturnType<typeof createTypewriterDisplay>
type Translate = (message: string, values?: I18nValues) => string

type StreamDispatchOptions = {
  sessionId: string
  agentId: string
  sessionStatesRef: React.MutableRefObject<Record<string, SessionState>>
  updateSessionState: (id: string, update: SessionStateUpdate) => void
  updateSessions: (
    update: SessionSummary[] | ((current: SessionSummary[]) => SessionSummary[]),
  ) => SessionSummary[]
  typewriter: Typewriter
  thinkingScheduler: TextScheduler
  toolScheduler: ToolScheduler
  t: Translate
}

export type StreamDispatchState = {
  responseText: string
  responseRenderingStreaming: boolean
  thinkingText: string
  thinkingPrefix: string
  queuedDuringRun: boolean
}

type TerminalStateOptions = {
  agentId: string
  responseText: string
  data: ApiRecord
  finishedAt: string
  error?: string
}

// 收尾一回合流：把流式现场结算为终态——工具调用统一落定、生命周期标记
// 完成/失败、agent 消息写入最终文本（失败时保留当前草稿与计划），
// 并清理审批与进行中的活动。
export function reconcileTerminalStreamState(
  current: SessionState,
  { agentId, responseText, data, finishedAt, error }: TerminalStateOptions,
): SessionState {
  const failed = Boolean(error)
  const lifecycle = data.lifecycle || current.lifecycle || {}
  return {
    ...current,
    streaming: false,
    runFinishedAt: finishedAt,
    lastActivityAt: finishedAt,
    ...(failed ? {} : { runNotice: '' }),
    currentActivity: data.currentActivity?.type === 'agent' ? data.currentActivity : null,
    lifecycle: {
      ...lifecycle,
      phase: failed ? 'failed' : 'completed',
      event: failed ? 'runtime_error' : 'runtime_done',
      updatedAt: finishedAt,
    },
    sessionTreeRevision: Number(data.sessionTreeRevision ?? current.sessionTreeRevision ?? 0),
    activityFeed: (data.activityFeed || []).filter(
      (activity: EntityRecord) => activity.type === 'agent',
    ),
    goal: failed ? current.goal : (data.goal ?? current.goal ?? null),
    plan: failed ? current.plan : planFromPayloadOr(data, current.plan),
    agents: data.agents || current.agents || [],
    contextUsage: data.contextUsage ?? current.contextUsage ?? null,
    sessionUsage: data.sessionUsage ?? current.sessionUsage ?? null,
    compaction: data.compaction ?? current.compaction ?? null,
    approvals: failed ? [] : data.approvals || [],
    tools: settleToolCalls(data.tools || current.tools, {
      finishedAt,
      ...(error ? { error } : {}),
    }),
    messages: current.messages.map((item) =>
      item.id === agentId
        ? {
            ...item,
            text: typeof data.text === 'string' ? data.text : responseText || item.text,
            streaming: false,
            ...(failed ? {} : data.assets?.length ? { attachments: data.assets } : {}),
          }
        : item,
    ),
  }
}

// 创建流事件分发器：把 SSE 流逐条记录转换为会话状态更新——
// 文本/思考经调度器合并，工具/agent/plan 活动归并进活动流，
// 收尾事件结算终态；同时维护排队输入与目标模式预算。
export function createStreamEventDispatcher({
  sessionId,
  agentId,
  sessionStatesRef,
  updateSessionState,
  updateSessions,
  typewriter,
  thinkingScheduler,
  toolScheduler,
  t,
}: StreamDispatchOptions) {
  const state: StreamDispatchState = {
    responseText: '',
    responseRenderingStreaming: true,
    thinkingText: '',
    thinkingPrefix: '',
    queuedDuringRun: false,
  }

  const updateSessionSummary = (update: (session: SessionSummary) => SessionSummary) => {
    updateSessions((current) =>
      current.map((session) => (session.id === sessionId ? update(session) : session)),
    )
  }

  const dispatch = (event: string, data: ApiRecord) => {
    const eventAt = new Date().toISOString()
    if (event === 'meta') {
      updateSessionState(sessionId, (current) => ({
        ...current,
        model: data.model || current.model,
        thinkingLevel: data.thinkingLevel || current.thinkingLevel,
        cwd: data.cwd,
        permissionMode: data.permissionMode,
        executionMode: data.executionMode,
        goal: data.goal ?? null,
        plan: planFromPayloadOr(data, current.plan),
        agents: data.agents || current.agents || [],
        currentActivity: data.currentActivity || current.currentActivity || null,
        activityFeed: data.activityFeed || current.activityFeed || [],
        lifecycle: data.lifecycle ?? current.lifecycle ?? null,
        sessionTreeRevision: Number(data.sessionTreeRevision ?? current.sessionTreeRevision ?? 0),
        thinkingText: data.thinkingText ?? current.thinkingText ?? '',
        queuedInputs: resolveQueuedInputs(current.queuedInputs, data.queuedInputs),
        hadQueuedInput: Boolean(current.hadQueuedInput || data.queuedInputs?.length),
        contextUsage: data.contextUsage ?? current.contextUsage ?? null,
        sessionUsage: data.sessionUsage ?? current.sessionUsage ?? null,
        runStartedAt: data.startedAt || current.runStartedAt,
        lastActivityAt: data.lastActivityAt || eventAt,
      }))
      if (
        data.model ||
        data.thinkingLevel ||
        data.cwd ||
        data.permissionMode ||
        data.executionMode ||
        data.goal !== undefined ||
        planFromPayload(data) !== undefined
      ) {
        updateSessionSummary((session) => ({
          ...session,
          model: data.model || session.model,
          thinkingLevel: data.thinkingLevel || session.thinkingLevel,
          cwd: data.cwd || session.cwd,
          permissionMode: data.permissionMode || session.permissionMode,
          executionMode: data.executionMode || session.executionMode,
          goal: data.goal ?? session.goal ?? null,
          plan: planFromPayloadOr(data, session.plan ?? null),
        }))
      }
    } else if (event === 'queue_update') {
      if (data.queuedInputs?.length) state.queuedDuringRun = true
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
    } else if (event === 'agent_lifecycle') {
      updateSessionState(sessionId, (current) => {
        const lifecycle = data.lifecycle || current.lifecycle
        const retryFinished = lifecycle?.event === 'auto_retry_end'
        const retryRestarted =
          lifecycle?.event === 'turn_start' &&
          (current.lifecycle?.phase === 'retrying' || current.currentActivity?.type === 'retry')
        return {
          ...current,
          lifecycle,
          currentActivity: data.currentActivity || current.currentActivity,
          lastActivityAt: lifecycle?.updatedAt || eventAt,
          runNotice:
            retryFinished && lifecycle?.retry?.success
              ? ''
              : retryFinished && lifecycle?.retry?.message
                ? lifecycle.retry.message
                : retryRestarted
                  ? ''
                  : current.runNotice,
        }
      })
    } else if (event === 'session_tree_changed') {
      updateSessionState(sessionId, {
        sessionTreeRevision: Number(data.revision || 0),
        lastActivityAt: eventAt,
      })
    } else if (event === 'thinking_level_changed') {
      updateSessionState(sessionId, { thinkingLevel: data.level, lastActivityAt: eventAt })
      updateSessionSummary((session) => ({ ...session, thinkingLevel: data.level }))
    } else if (event === 'context_usage') {
      updateSessionState(sessionId, { contextUsage: data })
    } else if (event === 'session_usage') {
      updateSessionState(sessionId, { sessionUsage: data })
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
      state.responseRenderingStreaming = true
      state.responseText = applyTextPatch(state.responseText, data)
      typewriter.setTarget(state.responseText, eventAt)
    } else if (event === 'text_delta') {
      state.responseRenderingStreaming = true
      state.responseText += data.delta || ''
      typewriter.setTarget(state.responseText, eventAt)
    } else if (event === 'text_end') {
      if (typeof data.text === 'string') state.responseText = data.text
      state.responseRenderingStreaming = false
      typewriter.setTarget(state.responseText, data.updatedAt || eventAt)
      typewriter.flush()
    } else if (event === 'thinking_reset') {
      state.thinkingText = ''
      state.thinkingPrefix = String(data.thinkingText || '').slice(-MAX_LIVE_THINKING_CHARS)
      thinkingScheduler.cancel()
      updateSessionState(sessionId, {
        thinkingText: state.thinkingPrefix,
        currentActivity: {
          type: 'model',
          stage: 'thinking',
          updatedAt: data.updatedAt || eventAt,
        },
        lastActivityAt: data.updatedAt || eventAt,
      })
    } else if (event === 'thinking_patch') {
      state.thinkingText = applyTextPatch(state.thinkingText, data).slice(-MAX_LIVE_THINKING_CHARS)
      const displayedThinking = [state.thinkingPrefix, state.thinkingText]
        .filter(Boolean)
        .join('\n\n')
      thinkingScheduler.push(displayedThinking.slice(-MAX_LIVE_THINKING_CHARS), eventAt)
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
          tools: pushCurrentActivity(current.tools, activity),
          currentActivity: activity,
          activityFeed: pushCurrentActivity(current.activityFeed, activity),
          messages: current.messages.map((item) =>
            item.id === agentId ? { ...item, text: state.responseText || item.text } : item,
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
        const completedToolName = String(completedTool?.name || '')
        const preserveEvent =
          (isPlanWriteTool(completedToolName) ||
            [
              'spawn_agent',
              'list_agents',
              'send_message',
              'followup_task',
              'wait_agent',
              'interrupt_agent',
            ].includes(completedToolName)) &&
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
        if (data.error || (!preserveEvent && !agentActivity)) {
          activityFeed = pushCurrentActivity(activityFeed, toolActivity)
        }
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
                  ...(item.attachments || []).filter((attachment) => attachment.id !== data.id),
                  data,
                ],
              }
            : item,
        ),
      }))
    } else if (event === 'goal_update') {
      updateSessionState(sessionId, { goal: data.goal ?? null })
      updateSessionSummary((session) => ({ ...session, goal: data.goal ?? null }))
    } else if (isPlanUpdateEvent(event)) {
      typewriter.flush()
      toolScheduler.flush()
      updateSessionState(sessionId, (current) => {
        const nextPlan = planFromPayloadOr(data, current.plan)
        const activity = data.currentActivity || {
          type: 'plan',
          plan: nextPlan,
          changes: planChanges(current.plan, nextPlan),
          updatedAt: nextPlan?.updatedAt || eventAt,
        }
        return {
          ...current,
          lastActivityAt: eventAt,
          plan: nextPlan,
          currentActivity: activity,
          activityFeed: pushCurrentActivity(current.activityFeed, activity),
        }
      })
      updateSessionSummary((session) => ({
        ...session,
        plan: planFromPayloadOr(data, session.plan ?? null),
      }))
    } else if (event === 'session_title') {
      updateSessionSummary((session) => ({ ...session, name: data.name }))
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
    } else if (event === 'done' || event === 'error') {
      state.queuedDuringRun ||= Boolean(
        sessionStatesRef.current[sessionId]?.hadQueuedInput ||
        sessionStatesRef.current[sessionId]?.queuedInputs?.length,
      )
      const finishedAt = data.finishedAt || eventAt
      if (typeof data.text === 'string') state.responseText = data.text
      state.responseRenderingStreaming = false
      typewriter.setTarget(state.responseText, finishedAt)
      typewriter.flush()
      thinkingScheduler.flush()
      toolScheduler.cancel()
      updateSessionState(sessionId, (current) =>
        reconcileTerminalStreamState(current, {
          agentId,
          responseText: state.responseText,
          data,
          finishedAt,
          ...(event === 'error' ? { error: data.message } : {}),
        }),
      )
      if (event === 'done') {
        updateSessionSummary((session) => ({
          ...session,
          streaming: false,
          goal: data.goal ?? session.goal ?? null,
          plan: planFromPayloadOr(data, session.plan ?? null),
        }))
        return false
      }
      throw new Error(data.message)
    }
  }

  return { dispatch, state }
}
