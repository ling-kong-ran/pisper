// Bot 群聊页（/bots/rooms/:roomId）：一份你和几个 Bot 共用的聊天记录。Bot 默认不说话，@ 谁（或“发给”谁）
// 就叫醒谁；叫醒一次就是一轮，在它自己的席位会话里干活，过程点“过程”看。页头写目标与用量；右栏是简报与成员
// （手机上收进页头的“成员”按钮）。数据由 useRoomLive 轮询，Bot 在干活时更勤。
import './bots-messages'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { Ellipsis, PanelRight, Target, Users } from 'lucide-react'
import type { ReactNode } from 'react'
import type { Notify } from '@/app/route-context'
import { pagePath, sessionPath } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { PageState } from '@/components/app/PageState'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { useBetaFeature } from '@/features/labs/public-flags'
import { usePhoneShell } from '@/hooks/usePhoneShell'
import { cn } from '@/lib/utils'
import type { ChatAttachment } from '@/types/chat'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { useBotsPolling, useBotsStore } from './bots-store'
import { compactTokens, timelineItems } from './room-presentation'
import { RoomComposer } from './RoomComposer'
import { RoomBriefDialog, RoomSettingsDialog, type RoomSettingsDraft } from './RoomDialogs'
import { RoomRail } from './RoomRail'
import { RoomTimeline, type TimelineActions } from './RoomTimeline'
import {
  deleteRoom,
  editRoomBrief,
  postRoomMessage,
  recallRoomMessage,
  refreshRoomBrief,
  resolveRoomApproval,
  stopRoomTurn,
  updateRoom,
  type RoomMember,
  type RoomMessage,
} from './rooms-api'
import { useRoomsStore } from './rooms-store'
import { useRoomLive } from './use-room-live'

type RoomPageProps = { notify: Notify; phoneLeading?: ReactNode }

// 每个群一份页面状态：换群时整页重挂，上一个群的“回复”引用、没发的草稿、开着的对话框不会带过来。
export function RoomPage(props: RoomPageProps) {
  const roomId = String(useParams().roomId || '')
  return <RoomView key={roomId} roomId={roomId} {...props} />
}

