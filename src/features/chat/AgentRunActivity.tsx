import { lazy, memo, Suspense, useEffect, useState, type ReactNode } from 'react'
import { AlertTriangle, Check, Clock3, RefreshCw, Square } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Plan } from '@/components/ai-elements/plan'
import { Reasoning } from '@/components/ai-elements/reasoning'
import { Task } from '@/components/ai-elements/task'
import { Tool } from '@/components/ai-elements/tool'
import { formatTokenCount } from '@/lib/format'
import type { I18nValues } from '@/app/i18n'
import type { EntityRecord } from '@/types/chat'
import {
  activityDurationMs,
  activityRenderKey,
  agentActivityState,
  formatRunDuration,
  primaryRunActivity,
  runDurationMs,
} from './run-activity'

const EMPTY_LIST: EntityRecord[] = []
const Terminal = lazy(() =>
  import('@/components/ai-elements/terminal').then((module) => ({ default: module.Terminal })),
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
  if (name === 'get_task_list') return t('chat:agentRunActivity.readingPlan')
  if (name === 'update_task_list') return t('chat:agentRunActivity.updatingPlan')
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
  if (name === 'update_task_list') return t('chat:agentRunActivity.planUpdated')
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

function planProgress(taskList: EntityRecord | null | undefined, t: Translate) {
  const items = taskList?.items || []
  const completed =
    taskList?.counts?.completed ??
    items.filter((item: EntityRecord) => item.status === 'completed').length
  const active =
    taskList?.counts?.inProgress ??
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
    title = t('chat:agentRunActivity.planUpdated')
    changes = activity.changes || EMPTY_LIST
    detail = changes.length
      ? t('chat:agentRunActivity.countPlanChanges', { count: changes.length })
      : planProgress(activity.taskList, t)
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
    } else if (activity.stage === 'responding')
      title = t('chat:agentRunActivity.preparingTheResponse')
    else if (activity.stage === 'processing_result')
      title = t('chat:agentRunActivity.processingToolResult')
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
  return <RefreshCw className="spin" size={14} />
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
      <Tool
        className={`${className} !mb-0 !rounded-none !border-0`}
        data-vesper-activity-type="tool"
      >
        {children}
      </Tool>
    )
  }
  if (activity.type === 'plan') {
    return (
      <Plan
        className={`${className} !gap-0 !rounded-none !border-0 !bg-transparent !p-0 !shadow-none`}
        data-vesper-activity-type="plan"
      >
        {children}
      </Plan>
    )
  }
  if (activity.type === 'agent') {
    return (
      <Task className={className} data-vesper-activity-type="agent">
        {children}
      </Task>
    )
  }
  return (
    <div className={className} data-vesper-activity-type={activity.type}>
      {children}
    </div>
  )
}

function AgentRunActivity({
  streaming,
  text,
  thinkingText,
  currentActivity,
  activityFeed = EMPTY_LIST,
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
  if (!streaming && !String(thinkingText || '').trim()) return null

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
    thinkingText,
    compaction,
    error,
    stopped,
    notice,
    lastActivityAt,
    now,
  })
  const primaryDuration = formatRunDuration(runDurationMs(startedAt, finishedAt, now), language)
  const primaryDetail =
    primary.command && activityFeed.length
      ? t('chat:agentRunActivity.countLiveOperations', { count: activityFeed.length })
      : primary.detail

  return (
    <section className={`agent-run-activity ${compact ? 'compact' : ''}`} aria-live="polite">
      <Reasoning
        className={`agent-run-overview ${primary.tone} !mb-0`}
        isStreaming={streaming}
        open
        data-vesper-activity-type="reasoning"
      >
        <span className="agent-run-status-icon">
          <ActivityIcon tone={primary.tone} />
        </span>
        <span className="agent-run-copy">
          <strong>{primary.title}</strong>
          {primaryDetail && <small title={primaryDetail}>{primaryDetail}</small>}
        </span>
        <span className="agent-run-duration">
          <Clock3 size={12} />
          {primaryDuration}
        </span>
      </Reasoning>
      {activityFeed.length > 0 && (
        <div className="agent-run-feed">
          {activityFeed.map((activity, index) => {
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
            const duration = formatRunDuration(
              activityDurationMs(activity, startedAt, now),
              language,
            )
            const key = activityRenderKey(activity, index)
            const showTerminal = activity.name === 'bash' && index === activityFeed.length - 1
            return (
              <ActivityElement
                activity={activity}
                className={`agent-run-summary ${presentation.tone} ${index === activityFeed.length - 1 ? 'current' : ''}`}
                key={key}
              >
                <span className="agent-run-status-icon">
                  <ActivityIcon tone={presentation.tone} />
                </span>
                <span className="agent-run-copy">
                  <strong>{presentation.title}</strong>
                  {presentation.detail &&
                    (presentation.command ? (
                      <code className="agent-run-command" title={presentation.detail}>
                        $ {presentation.detail}
                      </code>
                    ) : (
                      <small title={presentation.detail}>{presentation.detail}</small>
                    ))}
                  {presentation.changes.length > 0 && (
                    <span className="agent-run-plan-changes">
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
                  {!showTerminal &&
                    presentation.output &&
                    presentation.output !== presentation.detail && (
                      <small className="agent-run-output" title={presentation.output}>
                        {presentation.output}
                      </small>
                    )}
                </span>
                <span className="agent-run-duration">
                  <Clock3 size={12} />
                  {duration}
                </span>
                {showTerminal && (
                  <Suspense
                    fallback={
                      <pre className="agent-run-terminal agent-run-terminal-fallback">
                        {String(activity.output || '')}
                      </pre>
                    }
                  >
                    <Terminal
                      autoScroll
                      className="agent-run-terminal"
                      isStreaming={Boolean(streaming && activity.status === 'running')}
                      output={String(activity.output || '')}
                    />
                  </Suspense>
                )}
              </ActivityElement>
            )
          })}
        </div>
      )}
    </section>
  )
}

export default memo(AgentRunActivity)
