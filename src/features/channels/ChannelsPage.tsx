import { PageFrame } from '@/components/app/PageFrame'
import { ListDetailLayout } from '@/components/app/ListDetailLayout'
import { InlineEmptyState } from '@/components/app/PageState'
// 渠道页：外部消息渠道（飞书/微信/QQ/Telegram）连接管理、状态与测试。
import { useCallback, useEffect, useState } from 'react'
import {
  AlertTriangle,
  Bot,
  ExternalLink,
  FolderOpen,
  MessageCircle,
  MessageSquare,
  Plus,
  Send,
  RefreshCw,
  ShieldCheck,
  Trash2,
  Unplug,
  X,
} from 'lucide-react'
import {
  AppSwitch as Toggle,
  StatusBadge as Badge,
  AppCardHeader,
  AppError,
  AppEmptyState,
  AppNotice,
} from '@/components/ui/app-primitives'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { OrbitSpinner } from '@/components/ui/spinner'
import { apiJson } from '@/lib/api'
import { cn } from '@/lib/utils'
import { relativeTime } from '@/lib/format'
import { APP_NAME } from '@/app/brand'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import type { LucideIcon } from 'lucide-react'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'

type ChannelPlatform = 'feishu' | 'weixin' | 'qq' | 'telegram'
type ChannelStatus = 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'failed'
type OnboardingStatus =
  | 'starting'
  | 'waiting'
  | 'scanned'
  | 'verification_required'
  | 'authorizing'
  | 'connecting'
  | 'completed'
  | 'failed'
  | 'cancelled'
type BadgeTone = 'blue' | 'green' | 'red' | 'amber' | 'gray'
type ProviderDefinition = { Icon: LucideIcon }
type ReplyModel = { provider: string; model: string }
type ChannelConnection = {
  enabled: boolean
  status: ChannelStatus
  defaultCwd: string
  replyModel: ReplyModel | null
  executionMode: 'approval-required' | 'workspace-write' | 'full-access'
  runMode: 'plan' | 'goal' | 'team'
  accessMode: 'owner' | 'all'
  ownerConfigured: boolean
  lastError?: string
  connectedAt?: string | null
}
type ChannelScope = {
  key: string
  platform: ChannelPlatform
  title: string
  lastMessage?: string
  updatedAt?: string
  model?: string
  cwd?: string
}
type ChannelModel = { provider: string; model: string; label: string }
type ChannelsData = {
  providers: Array<Record<string, unknown>>
  connections: Record<ChannelPlatform, ChannelConnection | null>
  scopes: ChannelScope[]
  models: ChannelModel[]
}
type OnboardingJob = {
  id?: string
  platform: ChannelPlatform
  status: OnboardingStatus
  error?: string
  qrDataUrl?: string
  qrUrl?: string
  setupUrl?: string
  expireAt?: string
  needsVerifyCode?: boolean
  manual?: boolean
  mode?: 'manual' | 'qr'
  fields?: string[]
  required?: string[]
}
type ManualCredentials = {
  appId?: string
  appSecret?: string
  token?: string
}
type ManualOnboardingDescriptor = {
  mode: 'manual'
  platform: ChannelPlatform
  fields: string[]
  required: string[]
  setupUrl?: string
  qrDataUrl?: string
}
type ChannelsPageProps = {
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}
type OnboardingModalProps = {
  job: OnboardingJob
  onClose: () => void | Promise<void>
  onRetry: () => void | Promise<void>
  onSubmitCredentials: (credentials: ManualCredentials) => void | Promise<void>
  notify: Notify
}

// 平台标识统一为单色键帽图标：品牌差异由名称承担，强调色只留给选中刻度与主操作。
const PROVIDERS: Record<ChannelPlatform, ProviderDefinition> = {
  feishu: { Icon: Bot },
  weixin: { Icon: MessageCircle },
  qq: { Icon: MessageCircle },
  telegram: { Icon: Send },
}
const PROVIDER_ENTRIES = Object.entries(PROVIDERS) as Array<[ChannelPlatform, ProviderDefinition]>

// 卡片：1px 描边 + r-md，无投影；卡片头位于次级表面，13/500 标题。
const CARD = 'min-w-0 overflow-hidden rounded-md border border-stroke bg-panel'
const CARD_HEAD =
  'flex min-h-11 min-w-0 items-center justify-between gap-3 border-b border-stroke-soft bg-surface-subtle px-4 py-1.5'
const ICON_KEY =
  'grid size-8 shrink-0 place-items-center rounded-sm border border-key-border bg-key text-content-secondary shadow-key [&_svg]:size-4'
const SELECTED_TICK = 'absolute inset-y-2.5 left-0 w-[3px] rounded-r-[3px] bg-star'
// 只读键值行：左侧弱化标签，右侧值单行省略。
const KEY_VALUE =
  'flex min-h-[34px] min-w-0 items-center justify-between gap-4 border-t border-stroke-soft py-1.5 text-sm first:border-t-0 [&>dt]:shrink-0 [&>dt]:text-content-muted [&>dd]:min-w-0 [&>dd]:truncate [&>dd]:text-right [&>dd]:text-content'
// 标签在左的表单行：双栏布局时 168px 标签列，窄屏堆叠。
const FORM_ROW =
  'grid min-w-0 gap-1.5 border-t border-stroke-soft py-3 @min-[760px]/list-detail:grid-cols-[168px_minmax(0,1fr)] @min-[760px]/list-detail:items-center @min-[760px]/list-detail:gap-4'
