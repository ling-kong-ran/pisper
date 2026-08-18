// 渠道页：外部消息渠道（飞书/微信）连接管理、状态与测试。
import { useCallback, useEffect, useState } from 'react'
import {
  AlertTriangle,
  Bot,
  CheckCircle2,
  ExternalLink,
  FolderOpen,
  MessageCircle,
  MessageSquare,
  Plus,
  RefreshCw,
  ShieldCheck,
  Trash2,
  Unplug,
  X,
  Zap,
} from 'lucide-react'
import {
  AppCard as Panel,
  AppSectionTitle as SectionTitle,
  AppSwitch as Toggle,
  StatusBadge as Badge,
  AppCardHeader,
  AppError,
  AppEmptyState,
  AppNotice,
} from '@/components/ui/app-primitives'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { StarOrbit } from '@/components/StarOrbit'
import { apiJson } from '@/lib/api'
import { relativeTime } from '@/lib/format'
import { APP_AGENT_NAME, APP_NAME } from '@/app/brand'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import type { LucideIcon } from 'lucide-react'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

type ChannelPlatform = 'feishu' | 'weixin'
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
type ProviderDefinition = {
  Icon: LucideIcon
  tone: 'blue' | 'green'
}
type ReplyModel = { provider: string; model: string }
type ChannelConnection = {
  enabled: boolean
  status: ChannelStatus
  defaultCwd: string
  replyModel: ReplyModel | null
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
  expireAt?: string
  needsVerifyCode?: boolean
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
  notify: Notify
}

const PROVIDERS: Record<ChannelPlatform, ProviderDefinition> = {
  feishu: {
    Icon: Bot,
    tone: 'blue',
  },
  weixin: {
    Icon: MessageCircle,
    tone: 'green',
  },
}
const PROVIDER_ENTRIES = Object.entries(PROVIDERS) as Array<[ChannelPlatform, ProviderDefinition]>

function providerName(platform: ChannelPlatform, t: ReturnType<typeof useI18n>['t']) {
  return platform === 'feishu'
    ? t('channels:channelsPage.feishu')
    : t('channels:channelsPage.weChat')
}

function providerTitle(platform: ChannelPlatform, t: ReturnType<typeof useI18n>['t']) {
  return platform === 'feishu'
    ? t('channels:channelsPage.feishuAppBot')
    : t('channels:channelsPage.weChat')
}

function providerTransport(platform: ChannelPlatform, t: ReturnType<typeof useI18n>['t']) {
  return platform === 'feishu'
    ? t('channels:channelsPage.webSocketPersistentConnection')
    : t('channels:channelsPage.tencentILinkPersistentConnection')
}

