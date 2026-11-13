// Bot 群聊页（/bots/rooms/:roomId），第一版的样子：一份你和几个 Bot 共用的聊天记录。@ 谁（或“发给”谁）
// 就叫醒谁；叫醒一次就是一轮，在它自己的席位会话里干活，“查看过程”看。页头一行：成员头像、群名、
// “N 个 Bot · 目录”，右边用量、简报、成员、更多、右侧面板开关；简报与成员在右侧面板里（一次一个，
// 手机上从“更多”打开底部表单）。在干活的 Bot 画成输入框上方的状态行，聊长了、简报没人维护时问一句找谁整理。
// 数据由 useRoomLive 取（有 room 帧时按帧取，否则轮询）。
import './bots-messages'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { Ellipsis, NotebookPen, PanelRight, Users } from 'lucide-react'
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
import { botModelLabel, executionModeLabel } from './bot-labels'
import { useBotsPolling, useBotsStore } from './bots-store'
import {
  briefPromptStats,
  compactTokens,
  roomStatusEntries,
  timelineItems,
} from './room-presentation'
import { RoomAvatarStack } from './RoomAvatarStack'
import { RoomComposer } from './RoomComposer'
import { RoomBriefDialog } from './RoomDialogs'
import { RoomBriefPanel, RoomInfoPanel, type RoomSettingsPatch } from './RoomPanels'
import { RoomBriefPrompt, RoomStatusRows } from './RoomStatusRows'
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
  type RoomAnswerMode,
  type RoomMember,
  type RoomMessage,
} from './rooms-api'
import { useRoomsStore } from './rooms-store'
import { useRoomLive } from './use-room-live'

