// 详情栏里 Bot 的两种触发方式（B3），都就地编辑，不弹窗：
// - 可以叫它的聊天：哪个渠道、私聊还是群聊、哪些聊天、哪些人。群聊必须写明哪些人（Runtime 会拒绝空名单）；
//   私聊不写人时只有渠道的创建者能叫它。ID 在聊天里发 /whoami 就能看到。
// - 定时让它做：到点给它派一件事（每天、每周或某一天一次），就是 targetType = bot 的定时任务。
import { useCallback, useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { Switch } from '@/components/ui/switch'
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

function ChannelEditor({ bot, onError }: { bot: Bot; onError: (message: string) => void }) {
  const { t } = useI18n()
  const upsert = useBotsStore((state) => state.upsert)
  const [adding, setAdding] = useState(false)
  const [saving, setSaving] = useState(false)
  const [platform, setPlatform] = useState<BotPlatform>('feishu')
  const [chatType, setChatType] = useState<'p2p' | 'group'>('p2p')
  const [peers, setPeers] = useState('')
  const [senders, setSenders] = useState('')
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
  const add = async () => {
    const peerIds = splitIds(peers)
    const ok = await save([
      ...bot.channels,
      {
        platform,
        chatType,
        peerIds: peerIds.length ? peerIds : null,
        senderIds: splitIds(senders),
      },
    ])
    if (!ok) return
    setAdding(false)
    setPeers('')
    setSenders('')
  }
  return (
    <section aria-label={t('bots:triggers.chats')} className="border-b border-stroke py-3.5">
      <h3 className="mb-1 text-xs font-medium text-content-muted">{t('bots:triggers.chats')}</h3>
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
                    ? t('bots:triggers.peers', { ids: channel.peerIds.join('、') })
                    : t('bots:triggers.allPeers')}
                  {' · '}
                  {channel.senderIds.length
                    ? t('bots:triggers.senders', { ids: channel.senderIds.join('、') })
                    : t('bots:triggers.ownerOnly')}
                </span>
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
        <div className="flex flex-col gap-2 rounded-lg border border-stroke p-2.5">
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
            placeholder={
              chatType === 'group'
                ? t('bots:triggers.senderIdsRequired')
                : t('bots:triggers.senderIdsPlaceholder')
            }
            onChange={(event) => setSenders(event.target.value)}
          />
          <div className="flex justify-end gap-2">
            <Button size="sm" variant="ghost" onClick={() => setAdding(false)}>
              {t('bots:triggers.cancel')}
            </Button>
            <Button
              size="sm"
              disabled={saving || (chatType === 'group' && !splitIds(senders).length)}
              onClick={() => void add()}
            >
              {t('bots:triggers.add')}
            </Button>
          </div>
        </div>
      ) : (
        <Button size="sm" variant="outline" onClick={() => setAdding(true)}>
          {t('bots:triggers.addChat')}
        </Button>
      )}
    </section>
  )
}

const WEEKDAYS = [1, 2, 3, 4, 5, 6, 0] as const

function today() {
  const now = new Date()
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`
}

function ScheduleEditor({ bot, onError }: { bot: Bot; onError: (message: string) => void }) {
  const { t } = useI18n()
  const [schedules, setSchedules] = useState<BotSchedule[]>([])
  const [adding, setAdding] = useState(false)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState('')
  const [frequency, setFrequency] = useState<'daily' | 'weekly' | 'once'>('daily')
  const [time, setTime] = useState('09:00')
  const [date, setDate] = useState(today)
  const [dayOfWeek, setDayOfWeek] = useState(1)
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
      return true
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : String(caught))
      return false
    } finally {
      setBusy(false)
    }
  }
  const add = async () => {
    const text = message.trim()
    const ok = await run(() =>
      createBotSchedule({
        botId: bot.id,
        name: `${bot.name}：${text.slice(0, 40)}`,
        botMessage: text,
        frequency,
        time,
        date: frequency === 'once' ? date : '',
        dayOfWeek,
        timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'Asia/Hong_Kong',
      }),
    )
    if (!ok) return
    setAdding(false)
    setMessage('')
  }
  return (
    <section aria-label={t('bots:triggers.schedules')} className="border-b border-stroke py-3.5">
      <h3 className="mb-1 text-xs font-medium text-content-muted">
        {t('bots:triggers.schedules')}
      </h3>
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
            <Button size="sm" variant="ghost" onClick={() => setAdding(false)}>
              {t('bots:triggers.cancel')}
            </Button>
            <Button size="sm" disabled={busy || !message.trim()} onClick={() => void add()}>
              {t('bots:triggers.add')}
            </Button>
          </div>
        </div>
      ) : (
        <Button size="sm" variant="outline" onClick={() => setAdding(true)}>
          {t('bots:triggers.addSchedule')}
        </Button>
      )}
    </section>
  )
}

export function BotTriggers({ bot, onError }: { bot: Bot; onError: (message: string) => void }) {
  return (
    <>
      <ChannelEditor bot={bot} onError={onError} />
      <ScheduleEditor bot={bot} onError={onError} />
    </>
  )
}
