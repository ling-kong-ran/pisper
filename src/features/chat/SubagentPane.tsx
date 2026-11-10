// 检查器的“子代理”标签：只读看会话派出的一个子代理的完整过程。
// 顶部切换本会话的子代理、状态与用时、停止；下面是主代理交给它的任务（默认两行，可展开）；
// 正文复用会话转录，和普通会话一样显示它说的话、调的工具和命令输出，没有输入框也没有重试。
// 在跑时 2.5 秒拉一次，结束后停；子代理在保存过程之前派出时只给出它交回的结果。
import { useCallback, useEffect, useMemo, useState } from 'react'
import { Bot, ChevronDown, CornerDownLeft, Square } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState } from '@/components/app/PageState'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { Button } from '@/components/ui/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { cn } from '@/lib/utils'
import { ChatRequestNotice } from './ChatRequestNotice'
import { chatErrorMessage } from './chat-errors'
import { FocusTranscript } from './FocusTranscript'
import {
  fetchAgentTranscript,
  isActiveAgent,
  stopSessionAgent,
  useSessionAgents,
  type AgentTranscript,
  type SessionAgent,
} from './session-agents'
import { TranscriptMetaContext } from './session-workspace-context'
import {
  SUBAGENT_STATUS_GLYPH,
  subagentActivityText,
  subagentElapsed,
  subagentStatusText,
} from './subagent-presentation'
import { useSubagentViewStore } from './subagent-view-store'

const ACTIVE_POLL_MS = 2_500

function AgentGlyph({ agent }: { agent: SessionAgent }) {
  const glyph = SUBAGENT_STATUS_GLYPH[agent.status]
  return (
    <StatusGlyph
      status={glyph.kind}
      spin={glyph.kind === 'run'}
      className={cn('size-[15px] flex-none', glyph.color)}
    />
  )
}

function useNow(active: boolean) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!active) return undefined
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [active])
  return now
}

function clockTime(value: string | null, language: string) {
  const time = value ? Date.parse(value) : Number.NaN
  return Number.isFinite(time)
    ? new Date(time).toLocaleTimeString(language, { hour: '2-digit', minute: '2-digit' })
    : ''
}

