// 群聊时间线（第一版的样子）：不用气泡，每条是“头像 + 名字 + 时间 / 正文 / 操作”的列表行，像 Slack；
// 你自己的话也在左边——多人场合左右分不清谁是谁。同一个人两分钟内接着说的合并头部。
// Bot 的一轮做完了才贴成一行（名字后面是模型、右边“本轮结束”），还在做的画在输入框上方的状态行里。
// 长回复折到 8 行，渐隐，“展开全文”。撤回、系统行（链停了、暂停没叫醒、主持人分派）、出错 / 停下、
// 交接（谁 → 谁 · 第几跳）、“这一轮”（谁回了、谁跳过）都是居中的一行淡字。
// 键盘：整条时间线是一个 feed，每条一个 article，用 roving tabindex——Tab 进来落在最近（或上次）的那条，
// 上下方向键逐条移动、Home / End 到头尾；在某条上按 R 回复，Delete 撤回或隐藏，Enter 打开 Bot 那一轮的过程。
import {
  useId,
  useRef,
  useState,
  type ComponentProps,
  type KeyboardEvent,
  type ReactNode,
} from 'react'
import { ChevronDown, CornerUpLeft, EyeOff, ListTree, Undo2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import MarkdownMessage from '@/components/MarkdownMessage'
import { Button } from '@/components/ui/button'
import { MessageAttachments } from '@/features/chat/public-attachments'
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import { botAvatarTone } from './avatar'
import {
  isLiveTurn,
  mentionSegments,
  mergesWithPrevious,
  roomAttachmentViews,
  roomDayKey,
  timelineKeyAction,
  type RoundEntry,
  type TimelineItem,
} from './room-presentation'
import type { RoomMember, RoomMessage } from './rooms-api'

type Members = ReadonlyMap<string, RoomMember>
type Translate = ReturnType<typeof useI18n>['t']

export type TimelineActions = {
  onReply: (message: RoomMessage) => void
  onRecall: (message: RoomMessage) => void
  onOpenProcess: (sessionId: string) => void
  onJump: (messageId: string) => void
}

// 超过 8 行（或很长的一段）就折起来。
const COLLAPSE_LINES = 8
const COLLAPSE_CHARS = 600

function time(at: string) {
  const date = new Date(at)
  return Number.isNaN(date.getTime())
    ? ''
    : date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
}

function dayLabel(at: string, t: Translate) {
  const date = new Date(at)
  if (Number.isNaN(date.getTime())) return ''
  const today = new Date()
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1)
  const key = roomDayKey(at)
  if (key === roomDayKey(today.toISOString())) return t('bots:room.day.today')
  if (key === roomDayKey(yesterday.toISOString())) return t('bots:room.day.yesterday')
  return date.toLocaleDateString([], {
    month: 'long',
    day: 'numeric',
    ...(date.getFullYear() === today.getFullYear() ? {} : { year: 'numeric' }),
  })
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

// 你的头像：深色圆底上一个字（群里只有你一个人，不必上传头像）。
function UserAvatar({ label }: { label: string }) {
  return (
    <span
      aria-hidden="true"
      className="grid size-7 flex-none place-items-center rounded-full bg-content text-[12px] font-semibold text-panel"
    >
      {Array.from(label)[0] || ''}
    </span>
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
      <span className="truncate">
        {t('bots:room.quote.prefix')}
        <span className="font-medium text-content-secondary">{name}</span>
        {t('bots:room.quote.separator')}
        {text}
      </span>
    </button>
  )
}

// 居中的一行淡字：撤回、系统行、交接、“这一轮”、出错 / 停下。
function SystemLine({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <p className={cn('m-0 my-2 text-center text-[12px] text-content-muted', className)}>
      {children}
    </p>
  )
}

function roundLine(entries: RoundEntry[], name: (id: string) => string, t: Translate) {
  const parts = entries.map((entry) =>
    entry.state === 'skipped'
      ? t('bots:room.round.skippedBy', { name: name(entry.botId) })
      : entry.state === 'working'
        ? t('bots:room.round.workingBy', { name: name(entry.botId) })
        : t('bots:room.round.repliedBy', { name: name(entry.botId) }),
  )
  return t('bots:room.round.line', { parts: parts.join(' · ') })
}

// 分派的系统行：请了谁（或谁也不用）、一句理由；分派失败、主持人暂停时各一句。没有主持人时是群里判断的（triaged）。
function routeLine(
  event: NonNullable<RoomMessage['event']>,
  name: (id: string) => string,
  t: Translate,
) {
  if (event.type === 'triaged') {
    const reason = event.note
    if (!event.bots.length)
      return reason
        ? t('bots:room.system.triagedNobody', { reason })
        : t('bots:room.system.triagedNobodyPlain')
    const names = event.bots.map(name).join('、')
    return reason
      ? t('bots:room.system.triaged', { names, reason })
      : t('bots:room.system.triagedPlain', { names })
  }
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

// 一行的操作键：第一版的样式（淡字小键，悬停变深）。
function Act({
  icon,
  children,
  ...props
}: { icon: ReactNode; children: ReactNode } & Omit<
  ComponentProps<typeof Button>,
  'variant' | 'size'
>) {
  return (
    <Button variant="ghost" size="xs" className="text-content-muted hover:text-content" {...props}>
      {icon}
      {children}
    </Button>
  )
}

export function RoomTimeline({
  items: allItems,
  messages,
  members,
  modelLabel,
  actions,
}: {
  items: TimelineItem[]
  messages: readonly RoomMessage[]
  members: Members
  // Bot 名字后面的模型小标签；Bot 已删除或读不到时为空。
  modelLabel: (botId: string) => string
  actions: TimelineActions
}) {
  const { t } = useI18n()
  const byId = new Map(messages.map((message) => [message.id, message]))
  const name = (id: string) => members.get(id)?.name || t('bots:room.removedBot')
  const you = t('bots:room.you')
  const feed = useRef<HTMLDivElement>(null)
  const hintId = useId()
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set())
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
  // 键盘移动、“第几条 / 共几条”都按真正画出来的条目算：认不出类型的系统行不画，还在做的一轮画在输入框上方
  // 的状态行里；留在列表里的话方向键会停在一条不存在的消息上。
  const items = allItems.filter(
    (item) =>
      !(item.kind === 'card' && isLiveTurn(item.message)) &&
      (item.kind !== 'system' || systemText(item.message)),
  )
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

  // 每条消息的外壳：article + roving tabindex + 读屏用的“谁 · 几点”；换天的地方先画一条日期分隔线。
  const article = (
    message: RoomMessage,
    index: number,
    label: string,
    children: ReactNode,
    className = '',
  ) => {
    const previous = items[index - 1]?.message
    const day = roomDayKey(message.at)
    const divider = day && (!previous || roomDayKey(previous.at) !== day)
    return (
      <article
        key={message.id}
        data-room-message={message.id}
        tabIndex={message.id === activeId ? 0 : -1}
        aria-label={label}
        aria-posinset={index + 1}
        aria-setsize={items.length}
        onFocus={() => setCurrent(message.id)}
        className={cn(
          'group/msg rounded-lg outline-none focus-visible:ring-2 focus-visible:ring-focus-ring focus-visible:ring-offset-2 focus-visible:ring-offset-panel',
          className,
        )}
      >
        {divider && (
          <div
            aria-hidden="true"
            className="my-3 flex items-center gap-3 text-[12px] text-content-muted"
          >
            <span className="h-px flex-1 bg-stroke-soft" />
            {dayLabel(message.at, t)}
            <span className="h-px flex-1 bg-stroke-soft" />
          </div>
        )}
        {children}
      </article>
    )
  }

  // 一行消息：头像 + 名字（+ 模型）+ 时间（+ 右边的“本轮结束”）/ 正文 / 操作。合并头部时只留正文和操作。
  const row = ({
    avatar,
    author,
    meta,
    at,
    right,
    merged,
    children,
  }: {
    avatar: ReactNode
    author: string
    meta?: ReactNode
    at: string
    right?: ReactNode
    merged: boolean
    children: ReactNode
  }) => (
    <div
      className={cn(
        '-mx-3 flex gap-3 rounded-lg px-3 transition-colors group-hover/msg:bg-surface-subtle',
        merged ? 'py-1' : 'py-2.5',
      )}
    >
      <div className="w-7 flex-none pt-0.5">{!merged && avatar}</div>
      <div className="min-w-0 flex-1">
        {!merged && (
          <header className="flex min-w-0 items-center gap-2">
            <span className="truncate text-[13.5px] font-semibold text-content">{author}</span>
            {meta}
            <time dateTime={at} className="text-[12px] text-content-muted tabular-nums">
              {time(at)}
            </time>
            <span className="flex-1" />
            {right}
          </header>
        )}
        <div className="mt-0.5 text-[length:calc(var(--app-message-font-size)*.9375)] leading-[1.65] break-words text-content [font-family:var(--font-content-family)]">
          {children}
        </div>
      </div>
    </div>
  )

  // 操作行。always：Bot 的回复上常驻（展开全文、查看过程、回复）；其余悬停或聚焦时出现。
  // 看不见的时候不占位、也点不到（手机上点一下这一条、键盘移到这一条时出现）。
  const actionRow = (always: ReactNode, hover: ReactNode) => (
    <div className="mt-1 -ml-2 flex flex-wrap items-center gap-0.5">
      {always}
      {hover && (
        <span className="hidden items-center gap-0.5 group-focus-within/msg:flex group-hover/msg:flex">
          {hover}
        </span>
      )}
    </div>
  )

  const toggleExpanded = (id: string) =>
    setExpanded((currentSet) => {
      const next = new Set(currentSet)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  // Bot 的正文：长的折到 8 行，下面渐隐。
  const botBody = (message: RoomMessage) => {
    const long =
      message.text.length > COLLAPSE_CHARS || message.text.split('\n').length > COLLAPSE_LINES
    const open = expanded.has(message.id)
    return {
      long,
      open,
      body: (
        <div className={cn('relative', long && !open && 'max-h-[9.9em] overflow-hidden')}>
          <MarkdownMessage>{message.text}</MarkdownMessage>
          {long && !open && (
            <div className="pointer-events-none absolute inset-x-0 bottom-0 h-12 bg-gradient-to-t from-[var(--main-surface-bg,var(--panel))] to-transparent group-hover/msg:from-surface-subtle" />
          )}
        </div>
      ),
    }
  }

  const modelTag = (botId: string) => {
    const label = modelLabel(botId)
    const member = members.get(botId)
    if (!label || !member) return null
    const tone = botAvatarTone(member.avatar.color)
    return (
      <span
        className="max-w-40 flex-none truncate rounded-[4px] px-1.5 py-px text-[11px] font-medium"
        style={{ color: tone.shade, background: tone.tile }}
      >
        {label}
      </span>
    )
  }

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
        className="flex flex-col"
        onKeyDown={onKeyDown}
      >
        {items.map((item, index) => {
          const { message } = item
          const reply = message.replyTo ? byId.get(message.replyTo) : null
          const at = time(message.at)
          const previous = items[index - 1]
          const merged =
            Boolean(previous) &&
            (previous.kind === 'user' || previous.kind === 'post') &&
            (item.kind === 'user' || item.kind === 'post') &&
            mergesWithPrevious(previous.message, message)
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
              <SystemLine className="my-3">
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
                t('bots:room.a11y.message', { name: you, time: at }),
                message.attachments.length
                  ? t('bots:room.a11y.attachments', { count: message.attachments.length })
                  : '',
              ]
                .filter(Boolean)
                .join(' · '),
              <>
                {row({
                  avatar: <UserAvatar label={you} />,
                  author: you,
                  at: message.at,
                  merged,
                  children: (
                    <>
                      {reply && <Quote message={reply} members={members} onJump={actions.onJump} />}
                      {message.text && (
                        <div className="whitespace-pre-wrap">
                          <Mentions text={message.text} members={members} />
                        </div>
                      )}
                      {message.attachments.length > 0 && (
                        <div className="mt-1.5">
                          <MessageAttachments
                            attachments={roomAttachmentViews(message.attachments)}
                            compact
                          />
                        </div>
                      )}
                      {actionRow(
                        null,
                        <>
                          <Act
                            icon={<CornerUpLeft />}
                            aria-keyshortcuts="R"
                            onClick={() => actions.onReply(message)}
                          >
                            {t('bots:room.reply')}
                          </Act>
                          <Act
                            icon={<Undo2 />}
                            aria-keyshortcuts="Delete"
                            onClick={() => actions.onRecall(message)}
                          >
                            {t('bots:room.recall')}
                          </Act>
                        </>,
                      )}
                    </>
                  ),
                })}
                {item.round && <SystemLine>{roundLine(item.round, name, t)}</SystemLine>}
              </>,
            )
          if (item.kind === 'post') {
            const botId = message.author.kind === 'bot' ? message.author.botId : ''
            const author = members.get(botId)
            const { long, open, body } = botBody(message)
            return article(
              message,
              index,
              t('bots:room.a11y.message', { name: name(botId), time: at }),
              row({
                avatar: author && <BotAvatar avatar={author.avatar} size={28} animate={false} />,
                author: name(botId),
                meta: modelTag(botId),
                at: message.at,
                merged,
                children: (
                  <>
                    {reply && <Quote message={reply} members={members} onJump={actions.onJump} />}
                    {body}
                    {long &&
                      actionRow(
                        <Act
                          icon={<ChevronDown className={cn(open && 'rotate-180')} />}
                          aria-expanded={open}
                          onClick={() => toggleExpanded(message.id)}
                        >
                          {open ? t('bots:room.card.collapse') : t('bots:room.card.expand')}
                        </Act>,
                        null,
                      )}
                  </>
                ),
              }),
            )
          }
          // Bot 的一轮（做完、出错、停下；还在做的已经在上面滤掉，画在状态行里）。
          const botId = message.author.kind === 'bot' ? message.author.botId : ''
          const author = members.get(botId)
          const turn = message.turn!
          const handoff = item.handoff ? (
            <SystemLine>
              {t('bots:room.handoff.line', {
                from: name(item.handoff.from),
                to: name(botId),
                hop: item.handoff.hop,
              })}
            </SystemLine>
          ) : null
          if (turn.state === 'error' || turn.state === 'stopped') {
            const text =
              turn.state === 'error'
                ? turn.error
                  ? t('bots:room.turn.errorLine', { name: name(botId), error: turn.error })
                  : t('bots:room.turn.errorLinePlain', { name: name(botId) })
                : t('bots:room.turn.stoppedLine', { name: name(botId) })
            return article(
              message,
              index,
              text,
              <>
                {handoff}
                <SystemLine>{text}</SystemLine>
              </>,
            )
          }
          const answeredRefs = (turn.alsoAnswered || [])
            .map((id) => byId.get(id))
            .filter((target): target is RoomMessage => Boolean(target))
          const { long, open, body } = botBody(message)
          return article(
            message,
            index,
            t('bots:room.a11y.message', { name: name(botId), time: at }),
            <>
              {handoff}
              {row({
                avatar: author && <BotAvatar avatar={author.avatar} size={28} animate={false} />,
                author: name(botId),
                meta: modelTag(botId),
                at: message.at,
                right: (
                  <span className="flex-none rounded-full border border-stroke-soft px-1.5 py-px text-[11px] text-content-muted">
                    {t('bots:room.turnEnded')}
                  </span>
                ),
                merged: false,
                children: (
                  <>
                    {reply && <Quote message={reply} members={members} onJump={actions.onJump} />}
                    {answeredRefs.length > 0 && (
                      <p className="m-0 mb-1 text-[12px] text-content-muted">
                        {t('bots:room.alsoAnsweredPrefix')}
                        {answeredRefs.map((target, refIndex) => (
                          <span key={target.id}>
                            {refIndex > 0 && '、'}
                            <button
                              type="button"
                              className="underline decoration-stroke-strong underline-offset-2 hover:text-content-secondary focus-visible:outline-2 focus-visible:outline-focus"
                              onClick={() => actions.onJump(target.id)}
                            >
                              #{target.seq}
                            </button>
                          </span>
                        ))}
                      </p>
                    )}
                    {message.text ? (
                      body
                    ) : (
                      <p className="m-0 text-[13px] text-content-muted">
                        {t('bots:room.card.emptyReply')}
                      </p>
                    )}
                    {actionRow(
                      <>
                        {long && (
                          <Act
                            icon={<ChevronDown className={cn(open && 'rotate-180')} />}
                            aria-expanded={open}
                            onClick={() => toggleExpanded(message.id)}
                          >
                            {open ? t('bots:room.card.collapse') : t('bots:room.card.expand')}
                          </Act>
                        )}
                        {turn.sessionId && (
                          <Act
                            icon={<ListTree />}
                            title={t('bots:room.card.processHint')}
                            onClick={() => actions.onOpenProcess(turn.sessionId)}
                          >
                            {t('bots:room.viewProcess')}
                          </Act>
                        )}
                        <Act
                          icon={<CornerUpLeft />}
                          aria-keyshortcuts="R"
                          onClick={() => actions.onReply(message)}
                        >
                          {t('bots:room.reply')}
                        </Act>
                      </>,
                      <Act
                        icon={<EyeOff />}
                        aria-keyshortcuts="Delete"
                        onClick={() => actions.onRecall(message)}
                      >
                        {t('bots:room.hide')}
                      </Act>,
                    )}
                  </>
                ),
              })}
            </>,
          )
        })}
      </div>
    </>
  )
}