function RoomView({ roomId, notify, phoneLeading }: RoomPageProps & { roomId: string }) {
  const { t } = useI18n()
  const navigate = useNavigate()
  const enabled = useBetaFeature('bots')
  // 和应用壳同一个判断：手机外壳不画通用顶栏（见 App 的 phoneTopBar），这里自带顶栏。
  const phone = usePhoneShell()
  useBotsPolling(enabled)
  const bots = useBotsStore((state) => state.bots)
  const upsertRoom = useRoomsStore((state) => state.upsert)
  const removeRoom = useRoomsStore((state) => state.remove)
  const hostAvailable = useRuntimeCapabilitiesStore(
    (state) => state.capabilities.features['rooms.host'] === true,
  )
  const live = useRoomLive(roomId)
  const { detail, messages } = live
  const [replyTo, setReplyTo] = useState<RoomMessage | null>(null)
  const [recalling, setRecalling] = useState<RoomMessage | null>(null)
  const [railOpen, setRailOpen] = useState(true)
  const [sheetOpen, setSheetOpen] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [briefOpen, setBriefOpen] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const [briefing, setBriefing] = useState(false)
  // “发给”改过之后以本地为准：保存之前发出的轮询晚到时带的还是旧值，不能把刚选的盖回去。
  const [recipients, setRecipients] = useState<string[] | 'all' | null>(null)
  // 正在处理的批准 / 停下（按卡记），连点不重复发。
  const acting = useRef(new Set<string>())
  const listRef = useRef<HTMLDivElement>(null)
  // 撤回确认框是按状态打开的（没有触发按钮）：关掉时焦点回到打开它的那条消息或按钮上，
  // 键盘用户接着从原处往下走，不掉回页面开头。
  const recallReturn = useRef<{ node: HTMLElement | null; id: string } | null>(null)

  const members = useMemo(
    () => new Map((detail?.members || []).map((member) => [member.id, member])),
    [detail?.members],
  )
  const items = useMemo(() => timelineItems(messages), [messages])
  const reportError = useCallback(
    (caught: unknown) => notify(caught instanceof Error ? caught.message : String(caught), 'error'),
    [notify],
  )

  // 新消息到来时：本来就停在底部的话跟着滚到底；往上翻着看旧消息时不打扰。
  const lastId = messages.at(-1)?.id
  const stickRef = useRef(true)
  const listCallback = useCallback(
    (node: HTMLDivElement | null) => {
      listRef.current = node
      if (node && stickRef.current) node.scrollTop = node.scrollHeight
    },
    // lastId 变化时重新挂一次回调，借此滚到底。
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [lastId, live.loading],
  )

  // 群在别处被删了：说一声，退回 Bots 页。
  const goneName = detail?.room.name || ''
  useEffect(() => {
    if (!live.gone) return
    removeRoom(roomId)
    notify(t('bots:room.gone', { name: goneName }), 'error')
    navigate(pagePath('bots'), { replace: true })
  }, [goneName, live.gone, navigate, notify, removeRoom, roomId, t])

  const once = (key: string, task: () => Promise<unknown>) => {
    if (acting.current.has(key)) return
    acting.current.add(key)
    void task()
      .then(() => live.poll())
      .catch(reportError)
      .finally(() => acting.current.delete(key))
  }

  const nameOf = (author: RoomMessage['author']) =>
    author.kind === 'bot'
      ? members.get(author.botId)?.name || t('bots:room.removedBot')
      : t('bots:room.you')

  const actions: TimelineActions = {
    onReply: (message) => setReplyTo(message),
    onRecall: (message) => {
      recallReturn.current = {
        node: document.activeElement instanceof HTMLElement ? document.activeElement : null,
        id: message.id,
      }
      setRecalling(message)
    },
    onOpenProcess: (sessionId) => navigate(sessionPath(sessionId)),
    onApprove: (message, approvalId, approve) =>
      once(`approval:${approvalId}`, () =>
        resolveRoomApproval(roomId, message.id, approvalId, approve),
      ),
    onStop: (message) => once(`stop:${message.id}`, () => stopRoomTurn(roomId, message.id)),
    onJump: (messageId) => {
      const node = listRef.current?.querySelector(`[data-room-message="${CSS.escape(messageId)}"]`)
      if (node instanceof HTMLElement) {
        // 跳过去的那条同时拿到焦点：接着用方向键从这里往下看。
        node.focus({ preventScroll: true })
        node.scrollIntoView({ block: 'center', behavior: 'smooth' })
        node.classList.add('ring-2', 'ring-focus-ring', 'rounded-xl')
        window.setTimeout(
          () => node.classList.remove('ring-2', 'ring-focus-ring', 'rounded-xl'),
          1_200,
        )
      }
    },
  }

  const send = async (text: string, mentions: string[], attachments: ChatAttachment[]) => {
    if (!detail) return false
    try {
      const result = await postRoomMessage(roomId, {
        text,
        ...(attachments.length ? { attachments } : {}),
        mentions,
        recipients: recipients ?? detail.room.recipients,
        ...(replyTo ? { replyTo: replyTo.id } : {}),
      })
      live.replace(result.message)
      setReplyTo(null)
      stickRef.current = true
      live.pollSoon()
      return true
    } catch (caught) {
      reportError(caught)
      return false
    }
  }

  const changeRecipients = (value: string[] | 'all') => {
    if (!detail) return
    // 先改本地，输入框立刻反映；发消息时 Runtime 也会记下这次的选择。没存上就退回群里记着的。
    setRecipients(value)
    void updateRoom(roomId, { recipients: value })
      .then(upsertRoom)
      .catch((caught: unknown) => {
        setRecipients(null)
        reportError(caught)
      })
  }

  const confirmRecall = async () => {
    const message = recalling
    setRecalling(null)
    if (!message) return
    try {
      const result = await recallRoomMessage(roomId, message.id)
      live.replace({
        ...message,
        text: '',
        attachments: [],
        mentions: [],
        recalled: { seenBy: result.seenBy },
      })
      if (result.briefMayContain) notify(t('bots:room.recall.briefNote'))
    } catch (caught) {
      reportError(caught)
    }
  }

  const tidy = async (botId?: string) => {
    setBriefing(true)
    try {
      await refreshRoomBrief(roomId, botId)
      await live.poll()
    } catch (caught) {
      reportError(caught)
    } finally {
      setBriefing(false)
    }
  }

  const saveSettings = async (draft: Partial<RoomSettingsDraft>) => {
    const room = await updateRoom(roomId, draft)
    upsertRoom(room)
    await live.poll()
  }

  const remove = async () => {
    setDeleting(false)
    try {
      await deleteRoom(roomId)
      removeRoom(roomId)
      notify(t('bots:room.deleted', { name: detail?.room.name || '' }))
      navigate(pagePath('bots'))
    } catch (caught) {
      reportError(caught)
    }
  }

  if (!enabled)
    return (
      <PageState
        kind="empty"
        title={t('bots:room.labsOff')}
        description={t('bots:room.labsOffHint')}
      />
    )
  if (live.loading) return <PageState kind="loading" title={t('bots:room.loading')} />
  if (!detail)
    return (
      <PageState
        kind="error"
        title={t('bots:room.loadFailed')}
        description={live.error}
        action={
          <Button variant="outline" onClick={() => navigate(pagePath('bots'))}>
            {t('bots:room.backToBots')}
          </Button>
        }
      />
    )

  const { room } = detail
  const activeMembers = detail.members.filter((member) => !member.deleted)
  // 主持人分派：没点名的话不再是“只留言”，而是交给主持人。
  const hostName =
    hostAvailable && room.answerMode === 'host'
      ? members.get(room.hostBotId)?.name || t('bots:room.removedBot')
      : ''
  const rail = (
    <RoomRail
      detail={detail}
      briefing={briefing || detail.briefing}
      onTidy={(botId) => void tidy(botId)}
      onEditBrief={() => setBriefOpen(true)}
      onOpenMember={(member: RoomMember) =>
        member.sessionId && navigate(sessionPath(member.sessionId))
      }
    />
  )
  const recallSeen =
    recalling && !recalling.turn
      ? detail.members
          .filter((member) => member.readSeq >= recalling.seq)
          .map((member) => member.name)
      : recalling
        ? detail.members
            .filter(
              (member) =>
                member.readSeq >= recalling.seq &&
                !(recalling.author.kind === 'bot' && recalling.author.botId === member.id),
            )
            .map((member) => member.name)
        : []

  return (
    <div data-room-page={room.id} className="flex h-full min-h-0 bg-panel">
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <header className="flex min-h-[52px] flex-none items-center gap-2 px-4 max-[650px]:min-h-14 max-[650px]:px-3 max-[650px]:pt-[var(--pisper-safe-area-top,0px)]">
          {phone && phoneLeading}
          <div className="min-w-0 flex-1 py-1.5">
            <h1 className="m-0 truncate text-[15px] font-semibold text-content">{room.name}</h1>
            {room.goal && (
              <p className="m-0 flex min-w-0 items-center gap-1 text-[12px] text-content-muted">
                <Target className="size-3 flex-none" aria-hidden="true" />
                <span className="truncate">{room.goal}</span>
              </p>
            )}
          </div>
          {detail.totalTokens > 0 && (
            <span className="flex-none text-[12px] text-content-muted tabular-nums max-[650px]:hidden">
              {t('bots:room.usage', { tokens: compactTokens(detail.totalTokens) })}
            </span>
          )}
          {phone && (
            <Button
              variant="ghost"
              size="icon-sm"
              className="relative"
              aria-label={t('bots:room.openRail')}
              onClick={() => setSheetOpen(true)}
            >
              <Users />
              {detail.activity.needsYou && (
                <span
                  aria-hidden="true"
                  className="absolute top-1.5 right-1.5 size-1.5 rounded-full bg-st-attn"
                />
              )}
            </Button>
          )}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon-sm" aria-label={t('bots:room.more')}>
                <Ellipsis />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={() => setSettingsOpen(true)}>
                {t('bots:room.settings.title')}
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => setBriefOpen(true)}>
                {t('bots:room.brief.edit')}
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem variant="destructive" onSelect={() => setDeleting(true)}>
                {t('bots:room.delete')}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          {!phone && (
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={t('bots:room.toggleRail')}
              aria-pressed={railOpen}
              onClick={() => setRailOpen(!railOpen)}
              className="max-[1100px]:hidden"
            >
              <PanelRight />
            </Button>
          )}
        </header>

        <div
          ref={listCallback}
          data-room-timeline
          className="min-h-0 flex-1 overflow-y-auto"
          onScroll={(event) => {
            const node = event.currentTarget
            stickRef.current = node.scrollHeight - node.scrollTop - node.clientHeight < 80
          }}
        >
          <div className="mx-auto flex w-full max-w-[720px] flex-col gap-4 px-6 pt-3 pb-6 max-[650px]:px-4">
            {live.hasMore && (
              <Button
                variant="ghost"
                size="sm"
                className="self-center"
                onClick={() => void live.loadOlder().catch(reportError)}
              >
                {t('bots:room.loadOlder')}
              </Button>
            )}
            {items.length ? (
              <RoomTimeline
                items={items}
                messages={messages}
                members={members}
                canAct
                actions={actions}
              />
            ) : (
              <PageState
                kind="empty"
                title={t('bots:room.empty.title')}
                description={t('bots:room.empty.description')}
              />
            )}
          </div>
        </div>

        <div className="mx-auto w-full max-w-[768px] flex-none px-6 pb-4 max-[650px]:px-3 max-[650px]:pb-[calc(12px+var(--pisper-safe-area-bottom,0px))]">
          {live.error && (
            <p role="status" className="m-0 mb-2 text-center text-[12px] text-content-muted">
              {t('bots:room.reconnecting')}
            </p>
          )}
          <RoomComposer
            roomId={room.id}
            cwd={room.cwd || undefined}
            members={activeMembers}
            recipients={recipients ?? room.recipients}
            onRecipientsChange={changeRecipients}
            replyTo={replyTo}
            replyName={replyTo ? nameOf(replyTo.author) : ''}
            onCancelReply={() => setReplyTo(null)}
            onSend={send}
            placeholder={
              hostName
                ? phone
                  ? t('bots:room.composer.placeholderHostPhone', { name: hostName })
                  : t('bots:room.composer.placeholderHost', { name: hostName })
                : phone
                  ? t('bots:room.composer.placeholderPhone')
                  : t('bots:room.composer.placeholder')
            }
            idleLabel={hostName ? t('bots:room.composer.toHost') : undefined}
          />
        </div>
      </div>

      {!phone && railOpen && (
        <aside
          aria-label={t('bots:room.rail')}
          className={cn(
            'w-[288px] flex-none overflow-y-auto border-l border-stroke-soft px-3 py-3 max-[1100px]:hidden',
          )}
        >
          {rail}
        </aside>
      )}

      {phone && (
        <Sheet open={sheetOpen} onOpenChange={setSheetOpen}>
          <SheetContent
            side="bottom"
            className="max-h-[85dvh] overflow-y-auto px-4 pt-12 pb-[calc(16px+var(--pisper-safe-area-bottom,0px))]"
          >
            <SheetTitle className="sr-only">{t('bots:room.rail')}</SheetTitle>
            <SheetDescription className="sr-only">{room.name}</SheetDescription>
            {rail}
          </SheetContent>
        </Sheet>
      )}

      <RoomSettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        room={room}
        bots={bots}
        hostAvailable={hostAvailable}
        onSave={saveSettings}
      />
      <RoomBriefDialog
        open={briefOpen}
        onOpenChange={setBriefOpen}
        brief={detail.brief}
        members={activeMembers}
        onSave={async (brief) => {
          await editRoomBrief(roomId, brief)
          await live.poll()
        }}
      />
      <AlertDialog open={Boolean(recalling)} onOpenChange={(open) => !open && setRecalling(null)}>
        <AlertDialogContent
          onCloseAutoFocus={(event) => {
            const back = recallReturn.current
            recallReturn.current = null
            // 撤回之后那条换成了占位、按钮不在了：落到这条消息本身。
            const target = back?.node?.isConnected
              ? back.node
              : back
                ? listRef.current?.querySelector(`[data-room-message="${CSS.escape(back.id)}"]`)
                : null
            if (!(target instanceof HTMLElement)) return
            event.preventDefault()
            target.focus({ preventScroll: true })
          }}
        >
          <AlertDialogHeader>
            <AlertDialogTitle>
              {recalling?.author.kind === 'bot'
                ? t('bots:room.recallConfirm.hideTitle')
                : t('bots:room.recallConfirm.title')}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t('bots:room.recallConfirm.description')}
              {recallSeen.length > 0 && (
                <> {t('bots:room.recallConfirm.seen', { names: recallSeen.join('、') })}</>
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t('common:ui.cancel')}</AlertDialogCancel>
            <AlertDialogAction onClick={() => void confirmRecall()}>
              {recalling?.author.kind === 'bot' ? t('bots:room.hide') : t('bots:room.recall')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <AlertDialog open={deleting} onOpenChange={setDeleting}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t('bots:room.deleteConfirm.title', { name: room.name })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t('bots:room.deleteConfirm.description')}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t('common:ui.cancel')}</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={() => void remove()}>
              {t('bots:room.delete')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
