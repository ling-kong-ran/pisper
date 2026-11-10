// 子代理在界面上的状态写法：工具组的子代理行、运行中弹层与检查器的“子代理”标签共用。
import type { StatusGlyphKind } from '@/components/app/status-glyph-kinds'
import type { SessionAgent, SessionAgentStatus } from './session-agents'

type Translate = (key: string, values?: Record<string, unknown>) => string

export const SUBAGENT_STATUS_GLYPH: Record<
  SessionAgentStatus,
  { kind: StatusGlyphKind; color: string }
> = {
  queued: { kind: 'queued', color: 'text-st-queue' },
  starting: { kind: 'run', color: 'text-st-run' },
  running: { kind: 'run', color: 'text-st-run' },
  completed: { kind: 'done', color: 'text-st-done' },
  failed: { kind: 'failed', color: 'text-st-fail' },
  interrupted: { kind: 'stopped', color: 'text-st-queue' },
}

export function subagentStatusText(status: SessionAgentStatus, t: Translate) {
  if (status === 'queued') return t('board:subagent.status.queued')
  if (status === 'starting') return t('board:subagent.status.starting')
  if (status === 'running') return t('board:subagent.status.running')
  if (status === 'completed') return t('board:subagent.status.completed')
  if (status === 'failed') return t('board:subagent.status.failed')
  return t('board:subagent.status.interrupted')
}

// 子代理此刻在做的事：Runtime 给的是阶段名（thinking 等）或正在用的工具名。
export function subagentActivityText(agent: Pick<SessionAgent, 'activity'>, t: Translate) {
  const activity = agent.activity.trim()
  if (!activity || activity === 'thinking' || activity === 'queue')
    return t('board:subagent.activity.thinking')
  if (activity === 'processing_result') return t('board:subagent.activity.reading')
  return activity
}

// 用了多久：在跑的按 now 走，结束的按完成时间；“1:05”或“42 s”。
export function subagentElapsed(from: string, to: string | null, now: number) {
  const start = Date.parse(from)
  const end = to ? Date.parse(to) : now
  if (!Number.isFinite(start) || !Number.isFinite(end)) return ''
  const seconds = Math.max(0, Math.round((end - start) / 1000))
  const minutes = Math.floor(seconds / 60)
  return minutes ? `${minutes}:${String(seconds % 60).padStart(2, '0')}` : `${seconds} s`
}
