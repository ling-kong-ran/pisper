// Bot 对话顶部的招呼：头像、自我介绍，加三件现在就能定下来的事（每日简报、项目目录、聊天接入）。
// 只由 Bot 的设定算出来，不调用模型、不写进转录：模型没配好也能打招呼，也不占对话上下文。
// 对话开始后留在顶部（翻到最早的消息时能看到），卡片收起；要改设定去设置页。
import './bots-messages'
import { useEffect, useState, type FormEvent, type ReactNode } from 'react'
import { useNavigate } from 'react-router-dom'
import {
  ArrowRight,
  Check,
  FolderOpen,
  MessageCircle,
  Newspaper,
  type LucideIcon,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Item,
  ItemActions,
  ItemContent,
  ItemDescription,
  ItemMedia,
  ItemTitle,
} from '@/components/ui/item'
import { announceSessionsUpdated } from '@/features/chat/public-events'
import { useBetaFeature } from '@/features/labs/public-flags'
import { cn } from '@/lib/utils'
import { BotAvatar } from './BotAvatar'
import { botAvatarState, botDutyLine, botFolderName } from './bot-presentation'
import { BOT_PROACTIVE_HOURS, updateBot, type Bot, type BotProactive } from './bots-api'
import { useBotsStore } from './bots-store'

export function BotGreeting({
  botId,
  collapsed = false,
  fallback = null,
}: {
  botId: string
  collapsed?: boolean
  fallback?: ReactNode
}) {
  const enabled = useBetaFeature('bots')
  const bot = useBotsStore((state) => state.bots.find((item) => item.id === botId))
  const loaded = useBotsStore((state) => state.loaded)
  const load = useBotsStore((state) => state.load)
  // 手机页头不画头像，没有人轮询列表：还没加载过时这里读一次（加载在 store 里去重）。
  useEffect(() => {
    if (enabled && !loaded) void load()
  }, [enabled, load, loaded])
  if (!enabled) return fallback
  if (!bot) return loaded ? fallback : null
  return <GreetingCard bot={bot} collapsed={collapsed} />
}

