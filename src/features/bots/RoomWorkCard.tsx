// 群里的一张工作卡 = 某个 Bot 的一轮（学 Linear 的代理会话）：状态会变——在做（列最近几步）、等你批准
// （批准 / 拒绝就在卡上）、等你回答、做完了（留下回复与改动统计）、出错、已停止。“过程”打开它在这个群里的
// 席位会话。长回复折到 8 行，展开全文。批准只放行这一次，权限上限仍只在 Bot 设置里改。
import { useState, type ReactNode } from 'react'
import { Check, ChevronDown, CornerUpLeft, EyeOff, FileDiff, ListTree, Square } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import MarkdownMessage from '@/components/MarkdownMessage'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import type { BotAvatarValue } from './avatar'
import type { RoomApproval, RoomMessage, RoomTurn } from './rooms-api'

const COLLAPSE_CHARS = 600

export type CardMember = { id: string; name: string; avatar: BotAvatarValue | null }

function useDuration(turn: RoomTurn) {
  const { t } = useI18n()
  const started = Date.parse(turn.startedAt)
  const ended = Date.parse(turn.endedAt)
  if (!Number.isFinite(started) || !Number.isFinite(ended) || ended < started) return ''
  const seconds = Math.round((ended - started) / 1000)
  if (seconds < 60) return t('bots:room.duration.seconds', { count: Math.max(1, seconds) })
  if (seconds < 3600) return t('bots:room.duration.minutes', { count: Math.round(seconds / 60) })
  return t('bots:room.duration.hours', { count: Math.round(seconds / 3600) })
}

function StatePill({ turn }: { turn: RoomTurn }) {
  const { t } = useI18n()
  const waiting = turn.state === 'waiting'
  const running = turn.state === 'running'
  const duration = useDuration(turn)
  const label = waiting
    ? turn.waiting === 'question'
      ? t('bots:room.card.waitingQuestion')
      : t('bots:room.card.waitingApproval')
    : running
      ? turn.stepCount
        ? t('bots:room.card.running', { step: turn.stepCount })
        : t('bots:room.card.runningStart')
      : turn.state === 'error'
        ? t('bots:room.card.error')
        : turn.state === 'stopped'
          ? t('bots:room.card.stopped')
          : turn.stepCount && duration
            ? t('bots:room.card.done', { duration })
            : t('bots:room.card.replied')
  return (
    <span
      className={cn(
        'inline-flex flex-none items-center gap-1 rounded-full px-2 py-0.5 text-[11.5px] font-medium',
        waiting ? 'bg-st-attn-soft text-st-attn' : 'bg-surface-subtle',
        running ? 'text-content' : !waiting && 'text-content-muted',
      )}
    >
      {waiting ? (
        <span aria-hidden="true" className="size-1.5 rounded-full bg-st-attn-fill" />
      ) : running ? (
        <OrbitSpinner className="size-3" />
      ) : turn.state === 'done' ? (
        <Check className="size-3" aria-hidden="true" />
      ) : null}
      {label}
    </span>
  )
}

function Meta({ icon, children }: { icon: ReactNode; children: ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1 rounded-md border border-stroke-soft bg-solid px-2 py-0.5">
      {icon}
      {children}
    </span>
  )
}

// 在等你：批准（看它想用什么、参数，就在这里批准或拒绝；高风险的去过程里处理）或回答（去过程里答）。
function WaitingBlock({
  approval,
  canAct,
  onApprove,
  onOpenProcess,
}: {
  approval: RoomApproval | null
  canAct: boolean
  onApprove: (approvalId: string, approve: boolean) => void
  onOpenProcess: () => void
}) {
  const { t } = useI18n()
  if (!approval)
    return (
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-[13px] text-content-secondary">
          {t('bots:room.card.questionHint')}
        </span>
        <Button size="sm" variant="outline" onClick={onOpenProcess}>
          {t('bots:room.card.answer')}
        </Button>
      </div>
    )
  const critical = approval.severity === 'critical'
  return (
    <div>
      <p className="m-0 mb-2 text-[13px] text-content-secondary">
        {t('bots:room.card.wantsToRun', { tool: approval.toolName })}
      </p>
      {approval.argsPreview && (
        <pre className="m-0 mb-2.5 max-h-40 overflow-auto rounded-md bg-surface-subtle px-2.5 py-1.5 text-[12.5px] whitespace-pre-wrap">
          {approval.argsPreview}
        </pre>
      )}
      {canAct && (
        <div className="flex flex-wrap items-center gap-2">
          {critical ? (
            <>
              <Button size="sm" variant="outline" onClick={onOpenProcess}>
                {t('bots:room.card.process')}
              </Button>
              <span className="text-[12px] text-st-attn">{t('bots:room.card.critical')}</span>
            </>
          ) : (
            <>
              <Button size="sm" onClick={() => onApprove(approval.id, true)}>
                {t('bots:room.card.approve')}
              </Button>
              <Button size="sm" variant="outline" onClick={() => onApprove(approval.id, false)}>
                {t('bots:room.card.deny')}
              </Button>
            </>
          )}
        </div>
      )}
    </div>
  )
}

