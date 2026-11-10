// 群聊时间线：你的话是右侧气泡（和会话页一样）；Bot 的每一轮是一张工作卡；“所有人”那句下面一行“这一轮”
// （谁回了、谁跳过、谁还在做）；别的 Bot 叫起来的卡片前面一行交接；系统行（链停了、Bot 暂停没叫醒）居中淡字。
// 撤回的消息只留一行占位。你的话带的附件显示在气泡下面（和会话页同一个附件组件）。
// 键盘：整条时间线是一个 feed，每条消息一个 article，用 roving tabindex——Tab 进来落在最近（或上次）的那条，
// 上下方向键逐条移动、Home / End 到头尾；在某条上按 R 回复，Delete / Backspace 撤回或隐藏，Enter 打开工作卡的过程。
import { useId, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { ArrowRight, CornerUpLeft, Undo2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import MarkdownMessage from '@/components/MarkdownMessage'
import { Button } from '@/components/ui/button'
import { MessageAttachments } from '@/features/chat/public-attachments'
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import { botAvatarTone } from './avatar'
import {
  mentionSegments,
  roomAttachmentViews,
  timelineKeyAction,
  type RoundEntry,
  type TimelineItem,
} from './room-presentation'
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
  const text = message.recalled
    ? t('bots:room.recalledShort')
    : message.text.replace(/\s+/g, ' ') || message.attachments.map((file) => file.name).join('、')
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

// 主持人分派的系统行：请了谁（或谁也不用）、一句理由；分派失败、主持人暂停时各一句。
function routeLine(
  event: NonNullable<RoomMessage['event']>,
  name: (id: string) => string,
  t: ReturnType<typeof useI18n>['t'],
) {
  const host = name(event.botId)
  if (event.type === 'route_failed')
    return event.reason === 'host_paused'
      ? t('bots:room.system.routeHostPaused', { host })
      : t('bots:room.system.routeFailed', { host })
  if (event.type !== 'routed') return ''
  const reason = event.note
  if (!event.bots.length)
    return reason
      ? t('bots:room.system.routedNobody', { host, reason })
      : t('bots:room.system.routedNobodyPlain', { host })
  const names = event.bots.map(name).join('、')
  return reason
    ? t('bots:room.system.routed', { host, names, reason })
    : t('bots:room.system.routedPlain', { host, names })
}

export function RoomTimeline({
  items: allItems,
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
  const feed = useRef<HTMLDivElement>(null)
  const hintId = useId()
  const systemText = (message: RoomMessage) => {
    const event = message.event
    return event?.type === 'paused'
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
          ? event.reason === 'attachment'
            ? t('bots:room.system.wakeFailedAttachment', { name: name(event.botId) })
            : event.reason === 'model'
              ? t('bots:room.system.wakeFailedModel', { name: name(event.botId) })
              : t('bots:room.system.wakeFailed', { name: name(event.botId) })
          : event
            ? routeLine(event, name, t)
            : ''
  }
  // 键盘移动、“第几条 / 共几条”都按真正画出来的条目算：认不出类型的系统行不画，
  // 留在列表里的话方向键会停在一条不存在的消息上，它恰好是最后一条时整个时间线也 Tab 不进来。
  const items = allItems.filter((item) => item.kind !== 'system' || systemText(item.message))
  // 能 Tab 进来的那一条：上次聚焦过的；没有（或它已经不在了）时是最近的一条。
  const [current, setCurrent] = useState('')
  const activeId = items.some((item) => item.message.id === current)
    ? current
    : items.at(-1)?.message.id || ''

  const focusAt = (index: number) => {
    const id = items[index]?.message.id
    const node = id ? feed.current?.querySelector(`[data-room-message="${CSS.escape(id)}"]`) : null
    if (!(node instanceof HTMLElement)) return
    node.focus({ preventScroll: true })
    node.scrollIntoView({ block: 'nearest' })
  }

  // 只在焦点落在某条消息上（或它里面的按钮、链接上）时接键：输入框、对话框、菜单都不在这里面，
  // 时间线里的按钮自己的 Enter / 空格照常。
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const target = event.target
    if (!(target instanceof HTMLElement)) return
    const article = target.closest('[data-room-message]')
    if (!(article instanceof HTMLElement) || !feed.current?.contains(article)) return
    // 条目里的输入控件不接；按钮、链接上只接移动；其余（包括点一下工作卡后焦点落在卡片内层上）
    // 都当作焦点在这条消息上，方向键、R、Enter 照常。
    if (target.closest('input, textarea, select, [contenteditable="true"]')) return
    const onMessage = target === article || !target.closest('button, a, [role="button"]')
    const index = items.findIndex((item) => item.message.id === article.dataset.roomMessage)
    if (index < 0) return
    const action = timelineKeyAction(event, items[index], index, items.length, onMessage)
    if (!action) return
    event.preventDefault()
    const { message } = items[index]
    if (action.type === 'move') focusAt(action.index)
    else if (action.type === 'reply') actions.onReply(message)
    else if (action.type === 'recall') actions.onRecall(message)
    else if (message.turn?.sessionId) actions.onOpenProcess(message.turn.sessionId)
  }

  // 每条消息的外壳：article + roving tabindex + 读屏用的“谁 · 状态 · 几点”。
  const article = (
    message: RoomMessage,
    index: number,
    label: string,
    children: ReactNode,
    className = '',
  ) => (
    <article
      key={message.id}
      data-room-message={message.id}
      tabIndex={message.id === activeId ? 0 : -1}
      aria-label={label}
      aria-posinset={index + 1}
      aria-setsize={items.length}
      onFocus={() => setCurrent(message.id)}
      className={cn(
        'group/msg rounded-xl outline-none focus-visible:ring-2 focus-visible:ring-focus-ring focus-visible:ring-offset-4 focus-visible:ring-offset-panel',
        className,
      )}
    >
      {children}
    </article>
  )

  return (
    <>
      {/* 操作提示放在 feed 外面：feed 里只该有一条条 article。 */}
      <p id={hintId} className="sr-only">
        {t('bots:room.timeline.hint')}
      </p>
      <div
        ref={feed}
        role="feed"
        aria-label={t('bots:room.timeline.label')}
        aria-describedby={hintId}
        className="flex flex-col gap-4"
        onKeyDown={onKeyDown}
      >
        {items.map((item, index) => {
          const { message } = item
          const reply = message.replyTo ? byId.get(message.replyTo) : null
          const at = time(message.at)
          if (item.kind === 'system') {
            const text = systemText(message)
            return text ? article(message, index, text, <SystemLine>{text}</SystemLine>) : null
          }
          if (item.kind === 'recalled') {
            const seen = message.recalled?.seenBy.map(name) || []
            const text =
              message.author.kind === 'user'
                ? t('bots:room.recalledMine')
                : t('bots:room.recalledBot', {
                    name: message.author.kind === 'bot' ? name(message.author.botId) : '',
                  })
            return article(
              message,
              index,
              text,
              <SystemLine>
                {text}
                {seen.length > 0 && ` · ${t('bots:room.seenBy', { names: seen.join('、') })}`}
              </SystemLine>,
            )
          }
          if (item.kind === 'user')
            return article(
              message,
              index,
              [
                t('bots:room.a11y.message', { name: t('bots:room.you'), time: at }),
                message.attachments.length
                  ? t('bots:room.a11y.attachments', { count: message.attachments.length })
                  : '',
              ]
                .filter(Boolean)
                .join(' · '),
              <>
                <div className="flex flex-col items-end gap-1">
                  {reply && (
                    <div className="max-w-[78%]">
                      <Quote message={reply} members={members} onJump={actions.onJump} />
                    </div>
                  )}
                  {message.text && (
                    <div className="max-w-[78%] rounded-[18px] bg-[var(--user-bubble-bg)] px-4 py-2.5 text-[length:calc(var(--app-message-font-size)*.9375)] leading-[1.65] break-words whitespace-pre-wrap text-[var(--user-bubble-text)] max-[650px]:max-w-[88%]">
                      <Mentions text={message.text} members={members} />
                    </div>
                  )}
                  {message.attachments.length > 0 && (
                    <div className="flex max-w-[78%] justify-end max-[650px]:max-w-[88%] [&_.message-attachments]:justify-end">
                      <MessageAttachments
                        attachments={roomAttachmentViews(message.attachments)}
                        compact
                      />
                    </div>
                  )}
                  <div className="flex items-center gap-0.5 pr-1">
                    <span className="flex items-center gap-0.5 pointer-events-none opacity-0 transition-opacity group-hover/msg:pointer-events-auto group-hover/msg:opacity-100 group-focus-within/msg:pointer-events-auto group-focus-within/msg:opacity-100">
                      <Button
                        variant="ghost"
                        size="xs"
                        className="text-content-muted"
                        aria-keyshortcuts="R"
                        onClick={() => actions.onReply(message)}
                      >
                        <CornerUpLeft aria-hidden="true" />
                        {t('bots:room.reply')}
                      </Button>
                      <Button
                        variant="ghost"
                        size="xs"
                        className="text-content-muted"
                        aria-keyshortcuts="Delete"
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
                      {at}
                    </time>
                  </div>
                </div>
                {item.round && <Round entries={item.round} members={members} />}
              </>,
              'flex flex-col gap-2',
            )
          if (item.kind === 'post') {
            const author = message.author.kind === 'bot' ? members.get(message.author.botId) : null
            return article(
              message,
              index,
              t('bots:room.a11y.message', {
                name: author?.name || t('bots:room.removedBot'),
                time: at,
              }),
              <>
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
                      {at}
                    </time>
                  </div>
                  <MarkdownMessage>{message.text}</MarkdownMessage>
                </div>
              </>,
              'flex gap-2.5',
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
          const turn = message.turn
          const state =
            turn?.state === 'waiting'
              ? turn.waiting === 'question'
                ? t('bots:room.card.waitingQuestion')
                : t('bots:room.card.waitingApproval')
              : turn?.state === 'running'
                ? t('bots:room.card.runningStart')
                : turn?.state === 'error'
                  ? t('bots:room.card.error')
                  : turn?.state === 'stopped'
                    ? t('bots:room.card.stopped')
                    : t('bots:room.card.replied')
          return article(
            message,
            index,
            t('bots:room.a11y.card', {
              name: author?.name || t('bots:room.removedBot'),
              state,
              time: at,
            }),
            <>
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
            </>,
            'flex flex-col gap-2',
          )
        })}
      </div>
    </>
  )
}
