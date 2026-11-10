// 群聊输入框：外观是第一版的样子（沿用会话页的输入卡片）。打 @ 弹出成员补全（Popover + Command，键盘可选，
// 含“所有人”）。工具行：“+”（附件、@ 成员）、附件键，“发给”后面每个成员一个芯片（圆点是它的头像色，按下 = 选中，
// 暂停的点不了），最后是“所有人”；成员多了这一排横着滑（触屏手指滑、鼠标滚轮也能横滑，键盘 Tab 过去自动滚到）。
// 点了名（@ 谁、回复某个 Bot、@所有人）就只叫点到的；没点名才发给芯片里选的人。没有“只留言”：
// 一个都没选时就是所有人；只有主持人分派的群里，空着表示交给主持人。
// 要叫醒 3 个以上时在发送键旁写“会叫醒 N 个”（每个都要跑一整轮）。Enter 发送，Shift+Enter 换行；输入法组词时不发送。
// 附件和会话输入框是同一套（附件键、粘贴图片、拖文件进来、附件托盘），只带附件不写字也能发。
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type DragEvent,
  type KeyboardEvent,
  type WheelEvent,
} from 'react'
import { AtSign, Paperclip, Plus, Users, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Chip } from '@/components/ui/chip'
import { Command, CommandItem, CommandList } from '@/components/ui/command'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import {
  AttachmentTray,
  ComposerSendButton,
  isFileDrag,
  useAttachmentChooser,
  useAttachmentSelection,
} from '@/features/chat/public-attachments'
import { cn } from '@/lib/utils'
import type { ChatAttachment } from '@/types/chat'
import { BotAvatar } from './BotAvatar'
import { botAvatarTone } from './avatar'
import { activeMentionQuery, composerWakeTargets, mentionedIds } from './room-presentation'
import type { RoomMember, RoomMessage } from './rooms-api'

// “发给”芯片在触屏上看得见的仍是第一版的小芯片，透明的 ::after 把能点到的范围上下扩到 44px
// （滑动的那一排上下留了同样的内边距，扩出来的部分不会被横向滚动裁掉）。
const CHIP_TOUCH =
  'relative pointer-coarse:after:absolute pointer-coarse:after:inset-x-0 pointer-coarse:after:-inset-y-2.5'

const MAX_CHARS = 12_000
// @ 补全里“所有人”那一项的 id（不会和 Bot id 撞上）。
const EVERYONE = '@everyone'
const EVERYONE_WORDS = ['所有人', 'all', 'everyone']

