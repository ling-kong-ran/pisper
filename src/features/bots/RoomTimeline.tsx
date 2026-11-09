// 群聊时间线：你的话是右侧气泡（和会话页一样）；Bot 的每一轮是一张工作卡；“所有人”那句下面一行“这一轮”
// （谁回了、谁跳过、谁还在做）；别的 Bot 叫起来的卡片前面一行交接；系统行（链停了、Bot 暂停没叫醒）居中淡字。
// 撤回的消息只留一行占位。
import type { ReactNode } from 'react'
import { ArrowRight, CornerUpLeft, Undo2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import MarkdownMessage from '@/components/MarkdownMessage'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import { botAvatarTone } from './avatar'
import { mentionSegments, type RoundEntry, type TimelineItem } from './room-presentation'
import type { RoomMember, RoomMessage } from './rooms-api'
import { RoomWorkCard } from './RoomWorkCard'

type Members = ReadonlyMap<string, RoomMember>

export type TimelineActions = {
  onReply: (message: RoomMessage) => void
  onRecall: (message: RoomMessage) => void
  onOpenProcess: (sessionId: string) => void
  onApprove: (message: RoomMessage, approvalId: string, approve: boolean) => void
  onStop: (message: RoomMessage) => void
  onJump: (messageId: string) => void
}

function time(at: string) {
  const date = new Date(at)
  return Number.isNaN(date.getTime())
    ? ''
    : date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
}

function Mentions({ text, members }: { text: string; members: Members }) {
  const list = [...members.values()]
  return (
    <>
      {mentionSegments(text, list).map((segment, index) => {
        const member = segment.memberId ? members.get(segment.memberId) : null
        if (!member && !segment.everyone) return <span key={index}>{segment.text}</span>
        const tone = member ? botAvatarTone(member.avatar.color) : null
        return (
          <span
            key={index}
            className={cn('rounded-[4px] px-0.5 font-medium', !tone && 'bg-surface-active')}
            style={tone ? { color: tone.shade, background: tone.tile } : undefined}
          >
            {segment.text}
          </span>
        )
      })}
    </>
  )
}

function Quote({
  message,
  members,
  onJump,
}: {
  message: RoomMessage
  members: Members
  onJump: (id: string) => void
}) {
  const { t } = useI18n()
  const name =
    message.author.kind === 'bot'
      ? members.get(message.author.botId)?.name || t('bots:room.removedBot')
      : t('bots:room.you')
  const text = message.recalled ? t('bots:room.recalledShort') : message.text.replace(/\s+/g, ' ')
  return (
    <button
      type="button"
      onClick={() => onJump(message.id)}
      className="mb-1 flex max-w-full min-w-0 items-center gap-1.5 rounded-sm border-l-2 border-stroke-strong py-0.5 pl-2 text-left text-[12.5px] text-content-muted hover:text-content-secondary focus-visible:outline-2 focus-visible:outline-focus"
    >
      <CornerUpLeft className="size-3 flex-none" aria-hidden="true" />
      <span className="truncate">{t('bots:room.replyTo', { name, text })}</span>
    </button>
  )
}

function Round({ entries, members }: { entries: RoundEntry[]; members: Members }) {
  const { t } = useI18n()
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 pl-1 text-[12px] text-content-muted">
      <span>{t('bots:room.round')}</span>
      {entries.map((entry) => {
        const member = members.get(entry.botId)
        return (
          <span
            key={entry.botId}
            className={cn(
              'inline-flex items-center gap-1',
              entry.state === 'skipped' && 'opacity-60',
            )}
          >
            {member && <BotAvatar avatar={member.avatar} size={14} animate={false} />}
            {member?.name || t('bots:room.removedBot')}{' '}
            {entry.state === 'skipped'
              ? t('bots:room.round.skipped')
              : entry.state === 'working'
                ? t('bots:room.round.working')
                : t('bots:room.round.replied')}
          </span>
        )
      })}
    </div>
  )
}

function Handoff({
  from,
  to,
  hop,
  members,
}: {
  from: string
  to: string
  hop: number
  members: Members
}) {
  const { t } = useI18n()
  const source = members.get(from)
  const target = members.get(to)
  return (
    <div className="flex items-center gap-1.5 pl-1 text-[12px] text-content-muted">
      {source && <BotAvatar avatar={source.avatar} size={14} animate={false} />}
      {source?.name || t('bots:room.removedBot')}
      <ArrowRight className="size-3" aria-label={t('bots:room.handoff.to')} />
      {target && <BotAvatar avatar={target.avatar} size={14} animate={false} />}
      {target?.name || t('bots:room.removedBot')}
      <span>· {t('bots:room.handoff.hop', { hop })}</span>
    </div>
  )
}

function SystemLine({ children }: { children: ReactNode }) {
  return <p className="my-1 text-center text-[12px] text-content-muted">{children}</p>
}