const FORM_LABEL = 'text-sm font-medium text-content'
// 渠道弹窗沿用全局 .modal 浮层：纯色遮罩（无背景模糊）、浮层底色 + 环线投影。
const MODAL_BACKDROP =
  'modal-backdrop fixed inset-0 z-[70] grid place-items-center overflow-y-auto overscroll-contain bg-[var(--modal-overlay)] p-5 max-[650px]:p-2'
const MODAL_SURFACE =
  'modal max-h-[calc(100dvh_-_40px)] w-[min(470px,100%)] overflow-y-auto overscroll-contain rounded-dialog bg-popover p-5 shadow-float max-[650px]:max-h-[calc(100dvh_-_16px)]'
// 说明条保持单色：强调色只留给选中、焦点与主操作。
const NOTICE =
  'border border-stroke-soft bg-surface-subtle text-content-secondary [&_strong]:text-content [&>svg]:mt-0.5 [&>svg]:shrink-0'
const LIVE_DOT: Record<ChannelStatus, string> = {
  connected: 'bg-success shadow-[0_0_0_3px_var(--success-soft)]',
  connecting: '',
  reconnecting: '',
  failed: 'bg-danger shadow-[0_0_0_3px_var(--danger-soft)]',
  idle: 'bg-content-faint',
}

function providerName(platform: ChannelPlatform, t: ReturnType<typeof useI18n>['t']) {
  if (platform === 'feishu') return t('channels:channelsPage.feishu')
  if (platform === 'weixin') return t('channels:channelsPage.weChat')
  if (platform === 'qq') return t('channels:channelsPage.qq')
  return t('channels:channelsPage.telegram')
}

function providerTitle(platform: ChannelPlatform, t: ReturnType<typeof useI18n>['t']) {
  if (platform === 'feishu') return t('channels:channelsPage.feishuAppBot')
  if (platform === 'weixin') return t('channels:channelsPage.weChat')
  if (platform === 'qq') return t('channels:channelsPage.qqBot')
  return t('channels:channelsPage.telegramBot')
}

function providerTransport(platform: ChannelPlatform, t: ReturnType<typeof useI18n>['t']) {
  if (platform === 'feishu') return t('channels:channelsPage.webSocketPersistentConnection')
  if (platform === 'weixin') return t('channels:channelsPage.tencentILinkPersistentConnection')
  if (platform === 'qq') return t('channels:channelsPage.qqPersistentConnection')
  return t('channels:channelsPage.telegramPersistentConnection')
}

function providerCapability(platform: ChannelPlatform, t: ReturnType<typeof useI18n>['t']) {
  if (platform === 'feishu') return t('channels:channelsPage.feishuCapabilities')
  if (platform === 'weixin') return t('channels:channelsPage.weChatCapabilities')
  if (platform === 'qq') return t('channels:channelsPage.qqCapabilities')
  return t('channels:channelsPage.telegramCapabilities')
}

function isManualPlatform(platform: ChannelPlatform) {
  return platform === 'telegram'
}

function connectActionLabel(
  platform: ChannelPlatform,
  connected: boolean,
  t: ReturnType<typeof useI18n>['t'],
) {
  if (connected)
    return t('channels:channelsPage.reconnectName', { name: providerName(platform, t) })
  return isManualPlatform(platform)
    ? t('channels:channelsPage.configureName', { name: providerName(platform, t) })
    : t('channels:channelsPage.connectNameByQRCode', { name: providerName(platform, t) })
}

function channelStatusLabel(status: ChannelStatus, t: ReturnType<typeof useI18n>['t']) {
  if (status === 'connecting') return t('channels:channelsPage.connecting')
  if (status === 'connected') return t('channels:channelsPage.online')
  if (status === 'reconnecting') return t('channels:channelsPage.reconnecting')
  if (status === 'failed') return t('channels:channelsPage.connectionFailed')
  return t('channels:channelsPage.notConnected')
}

function channelStatusTone(status: ChannelStatus): BadgeTone {
  if (status === 'connected') return 'green'
  if (status === 'connecting' || status === 'reconnecting') return 'amber'
  if (status === 'failed') return 'red'
  return 'gray'
}

// 连接中/重连中属于「运行」状态，徽标内带品牌轨道指示。
function ChannelStatusBadge({ status }: { status: ChannelStatus }) {
  const { t } = useI18n()
  const busy = status === 'connecting' || status === 'reconnecting'
  return (
    <Badge tone={channelStatusTone(status)} className="gap-1">
      {busy && <OrbitSpinner className="size-3 text-current" />}
      {channelStatusLabel(status, t)}
    </Badge>
  )
}

function onboardingStatusLabel(status: OnboardingStatus, t: ReturnType<typeof useI18n>['t']) {
  if (status === 'starting') return t('channels:channelsPage.requestingLoginQrCode')
  if (status === 'waiting') return t('channels:channelsPage.scanAndConfirmConnection')
  if (status === 'scanned') return t('channels:channelsPage.scannedWaitingForConfirmation')
  if (status === 'verification_required') return t('channels:channelsPage.enterPairingCode')
  if (status === 'authorizing') return t('channels:channelsPage.confirmingAuthorization')
  if (status === 'connecting') return t('channels:channelsPage.authorizationSucceededConnecting')
  if (status === 'completed') return t('channels:channelsPage.channelConnected')
  if (status === 'failed') return t('channels:channelsPage.connectionFailed')
  return t('channels:channelsPage.cancelled')
}

function expiresIn(value: string | number | Date, locale = 'zh-CN') {
  const seconds = Math.max(0, Math.ceil((new Date(value).getTime() - Date.now()) / 1000))
  if (locale === 'en-US')
    return seconds >= 60 ? `in ${Math.ceil(seconds / 60)} min` : `in ${seconds} sec`
  return seconds >= 60 ? `${Math.ceil(seconds / 60)} 分钟后` : `${seconds} 秒后`
}

