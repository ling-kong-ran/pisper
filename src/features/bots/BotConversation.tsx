// 和 Bot 的对话（设计稿 botim-chat）：每个任务是一段往来——你说的每一轮是右侧气泡，Bot 的回复在左侧；
// 任务的状态是对话里的卡片：干活中、等你批准或回答、改动等你确认（查看 / 放弃 / 接受并应用）、
// 失败或中断（继续 / 放弃）、已完成。完整的工具步骤在 Bot 的会话里，卡片上给“打开完整记录”。
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Check, FileDiff, GitBranch, History, RotateCcw, Square, TriangleAlert } from 'lucide-react'
import type { Translate } from '@/app/navigation'
import { sessionPath } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import { BotAvatar } from './BotAvatar'
import { BotApprovalCard, BotQuestionCard } from './BotWaitingCard'
import { botTaskAction, type Bot, type BotTask } from './bots-api'

type Action = 'accept' | 'apply' | 'discard' | 'resume' | 'stop'

function doneLine(t: Translate, task: BotTask) {
  if (task.status === 'discarded') return t('bots:card.discarded')
  if (task.result?.applied) return t('bots:card.applied', { branch: task.result.branch ?? '' })
  if (task.result?.commit) return t('bots:card.accepted', { branch: task.result.branch ?? '' })
  return t('bots:card.noChanges')
}

function BotMessage({ bot, children }: { bot: Bot; children: React.ReactNode }) {
  return (
    <div className="mb-3.5 flex gap-2.5">
      <BotAvatar avatar={bot.avatar} size={30} />
      <div className="min-w-0 flex-1">
        <div className="text-[13px] font-semibold text-content">{bot.name}</div>
        <div className="mt-0.5 text-[14.5px] leading-relaxed whitespace-pre-wrap text-content">
          {children}
        </div>
      </div>
    </div>
  )
}

function TaskCard({
  bot,
  task,
  onAction,
  onShowChanges,
  onChanged,
  onError,
  busy,
}: {
  bot: Bot
  task: BotTask
  onAction: (task: BotTask, action: Action) => void
  onShowChanges: () => void
  onChanged: () => void
  onError: (message: string) => void
  busy: boolean
}) {
  const { t } = useI18n()
  const navigate = useNavigate()
  const openSession = task.sessionId ? (
    <Button size="sm" variant="ghost" onClick={() => navigate(sessionPath(task.sessionId ?? ''))}>
      <History aria-hidden="true" />
      {t('bots:card.openSession')}
    </Button>
  ) : null
  // 在等你：先处理审批（工具卡在那里），再处理问题。
  const waiting = task.status === 'running' && task.sessionId ? task.waiting : null
  if (waiting?.approval)
    return (
      <BotApprovalCard
        key={waiting.approval.id}
        approval={waiting.approval}
        sessionId={task.sessionId ?? ''}
        onDone={onChanged}
        onError={onError}
        openSession={openSession}
      />
    )
  if (waiting?.question)
    return (
      <BotQuestionCard
        key={waiting.question.id}
        bot={bot}
        question={waiting.question}
        sessionId={task.sessionId ?? ''}
        onDone={onChanged}
        onError={onError}
        openSession={openSession}
      />
    )
  if (['queued', 'preparing', 'running'].includes(task.status))
    return (
      <div className="mt-2 flex items-center gap-2 rounded-xl border border-stroke bg-surface-raised px-3.5 py-2.5 text-[13px]">
        <OrbitSpinner className="size-3.5" />
        <span className="flex-1 text-content-secondary">
          {task.status === 'queued' ? t('bots:card.queued') : t('bots:card.working')}
        </span>
        {openSession}
        <Button size="sm" variant="outline" disabled={busy} onClick={() => onAction(task, 'stop')}>
          <Square aria-hidden="true" />
          {t('bots:card.stop')}
        </Button>
      </div>
    )
  if (task.status === 'review')
    return (
      <div
        data-bot-review={task.id}
        className="mt-2 overflow-hidden rounded-xl border border-st-attn-border bg-surface-raised ring-[3px] ring-st-attn-soft"
      >
        <div className="flex items-center gap-2 px-3.5 py-2.5 text-[13px]">
          <FileDiff className="size-3.5 text-content-muted" aria-hidden="true" />
          <span className="flex-1 font-semibold text-content">
            {t('bots:card.review', { count: task.result?.changedFiles ?? 0 })}
          </span>
        </div>
        <div className="flex flex-wrap items-center gap-2 border-t border-stroke px-3.5 py-2.5">
          {task.workspace?.branch && (
            <span className="flex min-w-0 flex-1 items-center gap-1.5 font-mono text-xs text-content-muted">
              <GitBranch className="size-3.5 flex-none" aria-hidden="true" />
              <span className="truncate">{task.workspace.branch}</span>
            </span>
          )}
          <Button size="sm" variant="outline" onClick={onShowChanges}>
            {t('bots:card.showChanges')}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={() => onAction(task, 'discard')}
          >
            {t('bots:card.discard')}
          </Button>
          <Button size="sm" disabled={busy} onClick={() => onAction(task, 'apply')}>
            {t('bots:card.acceptApply')}
          </Button>
        </div>
        <p className="border-t border-stroke px-3.5 py-2 text-xs text-content-muted">
          {t('bots:card.reviewHint')}
        </p>
      </div>
    )
  if (task.status === 'failed' || task.status === 'interrupted')
    return (
      <div className="mt-2 flex flex-wrap items-center gap-2 rounded-xl border border-stroke bg-surface-subtle px-3.5 py-2.5 text-[13px]">
        <TriangleAlert className="size-3.5 text-content-muted" aria-hidden="true" />
        <span className="min-w-0 flex-1 text-content-secondary">
          {task.error?.message ||
            (task.status === 'interrupted' ? t('bots:card.interrupted') : t('bots:card.failed'))}
        </span>
        {openSession}
        <Button
          size="sm"
          variant="outline"
          disabled={busy}
          onClick={() => onAction(task, 'discard')}
        >
          {t('bots:card.discard')}
        </Button>
        <Button size="sm" disabled={busy} onClick={() => onAction(task, 'resume')}>
          <RotateCcw aria-hidden="true" />
          {t('bots:card.resume')}
        </Button>
      </div>
    )
  return (
    <div className="mt-1.5 flex items-center gap-1.5 text-xs text-content-muted">
      <Check className="size-3.5" aria-hidden="true" />
      <span>{doneLine(t, task)}</span>
      {openSession}
    </div>
  )
}