function providerCapability(platform: ChannelPlatform, t: ReturnType<typeof useI18n>['t']) {
  return platform === 'feishu'
    ? t('channels:channelsPage.feishuCapabilities')
    : t('channels:channelsPage.weChatCapabilities')
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
  const { t, language } = useI18n()
  const [data, setData] = useState<ChannelsData>({
    providers: [],
    connections: { feishu: null, weixin: null },
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
        message: t(
          'channels:channelsPage.scanningAgainWillReplaceTheCurrentNameConnectionContinue',
          {
            name: providerName(platform, t),
          },
        ),
        confirmLabel: t('channels:channelsPage.continueScanning'),
        tone: 'primary',
      })
      if (!approved) return
    }
    setSelectedPlatform(platform)
    setStarting(platform)
    setOnboarding({ platform, status: 'starting' })
    try {
      const job = await apiJson<Omit<OnboardingJob, 'platform'>>(
        `/api/channels/${platform}/onboarding`,
        {
          method: 'POST',
          body: '{}',
        },
      )
      setOnboarding({ ...job, platform })
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
    patch: Partial<Pick<ChannelConnection, 'enabled' | 'accessMode' | 'defaultCwd' | 'replyModel'>>,
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
        <RefreshCw className="animate-spin" size={23} />
        <h2>{t('channels:channelsPage.loadingChannels')}</h2>
      </AppEmptyState>
    )
  const selectedConnection = data.connections?.[selectedPlatform]
  return (
    <div className="flex flex-col gap-[12px]">
      {error && (
        <AppError>
          <AlertTriangle size={13} />
          {error}
        </AppError>
      )}
      <div className="channel-cards max-[900px]:grid-cols-[1fr] max-[650px]:grid-cols-[1fr] grid grid-cols-[repeat(2,minmax(0,1fr))] gap-[12px]">
        {PROVIDER_ENTRIES.map(([platform, provider]) => {
          const connection = data.connections?.[platform]
          const status = connection?.status || 'idle'
          const tone = channelStatusTone(status)
          const Icon = provider.Icon
          return (
            <Panel
              className={`provider-card channel-platform-card [transition:transform_var(--d2)_var(--ease-out),_box-shadow_var(--d2)_var(--ease-out),_border-color_var(--d2)_var(--ease-out)] hover:[transform:translateY(-2px)] hover:shadow-[var(--sh-2)] hover:border-[var(--star-border)] [&.selected]:border-[var(--accent-border)] [&.selected]:shadow-[0_10px_30px_-24px_var(--blue),0_0_0_2px_var(--selection-ring)] cursor-pointer [transition:border-color_var(--d1)_var(--ease-out),box-shadow_var(--d1)_var(--ease-out)] ${selectedPlatform === platform ? 'selected' : ''}`}
              key={platform}
              onClick={() => {
                setSelectedPlatform(platform)
                setCwd(connection?.defaultCwd || '')
              }}
            >
              <div className="provider-title [&_h2]:text-[14px] [&_p]:mt-[3px] [&_p]:text-[var(--text-muted)] [&_p]:text-[12px] grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[10px] [margin-bottom:10px]">
                <span
                  className={`provider-icon [&_svg]:w-[19px] [&.green]:bg-[var(--success-soft)] [&.green]:text-[var(--success)] [&.blue]:bg-[var(--brand-blue-soft)] [&.blue]:text-[var(--brand-blue-strong)] [.notification-option_&:not(.blue)]:bg-[var(--surface-muted)] [.notification-option_&:not(.blue)]:text-[var(--text-muted)] grid w-[38px] h-[38px] place-items-center rounded-[var(--r-md)] ${provider.tone}`}
                >
                  <Icon />
                </span>
                <div>
                  <h2>{providerTitle(platform, t)}</h2>
                  <p>{providerTransport(platform, t)}</p>
                </div>
                <Badge tone={tone}>{channelStatusLabel(status, t)}</Badge>
              </div>
              <FieldLabel variant="control">
                {t('channels:channelsPage.twoWayCapability')}
                <span className="flex min-h-[31px] items-center overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--surface-subtle)] [padding:0_9px] text-[var(--text-tertiary)] text-[12px] font-[400] text-ellipsis whitespace-nowrap">
                  {providerCapability(platform, t)}
                </span>
              </FieldLabel>
              <FieldLabel variant="control">
                {t('channels:channelsPage.replyModel')}
                <span className="flex min-h-[31px] items-center overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--surface-subtle)] [padding:0_9px] text-[var(--text-tertiary)] text-[12px] font-[400] text-ellipsis whitespace-nowrap">
                  {connection?.replyModel
                    ? `${connection.replyModel.provider}/${connection.replyModel.model}`
                    : t('channels:channelsPage.useApplicationDefaultModel')}
                </span>
              </FieldLabel>
              <Button
                variant={connection ? 'outline' : 'default'}
                size="lg"
                className="[margin-top:12px] w-full"
                disabled={starting === platform}
                onClick={(event) => {
                  event.stopPropagation()
                  beginOnboarding(platform)
                }}
              >
                {starting === platform ? (
                  <RefreshCw className="animate-spin" size={14} />
                ) : (
                  <Plus size={14} />
                )}
                {connection
                  ? t('channels:channelsPage.scanAgainToReconnect', {
                      name: providerName(platform, t),
                    })
                  : t('channels:channelsPage.connectNameByQRCode', {
                      name: providerName(platform, t),
                    })}
              </Button>
            </Panel>
          )
        })}
      </div>

      <div className="two-one-grid max-[900px]:grid-cols-[1fr] grid grid-cols-[minmax(0,2fr)_minmax(260px,1fr)] gap-[12px]">
        <Panel>
          <div className="channel-section-head [&_>_span]:text-[var(--text-muted)] [&_>_span]:text-[13px] flex items-center justify-between gap-[8px] [margin-bottom:8px]">
            <SectionTitle title={t('channels:channelsPage.channelChats')} />
            <span>{t('channels:channelsPage.countLinked', { count: data.scopes.length })}</span>
          </div>
          {data.scopes.length ? (
            data.scopes.map((scope) => (
              <div
                className="route-row [&_div]:flex [&_div]:flex-col [&_div]:gap-[3px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[9px] [border-top:1px_solid_var(--stroke-soft)] [padding:10px_2px]"
                key={scope.key}
              >
                <span className="route-icon grid w-[27px] h-[27px] place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)]">
                  {scope.platform === 'feishu' ? (
                    <MessageSquare size={14} />
                  ) : (
                    <MessageCircle size={14} />
                  )}
                </span>
                <div className="channel-route-copy [.route-row_&]:flex [.route-row_&]:min-w-0 [.route-row_&]:flex-col [.route-row_&]:gap-[3px] [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap">
                  <strong>
                    {scope.title}{' '}
                    <Badge tone={scope.platform === 'feishu' ? 'blue' : 'green'}>
                      {providerName(scope.platform, t)}
                    </Badge>
                  </strong>
                  <small>
                    {scope.lastMessage || t('channels:channelsPage.noMessages')} ·{' '}
                    {relativeTime(scope.updatedAt, language)}
                  </small>
                  <small>
                    {scope.model || t('channels:channelsPage.defaultModel')} · {scope.cwd}
                  </small>
                </div>
                <div className="channel-route-controls [.route-row_&]:flex [.route-row_&]:flex-row [.route-row_&]:items-center [.route-row_&]:gap-[4px]">
                  <Badge tone="green">{t('channels:channelsPage.twoWay')}</Badge>
                  <Button
                    variant="destructive"
                    size="icon"
                    title={t('channels:channelsPage.resetChat')}
                    onClick={() => resetScope(scope)}
                  >
                    <Trash2 size={13} />
                  </Button>
                </div>
              </div>
            ))
          ) : (
            <div className="channel-route-empty [&_strong]:mt-[9px] [&_strong]:text-[var(--text)] [&_strong]:text-[12px] [&_span]:mt-[4px] [&_span]:text-[13px] [&.compact]:min-h-[110px] [.workflow-assets-panel_&]:min-h-[150px] [.workflow-assets-panel_&]:border-0 [.workflow-assets-panel_&]:bg-transparent grid min-h-[185px] place-content-center justify-items-center text-[var(--text-muted)] text-center">
              <StarOrbit size={38} />
              <strong>{t('channels:channelsPage.waitingForTheFirstReplyFromAfar')}</strong>
              <span>
                {t(
                  'channels:channelsPage.onceConnectedMessagesFromFeishuOrWeixinWillArriveHereInPisper',
                )}
              </span>
            </div>
          )}
        </Panel>
        <Panel className="test-panel [&_>_p]:m-[8px_0_12px] [&_>_p]:text-[var(--text-muted)] [&_>_p]:text-[12px] [&_>_p]:leading-[1.5]">
          <div className="channel-section-head [&_>_span]:text-[var(--text-muted)] [&_>_span]:text-[13px] flex items-center justify-between gap-[8px] [margin-bottom:8px]">
            <SectionTitle
              title={t('channels:channelsPage.nameSettings', {
                name: providerName(selectedPlatform, t),
              })}
            />
            <span>{providerTransport(selectedPlatform, t)}</span>
          </div>
          {selectedConnection ? (
            <>
              <div
                className={`channel-live-status [&_>_div]:flex [&_>_div]:min-w-0 [&_>_div]:flex-col [&_>_div]:gap-[3px] [&_strong]:text-[13px] [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap flex items-center gap-[9px] [margin:11px_0_12px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] [padding:10px] ${selectedConnection.status}`}
              >
                <span className="channel-status-dot [.channel-live-status.connected_&]:bg-[var(--status-green)] [.channel-live-status.connected_&]:shadow-[0_0_0_4px_var(--success-soft)] [.channel-live-status.connecting_&]:bg-[var(--amber)] [.channel-live-status.connecting_&]:shadow-[0_0_0_4px_var(--warning-soft)] [.channel-live-status.connecting_&]:[animation:pulse-dot_1.2s_infinite] [.channel-live-status.reconnecting_&]:bg-[var(--amber)] [.channel-live-status.reconnecting_&]:shadow-[0_0_0_4px_var(--warning-soft)] [.channel-live-status.reconnecting_&]:[animation:pulse-dot_1.2s_infinite] [.channel-live-status.failed_&]:bg-[var(--danger)] [.channel-live-status.failed_&]:shadow-[0_0_0_4px_var(--danger-soft)] w-[8px] h-[8px] [flex:0_0_auto] rounded-[var(--r-pill)] bg-[var(--status-muted)] shadow-[0_0_0_4px_var(--surface-muted)]" />
                <div>
                  <strong>{channelStatusLabel(selectedConnection.status, t)}</strong>
                  <small>
                    {selectedConnection.lastError ||
                      (selectedConnection.status === 'connected'
                        ? t('channels:channelsPage.connectedTime', {
                            time: relativeTime(selectedConnection.connectedAt, language),
                          })
                        : t('channels:channelsPage.establishingPersistentConnection'))}
                  </small>
                </div>
              </div>
              <div className="modal-toggle-row [&_>_span]:flex [&_>_span]:flex-col [&_>_span]:gap-[3px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] dark:bg-[var(--surface-subtle)] flex min-h-[45px] items-center justify-between gap-[12px] [margin-top:10px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] [padding:8px_10px]">
                <span>
                  <strong>{t('channels:channelsPage.enableThisChannel')}</strong>
                  <small>
                    {t('channels:channelsPage.turnOffToDisconnectWhileKeepingSignInCredentials')}
                  </small>
                </span>
                <Toggle
                  value={selectedConnection.enabled}
                  disabled={saving}
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
              <FieldLabel variant="control">
                {t('channels:channelsPage.replyModel')}
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
                  <option value="">{t('channels:channelsPage.useApplicationDefaultModel')}</option>
                  {data.models.map((model) => (
                    <option
                      value={`${model.provider}/${model.model}`}
                      key={`${model.provider}/${model.model}`}
                    >
                      {model.label}
                    </option>
                  ))}
                </AppSelect>
              </FieldLabel>
              <FieldLabel variant="control">
                {t('channels:channelsPage.accessScope')}
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
                      : t('channels:channelsPage.allWeChatUsersWhoMessageTheBot')}
                  </option>
                </AppSelect>
              </FieldLabel>
              <FieldLabel variant="control">
                {t('channels:channelsPage.defaultWorkingDirectoryForNewChats')}
                <span className="channel-setting-input [&_input]:min-w-0 [&_input]:border-0 [&_input]:[outline:0] [&_input]:bg-transparent [&_input]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_input]:text-[12px] dark:[&_input]:bg-[var(--solid)] dark:[&_input]:text-[var(--text)] grid h-[34px] grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[7px] [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--surface-subtle)] [padding:3px_3px_3px_9px] text-[var(--text-muted)]">
                  <FolderOpen size={13} />
                  <input value={cwd} onChange={(event) => setCwd(event.target.value)} />
                  <Button
                    variant="outline"
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
              </FieldLabel>
              <AppNotice>
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
              <div className="mt-[15px] flex gap-2 max-[650px]:flex-wrap">
                <Button
                  variant="outline"
                  size="lg"
                  className="bg-surface-subtle"
                  onClick={() => reconnect(selectedPlatform)}
                  disabled={saving}
                >
                  <RefreshCw size={14} />
                  {t('channels:channelsPage.reconnect')}
                </Button>
                <Button variant="destructive" size="lg" onClick={() => remove(selectedPlatform)}>
                  <Unplug size={14} />
                  {t('channels:channelsPage.disconnect')}
                </Button>
              </div>
            </>
          ) : (
            <>
              <p>
                {t(
                  'channels:channelsPage.afterYouScanAndConfirmCredentialsAreSavedAndTheChannelStaysOnlineIncomingMessagesGoDirectlyToThe',
                  {
                    agent: APP_AGENT_NAME,
                  },
                )}
              </p>
              <div className="test-summary [.test-panel_&]:min-w-0 [.test-panel_&:last-of-type]:overflow-hidden [.test-panel_&:last-of-type]:text-ellipsis [.test-panel_&:last-of-type]:whitespace-nowrap flex items-center gap-[6px] [margin-top:8px] text-[var(--text-soft)] text-[12px]">
                <CheckCircle2 size={14} />
                {t('channels:channelsPage.trueTwoWayMessagingNoNotificationWebhookRequired')}
              </div>
              <div className="test-summary [.test-panel_&]:min-w-0 [.test-panel_&:last-of-type]:overflow-hidden [.test-panel_&:last-of-type]:text-ellipsis [.test-panel_&:last-of-type]:whitespace-nowrap flex items-center gap-[6px] [margin-top:8px] text-[var(--text-soft)] text-[12px]">
                <CheckCircle2 size={14} />
                {t('channels:channelsPage.eachContactOrChatMapsToAnIndependentAgentSession')}
              </div>
              <Button
                size="lg"
                className="w-full"
                onClick={() => beginOnboarding(selectedPlatform)}
              >
                <Zap size={15} />
                {t('channels:channelsPage.connectNameByQRCode', {
                  name: providerName(selectedPlatform, t),
                })}
              </Button>
            </>
          )}
        </Panel>
      </div>

      {onboarding && (
        <OnboardingModal
          job={onboarding}
          onClose={closeOnboarding}
          onRetry={() => beginOnboarding(onboarding.platform)}
          notify={notify}
        />
      )}
    </div>
  )
}