function errorMessage(caught: unknown) {
  return caught instanceof Error ? caught.message : String(caught)
}

export function ChannelsPage({ notify, registerPrimaryAction, requestConfirm }: ChannelsPageProps) {
  const [detailOpen, setDetailOpen] = useState(false)

  const { t, language } = useI18n()
  const [data, setData] = useState<ChannelsData>({
    providers: [],
    connections: { feishu: null, weixin: null, qq: null, telegram: null },
    scopes: [],
    models: [],
  })
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [selectedPlatform, setSelectedPlatform] = useState<ChannelPlatform>('feishu')
  const [onboarding, setOnboarding] = useState<OnboardingJob | null>(null)
  const [starting, setStarting] = useState<ChannelPlatform | ''>('')
  const [saving, setSaving] = useState(false)
  const [cwd, setCwd] = useState('')

  // 加载渠道连接状态，并同步选中平台的默认工作目录。
  const load = useCallback(async () => {
    try {
      setError('')
      const result = await apiJson<ChannelsData>('/api/channels')
      setData(result)
      setCwd(result.connections?.[selectedPlatform]?.defaultCwd || '')
    } catch (caught) {
      setError(errorMessage(caught))
    } finally {
      setLoading(false)
    }
  }, [selectedPlatform])

  useEffect(() => {
    load()
  }, [load])
  useEffect(() => {
    if (!onboarding?.id || ['completed', 'failed', 'cancelled'].includes(onboarding.status))
      return undefined
    const onboardingId = onboarding.id
    const timer = window.setInterval(async () => {
      try {
        const platform = onboarding.platform
        const next = await apiJson<Omit<OnboardingJob, 'platform'>>(
          `/api/channels/${platform}/onboarding/${encodeURIComponent(onboardingId)}`,
        )
        setOnboarding({ ...next, platform })
        if (next.status === 'completed') {
          window.clearInterval(timer)
          notify(
            t('channels:channelsPage.nameTwoWayConnectionEstablished', {
              name: providerName(platform, t),
            }),
          )
          await load()
          window.setTimeout(() => setOnboarding(null), 900)
        }
      } catch (caught) {
        setOnboarding((current) =>
          current ? { ...current, status: 'failed', error: errorMessage(caught) } : current,
        )
      }
    }, 1500)
    return () => window.clearInterval(timer)
  }, [onboarding?.id, onboarding?.platform, onboarding?.status, load, notify, t])

  // 开始渠道对接（onboarding）：已存在连接时先确认覆盖，
  // 然后发起扫描并轮询进度直到完成/失败。
  const beginOnboarding = async (platform: ChannelPlatform) => {
    const connection = data.connections?.[platform]
    if (connection) {
      const approved = await requestConfirm({
        title: t('channels:channelsPage.reconnectName', { name: providerName(platform, t) }),
        message: isManualPlatform(platform)
          ? t('channels:channelsPage.configuringAgainWillReplaceTheCurrentNameConnectionContinue', {
              name: providerName(platform, t),
            })
          : t('channels:channelsPage.scanningAgainWillReplaceTheCurrentNameConnectionContinue', {
              name: providerName(platform, t),
            }),
        confirmLabel: isManualPlatform(platform)
          ? t('channels:channelsPage.continueConfiguration')
          : t('channels:channelsPage.continueScanning'),
        tone: 'primary',
      })
      if (!approved) return
    }
    setSelectedPlatform(platform)
    setStarting(platform)
    setOnboarding({ platform, status: 'starting' })
    try {
      const result = await apiJson<Omit<OnboardingJob, 'platform'> | ManualOnboardingDescriptor>(
        `/api/channels/${platform}/onboarding`,
        {
          method: 'POST',
          body: '{}',
        },
      )
      if ('mode' in result && result.mode === 'manual') {
        setOnboarding({ ...result, platform, status: 'starting', manual: true })
      } else {
        setOnboarding({ ...result, platform })
      }
    } catch (caught) {
      setOnboarding({ platform, status: 'failed', error: errorMessage(caught) })
    } finally {
      setStarting('')
    }
  }

  usePagePrimaryAction(registerPrimaryAction, () => beginOnboarding(selectedPlatform))

  const closeOnboarding = async () => {
    if (onboarding?.id && !['completed', 'failed', 'cancelled'].includes(onboarding.status))
      await apiJson(
        `/api/channels/${onboarding.platform}/onboarding/${encodeURIComponent(onboarding.id)}`,
        { method: 'DELETE' },
      ).catch(() => {})
    setOnboarding(null)
  }

  // 更新渠道连接（启用/访问模式/工作目录/回复模型），成功后提示。
  const update = async (
    platform: ChannelPlatform,
    patch: Partial<
      Pick<
        ChannelConnection,
        'enabled' | 'accessMode' | 'defaultCwd' | 'replyModel' | 'executionMode' | 'runMode'
      >
    >,
    success: string,
  ) => {
    setSaving(true)
    try {
      const result = await apiJson<ChannelsData>(`/api/channels/${platform}`, {
        method: 'PATCH',
        body: JSON.stringify(patch),
      })
      setData(result)
      setCwd(result.connections?.[platform]?.defaultCwd || '')
      notify(success)
    } catch (caught) {
      notify(errorMessage(caught), 'error')
    } finally {
      setSaving(false)
    }
  }

  // 重连渠道：POST reconnect；失败时回滚加载最新状态并提示。
  const reconnect = async (platform: ChannelPlatform) => {
    setSaving(true)
    try {
      setData(
        await apiJson<ChannelsData>(`/api/channels/${platform}/reconnect`, {
          method: 'POST',
          body: '{}',
        }),
      )
      notify(t('channels:channelsPage.nameReconnected', { name: providerName(platform, t) }))
    } catch (caught) {
      notify(errorMessage(caught), 'error')
      load()
    } finally {
      setSaving(false)
    }
  }

  // 断开渠道（确认后删除本地凭据与映射）。
  const remove = async (platform: ChannelPlatform) => {
    const approved = await requestConfirm({
      title: t('channels:channelsPage.disconnectName', { name: providerName(platform, t) }),
      message: t('channels:channelsPage.localCredentialsAndChatMappingsWillBeDeleted'),
      confirmLabel: t('channels:channelsPage.disconnect'),
    })
    if (!approved) return
    try {
      await apiJson(`/api/channels/${platform}`, { method: 'DELETE' })
      await load()
      notify(t('channels:channelsPage.nameDisconnected', { name: providerName(platform, t) }))
    } catch (caught) {
      notify(errorMessage(caught), 'error')
    }
  }

  const resetScope = async (scope: ChannelScope) => {
    const approved = await requestConfirm({
      title: t('channels:channelsPage.resetChannelChat'),
      message: t('channels:channelsPage.resetTheAppChatLinkedToScope', {
        scope: scope.title,
        app: APP_NAME,
      }),
      confirmLabel: t('channels:channelsPage.reset'),
    })
    if (!approved) return
    try {
      await apiJson(`/api/channels/scopes/${encodeURIComponent(scope.key)}`, { method: 'DELETE' })
      await load()
      notify(t('channels:channelsPage.channelChatReset'))
    } catch (caught) {
      notify(errorMessage(caught), 'error')
    }
  }

  if (loading)
    return (
      <AppEmptyState>
        <OrbitSpinner className="size-5" />
        <h2>{t('channels:channelsPage.loadingChannels')}</h2>
      </AppEmptyState>
    )
  const selectedConnection = data.connections?.[selectedPlatform]
  const SelectedIcon = PROVIDERS[selectedPlatform].Icon
  const selectedStatus = selectedConnection?.status || 'idle'
  return (
    <PageFrame variant="list-detail">
      <div className="flex h-full min-h-0 flex-col gap-3">
        {error && (
          <AppError>
            <AlertTriangle size={13} />
            {error}
          </AppError>
        )}
        <ListDetailLayout
          selected={detailOpen}
          onBack={() => setDetailOpen(false)}
          backLabel={t('common:pageTemplate.backToList')}
          list={
            <section className="min-w-0">
              {/* 列表标题 12/500 弱化文字 + 等宽计数，与其它列表页同一节奏。 */}
              <h2 className="flex h-[30px] items-end justify-between px-1 pb-1.5 text-xs font-medium tracking-[.02em] text-content-muted">
                <span>{t('channels:channelsPage.platforms')}</span>
                <span className="font-normal tabular-nums">{PROVIDER_ENTRIES.length}</span>
              </h2>
              <ul className="flex flex-col gap-0.5">
                {PROVIDER_ENTRIES.map(([platform, provider]) => {
                  const connection = data.connections?.[platform]
                  const status = connection?.status || 'idle'
                  const active = selectedPlatform === platform
                  const Icon = provider.Icon
                  return (
                    <li
                      className={cn(
                        'provider-card channel-platform-card relative rounded-md transition-colors duration-(--d-fast) hover:bg-surface-subtle',
                        active && 'selected bg-surface-muted hover:bg-surface-muted',
                      )}
                      key={platform}
                    >
                      {/* 选中行：次级表面 + 3px 强调色刻度，两种主题下都不只靠底色区分。 */}
                      {active && <i aria-hidden="true" className={SELECTED_TICK} />}
                      <button
                        type="button"
                        aria-current={active || undefined}
                        className="grid min-h-[60px] w-full min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 py-2 pr-2.5 pl-3 text-left focus-visible:-outline-offset-2"
                        onClick={() => {
                          setDetailOpen(true)
                          setSelectedPlatform(platform)
                          setCwd(connection?.defaultCwd || '')
                        }}
                      >
                        <span className={ICON_KEY}>
                          <Icon />
                        </span>
                        <span className="flex min-w-0 flex-col gap-0.5">
                          <strong className="truncate text-sm font-medium text-content max-[650px]:text-[15px]">
                            {providerTitle(platform, t)}
                          </strong>
                          <small className="truncate text-xs text-content-muted">
                            {providerTransport(platform, t)}
                          </small>
                        </span>
                        <ChannelStatusBadge status={status} />
                      </button>
                    </li>
                  )
                })}
              </ul>
            </section>
          }
          detail={
            <div className="flex min-w-0 flex-col gap-4">
              <section className={CARD}>
                <header className="flex min-w-0 flex-wrap items-center gap-3 px-4 py-3.5">
                  <span className={ICON_KEY}>
                    <SelectedIcon />
                  </span>
                  <div className="min-w-0 flex-1">
                    <h2 className="flex min-w-0 flex-wrap items-center gap-2 text-[15px] leading-6 font-medium text-content">
                      {providerTitle(selectedPlatform, t)}
                      <ChannelStatusBadge status={selectedStatus} />
                    </h2>
                    <p className="truncate text-xs text-content-muted">
                      {providerTransport(selectedPlatform, t)}
                    </p>
                  </div>
                  {/* 页头已有同一动作的强调色主按钮，这里用键帽，避免一屏两个实心强调键。 */}
                  <Button
                    variant="secondary"
                    className="max-[650px]:h-11 max-[650px]:w-full"
                    loading={starting === selectedPlatform}
                    disabled={starting === selectedPlatform}
                    onClick={() => beginOnboarding(selectedPlatform)}
                  >
                    <Plus />
                    {connectActionLabel(selectedPlatform, Boolean(selectedConnection), t)}
                  </Button>
                </header>
                <dl className="border-t border-stroke-soft px-4 py-1">
                  <div className={KEY_VALUE}>
                    <dt>{t('channels:channelsPage.twoWayCapability')}</dt>
                    <dd>{providerCapability(selectedPlatform, t)}</dd>
                  </div>
                  <div className={KEY_VALUE}>
                    <dt>{t('channels:channelsPage.replyModel')}</dt>
                    <dd className={selectedConnection?.replyModel ? 'font-mono' : undefined}>
                      {selectedConnection?.replyModel
                        ? `${selectedConnection.replyModel.provider}/${selectedConnection.replyModel.model}`
                        : t('channels:channelsPage.useApplicationDefaultModel')}
                    </dd>
                  </div>
                </dl>
              </section>
              {selectedConnection && (
                <section className={cn('test-panel', CARD)}>
                  <header className={CARD_HEAD}>
                    <h2 className="text-sm font-medium text-content">
                      {t('channels:channelsPage.nameSettings', {
                        name: providerName(selectedPlatform, t),
                      })}
                    </h2>
                  </header>
                  <div className="px-4 pb-1">
                    <div
                      className={cn(
                        'channel-live-status flex min-w-0 items-center gap-2.5 py-3',
                        selectedConnection.status,
                      )}
                    >
                      {selectedConnection.status === 'connecting' ||
                      selectedConnection.status === 'reconnecting' ? (
                        <OrbitSpinner className="channel-status-dot" />
                      ) : (
                        <span
                          aria-hidden="true"
                          className={cn(
                            'channel-status-dot mx-1 size-2 shrink-0 rounded-full',
                            LIVE_DOT[selectedConnection.status],
                          )}
                        />
                      )}
                      <div className="flex min-w-0 flex-col gap-0.5">
                        <strong className="text-sm font-medium text-content">
                          {channelStatusLabel(selectedConnection.status, t)}
                        </strong>
                        <small
                          className={cn(
                            'truncate text-xs tabular-nums',
                            selectedConnection.lastError ? 'text-danger' : 'text-content-muted',
                          )}
                        >
                          {selectedConnection.lastError ||
                            (selectedConnection.status === 'connected'
                              ? t('channels:channelsPage.connectedTime', {
                                  time: relativeTime(selectedConnection.connectedAt, language),
                                })
                              : t('channels:channelsPage.establishingPersistentConnection'))}
                        </small>
                      </div>
                    </div>
                    <div className="modal-toggle-row flex min-w-0 items-center justify-between gap-4 border-t border-stroke-soft py-3">
                      <span className="flex min-w-0 flex-col gap-0.5">
                        <strong className="text-sm font-medium text-content">
                          {t('channels:channelsPage.enableThisChannel')}
                        </strong>
                        <small className="text-xs text-content-muted">
                          {t(
                            'channels:channelsPage.turnOffToDisconnectWhileKeepingSignInCredentials',
                          )}
                        </small>
                      </span>
                      <Toggle
                        value={selectedConnection.enabled}
                        disabled={saving}
                        ariaLabel={t('channels:channelsPage.enableThisChannel')}
                        onChange={(enabled) =>
                          update(
                            selectedPlatform,
                            { enabled },
                            enabled
                              ? t('channels:channelsPage.channelEnabled')
                              : t('channels:channelsPage.channelPaused'),
                          )
                        }
                      />
                    </div>
                    <label className={FORM_ROW}>
                      <span className={FORM_LABEL}>{t('channels:channelsPage.replyModel')}</span>
                      <AppSelect
                        value={
                          selectedConnection.replyModel
                            ? `${selectedConnection.replyModel.provider}/${selectedConnection.replyModel.model}`
                            : ''
                        }
                        onChange={(event) => {
                          const [provider, ...parts] = event.target.value.split('/')
                          update(
                            selectedPlatform,
                            {
                              replyModel: event.target.value
                                ? { provider, model: parts.join('/') }
                                : null,
                            },
                            t('channels:channelsPage.channelReplyModelUpdated'),
                          )
                        }}
                      >
                        <option value="">
                          {t('channels:channelsPage.useApplicationDefaultModel')}
                        </option>
                        {data.models.map((model) => (
                          <option
                            value={`${model.provider}/${model.model}`}
                            key={`${model.provider}/${model.model}`}
                          >
                            {model.label}
                          </option>
                        ))}
                      </AppSelect>
                    </label>
                    <label className={FORM_ROW}>
                      <span className={FORM_LABEL}>{t('channels:channelsPage.approvalMode')}</span>
                      <AppSelect
                        value={selectedConnection.executionMode}
                        onChange={(event) =>
                          update(
                            selectedPlatform,
                            {
                              executionMode: event.target
                                .value as ChannelConnection['executionMode'],
                            },
                            t('channels:channelsPage.approvalModeUpdated'),
                          )
                        }
                      >
                        <option value="approval-required">
                          {t('channels:channelsPage.approvalRequired')}
                        </option>
                        <option value="workspace-write">
                          {t('channels:channelsPage.workspaceWrite')}
                        </option>
                        <option value="full-access">{t('channels:channelsPage.fullAccess')}</option>
                      </AppSelect>
                    </label>
                    <label className={FORM_ROW}>
                      <span className={FORM_LABEL}>{t('channels:channelsPage.executionMode')}</span>
                      <AppSelect
                        value={selectedConnection.runMode}
                        onChange={(event) =>
                          update(
                            selectedPlatform,
                            { runMode: event.target.value as ChannelConnection['runMode'] },
                            t('channels:channelsPage.executionModeUpdated'),
                          )
                        }
                      >
                        <option value="plan">{t('channels:channelsPage.planMode')}</option>
                        <option value="goal">{t('channels:channelsPage.goalMode')}</option>
                        <option value="team">{t('channels:channelsPage.teamMode')}</option>
                      </AppSelect>
                    </label>
                    <label className={FORM_ROW}>
                      <span className={FORM_LABEL}>{t('channels:channelsPage.accessScope')}</span>
                      <AppSelect
                        value={selectedConnection.accessMode}
                        onChange={(event) =>
                          update(
                            selectedPlatform,
                            { accessMode: event.target.value === 'all' ? 'all' : 'owner' },
                            t('channels:channelsPage.accessScopeUpdated'),
                          )
                        }
                      >
                        <option value="owner" disabled={!selectedConnection.ownerConfigured}>
                          {t('channels:channelsPage.qrCodeOwnerOnly')}
                        </option>
                        <option value="all">
                          {selectedPlatform === 'feishu'
                            ? t('channels:channelsPage.allMembersInTheCurrentTenant')
                            : selectedPlatform === 'weixin'
                              ? t('channels:channelsPage.allWeChatUsersWhoMessageTheBot')
                              : t('channels:channelsPage.allUsersWhoMessageTheBot')}
                        </option>
                      </AppSelect>
                    </label>
                    <label className={FORM_ROW}>
                      <span className={FORM_LABEL}>
                        {t('channels:channelsPage.defaultWorkingDirectoryForNewChats')}
                      </span>
                      <span className="channel-setting-input grid h-[34px] min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2 rounded-sm border border-control bg-panel py-0.5 pr-0.5 pl-2.5 text-content-muted focus-within:border-focus focus-within:ring-3 focus-within:ring-focus-ring max-[650px]:h-11">
                        <FolderOpen className="size-3.5" />
                        <input
                          className="min-w-0 border-0 bg-transparent font-mono text-xs text-content outline-none"
                          value={cwd}
                          onChange={(event) => setCwd(event.target.value)}
                        />
                        <Button
                          variant="secondary"
                          size="sm"
                          className="max-[650px]:h-9"
                          disabled={saving || cwd === selectedConnection.defaultCwd}
                          onClick={() =>
                            update(
                              selectedPlatform,
                              { defaultCwd: cwd },
                              t('channels:channelsPage.defaultWorkingDirectorySaved'),
                            )
                          }
                        >
                          {t('channels:channelsPage.save')}
                        </Button>
                      </span>
                    </label>
                    <AppNotice className={cn('mb-3', NOTICE)}>
                      <ShieldCheck size={15} />
                      <span>
                        <strong>{t('channels:channelsPage.localSecurityBoundary')}</strong>
                        <small>
                          {t(
                            'channels:channelsPage.ownerOnlyAccessRespectsToolPermissionsAndExecutionMode',
                          )}
                        </small>
                      </span>
                    </AppNotice>
                  </div>
                  <footer className="flex flex-wrap items-center justify-end gap-2 border-t border-stroke-soft bg-surface-subtle px-4 py-2.5">
                    <Button
                      variant="ghost"
                      className="text-danger hover:bg-danger-soft hover:text-danger max-[650px]:h-11"
                      onClick={() => remove(selectedPlatform)}
                    >
                      <Unplug />
                      {t('channels:channelsPage.disconnect')}
                    </Button>
                    <Button
                      variant="secondary"
                      className="max-[650px]:h-11"
                      onClick={() => reconnect(selectedPlatform)}
                      disabled={saving}
                    >
                      <RefreshCw />
                      {t('channels:channelsPage.reconnect')}
                    </Button>
                  </footer>
                </section>
              )}
              <section className={CARD}>
                <header className={CARD_HEAD}>
                  <h2 className="flex min-w-0 items-baseline gap-2 text-sm font-medium text-content">
                    {t('channels:channelsPage.channelChats')}
                  </h2>
                  <span className="text-xs text-content-muted tabular-nums">
                    {t('channels:channelsPage.countLinked', { count: data.scopes.length })}
                  </span>
                </header>
                {data.scopes.length ? (
                  <ul className="divide-y divide-stroke-soft">
                    {data.scopes.map((scope) => (
                      <li
                        className="route-row grid min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 px-4 py-2.5"
                        key={scope.key}
                      >
                        <span className={ICON_KEY}>
                          {scope.platform === 'feishu' ? (
                            <MessageSquare />
                          ) : scope.platform === 'telegram' ? (
                            <Send />
                          ) : (
                            <MessageCircle />
                          )}
                        </span>
                        <div className="channel-route-copy flex min-w-0 flex-col gap-0.5">
                          <strong className="flex min-w-0 items-center gap-2 text-sm font-medium text-content">
                            <span className="truncate">{scope.title}</span>
                            <Badge tone="gray">{providerName(scope.platform, t)}</Badge>
                          </strong>
                          <small className="flex min-w-0 gap-1.5 text-xs text-content-secondary">
                            <span className="truncate">
                              {scope.lastMessage || t('channels:channelsPage.noMessages')}
                            </span>
                            <span className="shrink-0 text-content-muted tabular-nums">
                              {relativeTime(scope.updatedAt, language)}
                            </span>
                          </small>
                          <small className="truncate font-mono text-xs text-content-muted">
                            {scope.model || t('channels:channelsPage.defaultModel')} · {scope.cwd}
                          </small>
                        </div>
                        <div className="channel-route-controls flex items-center gap-1.5">
                          <Badge tone="green">{t('channels:channelsPage.twoWay')}</Badge>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="text-content-muted hover:bg-danger-soft hover:text-danger max-[650px]:size-11"
                            title={t('channels:channelsPage.resetChat')}
                            aria-label={`${t('channels:channelsPage.resetChat')}: ${scope.title}`}
                            onClick={() => resetScope(scope)}
                          >
                            <Trash2 />
                          </Button>
                        </div>
                      </li>
                    ))}
                  </ul>
                ) : (
                  <InlineEmptyState
                    compact
                    title={t('channels:channelsPage.waitingForTheFirstReplyFromAfar')}
                    description={t(
                      'channels:channelsPage.onceConnectedMessagesFromSupportedChannelsWillArriveHereInPisper',
                    )}
                  />
                )}
              </section>
            </div>
          }
        />

        {onboarding && (
          <OnboardingModal
            job={onboarding}
            onClose={closeOnboarding}
            onRetry={() => beginOnboarding(onboarding.platform)}
            onSubmitCredentials={async (credentials) => {
              try {
                const result = await apiJson<
                  Omit<OnboardingJob, 'platform'> | ManualOnboardingDescriptor | ChannelsData
                >(`/api/channels/${onboarding.platform}/onboarding`, {
                  method: 'POST',
                  body: JSON.stringify(credentials),
                })
                if ('connections' in result) {
                  setData(result)
                  setOnboarding(null)
                  notify(
                    t('channels:channelsPage.nameTwoWayConnectionEstablished', {
                      name: providerName(onboarding.platform, t),
                    }),
                  )
                  await load()
                } else if ('mode' in result && result.mode === 'manual') {
                  setOnboarding({
                    ...result,
                    platform: onboarding.platform,
                    status: 'starting',
                    manual: true,
                  })
                } else {
                  setOnboarding({
                    ...result,
                    platform: onboarding.platform,
                    manual: result.mode === 'manual',
                  })
                }
              } catch (caught) {
                setOnboarding((current) =>
                  current ? { ...current, status: 'failed', error: errorMessage(caught) } : current,
                )
              }
            }}
            notify={notify}
          />
        )}
      </div>
    </PageFrame>
  )
}

