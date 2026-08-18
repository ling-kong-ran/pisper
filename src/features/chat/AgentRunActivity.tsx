// Agent 运行活动面板：在消息下方展示当前运行的 Agent 状态——思考文本、
// 工具调用列表、计划预览与停止按钮，实时跟随 SSE 流更新。
import { lazy, memo, Suspense, useEffect, useRef, useState, type ReactNode } from 'react'
import { AlertTriangle, Check, ChevronRight, Clock3, Code2, RefreshCw, Square } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Plan } from '@/components/ai-elements/plan'
import { Task } from '@/components/ai-elements/task'
import MarkdownMessage from '@/components/MarkdownMessage'
import { formatTokenCount } from '@/lib/format'
import { isPlanReadTool, isPlanWriteTool, planFromActivity } from '@/lib/plan-protocol'
import { terminalDisplayOutput } from '@/lib/terminal-output'
import type { I18nValues } from '@/app/i18n'
import type { EntityRecord } from '@/types/chat'
import {
  activityDurationMs,
  activityRenderKey,
  activityScrollVersion,
  agentActivityState,
  formatRunDuration,
  primaryRunActivity,
  runDurationMs,
} from './run-activity'

const EMPTY_LIST: EntityRecord[] = []
const AnimatedList = lazy(() =>
  import('@/components/react-bits/AnimatedList').then((module) => ({
    default: module.AnimatedList,
  })),
)
const ShinyText = lazy(() =>
  import('@/components/react-bits/ShinyText').then((module) => ({ default: module.ShinyText })),
)

type Translate = (message: string, values?: I18nValues) => string

function toolActivityLabel(name: unknown, t: Translate) {
  if (name === 'read') return t('chat:agentRunActivity.readingFiles')
  if (name === 'grep') return t('chat:agentRunActivity.searchingContent')
  if (name === 'find') return t('chat:agentRunActivity.findingFiles')
  if (name === 'ls') return t('chat:agentRunActivity.browsingDirectories')
  if (name === 'edit') return t('chat:agentRunActivity.editingFiles')
  if (name === 'write') return t('chat:agentRunActivity.writingFiles')
  if (name === 'bash') return t('chat:agentRunActivity.runningCommands')
  if (name === 'memory_search') return t('chat:agentRunActivity.searchingMemory')
  if (name === 'memory_remember') return t('chat:agentRunActivity.savingMemory')
  if (name === 'spawn_agent') return t('chat:agentRunActivity.startingSubagent')
  if (name === 'list_agents') return t('chat:agentRunActivity.checkingSubagents')
  if (name === 'send_message') return t('chat:agentRunActivity.messagingSubagent')
  if (name === 'followup_task') return t('chat:agentRunActivity.addingSubagentTask')
  if (name === 'wait_agent') return t('chat:agentRunActivity.waitingForSubagent')
  if (name === 'interrupt_agent') return t('chat:agentRunActivity.interruptingSubagent')
  if (isPlanReadTool(name)) return t('chat:agentRunActivity.readingPlan')
  if (isPlanWriteTool(name)) return t('chat:agentRunActivity.updatingPlan')
  if (name === 'browser_automation') return t('chat:agentRunActivity.controllingBrowser')
  if (name === 'generate_visual') return t('chat:agentRunActivity.generatingVisualContent')
  if (name === 'discover_tools') return t('chat:agentRunActivity.discoveringTools')
  return t('chat:agentRunActivity.usingTool', {
    tool: String(name || t('chat:agentRunActivity.tools')),
  })
}

function toolCompletedLabel(name: unknown, t: Translate) {
  if (name === 'read') return t('chat:agentRunActivity.filesRead')
  if (name === 'grep') return t('chat:agentRunActivity.searchCompleted')
  if (name === 'find') return t('chat:agentRunActivity.findCompleted')
  if (name === 'ls') return t('chat:agentRunActivity.directoriesBrowsed')
  if (name === 'edit') return t('chat:agentRunActivity.filesEdited')
  if (name === 'write') return t('chat:agentRunActivity.filesWritten')
  if (name === 'bash') return t('chat:agentRunActivity.commandCompleted')
  if (name === 'spawn_agent') return t('chat:agentRunActivity.subagentStarted')
  if (name === 'wait_agent') return t('chat:agentRunActivity.subagentStatusUpdated')
  if (isPlanWriteTool(name)) return t('chat:agentRunActivity.planUpdated')
  if (name === 'discover_tools') return t('chat:agentRunActivity.toolsDiscovered')
  return t('chat:agentRunActivity.currentOperationCompleted')
}

function agentActivityTitle(status: unknown, name: string, t: Translate) {
  if (status === 'queued') return t('chat:agentRunActivity.agentQueued', { name })
  if (status === 'starting') return t('chat:agentRunActivity.agentStarting', { name })
  if (status === 'running') return t('chat:agentRunActivity.agentRunning', { name })
  if (status === 'completed') return t('chat:agentRunActivity.agentCompleted', { name })
  if (status === 'interrupted') return t('chat:agentRunActivity.agentInterrupted', { name })
  if (status === 'failed') return t('chat:agentRunActivity.agentFailed', { name })
  return t('chat:agentRunActivity.agentStatusUpdated', { name })
}