export function RoomComposer({
  roomId,
  cwd,
  members,
  recipients,
  onRecipientsChange,
  replyTo,
  replyName,
  onCancelReply,
  onSend,
  placeholder,
  idleLabel,
  idleHint,
}: {
  roomId: string
  cwd?: string
  members: readonly RoomMember[]
  recipients: string[] | 'all'
  onRecipientsChange: (value: string[] | 'all') => void
  replyTo: RoomMessage | null
  replyName: string
  onCancelReply: () => void
  onSend: (text: string, mentions: string[], attachments: ChatAttachment[]) => Promise<boolean>
  placeholder: string
  // 主持人分派的群：没选“发给”时芯片上写什么（交给主持人）。没给时一个人都不选就是所有人。
  idleLabel?: string
  // 主持人模式下没点名时发送键旁的那句（由主持人分派，多一次模型调用）。
  idleHint?: string
}) {
  const { t } = useI18n()
  const toId = useId()
  // “+”菜单里选了“@ 成员”：菜单关上时焦点留在输入框，不回到“+”键上。
  const mentionFromMenu = useRef(false)
  const [value, setValue] = useState('')
  const [caret, setCaret] = useState(0)
  const [highlight, setHighlight] = useState('')
  const [dismissed, setDismissed] = useState(-1)
  const [sending, setSending] = useState(false)
  const [dropActive, setDropActive] = useState(false)
  const composing = useRef(false)
  const input = useRef<HTMLTextAreaElement>(null)
  const active = members.filter((member) => !member.deleted)
  const selection = useAttachmentSelection([], undefined, roomId)
  const chooser = useAttachmentChooser({ cwd, selection })
  const hasAttachments = selection.attachments.length > 0
  // 附件还在读：先别发，不然这几个就丢了。
  const preparing = selection.preparingAttachments.length > 0
  const canSend = Boolean(value.trim() || hasAttachments) && !preparing && !sending

  // 点了“回复”（或在时间线上按 R）：光标回到输入框，接着就能打字。
  const replyId = replyTo?.id
  useEffect(() => {
    if (replyId) input.current?.focus()
  }, [replyId])

  const query = activeMentionQuery(value, caret)
  const everyoneName = t('bots:room.composer.everyone')
  const options = useMemo(() => {
    if (!query) return []
    const needle = query.query.toLowerCase()
    const matched: { id: string; name: string; member?: RoomMember }[] = active
      .filter((member) => member.name.toLowerCase().includes(needle))
      .slice(0, 8)
      .map((member) => ({ id: member.id, name: member.name, member }))
    // 两个以上成员才有“所有人”；排在最后，打“所”“all”时也能找到。
    if (
      active.length > 1 &&
      [everyoneName, ...EVERYONE_WORDS].some((word) => word.toLowerCase().startsWith(needle))
    )
      matched.push({ id: EVERYONE, name: everyoneName })
    return matched
  }, [active, query, everyoneName])
  const mentionOpen = Boolean(query && options.length && dismissed !== query.start)
  const selected = options.find((option) => option.id === highlight) || options[0]

  // “@ 谁谁答”的群没有“只留言”：一个人都没选就是所有人（老的群可能还存着空的）。
  // 选过的人都被移出了群也一样。
  const everyone =
    recipients === 'all' || (!idleLabel && !active.some((member) => recipients.includes(member.id)))
  const chosen = everyone ? active.map((member) => member.id) : recipients
  const chosenMembers = active.filter((member) => chosen.includes(member.id))
  // 点名：@成员、@所有人、回复某个 Bot。点了名就只叫点到的，“发给”这次不用（也不经过主持人）。
  const { addressed, mentionsAll, targets } = composerWakeTargets({
    text: value,
    members: active,
    chosen,
    repliedBot: replyTo?.author.kind === 'bot' ? replyTo.author.botId : '',
  })
  // 暂停的 Bot 叫不醒，不算进“会叫醒 N 个”。
  const awake = new Set(active.filter((member) => member.enabled).map((member) => member.id))
  const wakes = targets.filter((id) => awake.has(id)).length

  const insertMention = (option: { name: string }) => {
    if (!query) return
    const before = value.slice(0, query.start)
    const after = value.slice(caret)
    const next = `${before}@${option.name} ${after}`
    const position = before.length + option.name.length + 2
    setValue(next)
    setCaret(position)
    window.requestAnimationFrame(() => {
      input.current?.focus()
      input.current?.setSelectionRange(position, position)
    })
  }

  // “+”菜单里的“@ 成员”：在光标处插一个 @，补全列表随即弹出。
  const startMention = () => {
    const node = input.current
    const at = node?.selectionStart ?? value.length
    const before = value.slice(0, at)
    const prefix = before && !/\s$/.test(before) ? ' @' : '@'
    const next = `${before}${prefix}${value.slice(at)}`
    const position = at + prefix.length
    setValue(next)
    setCaret(position)
    setDismissed(-1)
    window.requestAnimationFrame(() => {
      input.current?.focus()
      input.current?.setSelectionRange(position, position)
    })
  }

  // 鼠标滚轮在芯片那一排上竖着滚时横着滑（触控板和触屏本来就能横滑）。
  const wheelChips = (event: WheelEvent<HTMLDivElement>) => {
    const node = event.currentTarget
    if (node.scrollWidth <= node.clientWidth || Math.abs(event.deltaX) > Math.abs(event.deltaY))
      return
    node.scrollLeft += event.deltaY
  }

  const toggle = (id: string, checked: boolean) => {
    const current = everyone ? active.map((member) => member.id) : recipients
    const next = checked ? [...new Set([...current, id])] : current.filter((item) => item !== id)
    onRecipientsChange(
      (next.length === active.length && active.length > 1) || (!next.length && !idleLabel)
        ? 'all'
        : next,
    )
  }

  const send = async () => {
    const text = value.trim()
    if (!canSend) return
    setSending(true)
    try {
      if (await onSend(text, mentionedIds(text, active), selection.attachments)) {
        setValue('')
        setCaret(0)
        selection.clearAttachments()
        // 高度是输入时按内容撑开的：清空后收回一行。
        if (input.current) input.current.style.height = 'auto'
      }
    } finally {
      setSending(false)
    }
  }

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (composing.current || event.nativeEvent.isComposing) return
    if (mentionOpen && selected) {
      const index = options.indexOf(selected)
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault()
        const step = event.key === 'ArrowDown' ? 1 : -1
        setHighlight(options[(index + step + options.length) % options.length].id)
        return
      }
      if (event.key === 'Enter' || event.key === 'Tab') {
        event.preventDefault()
        insertMention(selected)
        return
      }
      if (event.key === 'Escape') {
        event.preventDefault()
        setDismissed(query!.start)
        return
      }
    }
    if (event.key === 'Escape' && replyTo) {
      event.preventDefault()
      onCancelReply()
      return
    }
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault()
      void send()
    }
  }

  const showDrop = (event: DragEvent<HTMLFormElement>) => {
    if (!isFileDrag(event.dataTransfer)) return
    event.preventDefault()
    event.dataTransfer.dropEffect = 'copy'
    setDropActive(true)
  }
  const hideDrop = (event: DragEvent<HTMLFormElement>) => {
    const next = event.relatedTarget
    if (next instanceof Node && event.currentTarget.contains(next)) return
    setDropActive(false)
  }

  return (
    <Popover open={mentionOpen}>
      {selection.attachmentError && (
        <p role="alert" className="m-0 mb-2 text-[13px] text-danger">
          {selection.attachmentError}
        </p>
      )}
      <PopoverAnchor asChild>
        <form
          data-room-composer
          onSubmit={(event) => {
            event.preventDefault()
            void send()
          }}
          className={cn(
            'relative flex min-w-0 flex-col rounded-[var(--composer-radius,22px)] border border-control bg-solid shadow-composer transition-[border-color,box-shadow] duration-(--d-fast) has-[textarea:focus]:border-focus has-[textarea:focus]:ring-3 has-[textarea:focus]:ring-focus-ring max-[650px]:rounded-[28px] max-[650px]:border-stroke-strong max-[650px]:shadow-none max-[650px]:has-[textarea:focus]:border-control max-[650px]:has-[textarea:focus]:ring-0 [&>.attachment-tray]:px-3 [&>.attachment-tray]:pt-3',
            dropActive && 'border-focus ring-3 ring-focus-ring',
          )}
          onDragEnter={showDrop}
          onDragOver={showDrop}
          onDragLeave={hideDrop}
          onDrop={(event) => {
            setDropActive(false)
            selection.dropFiles(event)
          }}
        >
          {dropActive && (
            <div
              role="status"
              className="pointer-events-none absolute inset-0 z-10 grid place-items-center rounded-[inherit] border border-dashed border-focus bg-panel px-4 text-center text-sm text-content"
            >
              {t('chat:focusSession.dropAttachment')}
            </div>
          )}
          {replyTo && (
            <div className="mx-3 mt-2.5 flex min-w-0 items-center gap-2 rounded-lg bg-surface-subtle px-2.5 py-1.5 text-[12.5px] text-content-secondary">
              <span className="min-w-0 flex-1 truncate">
                {t('bots:room.replyTo', {
                  name: replyName,
                  text:
                    replyTo.text.replace(/\s+/g, ' ') ||
                    replyTo.attachments.map((file) => file.name).join('、') ||
                    '…',
                })}
              </span>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={t('bots:room.composer.cancelReply')}
                onClick={onCancelReply}
              >
                <X />
              </Button>
            </div>
          )}
          <AttachmentTray
            attachments={selection.attachments}
            preparing={selection.preparingAttachments}
            onRemove={selection.removeAttachment}
          />
          <textarea
            ref={input}
            rows={1}
            value={value}
            maxLength={MAX_CHARS}
            aria-label={t('bots:room.composer.label')}
            aria-autocomplete="list"
            aria-expanded={mentionOpen}
            placeholder={placeholder}
            enterKeyHint="send"
            className="max-h-[220px] min-h-11 w-full min-w-0 resize-none overflow-y-auto border-0 bg-transparent px-4 pt-3 pb-1 text-[length:calc(var(--app-message-font-size)*.9375)] leading-5 text-content [font-family:var(--font-content-family)] [outline:0] placeholder:text-content-muted max-[650px]:px-3.5 max-[650px]:text-[length:var(--app-message-font-size)]"
            onChange={(event) => {
              setValue(event.target.value)
              setCaret(event.target.selectionStart ?? event.target.value.length)
              event.currentTarget.style.height = 'auto'
              event.currentTarget.style.height = `${Math.min(event.currentTarget.scrollHeight, 220)}px`
            }}
            onSelect={(event) => setCaret(event.currentTarget.selectionStart ?? 0)}
            onPaste={selection.pasteFiles}
            onCompositionStart={() => (composing.current = true)}
            onCompositionEnd={() => window.setTimeout(() => (composing.current = false), 0)}
            onKeyDown={onKeyDown}
          />
          <div className="flex min-w-0 items-center gap-1 px-2 pt-1 pb-2 max-[650px]:gap-2 max-[650px]:pb-3">
            {chooser.input}
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  className="rounded-full"
                  aria-label={t('bots:room.composer.add')}
                  title={t('bots:room.composer.add')}
                >
                  <Plus />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent
                align="start"
                side="top"
                className="w-52"
                onCloseAutoFocus={(event) => {
                  if (!mentionFromMenu.current) return
                  mentionFromMenu.current = false
                  event.preventDefault()
                }}
              >
                <DropdownMenuItem onSelect={() => void chooser.choose()}>
                  <Paperclip aria-hidden="true" />
                  {t('chat:focusSession.addAttachment')}
                </DropdownMenuItem>
                <DropdownMenuItem
                  onSelect={() => {
                    mentionFromMenu.current = true
                    startMention()
                  }}
                >
                  <AtSign aria-hidden="true" />
                  {t('bots:room.composer.mentionMember')}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
            <Button
              variant="ghost"
              size="icon-sm"
              className="rounded-full max-[650px]:hidden"
              aria-label={t('chat:focusSession.addAttachment')}
              title={t('chat:focusSession.addAttachment')}
              onClick={() => void chooser.choose()}
            >
              <Paperclip />
            </Button>
            <span id={toId} className="ml-1 flex-none text-[12.5px] text-content-muted">
              {t('bots:room.composer.sendToLabel')}
            </span>
            <div
              role="group"
              aria-labelledby={toId}
              data-room-recipients
              onWheel={wheelChips}
              className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto overscroll-x-contain [scrollbar-width:none] pointer-coarse:-my-2.5 pointer-coarse:py-2.5 [&::-webkit-scrollbar]:hidden"
            >
              {active.map((member) => {
                const pressed = chosen.includes(member.id)
                return (
                  <Chip
                    key={member.id}
                    size="sm"
                    aria-pressed={pressed}
                    disabled={!member.enabled}
                    className={CHIP_TOUCH}
                    onClick={() => toggle(member.id, !pressed)}
                  >
                    <span
                      aria-hidden="true"
                      className="size-1.5 rounded-full"
                      style={{ background: botAvatarTone(member.avatar.color).shade }}
                    />
                    {member.name}
                    {!member.enabled && (
                      <span className="font-normal text-content-muted">
                        · {t('bots:room.paused')}
                      </span>
                    )}
                  </Chip>
                )
              })}
              {active.length > 1 && (
                <Chip
                  size="sm"
                  data-room-recipients-everyone
                  aria-pressed={everyone}
                  className={CHIP_TOUCH}
                  // 只有主持人分派的群能取消“所有人”（空着交给主持人）；别的群要缩小范围就点掉具体的人。
                  onClick={() => {
                    if (!everyone) onRecipientsChange('all')
                    else if (idleLabel) onRecipientsChange([])
                  }}
                >
                  {t('bots:room.composer.everyone')}
                </Chip>
              )}
            </div>
            {wakes >= 3 ? (
              <span className="flex-none px-1 text-[12px] text-content-muted max-[650px]:hidden">
                {t('bots:room.composer.wakes', { count: wakes })}
              </span>
            ) : addressed && !mentionsAll && chosenMembers.length > 0 ? (
              <span
                data-room-addressed-only
                className="max-w-[40%] min-w-0 flex-none truncate px-1 text-[12px] text-content-muted max-[650px]:hidden"
              >
                {t('bots:room.composer.addressedOnly')}
              </span>
            ) : idleHint && !addressed && chosenMembers.length === 0 ? (
              <span className="max-w-[40%] min-w-0 flex-none truncate px-1 text-[12px] text-content-muted max-[650px]:hidden">
                {idleHint}
              </span>
            ) : null}
            <ComposerSendButton
              queueing={false}
              submitting={sending}
              preparing={preparing}
              hasDraft={Boolean(value.trim() || hasAttachments)}
              disabled={!canSend}
            />
          </div>
        </form>
      </PopoverAnchor>
      <PopoverContent
        side="top"
        align="start"
        className="w-64 p-1"
        onOpenAutoFocus={(event) => event.preventDefault()}
        onCloseAutoFocus={(event) => event.preventDefault()}
      >
        <Command
          value={selected?.id || ''}
          shouldFilter={false}
          aria-label={t('bots:room.composer.mentionList')}
        >
          <CommandList>
            {options.map((option) => (
              <CommandItem
                key={option.id}
                value={option.id}
                data-room-mention-everyone={option.member ? undefined : ''}
                onMouseEnter={() => setHighlight(option.id)}
                onSelect={() => insertMention(option)}
              >
                {option.member ? (
                  <BotAvatar avatar={option.member.avatar} size={18} animate={false} />
                ) : (
                  <Users aria-hidden="true" className="size-[18px] p-0.5 text-content-muted" />
                )}
                <span className="min-w-0 flex-1 truncate">{option.name}</span>
              </CommandItem>
            ))}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
}