export function RoomTimeline({
  items,
  messages,
  members,
  canAct,
  actions,
}: {
  items: TimelineItem[]
  messages: readonly RoomMessage[]
  members: Members
  canAct: boolean
  actions: TimelineActions
}) {
  const { t } = useI18n()
  const byId = new Map(messages.map((message) => [message.id, message]))
  const name = (id: string) => members.get(id)?.name || t('bots:room.removedBot')
  return (
    <ol className="m-0 flex list-none flex-col gap-4 p-0">
      {items.map((item) => {
        const { message } = item
        const reply = message.replyTo ? byId.get(message.replyTo) : null
        if (item.kind === 'system') {
          const event = message.event
          const text =
            event?.type === 'paused'
              ? t('bots:room.system.paused', { name: name(event.botId) })
              : event?.type === 'chain_stopped'
                ? t('bots:room.system.chainStopped', {
                    from: name(event.from),
                    to: name(event.to),
                    reason:
                      event.reason === 'hops'
                        ? t('bots:room.system.reason.hops')
                        : event.reason === 'rate'
                          ? t('bots:room.system.reason.rate')
                          : t('bots:room.system.reason.total'),
                  })
                : event?.type === 'wake_failed'
                  ? t('bots:room.system.wakeFailed', { name: name(event.botId) })
                  : ''
          return text ? (
            <li key={message.id} data-room-message={message.id}>
              <SystemLine>{text}</SystemLine>
            </li>
          ) : null
        }
        if (item.kind === 'recalled') {
          const seen = message.recalled?.seenBy.map(name) || []
          return (
            <li key={message.id} data-room-message={message.id}>
              <SystemLine>
                {message.author.kind === 'user'
                  ? t('bots:room.recalledMine')
                  : t('bots:room.recalledBot', {
                      name: message.author.kind === 'bot' ? name(message.author.botId) : '',
                    })}
                {seen.length > 0 && ` · ${t('bots:room.seenBy', { names: seen.join('、') })}`}
              </SystemLine>
            </li>
          )
        }
        if (item.kind === 'user')
          return (
            <li key={message.id} data-room-message={message.id} className="flex flex-col gap-2">
              <div className="group/mine flex flex-col items-end gap-1">
                {reply && (
                  <div className="max-w-[78%]">
                    <Quote message={reply} members={members} onJump={actions.onJump} />
                  </div>
                )}
                <div
                  tabIndex={-1}
                  className="max-w-[78%] rounded-[18px] bg-[var(--user-bubble-bg)] outline-none px-4 py-2.5 text-[length:calc(var(--app-message-font-size)*.9375)] leading-[1.65] break-words whitespace-pre-wrap text-[var(--user-bubble-text)] max-[650px]:max-w-[88%]"
                >
                  <Mentions text={message.text} members={members} />
                </div>
                <div className="flex items-center gap-0.5 pr-1">
                  <span className="flex items-center gap-0.5 opacity-0 transition-opacity group-hover/mine:opacity-100 group-focus-within/mine:opacity-100">
                    <Button
                      variant="ghost"
                      size="xs"
                      className="text-content-muted"
                      onClick={() => actions.onReply(message)}
                    >
                      <CornerUpLeft aria-hidden="true" />
                      {t('bots:room.reply')}
                    </Button>
                    <Button
                      variant="ghost"
                      size="xs"
                      className="text-content-muted"
                      onClick={() => actions.onRecall(message)}
                    >
                      <Undo2 aria-hidden="true" />
                      {t('bots:room.recall')}
                    </Button>
                  </span>
                  <time
                    dateTime={message.at}
                    className="text-[11.5px] text-content-muted tabular-nums"
                  >
                    {time(message.at)}
                  </time>
                </div>
              </div>
              {item.round && <Round entries={item.round} members={members} />}
            </li>
          )
        if (item.kind === 'post') {
          const author = message.author.kind === 'bot' ? members.get(message.author.botId) : null
          return (
            <li key={message.id} data-room-message={message.id} className="flex gap-2.5">
              {author && <BotAvatar avatar={author.avatar} size={22} animate={false} />}
              <div className="min-w-0 flex-1 text-[length:calc(var(--app-message-font-size)*.9375)] leading-[1.65] text-content">
                <div className="flex items-baseline gap-2">
                  <span className="text-[13.5px] font-semibold">
                    {author?.name || t('bots:room.removedBot')}
                  </span>
                  <time
                    dateTime={message.at}
                    className="text-[11.5px] text-content-muted tabular-nums"
                  >
                    {time(message.at)}
                  </time>
                </div>
                <MarkdownMessage>{message.text}</MarkdownMessage>
              </div>
            </li>
          )
        }
        const author = message.author.kind === 'bot' ? members.get(message.author.botId) : null
        const answeredRefs = (message.turn?.alsoAnswered || [])
          .map((id) => byId.get(id)?.seq)
          .filter((seq): seq is number => typeof seq === 'number')
        const calledBy = item.handoff
          ? t('bots:room.card.calledBy', { name: name(item.handoff.from) })
          : message.turn?.reason === 'report'
            ? t('bots:room.card.report')
            : ''
        return (
          <li key={message.id} data-room-message={message.id} className="flex flex-col gap-2">
            {item.handoff && message.author.kind === 'bot' && (
              <Handoff
                from={item.handoff.from}
                to={message.author.botId}
                hop={item.handoff.hop}
                members={members}
              />
            )}
            <RoomWorkCard
              message={message}
              member={{
                id: author?.id || '',
                name: author?.name || '',
                avatar: author?.avatar || null,
              }}
              calledBy={calledBy}
              answeredRefs={answeredRefs}
              canAct={canAct}
              onOpenProcess={() =>
                message.turn?.sessionId && actions.onOpenProcess(message.turn.sessionId)
              }
              onApprove={(approvalId, approve) => actions.onApprove(message, approvalId, approve)}
              onStop={() => actions.onStop(message)}
              onReply={() => actions.onReply(message)}
              onHide={() => actions.onRecall(message)}
            />
          </li>
        )
      })}
    </ol>
  )
}
