// 会话页检查器的“计划”标签（规格 4.4）：目标、计划、团队、子代理四段，每段只在有数据时出现。
// 计划沿用 PlanBoard；目标、团队任务和子代理只读展示会话现有状态，不提供新的操作入口
// （暂停目标、改预算仍在输入框的执行模式控件里）。
import { ListTodo } from 'lucide-react'
import type { ReactNode } from 'react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState } from '@/components/app/PageState'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import type { StatusGlyphKind } from '@/components/app/status-glyph-kinds'
import { formatTokenCount } from '@/lib/format'
import type { EntityRecord, Plan, TeamState, TeamTask } from '@/types/chat'
import PlanBoard from './PlanBoard'

type Translate = (key: string, values?: Record<string, unknown>) => string

const STATUS_GLYPH: Record<string, { kind: StatusGlyphKind; color: string }> = {
  running: { kind: 'run', color: 'text-st-run' },
  starting: { kind: 'run', color: 'text-st-run' },
  queued: { kind: 'queued', color: 'text-st-queue' },
  completed: { kind: 'done', color: 'text-st-done' },
  failed: { kind: 'failed', color: 'text-st-fail' },
  interrupted: { kind: 'stopped', color: 'text-st-queue' },
  // 阻塞不是“需要你”：琥珀只留给人在回路的状态，这里用中性灰。
  blocked: { kind: 'queued', color: 'text-st-queue' },
}

function statusText(status: unknown, t: Translate) {
  if (status === 'queued') return t('chat:agentRunActivity.teamTaskQueued')
  if (status === 'starting') return t('chat:agentRunActivity.teamTaskStarting')
  if (status === 'running') return t('chat:agentRunActivity.teamTaskRunning')
  if (status === 'completed') return t('chat:agentRunActivity.teamTaskCompleted')
  if (status === 'failed') return t('chat:agentRunActivity.teamTaskFailed')
  if (status === 'interrupted') return t('chat:agentRunActivity.teamTaskInterrupted')
  if (status === 'blocked') return t('chat:agentRunActivity.teamTaskBlocked')
  return t('chat:agentRunActivity.teamTaskStatusUpdated')
}

function goalStatusText(goal: EntityRecord, t: Translate) {
  if (goal.status === 'active')
    return goal.mode === 'team'
      ? t('chat:focusSession.runningAsTeam')
      : t('chat:focusSession.runningAutonomously')
  if (goal.status === 'paused') return t('chat:focusSession.goalPaused')
  if (goal.status === 'complete') return t('chat:focusSession.goalComplete')
  if (goal.status === 'budget_limited') return t('chat:focusSession.goalReachedItsBudget')
  return ''
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="grid min-w-0 gap-2" aria-label={title}>
      <h3 className="text-xs font-semibold text-content-secondary">{title}</h3>
      {children}
    </section>
  )
}

function StatusRow({
  status,
  name,
  detail,
  t,
}: {
  status: unknown
  name: string
  detail?: string
  t: Translate
}) {
  const glyph = STATUS_GLYPH[String(status)] ?? { kind: 'queued' as const, color: 'text-st-queue' }
  return (
    <li className="flex min-w-0 items-start gap-2 py-1.5 text-xs">
      <StatusGlyph status={glyph.kind} className={`mt-px size-3.5 ${glyph.color}`} />
      <span className="grid min-w-0 flex-1 gap-0.5">
        <span className="flex min-w-0 items-center gap-2">
          <strong className="min-w-0 truncate font-medium text-content">{name}</strong>
          <span className="flex-none text-content-muted">{statusText(status, t)}</span>
        </span>
        {detail && <span className="min-w-0 break-words text-content-muted">{detail}</span>}
      </span>
    </li>
  )
}

function subAgents(activity: readonly EntityRecord[]) {
  // 同一个子代理会以多条活动出现；按 id 去重，保留最新的一条。
  const byId = new Map<string, EntityRecord>()
  for (const entry of activity) {
    if (entry?.type !== 'agent' || !entry.agent || typeof entry.agent !== 'object') continue
    const agent = entry.agent as EntityRecord
    const key = String(agent.id || agent.taskName || byId.size)
    byId.set(key, agent)
  }
  return [...byId.values()]
}

export function InspectorPlan({
  plan,
  goal,
  team,
  activity,
}: {
  plan: Plan | null
  goal: EntityRecord | null
  team: TeamState | null
  activity: readonly EntityRecord[]
}) {
  const { t } = useI18n()
  const tasks: TeamTask[] = Array.isArray(team?.tasks) ? team.tasks : []
  const agents = subAgents(activity)
  const hasGoal = Boolean(goal?.id && String(goal.objective || '').trim())
  const hasPlan = Boolean(plan?.items?.length)
  if (!hasGoal && !hasPlan && !tasks.length && !agents.length)
    return (
      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        <InlineEmptyState compact icon={ListTodo} title={t('chat:sessionContext.planEmpty')} />
      </div>
    )
  const budget = Number(goal?.mode === 'team' ? goal?.teamTokenBudget : goal?.tokenBudget) || 0
  return (
    <div
      className="grid min-h-0 flex-1 content-start gap-5 overflow-y-auto p-3"
      data-inspector-plan
    >
      {hasGoal && goal && (
        <Section title={t('board:inspector.goal')}>
          <div className="grid gap-1.5 rounded-md border border-stroke-soft bg-surface-subtle p-3 text-xs">
            <p className="whitespace-pre-wrap break-words text-content">{String(goal.objective)}</p>
            <p className="flex flex-wrap items-center gap-x-2 text-content-muted tabular-nums">
              <span>{goalStatusText(goal, t)}</span>
              <span aria-hidden="true">·</span>
              <span>
                {budget > 0
                  ? t('chat:focusSession.usedBudgetTokensUsed', {
                      used: formatTokenCount(goal.tokensUsed),
                      budget: formatTokenCount(budget),
                    })
                  : t('chat:focusSession.usedUnlimitedGoalTokens', {
                      used: formatTokenCount(goal.tokensUsed),
                    })}
              </span>
            </p>
          </div>
        </Section>
      )}
      {hasPlan && (
        <Section title={t('board:inspector.plan')}>
          <PlanBoard plan={plan} />
        </Section>
      )}
      {tasks.length > 0 && (
        <Section title={t('board:inspector.team')}>
          <ul className="divide-y divide-stroke-soft">
            {tasks.map((task, index) => (
              <StatusRow
                key={task.id || `${task.taskName}-${index}`}
                status={task.status}
                name={task.taskName || t('chat:agentRunActivity.subagent')}
                detail={task.error || task.output || ''}
                t={t}
              />
            ))}
          </ul>
        </Section>
      )}
      {agents.length > 0 && (
        <Section title={t('board:inspector.subAgents')}>
          <ul className="divide-y divide-stroke-soft">
            {agents.map((agent, index) => (
              <StatusRow
                key={String(agent.id || index)}
                status={agent.status}
                name={String(agent.taskName || '').trim() || t('chat:agentRunActivity.subagent')}
                detail={String(agent.error || agent.message || '').trim()}
                t={t}
              />
            ))}
          </ul>
        </Section>
      )}
    </div>
  )
}