function GreetingCard({ bot, collapsed }: { bot: Bot; collapsed: boolean }) {
  const { t } = useI18n()
  const navigate = useNavigate()
  const upsert = useBotsStore((state) => state.upsert)
  const [editingBrief, setEditingBrief] = useState(false)
  const [briefAt, setBriefAt] = useState(bot.proactive.briefAt)
  const [picking, setPicking] = useState(false)
  const [confirmingWatch, setConfirmingWatch] = useState(false)
  const [everyHours, setEveryHours] = useState(bot.proactive.everyHours)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const duty = botDutyLine(bot.instructions)
  // 执行节点上的 Bot：目录在那台电脑上，这里的目录选择器选不了；简报与巡看也只在本机跑。
  const local = bot.workspace.environmentId === 'local'
  const settingsPath = `/bots/${encodeURIComponent(bot.id)}`

  const save = async (patch: Parameters<typeof updateBot>[1]) => {
    setSaving(true)
    setError('')
    try {
      upsert(await updateBot(bot.id, patch))
      return true
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
      return false
    } finally {
      setSaving(false)
    }
  }

  const submitBrief = async (event: FormEvent) => {
    event.preventDefault()
    if (!/^\d{2}:\d{2}$/.test(briefAt)) return
    if (await save({ proactive: { brief: true, briefAt } })) setEditingBrief(false)
  }

  // 没写职责时这张卡片是在选职责：巡看要持续消耗模型用量，所以先在 watch 表单里讲清频率与用量，
  // 用户点了确认、选好目录才连同“盯着它”的说明一起打开；已有职责只换目录，不碰巡看。
  // 失败时抛给目录选择器，由它在对话框里显示原因（Runtime 校验目录是否存在）。
  const pickFolder = async (cwd: string) => {
    const updated = await updateBot(bot.id, {
      workspace: { ...bot.workspace, cwd },
      ...(duty
        ? {}
        : { instructions: t('bots:greeting.watchDuty'), proactive: { enabled: true, everyHours } }),
    })
    upsert(updated)
    // 会话目录跟着换了：打开中的会话直接同步页头的目录名。
    if (bot.threadSessionId)
      announceSessionsUpdated({ id: bot.threadSessionId, cwd: updated.workspace.cwd })
    // 保存后目录变了，选择器会按新的 initialPath 重置，不再自己关闭。
    setPicking(false)
    setConfirmingWatch(false)
  }

  return (
    <section
      data-bot-greeting={bot.id}
      data-collapsed={collapsed || undefined}
      aria-label={t('bots:greeting.hello', { name: bot.name })}
      className={cn(
        'mx-auto w-full max-w-[var(--chat-content-width,1040px)]',
        collapsed
          ? 'mb-[var(--chat-message-gap,32px)]'
          : '[animation:transcript-reveal-enter_.22s_var(--ease-out)_both]',
      )}
    >
      <div className="flex items-start gap-3.5">
        <BotAvatar
          avatar={bot.avatar}
          state={collapsed ? 'idle' : botAvatarState(bot)}
          animate={!collapsed}
          size={56}
        />
        <div className="min-w-0 flex-1 pt-0.5">
          <h2 className="text-[15px] leading-6 font-medium text-content">
            {t('bots:greeting.hello', { name: bot.name })}
          </h2>
          {duty ? (
            <p className="text-sm leading-6 [overflow-wrap:anywhere] text-content-secondary">
              {t('bots:greeting.duty', { duty })}{' '}
              <button
                type="button"
                data-bot-greeting-edit
                className="relative whitespace-nowrap text-content-muted underline underline-offset-2 after:absolute after:-inset-3 after:hidden after:content-[''] hover:text-content focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus pointer-coarse:after:block"
                onClick={() => navigate(settingsPath)}
              >
                {t('bots:greeting.editDuty')}
              </button>
            </p>
          ) : (
            <p className="text-sm leading-6 text-content-secondary">
              {collapsed ? t('bots:greeting.talk') : t('bots:greeting.ask')}
            </p>
          )}
        </div>
      </div>
      {!collapsed && (
        <div className="mt-4 flex max-w-[560px] flex-col gap-2">
          {local &&
            (editingBrief ? (
              <form
                data-bot-greeting-brief
                className="flex flex-col gap-3 rounded-[var(--r-md)] border border-stroke px-3 py-2.5"
                onSubmit={(event) => void submitBrief(event)}
                onKeyDown={(event) => {
                  if (event.key === 'Escape') setEditingBrief(false)
                }}
              >
                <label className="flex flex-col gap-2 text-sm font-medium text-content">
                  {t('bots:greeting.briefWhen')}
                  <Input
                    type="time"
                    value={briefAt}
                    autoFocus
                    disabled={saving}
                    className="w-32 font-normal"
                    onChange={(event) => setBriefAt(event.target.value)}
                  />
                </label>
                <p className="m-0 text-xs text-content-muted">{t('bots:greeting.briefNote')}</p>
                <div className="flex items-center gap-2">
                  <Button type="submit" disabled={saving || !briefAt}>
                    {t('bots:greeting.briefConfirm')}
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    disabled={saving}
                    onClick={() => setEditingBrief(false)}
                  >
                    {t('bots:new.cancel')}
                  </Button>
                </div>
              </form>
            ) : (
              <GreetingAction
                action="brief"
                icon={Newspaper}
                done={bot.proactive.brief}
                title={
                  bot.proactive.brief
                    ? t('bots:greeting.briefOn', { time: bot.proactive.briefAt })
                    : t('bots:greeting.brief')
                }
                hint={
                  bot.proactive.brief
                    ? t('bots:greeting.briefOnHint')
                    : t('bots:greeting.briefHint')
                }
                disabled={saving}
                onClick={() => {
                  setBriefAt(bot.proactive.briefAt)
                  setEditingBrief(true)
                }}
              />
            ))}
          {local &&
            (confirmingWatch ? (
              <form
                data-bot-greeting-watch
                className="flex flex-col gap-3 rounded-[var(--r-md)] border border-stroke px-3 py-2.5"
                onSubmit={(event) => {
                  event.preventDefault()
                  setPicking(true)
                }}
                onKeyDown={(event) => {
                  // 下拉菜单在 Portal 里，它的 Esc 只关菜单，不算取消这个表单。
                  if (event.key === 'Escape' && event.currentTarget.contains(event.target as Node))
                    setConfirmingWatch(false)
                }}
              >
                <label className="flex flex-col gap-2 text-sm font-medium text-content">
                  {t('bots:greeting.watchWhen')}
                  <AppSelect
                    value={String(everyHours)}
                    className="w-44 font-normal"
                    onChange={(event) =>
                      setEveryHours(Number(event.target.value) as BotProactive['everyHours'])
                    }
                  >
                    {BOT_PROACTIVE_HOURS.map((hours) => (
                      <option key={hours} value={hours}>
                        {t('bots:proactive.everyHours', { count: hours })}
                      </option>
                    ))}
                  </AppSelect>
                </label>
                <p className="m-0 text-xs text-content-muted">{t('bots:greeting.watchNote')}</p>
                <div className="flex flex-wrap items-center gap-2">
                  <Button type="submit" autoFocus>
                    {t('bots:greeting.watchConfirm')}
                  </Button>
                  <Button type="button" variant="ghost" onClick={() => setConfirmingWatch(false)}>
                    {t('bots:new.cancel')}
                  </Button>
                </div>
              </form>
            ) : (
              <GreetingAction
                action="folder"
                icon={FolderOpen}
                title={duty ? t('bots:greeting.folder') : t('bots:greeting.watch')}
                hint={
                  !duty
                    ? t('bots:greeting.watchHint', { count: bot.proactive.everyHours })
                    : bot.proactive.enabled
                      ? t('bots:greeting.folderWatchHint', {
                          folder: botFolderName(bot.workspace.cwd),
                          count: bot.proactive.everyHours,
                        })
                      : t('bots:greeting.folderHint', { folder: botFolderName(bot.workspace.cwd) })
                }
                disabled={saving}
                onClick={() => {
                  if (duty) {
                    setPicking(true)
                    return
                  }
                  setEveryHours(bot.proactive.everyHours)
                  setConfirmingWatch(true)
                }}
              />
            ))}
          <GreetingAction
            action="chats"
            icon={MessageCircle}
            done={bot.channels.length > 0}
            title={
              bot.channels.length
                ? t('bots:greeting.chatsOn', { count: bot.channels.length })
                : t('bots:greeting.chats')
            }
            hint={
              bot.channels.length ? t('bots:greeting.chatsOnHint') : t('bots:greeting.chatsHint')
            }
            disabled={saving}
            onClick={() => navigate(`${settingsPath}?section=chats`)}
          />
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <WorkspacePicker
            open={picking}
            initialPath={bot.workspace.cwd}
            onOpenChange={setPicking}
            onSelect={pickFolder}
          />
        </div>
      )}
    </section>
  )
}

function GreetingAction({
  action,
  icon: Icon,
  title,
  hint,
  done = false,
  disabled,
  onClick,
}: {
  action: string
  icon: LucideIcon
  title: string
  hint: string
  done?: boolean
  disabled: boolean
  onClick: () => void
}) {
  const Trailing = done ? Check : ArrowRight
  return (
    <Item asChild variant="outline">
      <button
        type="button"
        data-bot-greeting-action={action}
        disabled={disabled}
        className="group/action w-full cursor-pointer text-left hover:bg-surface-hover disabled:cursor-default disabled:opacity-60"
        onClick={onClick}
      >
        <ItemMedia variant="icon">
          <Icon className="text-content-secondary" strokeWidth={1.75} aria-hidden="true" />
        </ItemMedia>
        <ItemContent>
          <ItemTitle>{title}</ItemTitle>
          <ItemDescription>{hint}</ItemDescription>
        </ItemContent>
        <ItemActions>
          <Trailing
            className="size-4 text-content-faint group-hover/action:text-content-secondary"
            aria-hidden="true"
          />
        </ItemActions>
      </button>
    </Item>
  )
}
