// 设置里 Bot 的两种触发方式（B3），各在一个折叠分区里就地编辑，不弹窗：
// - 可以叫它的聊天：直接点选已接入渠道的私聊、或跟 Pisper 说过话的群，默认只有你能叫它；
//   要让别人也能叫，再手动填写聊天与人的 ID（在聊天里发 /whoami 就能看到）。没写人时私聊、群聊都只认渠道的创建者。
// - 定时让它做：到点给它派一件事（每天、每周或某一天一次），就是 targetType = bot 的定时任务。
import { useCallback, useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { MessageCircle, Plus, Users } from 'lucide-react'
import { PAGE_PATHS } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { InlineEmptyState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { OrbitSpinner } from '@/components/ui/spinner'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { Switch } from '@/components/ui/switch'
import { fetchChannelChats, type ChannelChats } from '@/features/channels/channel-chats'
import {
  BOT_PLATFORMS,
  createBotSchedule,
  deleteBotSchedule,
  fetchBotSchedules,
  setBotScheduleEnabled,
  updateBot,
  type Bot,
  type BotChannel,
  type BotPlatform,
  type BotSchedule,
} from './bots-api'
import { platformLabel, scheduleLabel, splitIds, weekdayLabel } from './bot-labels'
import { useBotsStore } from './bots-store'

// 一条可以直接点选的规则：已接入渠道的私聊，或跟 Pisper 说过话的群。没写名单 = 只有你能叫它。
type ChatOption = {
  key: string
  title: string
  detail: string
  channel: BotChannel
  unavailable: boolean
}

// 已有规则是否已经覆盖这个聊天（“所有聊天”的规则覆盖同一平台、同一类的任意聊天）。
function covered(channels: BotChannel[], option: BotChannel) {
  return channels.some(
    (channel) =>
      channel.platform === option.platform &&
      channel.chatType === option.chatType &&
      channel.senderIds.length === 0 &&
      (channel.peerIds === null ||
        (option.peerIds !== null && option.peerIds.every((id) => channel.peerIds?.includes(id)))),
  )
}

export function ChannelEditor({ bot, onError }: { bot: Bot; onError: (message: string) => void }) {
  const { t } = useI18n()
  const upsert = useBotsStore((state) => state.upsert)
  const [adding, setAdding] = useState(false)
  const [manual, setManual] = useState(false)
  const [saving, setSaving] = useState(false)
  const [chats, setChats] = useState<ChannelChats | null>(null)
  const [chatsError, setChatsError] = useState('')
  const [platform, setPlatform] = useState<BotPlatform>('feishu')
  const [chatType, setChatType] = useState<'p2p' | 'group'>('p2p')
  const [peers, setPeers] = useState('')
  const [senders, setSenders] = useState('')
  const [everyone, setEveryone] = useState(false)

  // 打开分区时问一次接入了哪些渠道（已有规则里的群用它显示名字），每次展开“添加聊天”再拉一次，
  // 接入新渠道后不用刷新页面。
  useEffect(() => {
    const controller = new AbortController()
    setChatsError('')
    fetchChannelChats(controller.signal).then(
      (value) => {
        if (!controller.signal.aborted) setChats(value)
      },
      (caught: unknown) => {
        if (!controller.signal.aborted)
          setChatsError(caught instanceof Error ? caught.message : String(caught))
      },
    )
    return () => controller.abort()
  }, [adding])

  const save = async (channels: BotChannel[]) => {
    setSaving(true)
    try {
      upsert(await updateBot(bot.id, { channels }))
      return true
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught))
      return false
    } finally {
      setSaving(false)
    }
  }
  const close = () => {
    setAdding(false)
    setManual(false)
    setPeers('')
    setSenders('')
    setEveryone(false)
  }
  const addChannel = async (channel: BotChannel) => {
    if (await save([...bot.channels, channel])) close()
  }
  const addManual = () => {
    const peerIds = splitIds(peers)
    void addChannel({
      platform,
      chatType,
      peerIds: peerIds.length ? peerIds : null,
      senderIds: splitIds(senders),
      everyone,
    })
  }

  const options: ChatOption[] = chats
    ? [
        ...chats.channels.map((channel) => ({
          key: `${channel.platform}:p2p`,
          title: t('bots:triggers.pickP2p', { platform: platformLabel(t, channel.platform) }),
          detail: channel.ownerConfigured
            ? t('bots:triggers.ownerOnly')
            : t('bots:triggers.ownerMissing'),
          channel: {
            platform: channel.platform,
            chatType: 'p2p' as const,
            peerIds: null,
            senderIds: [],
            everyone: false,
          },
          unavailable: !channel.ownerConfigured,
        })),
        ...chats.groups.map((group) => {
          const owner = chats.channels.find((channel) => channel.platform === group.platform)
          return {
            key: `${group.platform}:group:${group.peerId}`,
            title: group.title,
            detail: [
              t('bots:triggers.pickGroup', { platform: platformLabel(t, group.platform) }),
              group.lastMessage,
            ]
              .filter(Boolean)
              .join(' · '),
            channel: {
              platform: group.platform,
              chatType: 'group' as const,
              peerIds: [group.peerId],
              senderIds: [],
              everyone: false,
            },
            unavailable: !owner?.ownerConfigured,
          }
        }),
      ]
    : []

  return (
    <div className="flex flex-col">
      <p className="mb-2 text-xs text-content-muted">
        {t('bots:triggers.chatsHint', { name: bot.name })}
      </p>
      {bot.channels.length > 0 && (
        <ul role="list" className="m-0 mb-2 flex list-none flex-col gap-1.5 p-0">
          {bot.channels.map((channel, index) => (
            <li
              key={`${channel.platform}-${index}`}
              data-bot-channel={channel.platform}
              className="flex items-start gap-2 rounded-lg bg-surface-subtle px-2.5 py-2 text-xs"
            >
              <span className="min-w-0 flex-1">
                <b className="font-medium text-content">
                  {platformLabel(t, channel.platform)} ·{' '}
                  {channel.chatType === 'group' ? t('bots:triggers.group') : t('bots:triggers.p2p')}
                </b>
                <span className="block truncate text-content-muted">
                  {channel.peerIds
                    ? t('bots:triggers.peers', {
                        ids: channel.peerIds
                          .map(
                            (id) =>
                              chats?.groups.find(
                                (group) =>
                                  group.platform === channel.platform && group.peerId === id,
                              )?.title || id,
                          )
                          .join('、'),
                      })
                    : t('bots:triggers.allPeers')}
                  {' · '}
                  {channel.senderIds.length
                    ? t('bots:triggers.adminsWith', { ids: channel.senderIds.join('、') })
                    : t('bots:triggers.admins')}
                </span>
                {/* 打开后这个聊天里的其他人也能叫它，但只能用“指令”里公开的那些。 */}
                <label className="mt-1.5 flex items-center gap-2 text-content-secondary">
                  <Switch
                    size="sm"
                    checked={channel.everyone}
                    disabled={saving}
                    onCheckedChange={(checked) =>
                      void save(
                        bot.channels.map((item, position) =>
                          position === index ? { ...item, everyone: checked } : item,
                        ),
                      )
                    }
                  />
                  {t('bots:triggers.everyone')}
                </label>
              </span>
              <Button
                size="sm"
                variant="ghost"
                disabled={saving}
                onClick={() => void save(bot.channels.filter((_, item) => item !== index))}
              >
                {t('bots:triggers.remove')}
              </Button>
            </li>
          ))}
        </ul>
      )}
      {adding ? (
        <div
          data-bot-channel-picker
          className="flex flex-col gap-2 rounded-lg border border-stroke p-2.5"
        >
          {manual ? (
            <>
              <p className="m-0 text-xs text-content-muted">{t('bots:triggers.manualHint')}</p>
              <div className="flex items-center gap-2">
                <AppSelect
                  value={platform}
                  className="w-32"
                  aria-label={t('bots:triggers.platform')}
                  onChange={(event) => setPlatform(event.target.value as BotPlatform)}
                >
                  {BOT_PLATFORMS.map((item) => (
                    <option key={item} value={item}>
                      {platformLabel(t, item)}
                    </option>
                  ))}
                </AppSelect>
                <SegmentedControl
                  value={chatType}
                  onValueChange={(value) => setChatType(value)}
                  options={[
                    { value: 'p2p', label: t('bots:triggers.p2p') },
                    { value: 'group', label: t('bots:triggers.group') },
                  ]}
                />
              </div>
              <Input
                value={peers}
                aria-label={t('bots:triggers.peerIds')}
                placeholder={t('bots:triggers.peerIdsPlaceholder')}
                onChange={(event) => setPeers(event.target.value)}
              />
              <Input
                value={senders}
                aria-label={t('bots:triggers.senderIds')}
                placeholder={t('bots:triggers.senderIdsPlaceholder')}
                onChange={(event) => setSenders(event.target.value)}
              />
              <label className="flex items-center gap-2 text-xs text-content-secondary">
                <Switch size="sm" checked={everyone} onCheckedChange={setEveryone} />
                {t('bots:triggers.everyone')}
              </label>
            </>
          ) : chatsError ? (
            <p role="alert" className="m-0 text-xs text-destructive">
              {chatsError}
            </p>
          ) : !chats ? (
            <div role="status" className="grid min-h-16 place-items-center">
              <OrbitSpinner className="size-4" />
            </div>
          ) : options.length === 0 ? (
            <InlineEmptyState
              compact
              icon={MessageCircle}
              title={t('bots:triggers.noChannels')}
              description={t('bots:triggers.noChannelsHint')}
              action={
                <Button asChild size="sm" variant="outline">
                  <Link to={PAGE_PATHS.channels}>{t('bots:triggers.openChannels')}</Link>
                </Button>
              }
            />
          ) : (
            <ul
              role="list"
              aria-label={t('bots:triggers.pickTitle')}
              className="m-0 flex list-none flex-col p-0"
            >
              {options.map((option) => {
                const added = covered(bot.channels, option.channel)
                const Icon = option.channel.chatType === 'group' ? Users : MessageCircle
                return (
                  <li key={option.key}>
                    <button
                      type="button"
                      data-bot-chat-option={option.key}
                      className="flex min-h-11 w-full min-w-0 items-center gap-2.5 rounded-md px-2 py-1.5 text-left outline-none hover:bg-surface-hover focus-visible:ring-3 focus-visible:ring-focus-ring disabled:cursor-not-allowed disabled:opacity-50"
                      disabled={saving || added || option.unavailable}
                      onClick={() => void addChannel(option.channel)}
                    >
                      <Icon className="size-4 flex-none text-content-muted" aria-hidden="true" />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-sm text-content">{option.title}</span>
                        <span className="block truncate text-xs text-content-muted">
                          {option.detail}
                        </span>
                      </span>
                      {added ? (
                        <span className="flex-none text-xs text-content-muted">
                          {t('bots:triggers.added')}
                        </span>
                      ) : (
                        <Plus className="size-4 flex-none text-content-muted" aria-hidden="true" />
                      )}
                    </button>
                  </li>
                )
              })}
            </ul>
          )}
          <div className="flex items-center justify-between gap-2">
            {manual ? (
              <span />
            ) : (
              <Button size="sm" variant="ghost" onClick={() => setManual(true)}>
                {t('bots:triggers.manual')}
              </Button>
            )}
            <div className="flex gap-2">
              <Button size="sm" variant="ghost" onClick={close}>
                {t('bots:triggers.cancel')}
              </Button>
              {manual && (
                <Button size="sm" disabled={saving} onClick={addManual}>
                  {t('bots:triggers.add')}
                </Button>
              )}
            </div>
          </div>
        </div>
      ) : (
        <Button size="sm" variant="outline" onClick={() => setAdding(true)}>
          {t('bots:triggers.addChat')}
        </Button>
      )}
    </div>
  )
}