function OnboardingModal({
  job,
  onClose,
  onRetry,
  onSubmitCredentials,
  notify,
}: OnboardingModalProps) {
  const { t, language } = useI18n()
  const [code, setCode] = useState('')
  const [appId, setAppId] = useState('')
  const [appSecret, setAppSecret] = useState('')
  const [token, setToken] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const terminal = ['completed', 'failed', 'cancelled'].includes(job.status)
  const manual = job.manual || (isManualPlatform(job.platform) && job.mode !== 'qr')
  const credentialsValid =
    job.platform === 'qq'
      ? Boolean(appId.trim() && (appSecret.trim() || token.trim()))
      : Boolean(token.trim())
  const submitCredentials = async () => {
    if (!credentialsValid) return
    setSubmitting(true)
    try {
      await onSubmitCredentials({
        ...(appId.trim() ? { appId: appId.trim() } : {}),
        ...(appSecret.trim() ? { appSecret: appSecret.trim() } : {}),
        ...(token.trim() ? { token: token.trim() } : {}),
      })
    } finally {
      setSubmitting(false)
    }
  }
  const submitCode = async () => {
    try {
      await apiJson(
        `/api/channels/${job.platform}/onboarding/${encodeURIComponent(job.id || '')}/verify`,
        { method: 'POST', body: JSON.stringify({ code }) },
      )
      notify(t('channels:channelsPage.pairingCodeSubmitted'))
    } catch (caught) {
      notify(errorMessage(caught), 'error')
    }
  }
  return (
    <div
      className={MODAL_BACKDROP}
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section className={cn(MODAL_SURFACE, 'feishu-onboard-modal')}>
        <AppCardHeader className="[&_h2]:text-[15px] [&_h2]:font-medium">
          <div>
            <h2>
              {manual
                ? t('channels:channelsPage.configureName', { name: providerName(job.platform, t) })
                : t('channels:channelsPage.connectNameByQRCode', {
                    name: providerName(job.platform, t),
                  })}
            </h2>
            <p>
              {job.platform === 'feishu'
                ? t(
                    'channels:channelsPage.createTheBotAppThroughTheOfficialFeishuAuthorizationPage',
                  )
                : job.platform === 'weixin'
                  ? t('channels:channelsPage.signInToPersonalWeChatThroughTencentILinkBot')
                  : job.platform === 'qq'
                    ? t('channels:channelsPage.scanWithQQToCreateOfficialBot')
                    : t('channels:channelsPage.enterBotCredentialsForName', {
                        name: providerName(job.platform, t),
                      })}
            </p>
          </div>
          <Button
            variant="ghost"
            size="icon"
            className="-mt-1 -mr-1 max-[650px]:size-11"
            aria-label={t('channels:channelsPage.closeDialog')}
            onClick={onClose}
          >
            <X />
          </Button>
        </AppCardHeader>
        {manual && !job.id && !terminal && (
          <div className="mt-3.5 grid gap-2.5">
            {job.platform === 'qq' && (
              <>
                <FieldLabel variant="control">
                  {t('channels:channelsPage.qqAppId')}
                  <Input value={appId} onChange={(event) => setAppId(event.target.value)} />
                </FieldLabel>
                <FieldLabel variant="control">
                  {t('channels:channelsPage.qqAppSecret')}
                  <Input
                    type="password"
                    value={appSecret}
                    onChange={(event) => setAppSecret(event.target.value)}
                  />
                </FieldLabel>
                <small className="text-xs text-content-muted">
                  {t('channels:channelsPage.qqCredentialsOrToken')}
                </small>
              </>
            )}
            <FieldLabel variant="control">
              {t('channels:channelsPage.botToken')}
              <Input
                type="password"
                value={token}
                onChange={(event) => setToken(event.target.value)}
              />
            </FieldLabel>
            {job.setupUrl && (
              <AppNotice
                className={cn(
                  NOTICE,
                  '[&_a]:inline-flex [&_a]:items-center [&_a]:gap-1 [&_a]:text-xs [&_a]:text-content [&_a]:underline [&_a]:underline-offset-4',
                )}
              >
                {job.qrDataUrl && (
                  <img
                    src={job.qrDataUrl}
                    alt={t('channels:channelsPage.openOfficialSetupQRCode')}
                    className="size-[120px] rounded-sm bg-[var(--lightbox-action-bg)] p-1"
                  />
                )}
                <span>
                  <strong>{t('channels:channelsPage.officialSetupRequired')}</strong>
                  <small>{t('channels:channelsPage.officialSetupInstructions')}</small>
                  <a href={job.setupUrl} target="_blank" rel="noreferrer">
                    {t('channels:channelsPage.openOfficialSetup')}
                    <ExternalLink size={12} />
                  </a>
                </span>
              </AppNotice>
            )}
            <Button
              className="max-[650px]:h-11"
              loading={submitting}
              disabled={!credentialsValid || submitting}
              onClick={() => void submitCredentials()}
            >
              <Plus />
              {t('channels:channelsPage.connect')}
            </Button>
          </div>
        )}
        {(!manual || job.id || terminal) && (
          <div
            className={cn(
              'feishu-qr-stage mt-3.5 flex min-h-[min(330px,52dvh)] flex-col items-center justify-center gap-2.5 rounded-md border border-stroke bg-surface-subtle p-4 text-center [&_img]:w-[248px] [&_img]:max-w-[86%] [&_img]:rounded-md [&_img]:border [&_img]:border-stroke [&_img]:bg-[var(--lightbox-action-bg)] [&_img]:p-2 [&_p]:max-w-[330px] [&_p]:text-xs [&_p]:leading-normal [&_p]:text-danger [&_small]:text-xs [&_small]:text-content-muted [&_small]:tabular-nums [&_strong]:text-sm [&_strong]:font-medium [&.completed]:text-success [&.failed]:text-danger',
              job.status,
            )}
          >
            {job.qrDataUrl ? (
              <img
                src={job.qrDataUrl}
                alt={t('channels:channelsPage.nameConnectionQRCode', {
                  name: providerName(job.platform, t),
                })}
              />
            ) : job.status === 'failed' ? (
              <AlertTriangle className="size-8" strokeWidth={1.5} />
            ) : (
              <OrbitSpinner className="size-7" />
            )}
            <strong>
              {manual && !job.id
                ? t('channels:channelsPage.enterBotCredentialsForName', {
                    name: providerName(job.platform, t),
                  })
                : onboardingStatusLabel(job.status, t)}
            </strong>
            {job.error && <p>{job.error}</p>}
            {job.expireAt && !terminal && (
              <small>
                {t('channels:channelsPage.qrCodeExpiresTime', {
                  time: expiresIn(job.expireAt, language),
                })}
              </small>
            )}
          </div>
        )}
        {job.needsVerifyCode && (
          <div className="weixin-verify-code mt-2.5 grid grid-cols-[minmax(0,1fr)_auto] gap-2">
            <Input
              className="font-mono tracking-[.08em] tabular-nums"
              inputMode="numeric"
              value={code}
              onChange={(event) => setCode(event.target.value.replace(/\D/g, '').slice(0, 8))}
              placeholder={t('channels:channelsPage.enterTheNumberShownOnYourPhone')}
            />
            <Button className="max-[650px]:h-11" disabled={!code} onClick={submitCode}>
              {t('channels:channelsPage.submitPairingCode')}
            </Button>
          </div>
        )}
        {job.qrUrl && !terminal && (
          <Button
            asChild
            variant="secondary"
            className="mt-2.5 w-full no-underline max-[650px]:h-11"
          >
            <a href={job.qrUrl} target="_blank" rel="noreferrer">
              <ExternalLink />
              {t('channels:channelsPage.cannotScanOpenTheSignInLink')}
            </a>
          </Button>
        )}
        <AppNotice className={NOTICE}>
          <ShieldCheck size={15} />
          <span>
            <strong>{t('channels:channelsPage.persistentTwoWayConnection')}</strong>
            <small>
              {job.platform === 'feishu'
                ? t('channels:channelsPage.webSocketReceivesDirectMessagesAndGroupMentions')
                : job.platform === 'weixin'
                  ? t(
                      'channels:channelsPage.tencentILinkContinuouslyPollsDirectMessagesAndSupportsTextAndMediaReplies',
                    )
                  : providerCapability(job.platform, t)}
            </small>
          </span>
        </AppNotice>
        <div className="mt-4 flex justify-end gap-2">
          <Button variant="secondary" className="max-[650px]:h-11" onClick={onClose}>
            {terminal ? t('channels:channelsPage.off') : t('channels:channelsPage.cancel')}
          </Button>
          {job.status === 'failed' && (
            <Button className="max-[650px]:h-11" onClick={onRetry}>
              <RefreshCw />
              {manual
                ? t('channels:channelsPage.retry')
                : t('channels:channelsPage.generateANewQRCode')}
            </Button>
          )}
        </div>
      </section>
    </div>
  )
}