export type AgentRunActivityProps = {
  streaming?: boolean
  text?: string
  thinkingText?: string
  currentActivity?: EntityRecord | null
  activityFeed?: EntityRecord[]
  compaction?: EntityRecord | null
  error?: string
  stopped?: boolean
  notice?: string
  startedAt?: string | null
  lastActivityAt?: string | null
  finishedAt?: string | null
  compact?: boolean
  tools?: EntityRecord[]
}

type ActivityPresentationContext = {
  t: Translate
  streaming?: boolean
  text?: string
  thinkingText?: string
  compaction?: EntityRecord | null
  error?: string
  stopped?: boolean
  notice?: string
  lastActivityAt?: string | null
  now: number
}

type ActivityPresentation = {
  tone: string
  title: string
  detail: string
  output: string
  command: boolean
  startedAt: unknown
  changes: EntityRecord[]
}

function useRunActivityClock(streaming?: boolean) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    setNow(Date.now())
    if (!streaming) return undefined
    const timer = window.setInterval(() => setNow(Date.now()), 1_000)
    return () => window.clearInterval(timer)
  }, [streaming])
  return now
}

function cleanInline(value: unknown) {
  return String(value || '')
    .replace(/\s+/g, ' ')
    .trim()
}

function toolDetail(tool?: EntityRecord | null) {
  const args = tool?.args || {}
  if (tool?.name === 'bash') return { text: String(args.command || '').trim(), command: true }
  if (['read', 'edit', 'write', 'ls'].includes(tool?.name)) return { text: cleanInline(args.path) }
  if (tool?.name === 'grep')
    return {
      text: [args.pattern ? `“${cleanInline(args.pattern)}”` : '', cleanInline(args.path)]
        .filter(Boolean)
        .join(' · '),
    }
  if (tool?.name === 'find')
    return { text: [cleanInline(args.pattern), cleanInline(args.path)].filter(Boolean).join(' · ') }
  if (tool?.name === 'browser_automation')
    return {
      text: [cleanInline(args.action), cleanInline(args.url || args.selector)]
        .filter(Boolean)
        .join(' · '),
    }
  if (tool?.name === 'spawn_agent') return { text: cleanInline(args.taskName) }
  if (['send_message', 'followup_task', 'wait_agent', 'interrupt_agent'].includes(tool?.name))
    return { text: cleanInline(args.target) }
  if (tool?.name === 'generate_visual') return { text: cleanInline(args.outputName || args.kind) }
  if (tool?.name === 'discover_tools') return { text: cleanInline(args.query) }
  return { text: '' }
}

function planProgress(plan: EntityRecord | null | undefined, t: Translate) {
  const items = plan?.items || []
  const completed =
    plan?.counts?.completed ??
    items.filter((item: EntityRecord) => item.status === 'completed').length
  const active =
    plan?.counts?.inProgress ??
    items.filter((item: EntityRecord) => item.status === 'in_progress').length
  return (
    [
      items.length
        ? t('chat:agentRunActivity.completedTotalCompleted', { completed, total: items.length })
        : '',
      active ? t('chat:agentRunActivity.countInProgress', { count: active }) : '',
    ]
      .filter(Boolean)
      .join(' · ') || t('chat:agentRunActivity.planCleared')
  )
}

function planChangeText(change: EntityRecord, t: Translate) {
  if (change.kind === 'removed')
    return t('chat:agentRunActivity.removedTitle', { title: change.title })
  if (change.status === 'completed')
    return t('chat:agentRunActivity.completedTitle', { title: change.title })
  if (change.status === 'in_progress')
    return t('chat:agentRunActivity.inProgressTitle', { title: change.title })
  if (change.status === 'blocked')
    return t('chat:agentRunActivity.blockedTitle', { title: change.title })
  if (change.kind === 'added') return t('chat:agentRunActivity.addedTitle', { title: change.title })
  return t('chat:agentRunActivity.pendingTitle', { title: change.title })
}

function compactionText(compaction: EntityRecord | null | undefined, t: Translate) {
  if (compaction?.active) {
    return compaction.reason === 'overflow'
      ? t('chat:agentRunActivity.contextIsFullTheRequestWillRetryAfterCompaction')
      : compaction.reason === 'manual'
        ? t('chat:agentRunActivity.summarizingOlderMessagesAsRequested')
        : t('chat:agentRunActivity.contextIsNearingItsLimitSummarizingOlderMessages')
  }
  if (compaction?.tokensBefore != null && compaction?.estimatedTokensAfter != null) {
    return t('chat:agentRunActivity.contextCompactedBeforeAfterTokens', {
      before: formatTokenCount(compaction.tokensBefore),
      after: formatTokenCount(compaction.estimatedTokensAfter),
    })
  }
  return t('chat:agentRunActivity.summarizingOlderMessages')
}

