// 输入框上方的实时状态行（第一版的样子）：每个在干活的 Bot 一行——头像在动、名字、“正在做”、
// 这个 Bot 的执行方式、这一步在做什么，右边“停下”和“查看过程”。有批准或提问在等时这一行变成琥珀色的“等你批准 /
// 等你回答”：批准、拒绝就在这一行上（和“查看”同一种键），高风险的只给“查看”，去过程里处理。
// 一轮结束、它派出去的活还在等你时也是这样一行。做完这一行消失，回复贴进时间线。
// 没点名的话还在判断该谁答时，最上面一行“正在看该谁来回答”（主持人分派的群是主持人的头像和“正在分派”），
// 不然发出去到有人接话之间什么动静都没有。
//
// 下面的“找谁写一份简报”提示条也在这里：聊长了、简报没人维护时问一句，选一个 Bot 就让它整理。
import { ListTree, NotebookPen, Square, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Chip } from '@/components/ui/chip'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import type { RoomStatusEntry } from './room-presentation'
import type { RoomMember, RoomMessage, RoomRouting } from './rooms-api'

export function RoomStatusRows({
  entries,
  routing,
  host,
  members,
  modeLabel,
  onOpenProcess,
  onApprove,
  onStop,
}: {
  entries: readonly RoomStatusEntry[]
  // 还在判断该谁答的话（按发出的顺序）；host 是主持人分派的群的主持人。
  routing: readonly RoomRouting[]
  host: RoomMember | null
  members: ReadonlyMap<string, RoomMember>
  // 这个 Bot 的执行方式（审批后写入 / 工作区可写 / 完全访问）；读不到时为空。
  modeLabel: (botId: string) => string
  onOpenProcess: (sessionId: string) => void
  onApprove: (message: RoomMessage, approvalId: string, approve: boolean) => void
  // 停下这一轮（只停它自己这张卡对应的那一轮）。
  onStop: (message: RoomMessage) => void
}) {
  const { t } = useI18n()
  return (
    <div aria-live="polite" className="mb-1.5 flex flex-col gap-0.5 empty:hidden">
      {routing.length > 0 && (
        <div
          data-room-status="routing"
          className="flex min-w-0 items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-[13px]"
        >
          {routing[0].kind === 'host' && host ? (
            <>
              <BotAvatar avatar={host.avatar} state="running" size={22} />
              <span className="flex-none font-medium text-content">{host.name}</span>
              <span className="flex-none text-content-muted">{t('bots:room.status.routing')}</span>
            </>
          ) : (
            <>
              <span className="flex size-[22px] flex-none items-center justify-center">
                <OrbitSpinner />
              </span>
              <span className="min-w-0 truncate text-content-muted">
                {t('bots:room.status.triaging')}
              </span>
            </>
          )}
          {routing.length > 1 && (
            <span className="min-w-0 truncate text-[12px] text-content-muted">
              · {t('bots:room.status.routingMore', { count: routing.length - 1 })}
            </span>
          )}
        </div>
      )}
      {entries.map((entry) => {
        const member = members.get(entry.botId)
        const turn = entry.message.turn!
        const waiting = Boolean(entry.waiting)
        const mode = modeLabel(entry.botId)
        const preview = entry.approval
          ? (entry.approval.argsPreview.split('\n').find((line) => line.trim()) || '').trim() ||
            entry.approval.toolName
          : ''
        const step = entry.waiting
          ? entry.waiting === 'approval'
            ? preview
              ? t('bots:room.status.wantsToRun', { what: preview })
              : ''
            : turn.current || t('bots:room.card.questionHint')
          : turn.current ||
            (turn.steps.at(-1)
              ? `${turn.steps.at(-1)!.name} ${turn.steps.at(-1)!.summary}`.trim()
              : t('bots:room.card.reading'))
        const label = entry.waiting
          ? entry.waiting === 'question'
            ? t('bots:room.card.waitingQuestion')
            : t('bots:room.card.waitingApproval')
          : t('bots:room.status.working')
        const open = () => turn.sessionId && onOpenProcess(turn.sessionId)
        const approval = entry.waiting === 'approval' ? entry.approval : null
        return (
          <div
            key={entry.message.id}
            data-room-status={entry.waiting || 'running'}
            className={cn(
              'flex min-w-0 items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-[13px]',
              // 手机上一行放不下按钮：按钮换到下一行，不把“想运行 …”挤成一个字。
              waiting && 'bg-st-attn-soft max-[650px]:flex-wrap',
            )}
          >
            {member && (
              <BotAvatar avatar={member.avatar} state={waiting ? 'need' : 'running'} size={22} />
            )}
            <span className="flex-none font-medium text-content">
              {member?.name || t('bots:room.removedBot')}
            </span>
            <span
              className={cn(
                'flex-none',
                waiting ? 'font-medium text-st-attn' : 'text-content-muted',
              )}
            >
              {label}
            </span>
            {mode && (
              <span className="flex-none text-content-muted max-[650px]:hidden">· {mode}</span>
            )}
            <span
              className="min-w-0 flex-1 truncate font-mono text-[12px] text-content-secondary"
              title={entry.pending ? t('bots:room.card.pendingApproval') : undefined}
            >
              {step}
            </span>
            {waiting ? (
              <span className="flex flex-none items-center gap-2.5 max-[650px]:basis-full max-[650px]:justify-end">
                {approval && approval.severity !== 'critical' && (
                  <>
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => onApprove(entry.message, approval.id, true)}
                    >
                      {t('bots:room.card.approve')}
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => onApprove(entry.message, approval.id, false)}
                    >
                      {t('bots:room.card.deny')}
                    </Button>
                  </>
                )}
                <Button
                  size="sm"
                  variant="outline"
                  disabled={!turn.sessionId}
                  title={
                    approval?.severity === 'critical'
                      ? t('bots:room.card.critical')
                      : t('bots:room.card.processHint')
                  }
                  onClick={open}
                >
                  {t('bots:room.status.view')}
                </Button>
              </span>
            ) : (
              <>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label={t('bots:room.card.stop')}
                  title={t('bots:room.card.stop')}
                  onClick={() => onStop(entry.message)}
                >
                  <Square />
                </Button>
                {turn.sessionId && (
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={t('bots:room.viewProcess')}
                    title={t('bots:room.viewProcess')}
                    onClick={open}
                  >
                    <ListTree />
                  </Button>
                )}
              </>
            )}
          </div>
        )
      })}
    </div>
  )
}