function OnboardingModal({ job, onClose, onRetry, notify }: OnboardingModalProps) {
  const { t, language } = useI18n()
  const [code, setCode] = useState('')
  const terminal = ['completed', 'failed', 'cancelled'].includes(job.status)
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
      className="modal-backdrop max-[650px]:p-[8px] fixed z-[70] inset-0 grid place-items-center overflow-y-auto bg-[var(--modal-overlay)] [backdrop-filter:blur(3px)] [padding:20px] [overscroll-behavior:contain] [animation:fade-in_var(--d1)_var(--ease-out)]"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section className="modal !w-[min(430px,100%)] max-h-[calc(100dvh_-_40px)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)] feishu-onboard-modal w-[min(470px,100%)]">
        <AppCardHeader>
          <div>
            <h2>
              {t('channels:channelsPage.connectNameByQRCode', {
                name: providerName(job.platform, t),
              })}
            </h2>
            <p>
              {job.platform === 'feishu'
                ? t(
                    'channels:channelsPage.createTheBotAppThroughTheOfficialFeishuAuthorizationPage',
                  )
                : t('channels:channelsPage.signInToPersonalWeChatThroughTencentILinkBot')}
            </p>
          </div>
          <Button
            variant="ghost"
            size="icon"
            aria-label={t('channels:channelsPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </Button>
        </AppCardHeader>
        <div
          className={`feishu-qr-stage [&_img]:w-[248px] [&_img]:max-w-[86%] [&_img]:[border:1px_solid_var(--stroke-soft)] [&_img]:rounded-[var(--r-md)] [&_img]:bg-[var(--lightbox-action-bg)] [&_img]:p-[8px] [&_img]:shadow-[0_12px_30px_-24px_var(--ink-strong)] [&_strong]:text-[13px] [&_p]:max-w-[330px] [&_p]:text-[var(--danger)] [&_p]:text-[12px] [&_p]:leading-[1.5] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&.completed]:text-[var(--success)] [&.failed]:text-[var(--danger)] flex min-h-[min(330px,52dvh)] flex-col items-center justify-center gap-[10px] [margin-top:14px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-md)] bg-[linear-gradient(180deg,var(--surface-highlight),var(--surface-subtle))] [padding:18px] text-center ${job.status}`}
        >
          {job.qrDataUrl ? (
            <img
              src={job.qrDataUrl}
              alt={t('channels:channelsPage.nameConnectionQRCode', {
                name: providerName(job.platform, t),
              })}
            />
          ) : job.status === 'failed' ? (
            <AlertTriangle size={42} />
          ) : (
            <RefreshCw className="animate-spin" size={32} />
          )}
          <strong>{onboardingStatusLabel(job.status, t)}</strong>
          {job.error && <p>{job.error}</p>}
          {job.expireAt && !terminal && (
            <small>
              {t('channels:channelsPage.qrCodeExpiresTime', {
                time: expiresIn(job.expireAt, language),
              })}
            </small>
          )}
        </div>
        {job.needsVerifyCode && (
          <div className="weixin-verify-code [&_input]:min-w-0 [&_input]:h-[34px] [&_input]:[border:1px_solid_var(--stroke)] [&_input]:rounded-[var(--r-sm)] [&_input]:bg-[var(--solid)] [&_input]:p-[0_10px] [&_input]:text-[var(--text)] [&_input]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_input]:text-[13px] [&_input]:tracking-[.08em] grid grid-cols-[minmax(0,1fr)_auto] gap-[7px] [margin-top:9px]">
            <input
              value={code}
              onChange={(event) => setCode(event.target.value.replace(/\D/g, '').slice(0, 8))}
              placeholder={t('channels:channelsPage.enterTheNumberShownOnYourPhone')}
            />
            <Button size="lg" disabled={!code} onClick={submitCode}>
              {t('channels:channelsPage.submitPairingCode')}
            </Button>
          </div>
        )}
        {job.qrUrl && !terminal && (
          <Button
            asChild
            variant="outline"
            size="lg"
            className="[margin-top:9px] no-underline w-full bg-surface-subtle"
          >
            <a href={job.qrUrl} target="_blank" rel="noreferrer">
              <ExternalLink size={14} />
              {t('channels:channelsPage.cannotScanOpenTheSignInLink')}
            </a>
          </Button>
        )}
        <AppNotice>
          <ShieldCheck size={15} />
          <span>
            <strong>{t('channels:channelsPage.persistentTwoWayConnection')}</strong>
            <small>
              {job.platform === 'feishu'
                ? t('channels:channelsPage.webSocketReceivesDirectMessagesAndGroupMentions')
                : t(
                    'channels:channelsPage.tencentILinkContinuouslyPollsDirectMessagesAndSupportsTextAndMediaReplies',
                  )}
            </small>
          </span>
        </AppNotice>
        <div className="flex justify-end gap-[8px] [margin-top:18px]">
          <Button variant="outline" size="lg" className="bg-surface-subtle" onClick={onClose}>
            {terminal ? t('channels:channelsPage.off') : t('channels:channelsPage.cancel')}
          </Button>
          {job.status === 'failed' && (
            <Button size="lg" onClick={onRetry}>
              <RefreshCw size={14} />
              {t('channels:channelsPage.generateANewQRCode')}
            </Button>
          )}
        </div>
      </section>
    </div>
  )
}
