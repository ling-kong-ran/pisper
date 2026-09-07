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
import { reconcileQueuedInputSnapshot } from '@/lib/session-state'
import type { SessionStateUpdate } from '@/lib/session-state'
import type {
  createStreamingTextScheduler,
  createToolUpdateScheduler,
  createTypewriterDisplay,
} from '@/lib/streaming-ui'
import type { EntityRecord, SessionState, SessionSummary } from '@/types/chat'
import type { ApiRecord } from './chat-api'
import {
  handleMobileOperationCancellation,
  handleMobileOperationRequest,
} from './mobile-operations'
import { planChanges, pushCurrentActivity, settleToolCalls } from './run-activity'
import { publishVoiceResponse, type VoiceResponseUpdate } from './voice-response-stream'

const MAX_LIVE_THINKING_CHARS = 6_000

type TextScheduler = ReturnType<typeof createStreamingTextScheduler>
type ToolScheduler = ReturnType<typeof createToolUpdateScheduler>
type Typewriter = ReturnType<typeof createTypewriterDisplay>
type Translate = (message: string, values?: I18nValues) => string

// Team 快照中的 null 是清除信号；只有缺少 team 属性时才沿用内存状态。
function teamFromPayload(
  payload: ApiRecord,
  fallback: EntityRecord | null | undefined,
): EntityRecord | null {
  return Object.hasOwn(payload, 'team') ? (payload.team ?? null) : (fallback ?? null)
}

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
  runId?: string
  startedAt?: string
  terminal?: boolean
}

type TerminalStateOptions = {
  agentId: string
  responseText: string
  data: ApiRecord
  finishedAt: string
  error?: string
  preserveDisplayedText?: boolean
}