function activityPresentation(
  activity: EntityRecord,
  {
    t,
    streaming,
    text,
    thinkingText,
    compaction,
    error,
    stopped,
    notice,
    lastActivityAt,
    now,
  }: ActivityPresentationContext,
): ActivityPresentation {
  let tone = 'running'
  let title = t('chat:agentRunActivity.understandingTheTask')
  let detail = notice || ''
  let output = ''
  let command = false
  let startedAt = activity.startedAt
  let changes = EMPTY_LIST

  if (activity.type === 'tool') {
    const failed = activity.status === 'error'
    const completed = activity.status === 'done'
    tone = failed ? 'failed' : completed ? 'completed' : 'running'
    title = failed
      ? t('chat:agentRunActivity.toolFailed', {
          tool: toolActivityLabel(activity.name, t),
        })
      : completed
        ? toolCompletedLabel(activity.name, t)
        : toolActivityLabel(activity.name, t)
    const toolInfo = toolDetail(activity)
    detail = toolInfo.text
    command = Boolean(toolInfo.command)
    output = cleanInline(activity.message)
  } else if (activity.type === 'plan') {
    tone = 'plan'
    const activityPlan = planFromActivity(activity)
    title = activityPlan?.items?.length
      ? t('chat:agentRunActivity.planUpdated')
      : t('chat:agentRunActivity.planCleared')
    changes = activity.changes || EMPTY_LIST
    detail = changes.length
      ? t('chat:agentRunActivity.countPlanChanges', { count: changes.length })
      : planProgress(activityPlan, t)
  } else if (activity.type === 'agent') {
    const agent = activity.agent || {}
    const name = agent.canonicalName || agent.taskName || t('chat:agentRunActivity.subagent')
    const state = agentActivityState(agent.status)
    title = agentActivityTitle(agent.status, name, t)
    tone = state.tone
    const nestedTool =
      agent.currentActivity?.type === 'tool'
        ? toolDetail(agent.currentActivity).text || agent.currentActivity.name
        : ''
    detail = nestedTool || cleanInline(agent.error || agent.message)
    output = agent.status === 'running' ? cleanInline(agent.output) : ''
    startedAt = agent.startedAt || startedAt
  } else if (activity.type === 'compaction') {
    tone = 'compacting'
    title = t('chat:agentRunActivity.compactingContext')
    detail = compactionText(activity.compaction || compaction, t)
  } else if (activity.type === 'retry') {
    tone = 'waiting'
    title = t('chat:agentRunActivity.retryingRequest')
    detail = activity.message || notice || ''
  } else {
    const inactiveMs = Math.max(0, now - new Date(lastActivityAt || now).getTime())
    if (error) {
      tone = 'failed'
      title = t('chat:agentRunActivity.thisRunFailed')
      detail = error
    } else if (stopped) {
      tone = 'stopped'
      title = t('chat:agentRunActivity.runStopped')
    } else if (!streaming) {
      tone = 'completed'
      title = t('chat:agentRunActivity.reasoningCompleted')
      detail = cleanInline(thinkingText)
    } else if (inactiveMs >= 10_000) {
      tone = 'waiting'
      title = t('chat:agentRunActivity.waitingForTheModel')
      detail = t('chat:agentRunActivity.noNewProgressForCountS', {
        count: Math.floor(inactiveMs / 1000),
      })
    } else if (activity.stage === 'starting') title = t('chat:agentRunActivity.startingTheModel')
    else if (activity.stage === 'responding')
      title = t('chat:agentRunActivity.preparingTheResponse')
    else if (activity.stage === 'processing_result')
      title = t('chat:agentRunActivity.processingToolResult')
    else if (activity.stage === 'waiting_retry') title = t('chat:agentRunActivity.waitingToRetry')
    else if (activity.stage === 'finalizing') title = t('chat:agentRunActivity.finalizingTheRun')
    else if (activity.stage === 'working') title = t('chat:agentRunActivity.advancingTheTask')
    else
      title = String(thinkingText || '').trim()
        ? t('chat:agentRunActivity.reasoningAboutTheNextStep')
        : String(text || '').trim()
          ? t('chat:agentRunActivity.preparingTheResponse')
          : t('chat:agentRunActivity.understandingTheTask')
    if (String(thinkingText || '').trim()) detail = cleanInline(thinkingText)
  }

  return { tone, title, detail, output, command, startedAt, changes }
}

function ActivityIcon({ tone }: { tone: string }) {
  if (tone === 'failed') return <AlertTriangle size={14} />
  if (tone === 'stopped') return <Square size={12} />
  if (['completed', 'plan'].includes(tone)) return <Check size={14} />
  return <RefreshCw className="animate-spin" size={14} />
}

function ActivityElement({
  activity,
  className,
  children,
}: {
  activity: EntityRecord
  className: string
  children: ReactNode
}) {
  if (activity.type === 'tool') {
    return (
      <div className={className} data-pisper-activity-type="tool">
        {children}
      </div>
    )
  }
  if (activity.type === 'plan') {
    return (
      <Plan
        className={`${className} !gap-0 !rounded-none !border-0 !bg-transparent !p-0 !shadow-none`}
        data-pisper-activity-type="plan"
      >
        {children}
      </Plan>
    )
  }
  if (activity.type === 'agent') {
    return (
      <Task className={className} data-pisper-activity-type="agent">
        {children}
      </Task>
    )
  }
  return (
    <div className={className} data-pisper-activity-type={activity.type}>
      {children}
    </div>
  )
}

