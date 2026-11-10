// 时间线里的一件活（Bot 派出的子 agent）：一行写它在做什么、到哪一步、用了多久，在跑时可以停；
// 展开看交代、模型和最近的输出；“查看过程”在检查器里只读看它的完整过程。结果由 Bot 在对话里用一句话汇报，这里不重复贴整段。
import { useContext, useEffect, useState } from 'react'
import { Check, ChevronDown, ChevronRight, Square, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
import {
  SessionSubagentsContext,
  isActiveAgent,
  stopSessionAgent,
  type SessionAgent,
} from './session-agents'

function duration(from: string, to: string | null, now: number) {
  const start = Date.parse(from)
  const end = to ? Date.parse(to) : now
  if (!Number.isFinite(start) || !Number.isFinite(end)) return ''
  const seconds = Math.max(0, Math.round((end - start) / 1000))
  const minutes = Math.floor(seconds / 60)
  return minutes ? `${minutes}:${String(seconds % 60).padStart(2, '0')}` : `${seconds}s`
}

export function AgentJobEvent({
  sessionId,
  agent,
  onChanged,
}: {
  sessionId: string
  agent: SessionAgent
  onChanged: () => void
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [stopping, setStopping] = useState(false)
  const [now, setNow] = useState(() => Date.now())
  const subagents = useContext(SessionSubagentsContext)
  const active = isActiveAgent(agent)
  // 在跑的那件每秒走一下时间；结束了就不再刷新。
  useEffect(() => {
    if (!active) return undefined
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [active])
  const status =
    agent.status === 'completed'
      ? t('chat:agentJob.completed')
      : agent.status === 'failed'
        ? t('chat:agentJob.failed')
        : agent.status === 'interrupted'
          ? t('chat:agentJob.interrupted')
          : agent.status === 'running'
            ? t('chat:agentJob.running')
            : t('chat:agentJob.queued')
  const elapsed = duration(agent.startedAt, agent.completedAt, now)
  const detailsId = `agent-job-${agent.id}`
  const stop = async () => {
    setStopping(true)
    try {
      await stopSessionAgent(sessionId, agent.id)
    } catch {
      // 已经结束了也算停下：下一次刷新会显示最终状态。
    } finally {
      setStopping(false)
      onChanged()
    }
  }
  return (
    <div
      data-agent-job={agent.id}
      data-agent-job-status={agent.status}
      className="mx-auto mb-[var(--chat-message-gap,32px)] w-full max-w-[var(--chat-content-width,1040px)] min-w-0 rounded-lg border border-stroke-soft bg-surface-subtle text-[13px]"
    >
      <div className="flex min-h-10 items-center gap-2 px-3 py-1.5">
        <span aria-hidden="true" className="grid size-4 flex-none place-items-center">
          {active ? (
            <OrbitSpinner className="size-3.5" />
          ) : agent.status === 'completed' ? (
            <Check size={14} className="text-st-done" />
          ) : (
            <X size={14} className="text-content-muted" />
          )}
        </span>
        <span className="min-w-0 flex-1 truncate">
          <span className="font-medium text-content">
            {agent.kind === 'command'
              ? t('chat:agentJob.command', { command: agent.taskName })
              : t('chat:agentJob.title', {
                  task:
                    agent.kind === 'proactive' ? t('chat:agentJob.proactiveCheck') : agent.taskName,
                })}
          </span>
          <span className="text-content-muted">
            {' · '}
            {status}
            {active && agent.activity ? ` · ${agent.activity}` : ''}
          </span>
        </span>
        {elapsed && (
          <span className="flex-none font-mono text-xs text-content-muted tabular-nums">
            {elapsed}
          </span>
        )}
        {active && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="flex-none pointer-coarse:h-11"
            disabled={stopping}
            onClick={() => void stop()}
          >
            <Square aria-hidden="true" />
            {t('chat:agentJob.stop')}
          </Button>
        )}
        {subagents && agent.kind !== 'command' && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="flex-none text-content-secondary pointer-coarse:h-11"
            aria-label={t('board:subagent.open', { name: agent.taskName })}
            onClick={() => subagents.open(agent.id)}
          >
            {t('board:subagent.view')}
            <ChevronRight aria-hidden="true" data-icon="inline-end" />
          </Button>
        )}
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="flex-none pointer-coarse:size-11"
          aria-expanded={open}
          aria-controls={detailsId}
          aria-label={open ? t('chat:agentJob.collapse') : t('chat:agentJob.expand')}
          title={open ? t('chat:agentJob.collapse') : t('chat:agentJob.expand')}
          onClick={() => setOpen((value) => !value)}
        >
          <ChevronDown
            aria-hidden="true"
            className={cn('transition-transform', open && 'rotate-180')}
          />
        </Button>
      </div>
      {open && (
        <div id={detailsId} className="flex flex-col gap-2 border-t border-stroke-soft px-3 py-2.5">
          <p className="m-0 text-xs text-content-muted">
            {[
              agent.model ? t('chat:agentJob.model', { model: agent.model }) : '',
              agent.toolCallCount ? t('chat:agentJob.tools', { count: agent.toolCallCount }) : '',
            ]
              .filter(Boolean)
              .join(' · ')}
          </p>
          <p className="m-0 line-clamp-4 whitespace-pre-wrap text-content-secondary">
            {agent.message}
          </p>
          {(agent.output || agent.error) && (
            <p className="m-0 line-clamp-6 whitespace-pre-wrap text-content">
              {agent.error || agent.output}
            </p>
          )}
        </div>
      )}
    </div>
  )
}