type RoomPageProps = { notify: Notify; phoneLeading?: ReactNode }
type RoomPanel = 'brief' | 'members'

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
  // 右侧面板（桌面）/ 底部表单（手机）：开着哪一个；panelTab 记着上次开的，面板开关键按它重新打开。
  const [panel, setPanel] = useState<RoomPanel | null>(null)
  const [panelTab, setPanelTab] = useState<RoomPanel>('brief')
  // “找谁写一份简报”：稍后 = 这次打开群聊不再问；这段不用 = 到这一条为止的都不必整理。
  const [promptLater, setPromptLater] = useState(false)
  const [promptSkipUpTo, setPromptSkipUpTo] = useState(0)
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

  // 时间线里的名字和头像：现在的成员，加上移出群但还在的 Bot（它之前说的话照常署名，
  // 不显示成“已删除的 Bot”）。真正删掉的 Bot 两边都找不到，才退回“已删除的 Bot”。
  const members = useMemo(() => {
    const map = new Map((detail?.members || []).map((member) => [member.id, member]))
    for (const bot of bots)
      if (!map.has(bot.id))
        map.set(bot.id, {
          id: bot.id,
          name: bot.name,
          avatar: bot.avatar,
          enabled: bot.enabled,
          deleted: false,
          sessionId: '',
          readSeq: 0,
          unread: 0,
          status: { state: 'idle', waiting: null, current: '' },
          agents: { working: 0, current: '' },
        })
    return map
  }, [detail?.members, bots])
  const items = useMemo(() => timelineItems(messages), [messages])
  const statusEntries = useMemo(() => roomStatusEntries(messages), [messages])
  const botsById = useMemo(() => new Map(bots.map((bot) => [bot.id, bot])), [bots])
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
    onJump: (messageId) => {
      const node = listRef.current?.querySelector(`[data-room-message="${CSS.escape(messageId)}"]`)
      if (node instanceof HTMLElement) {
        // 跳过去的那条同时拿到焦点：接着用方向键从这里往下看。
        node.focus({ preventScroll: true })
        node.scrollIntoView({ block: 'center', behavior: 'smooth' })
        node.classList.add('ring-2', 'ring-focus-ring')
        window.setTimeout(() => node.classList.remove('ring-2', 'ring-focus-ring'), 1_200)
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
      if (result.stopped.length)
        notify(
          t('bots:room.recall.stopped', {
            names: result.stopped
              .map((botId) => members.get(botId)?.name || t('bots:room.removedBot'))
              .join('、'),
          }),
        )
      if (result.briefMayContain) notify(t('bots:room.recall.briefNote'))
      if (result.attachmentsKept > 0)
        notify(t('bots:room.recall.attachmentsKept', { count: result.attachmentsKept }))
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

  const approve = (message: RoomMessage, approvalId: string, allow: boolean) =>
    once(`approval:${approvalId}`, () => resolveRoomApproval(roomId, message.id, approvalId, allow))

  const changeAnswerMode = (answerMode: RoomAnswerMode, hostBotId: string) => {
    // 换了谁来回答，Runtime 会按新模式给“发给”一个默认值：本地记着的选择作废，以群里存的为准。
    void updateRoom(roomId, { answerMode, hostBotId })
      .then((next) => {
        setRecipients(null)
        upsertRoom(next)
        return live.poll()
      })
      .catch(reportError)
  }

  // 简报 / 成员：同一个再点一下收起；面板开关键重新打开上次那个。
  const togglePanel = (next: RoomPanel) => {
    setPanelTab(next)
    setPanel((current) => (current === next ? null : next))
  }

  // 群信息面板里改一项就存一项；失败时报错并告诉面板没存上，草稿留着。
  const saveSettings = async (patch: RoomSettingsPatch) => {
    try {
      upsertRoom(await updateRoom(roomId, patch))
      await live.poll()
      return true
    } catch (caught) {
      reportError(caught)
      return false
    }
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
  const panelContent =
    panel === 'members' ? (
      <RoomInfoPanel
        detail={detail}
        bots={bots}
        hostAvailable={hostAvailable}
        onOpenMember={(member: RoomMember) =>
          member.sessionId && navigate(sessionPath(member.sessionId))
        }
        onSave={saveSettings}
        onAnswerModeChange={changeAnswerMode}
        onDelete={() => setDeleting(true)}
        onError={reportError}
      />
    ) : (
      <RoomBriefPanel
        detail={detail}
        briefing={briefing || detail.briefing}
        onTidy={(botId) => void tidy(botId)}
        onEditBrief={() => setBriefOpen(true)}
      />
    )
  const panelTitle = panel === 'members' ? t('bots:room.header.info') : t('bots:room.brief')
  const prompt =
    promptLater || briefing || detail.briefing
      ? null
      : briefPromptStats({
          messages,
          brief: detail.brief,
          lastSeq: detail.lastSeq,
          maintained: Boolean(room.briefBotId),
          skipUpTo: promptSkipUpTo,
        })
  // 页头“N 个 Bot · 目录”只写目录的最后一段（完整路径在群设置里）。
  const folder = room.cwd.split(/[\\/]/).filter(Boolean).at(-1) || ''
  const modelLabel = (botId: string) => {
    const bot = botsById.get(botId)
    return bot ? botModelLabel(t, bot.model) : ''
  }
  const modeLabel = (botId: string) => {
    const bot = botsById.get(botId)
    return bot ? executionModeLabel(t, bot.executionMode) : ''
  }
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
    <div
      data-room-page={room.id}
      className="flex h-full min-h-0 bg-[var(--main-surface-bg,var(--panel))]"
    >
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        {/* Windows 自绘的窗口按钮压在窗口右上角：右侧面板没开时页头右边给它们让出位置（开着时它们压在面板上）。 */}
        <header
          className={cn(
            'flex h-[52px] flex-none items-center gap-2 px-4 max-[650px]:h-[calc(52px+var(--pisper-safe-area-top,0px))] max-[650px]:px-2 max-[650px]:pt-[var(--pisper-safe-area-top,0px)]',
            !(panel && !phone) && '[[data-custom-titlebar]_&]:pr-[144px]',
          )}
        >
          {phone && phoneLeading}
          <RoomAvatarStack avatars={activeMembers.map((member) => member.avatar)} />
          <h1 className="m-0 min-w-0 truncate text-[15px] font-semibold text-content">
            {room.name}
          </h1>
          <span className="flex-none text-[13px] text-content-muted max-[650px]:hidden">
            {folder
              ? t('bots:room.header.summaryWithFolder', {
                  count: activeMembers.length,
                  folder,
                })
              : t('bots:room.header.summary', { count: activeMembers.length })}
          </span>
          <span className="flex-1" />
          {detail.totalTokens > 0 && (
            <span className="flex-none text-[12px] text-content-muted tabular-nums max-[650px]:hidden">
              {t('bots:room.usage', { tokens: compactTokens(detail.totalTokens) })}
            </span>
          )}
          {!phone && (
            <>
              <Button
                variant="ghost"
                size="sm"
                aria-pressed={panel === 'brief'}
                onClick={() => togglePanel('brief')}
              >
                <NotebookPen />
                {t('bots:room.brief')}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                aria-pressed={panel === 'members'}
                onClick={() => togglePanel('members')}
              >
                <Users />
                {t('bots:room.header.info')}
              </Button>
            </>
          )}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon-sm" aria-label={t('bots:room.more')}>
                <Ellipsis />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {phone && (
                <>
                  <DropdownMenuItem onSelect={() => setPanel('brief')}>
                    <NotebookPen aria-hidden="true" />
                    {t('bots:room.brief')}
                  </DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => setPanel('members')}>
                    <Users aria-hidden="true" />
                    {t('bots:room.header.info')}
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                </>
              )}
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
              aria-pressed={Boolean(panel)}
              onClick={() => setPanel(panel ? null : panelTab)}
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
          <div className="mx-auto w-full max-w-[768px] px-6 pt-2 pb-4 max-[650px]:px-4">
            {live.hasMore && (
              <div className="flex justify-center">
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => void live.loadOlder().catch(reportError)}
                >
                  {t('bots:room.loadOlder')}
                </Button>
              </div>
            )}
            {items.length ? (
              <RoomTimeline
                items={items}
                messages={messages}
                members={members}
                modelLabel={modelLabel}
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

        <div className="mx-auto w-full max-w-[816px] flex-none px-6 pb-4 max-[650px]:px-3 max-[650px]:pb-[calc(12px+var(--pisper-safe-area-bottom,0px))]">
          {live.error && (
            <p role="status" className="m-0 mb-2 text-center text-[12px] text-content-muted">
              {t('bots:room.reconnecting')}
            </p>
          )}
          <RoomStatusRows
            entries={statusEntries}
            routing={detail.routing}
            host={(room.answerMode === 'host' && members.get(room.hostBotId)) || null}
            members={members}
            modeLabel={modeLabel}
            onOpenProcess={(sessionId) => navigate(sessionPath(sessionId))}
            onApprove={approve}
            onStop={(message) => once(`stop:${message.id}`, () => stopRoomTurn(roomId, message.id))}
          />
          {prompt && (
            <RoomBriefPrompt
              count={prompt.count}
              chars={prompt.chars}
              members={activeMembers}
              phone={phone}
              onPick={(botId) => void tidy(botId)}
              onWrite={() => setBriefOpen(true)}
              onLater={() => setPromptLater(true)}
              onSkip={() => setPromptSkipUpTo(detail.lastSeq)}
              onOpen={() => setPanel('brief')}
            />
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
            idleHint={hostName ? t('bots:room.composer.hostHint', { name: hostName }) : undefined}
          />
        </div>
      </div>

      {!phone && panel && (
        <aside
          aria-label={panelTitle}
          // 面板顶上是窗口按钮（Windows 自绘标题栏）：内容从按钮下面开始，简报的编辑键不会被压住。
          className="w-[288px] flex-none overflow-y-auto border-l border-stroke-soft px-3 py-3 [[data-custom-titlebar]_&]:pt-[52px]"
        >
          {panelContent}
        </aside>
      )}

      {phone && (
        <Sheet open={Boolean(panel)} onOpenChange={(open) => !open && setPanel(null)}>
          <SheetContent
            side="bottom"
            className="max-h-[85dvh] overflow-y-auto px-4 pt-12 pb-[calc(16px+var(--pisper-safe-area-bottom,0px))]"
          >
            <SheetTitle className="sr-only">{panelTitle}</SheetTitle>
            <SheetDescription className="sr-only">{room.name}</SheetDescription>
            {panelContent}
          </SheetContent>
        </Sheet>
      )}

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
              {recallSeen.length > 0 && Boolean(recalling?.attachments.length) && (
                <> {t('bots:room.recallConfirm.attachments')}</>
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