// 运行终态立即结算工具、审批和活动；成功时可保留正文显示进度，
// 让最终目标继续逐帧排空，错误则立即校准文本。
export function reconcileTerminalStreamState(
  current: SessionState,
  { agentId, responseText, data, finishedAt, error, preserveDisplayedText }: TerminalStateOptions,
): SessionState {
  const failed = Boolean(error)
  const lifecycle = data.lifecycle || current.lifecycle || {}
  const queueState = reconcileQueuedInputSnapshot(current, data)
  return {
    ...queueState,
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
    team: teamFromPayload(data, current.team),
    agents: data.agents || current.agents || [],
    contextUsage: data.contextUsage ?? current.contextUsage ?? null,
    sessionUsage: data.sessionUsage ?? current.sessionUsage ?? null,
    compaction: data.compaction ?? current.compaction ?? null,
    approvals: failed ? [] : data.approvals || [],
    tools: settleToolCalls(data.tools || current.tools, {
      finishedAt,
      ...(error ? { error } : {}),
    }),
    messages: queueState.messages.map((item) =>
      item.id === agentId
        ? {
            ...item,
            text: preserveDisplayedText
              ? item.text
              : typeof data.text === 'string'
                ? data.text
                : responseText || item.text,
            streaming: preserveDisplayedText ? item.text !== responseText : false,
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

  const publishResponse = (status: VoiceResponseUpdate['status'], error?: string) => {
    const messages = sessionStatesRef.current[sessionId]?.messages ?? []
    const agentIndex = messages.findIndex((message) => message.id === agentId)
    const prompt = messages
      .slice(0, Math.max(0, agentIndex))
      .filter((message) => message.role === 'user')
      .at(-1)?.text
    publishVoiceResponse({
      sessionId,
      messageId: agentId,
      prompt,
      text: state.responseText,
      runId: state.runId,
      startedAt: state.startedAt,
      status,
      error,
    })
  }

  const dispatch = (event: string, data: ApiRecord) => {
    const eventAt = new Date().toISOString()
    if (event === 'run') {
      state.runId = typeof data.runId === 'string' ? data.runId : undefined
      publishResponse('started')
    } else if (event === 'resync_required') {
      publishResponse('recovering')
    } else if (event === 'meta') {
      state.startedAt = typeof data.startedAt === 'string' ? data.startedAt : undefined
      publishResponse('started')
      updateSessionState(sessionId, (current) => ({
        ...reconcileQueuedInputSnapshot(current, data),
        model: data.model || current.model,
        thinkingLevel: data.thinkingLevel || current.thinkingLevel,
        cwd: data.cwd,
        permissionMode: data.permissionMode,
        executionMode: data.executionMode,
        runMode: data.runMode || current.runMode,
        goal: data.goal ?? null,
        plan: planFromPayloadOr(data, current.plan),
        team: teamFromPayload(data, current.team),
        agents: data.agents || current.agents || [],
        currentActivity: data.currentActivity || current.currentActivity || null,
        activityFeed: data.activityFeed || current.activityFeed || [],
        lifecycle: data.lifecycle ?? current.lifecycle ?? null,
        sessionTreeRevision: Number(data.sessionTreeRevision ?? current.sessionTreeRevision ?? 0),
        thinkingText: data.thinkingText ?? current.thinkingText ?? '',
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
        data.team !== undefined ||
        planFromPayload(data) !== undefined
      ) {
        updateSessionSummary((session) => ({
          ...session,
          model: data.model || session.model,
          thinkingLevel: data.thinkingLevel || session.thinkingLevel,
          cwd: data.cwd || session.cwd,
          permissionMode: data.permissionMode || session.permissionMode,
          executionMode: data.executionMode || session.executionMode,
          runMode: data.runMode || session.runMode,
          goal: data.goal ?? session.goal ?? null,
          team: teamFromPayload(data, session.team),
          plan: planFromPayloadOr(data, session.plan ?? null),
        }))
      }
    } else if (event === 'queue_update') {
      if (data.queuedInputs?.length) state.queuedDuringRun = true
      updateSessionState(sessionId, (current) =>
        reconcileQueuedInputSnapshot(current, { ...data, queuedInputs: data.queuedInputs || [] }),
      )
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
          team: teamFromPayload(data, current.team),
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
      if (Object.hasOwn(data, 'team')) {
        updateSessionSummary((session) => ({
          ...session,
          team: teamFromPayload(data, session.team),
        }))
      }
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
      publishResponse('streaming')
      typewriter.setTarget(state.responseText, eventAt)
    } else if (event === 'text_delta') {
      state.responseRenderingStreaming = true
      state.responseText += data.delta || ''
      publishResponse('streaming')
      typewriter.setTarget(state.responseText, eventAt)
    } else if (event === 'text_end') {
      if (typeof data.text === 'string') state.responseText = data.text
      state.responseRenderingStreaming = false
      publishResponse('streaming')
      typewriter.setTarget(state.responseText, data.updatedAt || eventAt)
      updateSessionState(sessionId, (current) => ({
        ...current,
        messages: current.messages.map((item) =>
          item.id === agentId ? { ...item, streaming: item.text !== state.responseText } : item,
        ),
      }))
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
    } else if (event === 'mobile_operation_request') {
      handleMobileOperationRequest(sessionId, data)
    } else if (event === 'mobile_operation_cancel') {
      handleMobileOperationCancellation(data)
    } else if (event === 'permission_request') {
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
      updateSessionState(sessionId, (current) => ({
        ...current,
        goal: data.goal ?? null,
        team: teamFromPayload(data, current.team),
      }))
      updateSessionSummary((session) => ({
        ...session,
        goal: data.goal ?? null,
        team: teamFromPayload(data, session.team),
      }))
    } else if (isPlanUpdateEvent(event)) {
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
      state.terminal = true
      publishResponse(
        event === 'done' ? 'completed' : 'failed',
        event === 'error' ? data.message || 'Speech response failed.' : undefined,
      )
      typewriter.setTarget(state.responseText, finishedAt)
      if (event === 'error') typewriter.flush()
      thinkingScheduler.flush()
      toolScheduler.cancel()
      updateSessionState(sessionId, (current) =>
        reconcileTerminalStreamState(current, {
          agentId,
          responseText: state.responseText,
          data,
          finishedAt,
          preserveDisplayedText: event === 'done',
          ...(event === 'error' ? { error: data.message } : {}),
        }),
      )
      if (event === 'done') {
        updateSessionSummary((session) => ({
          ...session,
          streaming: false,
          goal: data.goal ?? session.goal ?? null,
          team: teamFromPayload(data, session.team),
          plan: planFromPayloadOr(data, session.plan ?? null),
        }))
        return false
      }
      throw new Error(data.message)
    }
  }

  return { dispatch, state }
}