const WEEKDAYS = [1, 2, 3, 4, 5, 6, 0] as const

function today() {
  const now = new Date()
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`
}

// 新建一条定时（设置页“定时”分区与“它学会的”里把技能设成定时共用）：到点把这句话发给 Bot。
export function ScheduleForm({
  bot,
  initialMessage = '',
  onCancel,
  onCreated,
  onError,
}: {
  bot: Bot
  initialMessage?: string
  onCancel: () => void
  onCreated: () => void
  onError: (message: string) => void
}) {
  const { t } = useI18n()
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState(initialMessage)
  const [frequency, setFrequency] = useState<'daily' | 'weekly' | 'once'>(
    initialMessage ? 'weekly' : 'daily',
  )
  const [time, setTime] = useState('09:00')
  const [date, setDate] = useState(today)
  const [dayOfWeek, setDayOfWeek] = useState(initialMessage ? 5 : 1)
  const add = async () => {
    const text = message.trim()
    setBusy(true)
    try {
      await createBotSchedule({
        botId: bot.id,
        name: `${bot.name}：${text.slice(0, 40)}`,
        botMessage: text,
        frequency,
        time,
        date: frequency === 'once' ? date : '',
        dayOfWeek,
        timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'Asia/Hong_Kong',
      })
      onCreated()
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-stroke p-2.5">
      <Input
        value={message}
        maxLength={12_000}
        aria-label={t('bots:triggers.scheduleMessage')}
        placeholder={t('bots:triggers.scheduleMessagePlaceholder')}
        onChange={(event) => setMessage(event.target.value)}
      />
      <div className="flex flex-wrap items-center gap-2">
        <AppSelect
          value={frequency}
          className="w-28"
          aria-label={t('bots:triggers.frequency')}
          onChange={(event) => setFrequency(event.target.value as typeof frequency)}
        >
          <option value="daily">{t('bots:triggers.daily')}</option>
          <option value="weekly">{t('bots:triggers.weekly')}</option>
          <option value="once">{t('bots:triggers.once')}</option>
        </AppSelect>
        {frequency === 'weekly' && (
          <AppSelect
            value={String(dayOfWeek)}
            className="w-24"
            aria-label={t('bots:triggers.weekday')}
            onChange={(event) => setDayOfWeek(Number(event.target.value))}
          >
            {WEEKDAYS.map((day) => (
              <option key={day} value={day}>
                {weekdayLabel(t, day)}
              </option>
            ))}
          </AppSelect>
        )}
        {frequency === 'once' && (
          <Input
            type="date"
            value={date}
            className="w-40"
            aria-label={t('bots:triggers.date')}
            onChange={(event) => setDate(event.target.value)}
          />
        )}
        <Input
          type="time"
          value={time}
          className="w-28"
          aria-label={t('bots:triggers.time')}
          onChange={(event) => setTime(event.target.value)}
        />
      </div>
      <div className="flex justify-end gap-2">
        <Button size="sm" variant="ghost" onClick={onCancel}>
          {t('bots:triggers.cancel')}
        </Button>
        <Button size="sm" disabled={busy || !message.trim()} onClick={() => void add()}>
          {t('bots:triggers.add')}
        </Button>
      </div>
    </div>
  )
}

export function ScheduleEditor({ bot, onError }: { bot: Bot; onError: (message: string) => void }) {
  const { t } = useI18n()
  const [schedules, setSchedules] = useState<BotSchedule[]>([])
  const [adding, setAdding] = useState(false)
  const [busy, setBusy] = useState(false)
  const load = useCallback(async () => {
    try {
      setSchedules(await fetchBotSchedules(bot.id))
    } catch {
      // 定时任务能力不可用时只是不显示这一节的列表。
    }
  }, [bot.id])
  useEffect(() => {
    void load()
  }, [load])
  const run = async (action: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await action()
      await load()
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="flex flex-col">
      {schedules.length > 0 && (
        <ul role="list" className="m-0 mb-2 flex list-none flex-col gap-1.5 p-0">
          {schedules.map((schedule) => (
            <li
              key={schedule.id}
              data-bot-schedule={schedule.id}
              className="flex items-center gap-2 rounded-lg bg-surface-subtle px-2.5 py-2 text-xs"
            >
              <span className="min-w-0 flex-1">
                <b className="block truncate font-medium text-content">{schedule.botMessage}</b>
                <span className="text-content-muted">{scheduleLabel(t, schedule)}</span>
              </span>
              <Switch
                checked={schedule.enabled}
                disabled={busy}
                aria-label={t('bots:triggers.scheduleEnabled')}
                onCheckedChange={(enabled) =>
                  void run(() => setBotScheduleEnabled(schedule.id, enabled))
                }
              />
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                onClick={() => void run(() => deleteBotSchedule(schedule.id))}
              >
                {t('bots:triggers.remove')}
              </Button>
            </li>
          ))}
        </ul>
      )}
      {adding ? (
        <ScheduleForm
          bot={bot}
          onCancel={() => setAdding(false)}
          onCreated={() => {
            setAdding(false)
            void load()
          }}
          onError={onError}
        />
      ) : (
        <Button size="sm" variant="outline" onClick={() => setAdding(true)}>
          {t('bots:triggers.addSchedule')}
        </Button>
      )}
    </div>
  )
}