function CommandOutput({
  output,
  streaming,
  t,
}: {
  output: unknown
  streaming: boolean
  t: Translate
}) {
  const display = terminalDisplayOutput(output)
  const outputRef = useRef<HTMLPreElement>(null)

  useEffect(() => {
    if (!streaming) return undefined
    const frame = window.requestAnimationFrame(() => {
      const node = outputRef.current
      if (node) node.scrollTop = node.scrollHeight
    })
    return () => window.cancelAnimationFrame(frame)
  }, [display.text, streaming])

  if (!display.text.trim()) return null

  return (
    <details
      className="agent-run-command-output [.agent-run-feed:has(&[open])]:max-h-[320px] [&_>_summary]:grid [&_>_summary]:min-h-[30px] [&_>_summary]:grid-cols-[auto_minmax(0,1fr)_auto] [&_>_summary]:items-center [&_>_summary]:gap-[7px] [&_>_summary]:[list-style:none] [&_>_summary]:p-[5px_8px] [&_>_summary]:text-[var(--text-muted)] [&_>_summary]:text-[11px] [&_>_summary]:font-[600] [&_>_summary]:cursor-pointer [&_>_summary::-webkit-details-marker]:hidden [&_>_summary:hover]:bg-[var(--surface-hover)] [&_>_summary:hover]:text-[var(--text-secondary)] [&_>_summary:focus-visible]:[outline:2px_solid_var(--accent-border)] [&_>_summary:focus-visible]:[outline-offset:-2px] [&_>_summary_>_svg:first-child]:text-[var(--brand-blue-strong)] [&_>_pre]:max-h-[112px] [&_>_pre]:overflow-auto [&_>_pre]:m-0 [&_>_pre]:[border-top:1px_solid_var(--stroke-soft)] [&_>_pre]:bg-[var(--surface-subtle)] [&_>_pre]:p-[9px_10px] [&_>_pre]:text-[var(--text-secondary)] [&_>_pre]:font-[ui-monospace,SFMono-Regular,Consolas,'Liberation_Mono',monospace] [&_>_pre]:text-[11px] [&_>_pre]:leading-[1.55] [&_>_pre]:whitespace-pre-wrap [&_>_pre]:[overflow-wrap:anywhere] [.agent-run-activity.compact_&]:[grid-column:1/-1] min-w-0 [grid-column:2/-1] overflow-hidden [margin:2px_0_1px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--solid)]"
      data-truncated={display.truncated || undefined}
      open={streaming || undefined}
    >
      <summary>
        <Code2 size={13} />
        <span>
          {streaming
            ? t('chat:agentRunActivity.liveCommandOutput')
            : t('chat:agentRunActivity.commandOutput')}
        </span>
        <ChevronRight
          className="agent-run-disclosure [details[open]_>_summary_&]:[transform:rotate(90deg)] [transition:transform_var(--d1)_var(--ease-out)]"
          size={13}
        />
      </summary>
      <pre ref={outputRef}>{display.text}</pre>
    </details>
  )
}

type ActivityCardProps = {
  activity: EntityRecord
  latest: boolean
  t: Translate
  language: string
  now: number
  runStartedAt?: string | null
  streaming?: boolean
  text?: string
  thinkingText?: string
  compaction?: EntityRecord | null
  error?: string
  stopped?: boolean
  notice?: string
  lastActivityAt?: string | null
}

function activityHasLiveClock(activity: EntityRecord) {
  if (activity.type === 'plan') return !activity.updatedAt
  if (activity.type === 'tool')
    return (
      !activity.status ||
      activity.status === 'running' ||
      (!activity.finishedAt && !activity.updatedAt)
    )
  if (activity.type === 'agent') {
    const agent = activity.agent || {}
    return (
      !agent.status ||
      ['queued', 'starting', 'running'].includes(agent.status) ||
      (!agent.completedAt && !agent.lastActivityAt && !activity.updatedAt)
    )
  }
  return true
}

function activityCardPropsEqual(prev: ActivityCardProps, next: ActivityCardProps) {
  if (
    prev.activity !== next.activity ||
    prev.latest !== next.latest ||
    prev.t !== next.t ||
    prev.language !== next.language ||
    prev.runStartedAt !== next.runStartedAt ||
    prev.streaming !== next.streaming
  )
    return false
  if (prev.now !== next.now && activityHasLiveClock(next.activity)) return false
  if (['tool', 'plan', 'agent'].includes(next.activity.type)) return true
  return (
    prev.text === next.text &&
    prev.thinkingText === next.thinkingText &&
    prev.compaction === next.compaction &&
    prev.error === next.error &&
    prev.stopped === next.stopped &&
    prev.notice === next.notice &&
    prev.lastActivityAt === next.lastActivityAt
  )
}