export function SubagentPane({ sessionId, sheet = false }: { sessionId: string; sheet?: boolean }) {
  const { t, language } = useI18n()
  const selectedId = useSubagentViewStore((state) => state.selected[sessionId] ?? '')
  const select = useSubagentViewStore((state) => state.select)
  const { agents: all, reload } = useSessionAgents(sessionId, true)
  // 后台命令没有对话可看，不在切换列表里。
  const agents = useMemo(() => all.filter((agent) => agent.kind !== 'command'), [all])
  const agent = agents.find((item) => item.id === selectedId)
  const active = Boolean(agent && isActiveAgent(agent))
  const now = useNow(active)
  const [transcript, setTranscript] = useState<AgentTranscript | null>(null)
  const [error, setError] = useState('')
  const [taskOpen, setTaskOpen] = useState(false)
  const [stopping, setStopping] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const agentId = agent?.id ?? ''
  const status = agent?.status ?? ''

  useEffect(() => {
    setTranscript(null)
    setError('')
    setTaskOpen(false)
  }, [agentId])

  // 子代理的状态变了（比如刚跑完）也重拉一次，拿到最后一段回复。
  useEffect(() => {
    if (!agentId) return undefined
    const controller = new AbortController()
    let timer = 0
    const load = async () => {
      try {
        const next = await fetchAgentTranscript(sessionId, agentId, controller.signal)
        if (controller.signal.aborted) return
        setTranscript(next)
        setError('')
        if (next.active) timer = window.setTimeout(load, ACTIVE_POLL_MS)
      } catch (cause) {
        if (!controller.signal.aborted) setError(chatErrorMessage(cause))
      }
    }
    void load()
    return () => {
      controller.abort()
      window.clearTimeout(timer)
    }
  }, [agentId, attempt, sessionId, status])

  const stop = useCallback(async () => {
    if (!agentId) return
    setStopping(true)
    try {
      await stopSessionAgent(sessionId, agentId)
    } catch {
      // 已经结束了也算停下：下一次刷新显示最终状态。
    } finally {
      setStopping(false)
      reload()
    }
  }, [agentId, reload, sessionId])

  const meta = useMemo(() => ({ model: '', name: agent?.taskName ?? '' }), [agent?.taskName])
  const messages = useMemo(
    () => (transcript?.taskShown ? transcript.messages.slice(1) : (transcript?.messages ?? [])),
    [transcript],
  )

  if (!agent)
    return (
      <InlineEmptyState
        compact
        icon={Bot}
        title={agents.length ? t('board:subagent.pick') : t('board:subagent.empty')}
      />
    )

  const task = transcript?.task || agent.message
  const details = [
    agent.model.split('/').pop(),
    agent.toolCallCount ? t('board:subagent.toolCalls', { count: agent.toolCallCount }) : '',
    t('board:subagent.startedAt', { time: clockTime(agent.startedAt, language) }),
  ]
    .filter(Boolean)
    .join(' · ')

  let body
  if (error)
    body = (
      <div className="p-3">
        <ChatRequestNotice error={error} onRetry={() => setAttempt((value) => value + 1)} />
      </div>
    )
  else if (!transcript)
    body = (
      <p role="status" className="p-4 text-sm text-content-muted">
        {t('board:subagent.loading')}
      </p>
    )
  else if (!transcript.available)
    // 保存过程之前派出的子代理：只有它交回的结果。
    body = (
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-3">
        <InlineEmptyState compact icon={Bot} title={t('board:subagent.notRecorded')} />
        {(agent.output || agent.error) && (
          <p className="m-0 text-[13px] leading-relaxed whitespace-pre-wrap text-content">
            {agent.error || agent.output}
          </p>
        )}
      </div>
    )
  else
    body = (
      <TranscriptMetaContext.Provider value={meta}>
        {transcript.omitted > 0 && (
          <p className="m-0 shrink-0 px-3 pt-2 text-xs text-content-muted">
            {t('board:subagent.omitted', { count: transcript.omitted })}
          </p>
        )}
        <FocusTranscript
          sessionId={`${sessionId}:agent:${agent.id}`}
          messages={messages}
          transcriptLoadState="ready"
          emptyState={
            <InlineEmptyState compact icon={Bot} title={t('board:subagent.noMessages')} />
          }
          activityFeed={[]}
          tools={[]}
          streaming={false}
          readOnly
          onRetryLastTurn={() => {}}
        />
      </TranscriptMetaContext.Provider>
    )

  return (
    <section
      aria-label={t('board:subagent.label', { name: agent.taskName })}
      data-subagent-pane={agent.id}
      className="flex h-full min-h-0 min-w-0 flex-1 flex-col [container-type:inline-size]"
    >
      <div className="flex shrink-0 flex-col gap-1.5 border-b border-stroke-soft px-3 pt-2.5 pb-2">
        <div className="flex min-w-0 items-center gap-2">
          <Select value={agent.id} onValueChange={(id) => select(sessionId, id)}>
            <SelectTrigger
              aria-label={t('board:subagent.switch')}
              className="min-w-0 flex-1 font-mono text-[13px] font-semibold"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="start">
              {agents.map((item) => (
                <SelectItem key={item.id} value={item.id}>
                  <AgentGlyph agent={item} />
                  <span className="truncate font-mono text-[13px]">{item.taskName}</span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <span className="flex-none text-xs text-content-muted tabular-nums">
            {subagentStatusText(agent.status, t)} ·{' '}
            {subagentElapsed(agent.startedAt, agent.completedAt, now)}
          </span>
          {active && (
            <Button
              type="button"
              variant="secondary"
              size="sm"
              className="flex-none"
              disabled={stopping}
              onClick={() => void stop()}
            >
              <Square aria-hidden="true" />
              {t('board:subagent.stop')}
            </Button>
          )}
        </div>
        <p className="m-0 truncate px-1 text-xs text-content-muted">
          {active ? `${subagentActivityText(agent, t)} · ${details}` : details}
        </p>
        {task && (
          <div className="mt-1 rounded-md border border-stroke-soft bg-surface-subtle">
            <button
              type="button"
              aria-expanded={taskOpen}
              className="flex w-full min-w-0 items-center gap-2 rounded-md px-2.5 pt-1.5 text-left text-xs font-medium text-content-secondary focus-visible:outline-2 focus-visible:outline-focus pointer-coarse:min-h-11"
              onClick={() => setTaskOpen((value) => !value)}
            >
              <span className="flex-1">{t('board:subagent.task')}</span>
              <ChevronDown
                aria-hidden="true"
                className={cn('size-3.5 transition-transform', taskOpen && 'rotate-180')}
              />
            </button>
            <p
              className={cn(
                'm-0 max-h-60 overflow-auto px-2.5 pt-0.5 pb-2 text-[13px] leading-relaxed whitespace-pre-wrap text-content',
                !taskOpen && 'line-clamp-2',
              )}
            >
              {task}
            </p>
          </div>
        )}
      </div>
      {body}
      <p
        className={cn(
          'm-0 flex shrink-0 items-center gap-1.5 border-t border-stroke-soft px-3 text-xs text-content-muted',
          sheet ? 'min-h-11' : 'min-h-9',
        )}
      >
        <CornerDownLeft aria-hidden="true" className="size-3.5 flex-none" />
        {active
          ? t('board:subagent.footerRunning')
          : agent.status === 'completed'
            ? t('board:subagent.footerReturned', {
                time: clockTime(agent.completedAt, language),
              })
            : t('board:subagent.footerEnded')}
      </p>
    </section>
  )
}