export function RoomBriefPrompt({
  count,
  chars,
  members,
  phone,
  onPick,
  onWrite,
  onLater,
  onSkip,
  onOpen,
}: {
  count: number
  chars: number
  members: readonly RoomMember[]
  phone: boolean
  onPick: (botId: string) => void
  onWrite: () => void
  onLater: () => void
  onSkip: () => void
  // 手机上收成一行，点开再选人（打开简报面板）。
  onOpen: () => void
}) {
  const { t } = useI18n()
  if (phone)
    return (
      <button
        type="button"
        onClick={onOpen}
        className="mb-2 flex w-full items-center gap-2 rounded-xl border border-stroke-soft bg-surface-subtle px-3 py-2.5 text-left text-[13px] text-content-secondary focus-visible:outline-2 focus-visible:outline-focus"
      >
        <NotebookPen className="size-4 flex-none text-content-muted" aria-hidden="true" />
        <span className="min-w-0 flex-1 truncate">
          {t('bots:room.briefPrompt.short', { count })}
        </span>
        <span className="flex-none font-medium text-content">
          {t('bots:room.briefPrompt.shortAction')}
        </span>
      </button>
    )
  return (
    <div
      role="region"
      aria-label={t('bots:room.briefPrompt.ask')}
      className="mb-2 rounded-xl border border-stroke-soft bg-surface-subtle px-3 py-2.5"
    >
      <div className="flex items-start gap-2">
        <NotebookPen className="mt-0.5 size-4 flex-none text-content-muted" aria-hidden="true" />
        <p className="m-0 flex-1 text-[13px] text-content-secondary">
          {t('bots:room.briefPrompt.text', { count, thousands: Math.round(chars / 1000) })}
          <span className="font-medium text-content">{t('bots:room.briefPrompt.ask')}</span>
        </p>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label={t('bots:room.briefPrompt.close')}
          onClick={onLater}
        >
          <X />
        </Button>
      </div>
      <div className="mt-2 flex items-center gap-1.5 pl-6">
        {/* 成员多了这一排横着滑（和输入框的“发给”一样），右边三个键不被挤到下一行。 */}
        <div className="flex min-w-0 flex-1 items-center gap-1.5 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          {members
            .filter((member) => !member.deleted && member.enabled)
            .map((member) => (
              <Chip key={member.id} size="sm" onClick={() => onPick(member.id)}>
                <BotAvatar avatar={member.avatar} size={14} animate={false} />
                {member.name}
              </Chip>
            ))}
        </div>
        <Button variant="ghost" size="xs" onClick={onWrite}>
          {t('bots:room.briefPrompt.write')}
        </Button>
        <Button variant="ghost" size="xs" onClick={onLater}>
          {t('bots:room.briefPrompt.later')}
        </Button>
        <Button variant="ghost" size="xs" onClick={onSkip}>
          {t('bots:room.briefPrompt.skip')}
        </Button>
      </div>
    </div>
  )
}