const ActivityCard = memo(function ActivityCard({
  activity,
  latest,
  t,
  language,
  now,
  runStartedAt,
  streaming,
  text,
  thinkingText,
  compaction,
  error,
  stopped,
  notice,
  lastActivityAt,
}: ActivityCardProps) {
  const presentation = activityPresentation(activity, {
    t,
    streaming,
    text,
    thinkingText,
    compaction,
    error,
    stopped,
    notice,
    lastActivityAt,
    now,
  })
  const duration = formatRunDuration(activityDurationMs(activity, runStartedAt, now), language)
  const showCommandOutput = activity.name === 'bash' && latest && Boolean(activity.output)
  return (
    <ActivityElement
      activity={activity}
      className={`agent-run-summary grid w-full min-h-[42px] grid-cols-[28px_minmax(0,1fr)_auto] [align-items:start] gap-[9px] p-[6px_8px] hover:bg-[var(--surface-hover)] hover:opacity-100 [.agent-run-activity.compact_&]:min-h-[34px] [.agent-run-activity.compact_&]:grid-cols-[24px_minmax(0,1fr)_auto] [.agent-run-activity.compact_&]:gap-[7px] [.agent-run-activity.compact_&]:p-[4px_5px] @max-[700px]:grid-cols-[28px_minmax(0,1fr)_auto] @max-[700px]:[&_>_svg]:hidden flex-none [border:1px_solid_transparent] rounded-[var(--r-sm)] opacity-[.82] [transition:border-color_var(--d1)_var(--ease-out),_background_var(--d1)_var(--ease-out),_opacity_var(--d1)_var(--ease-out),_transform_var(--d1)_var(--ease-out)] ${presentation.tone}    ${latest ? 'current [.agent-run-summary&]:border-[var(--stroke-soft)] [.agent-run-summary&]:bg-[var(--surface-subtle)] [.agent-run-summary&]:opacity-100' : ''}`}
    >
      <span className="agent-run-status-icon [.agent-thinking-window.running_&]:text-[var(--brand-blue-strong)] [.agent-run-summary.completed_&]:bg-[var(--success-soft)] [.agent-run-summary.completed_&]:text-[var(--success)] [.agent-run-summary.plan_&]:bg-[var(--success-soft)] [.agent-run-summary.plan_&]:text-[var(--success)] [.agent-run-overview.completed_&]:bg-[var(--success-soft)] [.agent-run-overview.completed_&]:text-[var(--success)] [.agent-run-overview.plan_&]:bg-[var(--success-soft)] [.agent-run-overview.plan_&]:text-[var(--success)] [.agent-run-summary.compacting_&]:bg-[var(--warning-soft)] [.agent-run-summary.compacting_&]:text-[var(--star-strong)] [.agent-run-overview.compacting_&]:bg-[var(--warning-soft)] [.agent-run-overview.compacting_&]:text-[var(--star-strong)] [.agent-run-summary.failed_&]:bg-[var(--danger-soft)] [.agent-run-summary.failed_&]:text-[var(--danger)] [.agent-run-overview.failed_&]:bg-[var(--danger-soft)] [.agent-run-overview.failed_&]:text-[var(--danger)] [.agent-run-summary.stopped_&]:text-[var(--text-muted)] [.agent-run-overview.stopped_&]:text-[var(--text-muted)] [.agent-run-activity.compact_&]:w-[24px] [.agent-run-activity.compact_&]:h-[24px] grid w-[28px] h-[28px] place-items-center rounded-[var(--r-xs)] bg-[var(--blue-soft)] text-[var(--brand-blue-strong)]">
        <ActivityIcon tone={presentation.tone} />
      </span>
      <span className="agent-run-copy [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:font-[620] [&_strong]:leading-[1.4] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[12px] [&_small]:leading-[1.45] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [.agent-run-activity.compact_&_strong]:text-[12px] [.agent-run-activity.compact_&_small]:text-[11px] flex min-w-0 flex-col gap-[3px] [padding-top:1px]">
        <strong>{presentation.title}</strong>
        {presentation.detail &&
          (presentation.command ? (
            <code
              className="agent-run-command overflow-hidden text-[var(--text-muted)] text-[12px] leading-[1.45] text-ellipsis whitespace-nowrap block max-h-[2.9em] p-[3px_7px] rounded-[var(--r-xs)] bg-[var(--surface-subtle)] text-[var(--text-secondary)] font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] whitespace-pre-wrap [overflow-wrap:anywhere] [.agent-run-activity.compact_&]:text-[11px]"
              title={presentation.detail}
            >
              $ {presentation.detail}
            </code>
          ) : (
            <small title={presentation.detail}>{presentation.detail}</small>
          ))}
        {presentation.changes.length > 0 && (
          <span className="agent-run-plan-changes [&_small]:text-[var(--text-secondary)] flex min-w-0 flex-col gap-[1px] [padding:1px_0_0_7px] [border-left:1px_solid_var(--stroke-soft)]">
            {presentation.changes.slice(0, 4).map((change) => (
              <small key={`${change.id}-${change.kind}-${change.status}`}>
                {planChangeText(change, t)}
              </small>
            ))}
            {presentation.changes.length > 4 && (
              <small>
                {t('chat:agentRunActivity.countMoreChanges', {
                  count: presentation.changes.length - 4,
                })}
              </small>
            )}
          </span>
        )}
        {!showCommandOutput &&
          presentation.output &&
          presentation.output !== presentation.detail && (
            <small className="text-[var(--text-tertiary)]" title={presentation.output}>
              {presentation.output}
            </small>
          )}
      </span>
      <span className="agent-run-duration inline-flex items-center gap-[4px] pt-[3px] text-[var(--text-muted)] font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] text-[11px] whitespace-nowrap [.agent-run-activity.compact_&]:text-[10px]">
        <Clock3 size={12} />
        {duration}
      </span>
      {showCommandOutput && (
        <CommandOutput
          output={activity.output}
          streaming={Boolean(streaming && activity.status === 'running')}
          t={t}
        />
      )}
    </ActivityElement>
  )
}, activityCardPropsEqual)