export function RoomWorkCard({
  message,
  member,
  calledBy,
  answeredRefs,
  canAct,
  onOpenProcess,
  onApprove,
  onStop,
  onReply,
  onHide,
}: {
  message: RoomMessage
  member: CardMember
  calledBy: string
  answeredRefs: number[]
  canAct: boolean
  onOpenProcess: () => void
  onApprove: (approvalId: string, approve: boolean) => void
  onStop: () => void
  onReply: () => void
  onHide: () => void
}) {
  const { t } = useI18n()
  const turn = message.turn!
  const [expanded, setExpanded] = useState(false)
  const waiting = turn.state === 'waiting'
  const live = turn.state === 'running' || waiting
  const long = message.text.length > COLLAPSE_CHARS || message.text.split('\n').length > 8
  return (
    <section
      tabIndex={-1}
      data-room-card={message.id}
      data-room-card-state={turn.state}
      aria-label={member.name}
      className={cn(
        'group/card max-w-[92%] overflow-hidden rounded-xl border bg-solid shadow-edge outline-none max-[650px]:max-w-full',
        waiting || (!live && turn.pending) ? 'border-st-attn-border' : 'border-stroke-soft',
      )}
    >
      <header className="flex min-w-0 items-center gap-2 px-3.5 pt-3">
        {member.avatar && (
          <BotAvatar
            avatar={member.avatar}
            state={waiting ? 'need' : live ? 'running' : 'idle'}
            size={22}
          />
        )}
        <span className="flex-none text-[13.5px] font-semibold text-content">
          {member.name || t('bots:room.removedBot')}
        </span>
        <StatePill turn={turn} />
        {calledBy && (
          <span className="min-w-0 truncate text-[12px] text-content-muted max-[650px]:hidden">
            {calledBy}
          </span>
        )}
        <span className="flex-1" />
        {/* 卡上的操作：悬停或聚焦（手机上点一下卡片）时出现，平时只留“过程”。看不见的时候点不到：
            不然在手机上点卡片头部的空白会直接点中“停下”。 */}
        <span className="flex items-center pointer-events-none opacity-0 transition-opacity group-hover/card:pointer-events-auto group-hover/card:opacity-100 group-focus-within/card:pointer-events-auto group-focus-within/card:opacity-100">
          {live && canAct ? (
            <Button
              variant="ghost"
              size="icon-xs"
              className="text-content-muted"
              aria-label={t('bots:room.card.stop')}
              title={t('bots:room.card.stop')}
              onClick={onStop}
            >
              <Square />
            </Button>
          ) : (
            <>
              <Button
                variant="ghost"
                size="icon-xs"
                className="text-content-muted"
                aria-label={t('bots:room.reply')}
                title={t('bots:room.reply')}
                onClick={onReply}
              >
                <CornerUpLeft />
              </Button>
              <Button
                variant="ghost"
                size="icon-xs"
                className="text-content-muted"
                aria-label={t('bots:room.hide')}
                title={t('bots:room.hide')}
                onClick={onHide}
              >
                <EyeOff />
              </Button>
            </>
          )}
        </span>
        {turn.sessionId && (
          <Button
            variant="ghost"
            size="xs"
            className="text-content-muted"
            onClick={onOpenProcess}
            title={t('bots:room.card.processHint')}
          >
            <ListTree aria-hidden="true" />
            {t('bots:room.card.process')}
          </Button>
        )}
      </header>

      <div className="px-3.5 pt-2 pb-3 pl-[44px] text-[length:calc(var(--app-message-font-size)*.9375)] leading-[1.65] text-content max-[650px]:pl-3.5">
        {answeredRefs.length > 0 && (
          <p className="m-0 mb-1 text-[12px] text-content-muted">
            {t('bots:room.card.alsoAnswered', {
              refs: answeredRefs.map((seq) => `#${seq}`).join('、'),
            })}
          </p>
        )}
        {live && !waiting && (
          <ul aria-live="polite" className="m-0 flex list-none flex-col gap-1 p-0">
            {turn.steps.map((step, index) => {
              const last = index === turn.steps.length - 1
              return (
                <li key={index} className="flex min-w-0 items-center gap-2 text-[13px]">
                  {last ? (
                    <OrbitSpinner className="size-3.5 flex-none" />
                  ) : (
                    <Check className="size-3.5 flex-none text-content-muted" aria-hidden="true" />
                  )}
                  <span
                    className={cn('min-w-0 truncate', last ? 'text-content' : 'text-content-muted')}
                  >
                    <span className="font-medium">{step.name}</span>
                    {step.summary && (
                      <span className="ml-1.5 font-mono text-[12px]">{step.summary}</span>
                    )}
                  </span>
                </li>
              )
            })}
            {turn.steps.length === 0 && (
              <li className="text-[13px] text-content-muted">{t('bots:room.card.reading')}</li>
            )}
          </ul>
        )}
        {waiting && (
          <WaitingBlock
            approval={turn.approval}
            canAct={canAct}
            onApprove={onApprove}
            onOpenProcess={onOpenProcess}
          />
        )}
        {turn.state === 'done' &&
          (message.text ? (
            <div className={cn('relative', long && !expanded && 'max-h-[11.5em] overflow-hidden')}>
              <MarkdownMessage>{message.text}</MarkdownMessage>
              {long && !expanded && (
                <div className="pointer-events-none absolute inset-x-0 bottom-0 h-12 bg-gradient-to-t from-solid to-transparent" />
              )}
            </div>
          ) : (
            <p className="m-0 text-[13px] text-content-muted">{t('bots:room.card.emptyReply')}</p>
          ))}
        {turn.state === 'error' && (
          <p role="alert" className="m-0 text-[13px] text-destructive">
            {turn.error || t('bots:room.card.error')}
          </p>
        )}
        {turn.state === 'stopped' && (
          <p className="m-0 text-[13px] text-content-muted">{t('bots:room.card.stoppedHint')}</p>
        )}
        {!live && turn.pending && (
          <div className="mt-2.5 rounded-lg border border-st-attn-border bg-st-attn-soft/50 px-3 py-2.5">
            <p className="m-0 mb-1.5 text-[12.5px] font-medium text-st-attn">
              {turn.pending.waiting === 'question'
                ? t('bots:room.card.pendingQuestion')
                : t('bots:room.card.pendingApproval')}
            </p>
            <WaitingBlock
              approval={turn.pending.approval}
              canAct={canAct}
              onApprove={onApprove}
              onOpenProcess={onOpenProcess}
            />
          </div>
        )}
        {turn.state === 'done' && long && (
          <Button
            variant="ghost"
            size="xs"
            className="-ml-2 mt-1 text-content-muted"
            onClick={() => setExpanded(!expanded)}
          >
            <ChevronDown aria-hidden="true" className={cn(expanded && 'rotate-180')} />
            {expanded ? t('bots:room.card.collapse') : t('bots:room.card.expand')}
          </Button>
        )}
      </div>

      {turn.changes && (
        <footer className="flex flex-wrap items-center gap-1.5 border-t border-stroke-soft bg-surface-subtle px-3.5 py-2 pl-[44px] text-[12.5px] text-content-secondary max-[650px]:pl-3.5">
          <Meta icon={<FileDiff className="size-3.5 text-content-muted" aria-hidden="true" />}>
            {t('bots:room.card.changes', { count: turn.changes.files })}
            {turn.changes.added !== null && turn.changes.removed !== null && (
              <span className="text-content-muted tabular-nums">
                +{turn.changes.added} −{turn.changes.removed}
              </span>
            )}
          </Meta>
          <span className="flex-1" />
          {turn.sessionId && (
            <Button variant="ghost" size="xs" onClick={onOpenProcess}>
              {t('bots:room.card.viewChanges')}
            </Button>
          )}
        </footer>
      )}
    </section>
  )
}