export function BotConversation({
  bot,
  tasks,
  onChanged,
  onShowChanges,
  onError,
}: {
  bot: Bot
  tasks: BotTask[]
  onChanged: () => void
  onShowChanges: (task: BotTask) => void
  onError: (message: string) => void
}) {
  const { t } = useI18n()
  const [busyId, setBusyId] = useState('')
  const act = async (task: BotTask, action: Action) => {
    setBusyId(task.id)
    try {
      if (action === 'apply' || action === 'accept')
        await botTaskAction(task.id, 'accept', { apply: action === 'apply' })
      else await botTaskAction(task.id, action)
      onChanged()
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusyId('')
    }
  }
  if (tasks.length === 0)
    return (
      <BotMessage bot={bot}>
        <p>{t('bots:conversation.greeting', { name: bot.name })}</p>
        <p className="mt-1 text-[13px] text-content-muted">{t('bots:conversation.greetingHint')}</p>
      </BotMessage>
    )
  return (
    <ol className="m-0 list-none p-0" aria-label={t('bots:conversation.label', { name: bot.name })}>
      {tasks.map((task) => (
        <li key={task.id} data-bot-task={task.id} className="mb-5">
          {task.turns.map((turn, index) => {
            const last = index === task.turns.length - 1
            return (
              <div key={`${task.id}-${index}`}>
                <div className="my-2 flex justify-end">
                  <div className="max-w-[78%] rounded-[18px] bg-surface-subtle px-4 py-2 text-[14.5px] leading-relaxed whitespace-pre-wrap text-content">
                    {turn.text}
                  </div>
                </div>
                {(turn.reply || last) && (
                  <BotMessage bot={bot}>
                    {turn.reply ? <p>{turn.reply}</p> : null}
                    {last && (
                      <TaskCard
                        bot={bot}
                        task={task}
                        busy={busyId === task.id}
                        onAction={act}
                        onShowChanges={() => onShowChanges(task)}
                        onChanged={onChanged}
                        onError={onError}
                      />
                    )}
                  </BotMessage>
                )}
              </div>
            )
          })}
        </li>
      ))}
    </ol>
  )
}