function AgentRunActivity({
  streaming,
  text,
  thinkingText,
  currentActivity,
  activityFeed = EMPTY_LIST,
  tools = EMPTY_LIST,
  compaction,
  error,
  stopped,
  notice,
  startedAt,
  lastActivityAt,
  finishedAt,
  compact = false,
}: AgentRunActivityProps) {
  const { t, language } = useI18n()
  const now = useRunActivityClock(streaming)
  const thinking = String(thinkingText || '').trim()
  const thinkingScrollRef = useRef<HTMLDivElement>(null)
  const liveFeedRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!thinking) return undefined
    const frame = window.requestAnimationFrame(() => {
      const node = thinkingScrollRef.current
      if (node) node.scrollTop = node.scrollHeight
    })
    return () => window.cancelAnimationFrame(frame)
  }, [thinking])

  const activities = activityFeed.length ? activityFeed : tools
  const activityVersion = activityScrollVersion(activities)

  useEffect(() => {
    if (!streaming || !activities.length) return undefined
    const frame = window.requestAnimationFrame(() => {
      const node = liveFeedRef.current
      if (node) node.scrollTop = node.scrollHeight
    })
    return () => window.cancelAnimationFrame(frame)
  }, [activityVersion, activities.length, streaming])
  if (!streaming && !thinking && !activities.length) return null

  const primaryActivity = primaryRunActivity({
    currentActivity,
    compaction,
    text,
    thinkingText,
    lastActivityAt,
  })
  const primary = activityPresentation(primaryActivity, {
    t,
    streaming,
    text,
    thinkingText: thinking ? '' : thinkingText,
    compaction,
    error,
    stopped,
    notice,
    lastActivityAt,
    now,
  })
  if (!streaming && activities.length) {
    primary.title = t('chat:agentRunActivity.currentOperationCompleted')
    primary.detail = ''
  }
  const completedActivityCount = !streaming ? activities.length : 0
  const showOverview = Boolean(streaming || compaction?.active || error || stopped)
  const primaryDuration = formatRunDuration(runDurationMs(startedAt, finishedAt, now), language)
  const primaryDetail =
    primary.command && activities.length
      ? t('chat:agentRunActivity.countLiveOperations', { count: activities.length })
      : primary.detail
  const activityCards = activities.map((activity, index) => (
    <ActivityCard
      activity={activity}
      compaction={compaction}
      error={error}
      key={activityRenderKey(activity, index)}
      language={language}
      lastActivityAt={lastActivityAt}
      latest={index === activities.length - 1}
      notice={notice}
      now={now}
      runStartedAt={startedAt}
      stopped={stopped}
      streaming={streaming}
      t={t}
      text={text}
      thinkingText={thinking ? '' : thinkingText}
    />
  ))

  return (
    <section
      className={`agent-run-activity [&.compact]:m-[4px_0_0_40px] w-full overflow-hidden [margin:1px_0_9px] text-[var(--text)] ${compact ? 'compact' : ''}`}
      aria-live="polite"
    >
      {thinking && (
        <details
          className={`agent-thinking-window [&[open]]:border-[var(--stroke-soft)] [&.completed]:border-[transparent] [&.completed[open]]:border-[var(--stroke-soft)] overflow-hidden [margin:1px_0_8px] [border:1px_solid_transparent] rounded-[var(--r-sm)] bg-transparent [transition:border-color_var(--d1)_var(--ease-out)] ${streaming ? 'running' : 'completed'}`}
          data-pisper-activity-type="reasoning"
        >
          <summary
            className="agent-thinking-head [summary&]:[list-style:none] [summary&]:cursor-pointer [summary&::-webkit-details-marker]:hidden [summary&:hover]:bg-[var(--surface-hover)] [summary&:focus-visible]:[outline:2px_solid_var(--accent-border)] [summary&:focus-visible]:[outline-offset:-2px] grid min-h-[40px] grid-cols-[28px_minmax(0,1fr)_auto] items-center gap-[9px] [padding:6px_8px]"
            aria-label={t('chat:agentRunActivity.toggleReasoning')}
          >
            <span className="agent-run-status-icon [.agent-thinking-window.running_&]:text-[var(--brand-blue-strong)] [.agent-run-summary.completed_&]:bg-[var(--success-soft)] [.agent-run-summary.completed_&]:text-[var(--success)] [.agent-run-summary.plan_&]:bg-[var(--success-soft)] [.agent-run-summary.plan_&]:text-[var(--success)] [.agent-run-overview.completed_&]:bg-[var(--success-soft)] [.agent-run-overview.completed_&]:text-[var(--success)] [.agent-run-overview.plan_&]:bg-[var(--success-soft)] [.agent-run-overview.plan_&]:text-[var(--success)] [.agent-run-summary.compacting_&]:bg-[var(--warning-soft)] [.agent-run-summary.compacting_&]:text-[var(--star-strong)] [.agent-run-overview.compacting_&]:bg-[var(--warning-soft)] [.agent-run-overview.compacting_&]:text-[var(--star-strong)] [.agent-run-summary.failed_&]:bg-[var(--danger-soft)] [.agent-run-summary.failed_&]:text-[var(--danger)] [.agent-run-overview.failed_&]:bg-[var(--danger-soft)] [.agent-run-overview.failed_&]:text-[var(--danger)] [.agent-run-summary.stopped_&]:text-[var(--text-muted)] [.agent-run-overview.stopped_&]:text-[var(--text-muted)] [.agent-run-activity.compact_&]:w-[24px] [.agent-run-activity.compact_&]:h-[24px] grid w-[28px] h-[28px] place-items-center rounded-[var(--r-xs)] bg-[var(--blue-soft)] text-[var(--brand-blue-strong)]">
              <ChevronRight
                className="agent-run-disclosure [details[open]_>_summary_&]:[transform:rotate(90deg)] [transition:transform_var(--d1)_var(--ease-out)]"
                size={14}
              />
            </span>
            <span className="agent-run-copy [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:font-[620] [&_strong]:leading-[1.4] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[12px] [&_small]:leading-[1.45] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [.agent-run-activity.compact_&_strong]:text-[12px] [.agent-run-activity.compact_&_small]:text-[11px] flex min-w-0 flex-col gap-[3px] [padding-top:1px]">
              <strong>
                {streaming
                  ? t('chat:agentRunActivity.reasoningInProgress')
                  : t('chat:agentRunActivity.reasoningCompleted')}
                {streaming ? (
                  <span
                    className="agent-thinking-dots [&_i]:w-[3px] [&_i]:h-[3px] [&_i]:rounded-[50%] [&_i]:bg-[currentColor] [&_i]:[animation:agent-thinking-dot_1.15s_ease-in-out_infinite] [&_i:nth-child(2)]:[animation-delay:.14s] [&_i:nth-child(3)]:[animation-delay:.28s] inline-flex items-end gap-[2px] [margin-left:5px]"
                    aria-hidden="true"
                  >
                    <i />
                    <i />
                    <i />
                  </span>
                ) : null}
              </strong>
            </span>
            <span className="agent-run-duration inline-flex items-center gap-[4px] pt-[3px] text-[var(--text-muted)] font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] text-[11px] whitespace-nowrap [.agent-run-activity.compact_&]:text-[10px]">
              <Clock3 size={12} />
              {primaryDuration}
            </span>
          </summary>
          <div
            ref={thinkingScrollRef}
            className="agent-thinking-scroll max-h-[190px] overflow-auto [border-top:1px_solid_var(--stroke-soft)] [padding:9px_12px] [scroll-behavior:smooth]"
          >
            <MarkdownMessage streaming={streaming}>{thinking}</MarkdownMessage>
          </div>
        </details>
      )}
      {showOverview && (
        <div
          className={`agent-run-overview grid w-full min-h-[42px] grid-cols-[28px_minmax(0,1fr)_auto] [align-items:start] gap-[9px] p-[6px_8px] [.agent-run-activity.compact_&]:min-h-[34px] [.agent-run-activity.compact_&]:grid-cols-[24px_minmax(0,1fr)_auto] [.agent-run-activity.compact_&]:gap-[7px] [.agent-run-activity.compact_&]:p-[4px_5px] ${primary.tone}`}
          data-pisper-activity-type="status"
        >
          <span className="agent-run-status-icon [.agent-thinking-window.running_&]:text-[var(--brand-blue-strong)] [.agent-run-summary.completed_&]:bg-[var(--success-soft)] [.agent-run-summary.completed_&]:text-[var(--success)] [.agent-run-summary.plan_&]:bg-[var(--success-soft)] [.agent-run-summary.plan_&]:text-[var(--success)] [.agent-run-overview.completed_&]:bg-[var(--success-soft)] [.agent-run-overview.completed_&]:text-[var(--success)] [.agent-run-overview.plan_&]:bg-[var(--success-soft)] [.agent-run-overview.plan_&]:text-[var(--success)] [.agent-run-summary.compacting_&]:bg-[var(--warning-soft)] [.agent-run-summary.compacting_&]:text-[var(--star-strong)] [.agent-run-overview.compacting_&]:bg-[var(--warning-soft)] [.agent-run-overview.compacting_&]:text-[var(--star-strong)] [.agent-run-summary.failed_&]:bg-[var(--danger-soft)] [.agent-run-summary.failed_&]:text-[var(--danger)] [.agent-run-overview.failed_&]:bg-[var(--danger-soft)] [.agent-run-overview.failed_&]:text-[var(--danger)] [.agent-run-summary.stopped_&]:text-[var(--text-muted)] [.agent-run-overview.stopped_&]:text-[var(--text-muted)] [.agent-run-activity.compact_&]:w-[24px] [.agent-run-activity.compact_&]:h-[24px] grid w-[28px] h-[28px] place-items-center rounded-[var(--r-xs)] bg-[var(--blue-soft)] text-[var(--brand-blue-strong)]">
            <ActivityIcon tone={primary.tone} />
          </span>
          <span className="agent-run-copy [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:font-[620] [&_strong]:leading-[1.4] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[12px] [&_small]:leading-[1.45] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [.agent-run-activity.compact_&_strong]:text-[12px] [.agent-run-activity.compact_&_small]:text-[11px] flex min-w-0 flex-col gap-[3px] [padding-top:1px]">
            <strong>
              {streaming && ['running', 'waiting', 'compacting'].includes(primary.tone) ? (
                <Suspense fallback={primary.title}>
                  <ShinyText>{primary.title}</ShinyText>
                </Suspense>
              ) : (
                primary.title
              )}
            </strong>
            {primaryDetail && <small title={primaryDetail}>{primaryDetail}</small>}
          </span>
          <span className="agent-run-duration inline-flex items-center gap-[4px] pt-[3px] text-[var(--text-muted)] font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] text-[11px] whitespace-nowrap [.agent-run-activity.compact_&]:text-[10px]">
            <Clock3 size={12} />
            {primaryDuration}
          </span>
        </div>
      )}
      {streaming && activities.length > 0 && (
        <div
          ref={liveFeedRef}
          className="agent-run-feed focus-visible:[outline:2px_solid_var(--accent-border)] focus-visible:[outline-offset:2px] [.agent-run-history_&.completed]:mt-[3px] flex max-h-[184px] flex-col gap-[4px] overflow-y-auto [overscroll-behavior:contain] [margin:5px_0_2px] [padding:2px_4px_2px_0] [scrollbar-gutter:stable] live"
          aria-label={t('chat:agentRunActivity.toolActivityAriaLabel')}
          tabIndex={activities.length > 3 ? 0 : undefined}
        >
          <Suspense fallback={activityCards}>
            <AnimatedList>{activityCards}</AnimatedList>
          </Suspense>
        </div>
      )}
      {completedActivityCount > 0 && (
        <details className="agent-run-history [&_>_summary]:grid [&_>_summary]:min-h-[38px] [&_>_summary]:grid-cols-[28px_minmax(0,1fr)_auto] [&_>_summary]:items-center [&_>_summary]:gap-[9px] [&_>_summary]:[list-style:none] [&_>_summary]:rounded-[var(--r-sm)] [&_>_summary]:p-[5px_8px] [&_>_summary]:text-[var(--text-muted)] [&_>_summary]:cursor-pointer [&_>_summary::-webkit-details-marker]:hidden [&_>_summary:hover]:bg-[var(--surface-hover)] [&_>_summary:hover]:text-[var(--text)] [&_>_summary:focus-visible]:[outline:2px_solid_var(--accent-border)] [&_>_summary:focus-visible]:[outline-offset:-2px] w-full">
          <summary>
            <span className="agent-run-status-icon [.agent-thinking-window.running_&]:text-[var(--brand-blue-strong)] [.agent-run-summary.completed_&]:bg-[var(--success-soft)] [.agent-run-summary.completed_&]:text-[var(--success)] [.agent-run-summary.plan_&]:bg-[var(--success-soft)] [.agent-run-summary.plan_&]:text-[var(--success)] [.agent-run-overview.completed_&]:bg-[var(--success-soft)] [.agent-run-overview.completed_&]:text-[var(--success)] [.agent-run-overview.plan_&]:bg-[var(--success-soft)] [.agent-run-overview.plan_&]:text-[var(--success)] [.agent-run-summary.compacting_&]:bg-[var(--warning-soft)] [.agent-run-summary.compacting_&]:text-[var(--star-strong)] [.agent-run-overview.compacting_&]:bg-[var(--warning-soft)] [.agent-run-overview.compacting_&]:text-[var(--star-strong)] [.agent-run-summary.failed_&]:bg-[var(--danger-soft)] [.agent-run-summary.failed_&]:text-[var(--danger)] [.agent-run-overview.failed_&]:bg-[var(--danger-soft)] [.agent-run-overview.failed_&]:text-[var(--danger)] [.agent-run-summary.stopped_&]:text-[var(--text-muted)] [.agent-run-overview.stopped_&]:text-[var(--text-muted)] [.agent-run-activity.compact_&]:w-[24px] [.agent-run-activity.compact_&]:h-[24px] grid w-[28px] h-[28px] place-items-center rounded-[var(--r-xs)] bg-[var(--blue-soft)] text-[var(--brand-blue-strong)]">
              <ChevronRight
                className="agent-run-disclosure [details[open]_>_summary_&]:[transform:rotate(90deg)] [transition:transform_var(--d1)_var(--ease-out)]"
                size={14}
              />
            </span>
            <span className="agent-run-copy [&_strong]:overflow-hidden [&_strong]:text-[13px] [&_strong]:font-[620] [&_strong]:leading-[1.4] [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[12px] [&_small]:leading-[1.45] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [.agent-run-activity.compact_&_strong]:text-[12px] [.agent-run-activity.compact_&_small]:text-[11px] flex min-w-0 flex-col gap-[3px] [padding-top:1px]">
              <strong>
                {t('chat:agentRunActivity.countCompletedOperations', {
                  count: completedActivityCount,
                })}
              </strong>
            </span>
            <span className="agent-run-duration inline-flex items-center gap-[4px] pt-[3px] text-[var(--text-muted)] font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] text-[11px] whitespace-nowrap [.agent-run-activity.compact_&]:text-[10px]">
              <Clock3 size={12} />
              {primaryDuration}
            </span>
          </summary>
          <div
            className="agent-run-feed focus-visible:[outline:2px_solid_var(--accent-border)] focus-visible:[outline-offset:2px] [.agent-run-history_&.completed]:mt-[3px] flex max-h-[184px] flex-col gap-[4px] overflow-y-auto [overscroll-behavior:contain] [margin:5px_0_2px] [padding:2px_4px_2px_0] [scrollbar-gutter:stable] completed"
            aria-label={t('chat:agentRunActivity.toolActivityAriaLabel')}
            tabIndex={activities.length > 3 ? 0 : undefined}
          >
            <Suspense fallback={activityCards}>{activityCards}</Suspense>
          </div>
        </details>
      )}
    </section>
  )
}

function agentRunActivityPropsEqual(prev: AgentRunActivityProps, next: AgentRunActivityProps) {
  return (
    prev.streaming === next.streaming &&
    prev.text === next.text &&
    prev.thinkingText === next.thinkingText &&
    prev.currentActivity === next.currentActivity &&
    prev.activityFeed === next.activityFeed &&
    prev.compaction === next.compaction &&
    prev.error === next.error &&
    prev.stopped === next.stopped &&
    prev.notice === next.notice &&
    prev.startedAt === next.startedAt &&
    prev.lastActivityAt === next.lastActivityAt &&
    prev.finishedAt === next.finishedAt &&
    prev.compact === next.compact &&
    prev.tools === next.tools
  )
}

export default memo(AgentRunActivity, agentRunActivityPropsEqual)
