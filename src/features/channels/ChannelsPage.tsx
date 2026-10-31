import { FEATURE_SEARCH_INPUT_ID } from '@/app/more-pages'
import {
  FeatureList,
  FeaturePage,
  FeatureRow,
  FeatureSearch,
  FeatureSection,
  FeatureToolbar,
} from '@/components/app/FeaturePage'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { StatusGlyph } from '@/components/app/StatusGlyph'
// 渠道页（功能页模板，规格 v2 第 10 节 A6 第 3 批）：外部消息渠道（飞书 / 微信 / QQ / Telegram）每个平台一行，
// 右侧是状态与连接 / 重新连接；展开在行下方调整该平台的设置。下面是已绑定的渠道会话。
// “断开”（启用了却没连上）是琥珀——需要人去处理（规格 2.7）；其余状态中性。
import { useCallback, useEffect, useState, type ReactNode } from 'react'
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
import { AppSwitch as Toggle, AppCardHeader, AppNotice } from '@/components/ui/app-primitives'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { OrbitSpinner } from '@/components/ui/spinner'
import { apiJson } from '@/lib/api'
import { cn } from '@/lib/utils'
import { relativeTime } from '@/lib/format'
import { APP_NAME } from '@/app/brand'
import { useShortcutLabel } from '@/lib/shortcuts'
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
  // “‹ 功能”：从功能进入时由应用壳给出（规格 2.4）。
  onBack?: () => void
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

// 只读键值行：左侧弱化标签，右侧值单行省略。
const KEY_VALUE =
  'flex min-h-[34px] min-w-0 items-center justify-between gap-4 border-t border-stroke-soft py-1.5 text-sm first:border-t-0 [&>dt]:shrink-0 [&>dt]:text-content-muted [&>dd]:min-w-0 [&>dd]:truncate [&>dd]:text-right [&>dd]:text-content'
// 标签在左的表单行：页面栏够宽时 168px 标签列，窄屏堆叠。
const FORM_ROW =
  'grid min-w-0 gap-1.5 border-t border-stroke-soft py-3 @min-[600px]/page:grid-cols-[168px_minmax(0,1fr)] @min-[600px]/page:items-center @min-[600px]/page:gap-4'
const FORM_LABEL = 'text-sm font-medium text-content'
// 渠道弹窗沿用全局 .modal 浮层：纯色遮罩（无背景模糊）、浮层底色 + 环线投影。
const MODAL_BACKDROP =
  'modal-backdrop fixed inset-0 z-[70] grid place-items-center overflow-y-auto overscroll-contain bg-[var(--modal-overlay)] p-5 max-[650px]:p-2'
const MODAL_SURFACE =
  'modal max-h-[calc(100dvh_-_40px)] w-[min(470px,100%)] overflow-y-auto overscroll-contain rounded-dialog bg-popover p-5 shadow-float max-[650px]:max-h-[calc(100dvh_-_16px)]'
// 说明条保持单色：强调色只留给选中、焦点与主操作。
const NOTICE =
  'border border-stroke-soft bg-surface-subtle text-content-secondary [&_strong]:text-content [&>svg]:mt-0.5 [&>svg]:shrink-0'
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

export function ChannelsPage({ notify, onBack, requestConfirm }: ChannelsPageProps) {
  const { t, language } = useI18n()
  const [data, setData] = useState<ChannelsData>({
    providers: [],
    connections: { feishu: null, weixin: null, qq: null, telegram: null },
    scopes: [],
    models: [],
  })
  const [loading, setLoading] = useState(true)
  // 至少读成功过一次：首次读取失败显示可重试的错误，而不是“全部未连接”。
  const [loaded, setLoaded] = useState(false)
  const [error, setError] = useState('')
  // 展开的平台（同一时刻只展开一行）：设置、工作目录草稿都属于它；收起时为空。
  const [expandedPlatform, setExpandedPlatform] = useState<ChannelPlatform | ''>('')
  // 搜索词由页面自己持有（换页即清空），同时筛选平台与渠道会话。
  const [query, setQuery] = useState('')
  const [onboarding, setOnboarding] = useState<OnboardingJob | null>(null)
  const [starting, setStarting] = useState<ChannelPlatform | ''>('')
  const [saving, setSaving] = useState(false)
  const [cwd, setCwd] = useState('')

  // 加载渠道连接状态，并同步展开平台的默认工作目录。
  const load = useCallback(async () => {
    try {
      setError('')
      const result = await apiJson<ChannelsData>('/api/channels')
      setData(result)
      setLoaded(true)
      if (expandedPlatform) setCwd(result.connections?.[expandedPlatform]?.defaultCwd || '')
    } catch (caught) {
      setError(errorMessage(caught))
    } finally {
      setLoading(false)
    }
  }, [expandedPlatform])

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
      <ChannelsFrame onBack={onBack} query={query} onQueryChange={setQuery}>
        <PageState kind="loading" title={t('channels:channelsPage.loadingChannels')} />
      </ChannelsFrame>
    )
  if (error && !loaded)
    return (
      // 首次读取失败不显示成“全部未连接”：写明错误并给重试（老问题，原来是默认空数据 + 顶部错误条）。
      <ChannelsFrame onBack={onBack} query={query} onQueryChange={setQuery}>
        <PageState
          kind="error"
          title={t('channels:channelsPage.loadFailed')}
          description={error}
          action={
            <Button
              variant="secondary"
              className="max-[650px]:h-11"
              onClick={() => {
                setLoading(true)
                void load()
              }}
            >
              {t('common:pageTemplate.retry')}
            </Button>
          }
        />
      </ChannelsFrame>
    )
  const needle = query.trim().toLocaleLowerCase()
  const visiblePlatforms = PROVIDER_ENTRIES.filter(([platform]) =>
    `${providerTitle(platform, t)} ${providerName(platform, t)}`
      .toLocaleLowerCase()
      .includes(needle),
  )
  const visibleScopes = needle
    ? data.scopes.filter((scope) =>
        `${scope.title} ${scope.lastMessage || ''} ${providerName(scope.platform, t)}`
          .toLocaleLowerCase()
          .includes(needle),
      )
    : data.scopes
  const disconnected = PROVIDER_ENTRIES.filter(
    ([platform]) => channelState(data.connections?.[platform]) === 'disconnected',
  ).length
  return (
    <ChannelsFrame onBack={onBack} query={query} onQueryChange={setQuery}>
      {error && (
        <p
          role="alert"
          className="mt-4 flex items-start gap-1.5 rounded-[var(--r-md)] border border-danger-border bg-danger-soft px-3 py-2 text-xs [overflow-wrap:anywhere] text-danger"
        >
          <AlertTriangle className="mt-px size-3.5 flex-none" aria-hidden="true" />
          {error}
        </p>
      )}
      <FeatureSection
        title={t('channels:channelsPage.platforms')}
        count={PROVIDER_ENTRIES.length}
        meta={
          disconnected ? (
            // 断开是需要人处理的事：与功能索引的“1 断开”同一条规则，琥珀（规格 2.7、6.1）。
            <span data-channels-disconnected className="font-medium text-st-attn">
              {t('channels:channelsPage.disconnectedCount', { count: disconnected })}
            </span>
          ) : undefined
        }
      >
        {visiblePlatforms.length ? (
          <FeatureList>
            {visiblePlatforms.map(([platform, provider]) => {
              const connection = data.connections?.[platform]
              const expanded = expandedPlatform === platform
              const Icon = provider.Icon
              return (
                <FeatureRow
                  key={platform}
                  data-channel-platform={platform}
                  className="channel-platform-card"
                  icon={<Icon />}
                  name={providerTitle(platform, t)}
                  description={providerTransport(platform, t)}
                  status={<ChannelStateLabel connection={connection} />}
                  actions={
                    <Button
                      variant="secondary"
                      size="sm"
                      loading={starting === platform}
                      disabled={starting === platform}
                      title={connectActionLabel(platform, Boolean(connection), t)}
                      onClick={() => void beginOnboarding(platform)}
                    >
                      {connection ? <RefreshCw /> : <Plus />}
                      <span className="max-[650px]:sr-only">
                        {connectActionLabel(platform, Boolean(connection), t)}
                      </span>
                    </Button>
                  }
                  expanded={expanded}
                  onExpandedChange={(open) => {
                    setExpandedPlatform(open ? platform : '')
                    if (open) setCwd(connection?.defaultCwd || '')
                  }}
                  toggleLabel={t('channels:channelsPage.togglePlatformDetails', {
                    name: providerName(platform, t),
                  })}
                  detail={
                    expanded ? (
                      <ChannelPlatformDetail
                        platform={platform}
                        connection={connection}
                        models={data.models}
                        cwd={cwd}
                        saving={saving}
                        onCwdChange={setCwd}
                        onUpdate={update}
                        onReconnect={reconnect}
                        onRemove={remove}
                      />
                    ) : null
                  }
                />
              )
            })}
          </FeatureList>
        ) : (
          <InlineEmptyState compact title={t('channels:channelsPage.noMatchingChannels')} />
        )}
      </FeatureSection>
      <FeatureSection
        title={t('channels:channelsPage.channelChats')}
        count={data.scopes.length}
        description={t(
          'channels:channelsPage.onceConnectedMessagesFromSupportedChannelsWillArriveHereInPisper',
        )}
      >
        {visibleScopes.length ? (
          <FeatureList>
            {visibleScopes.map((scope) => (
              <FeatureRow
                key={scope.key}
                data-channel-scope={scope.key}
                className="route-row"
                icon={
                  scope.platform === 'feishu' ? (
                    <MessageSquare />
                  ) : scope.platform === 'telegram' ? (
                    <Send />
                  ) : (
                    <MessageCircle />
                  )
                }
                name={scope.title}
                nameMeta={providerName(scope.platform, t)}
                description={scope.lastMessage || t('channels:channelsPage.noMessages')}
                meta={
                  <span className="truncate font-mono">
                    {scope.model || t('channels:channelsPage.defaultModel')} · {scope.cwd}
                  </span>
                }
                status={relativeTime(scope.updatedAt, language)}
                actions={
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className="text-content-muted hover:bg-danger-soft hover:text-danger max-[650px]:size-11"
                    title={t('channels:channelsPage.resetChat')}
                    aria-label={`${t('channels:channelsPage.resetChat')}: ${scope.title}`}
                    onClick={() => void resetScope(scope)}
                  >
                    <Trash2 />
                  </Button>
                }
              />
            ))}
          </FeatureList>
        ) : (
          <InlineEmptyState
            title={
              data.scopes.length
                ? t('channels:channelsPage.noMatchingChats')
                : t('channels:channelsPage.waitingForTheFirstReplyFromAfar')
            }
          />
        )}
      </FeatureSection>

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
    </ChannelsFrame>
  )
}

// 页面骨架：标题、一句说明、“‹ 功能”与工具行（搜索平台与渠道会话）；读取中 / 失败也保留它们。
function ChannelsFrame({
  onBack,
  query,
  onQueryChange,
  children,
}: {
  onBack?: () => void
  query: string
  onQueryChange: (value: string) => void
  children: ReactNode
}) {
  const { t } = useI18n()
  const searchShortcut = useShortcutLabel('focusSearch')
  const title = t('navigation:navigation.channels')
  return (
    <FeaturePage
      title={title}
      description={t('channels:channelsPage.lede')}
      back={
        onBack && {
          label: t('navigation:sidebar.features'),
          ariaLabel: t('navigation:shell.backToFeaturesLabel'),
          onBack,
        }
      }
      toolbar={
        <FeatureToolbar
          search={
            <FeatureSearch
              id={FEATURE_SEARCH_INPUT_ID}
              value={query}
              onValueChange={onQueryChange}
              placeholder={t('channels:channelsPage.searchPlaceholder')}
              shortcut={searchShortcut}
              clearLabel={t('navigation:more.clearSearch')}
            />
          }
        />
      }
    >
      {children}
    </FeaturePage>
  )
}

// 渠道的人话状态（与功能索引角标 parseDisconnectedChannelCount 同一判断）：启用了却没连上 = 断开，需要人处理；
// 正在连接 / 重连中不算断开；没配置过的平台只是“未连接”，暂停的渠道是“已暂停”。
type ChannelState = 'none' | 'paused' | 'live' | 'busy' | 'disconnected'

function channelState(connection: ChannelConnection | null | undefined): ChannelState {
  if (!connection) return 'none'
  if (connection.enabled === false) return 'paused'
  if (connection.status === 'connected') return 'live'
  if (connection.status === 'connecting' || connection.status === 'reconnecting') return 'busy'
  return 'disconnected'
}

// 行右侧的状态（规格 2.7、6.1）：只有“断开”是琥珀圆点 + 琥珀文字，其余（在线、连接中、未连接、已暂停）中性。
function ChannelStateLabel({ connection }: { connection: ChannelConnection | null | undefined }) {
  const { t } = useI18n()
  const state = channelState(connection)
  const label =
    state === 'none'
      ? t('channels:channelsPage.notConnected')
      : state === 'paused'
        ? t('channels:channelsPage.paused')
        : state === 'disconnected'
          ? connection?.status === 'failed'
            ? t('channels:channelsPage.connectionFailed')
            : t('channels:channelsPage.disconnected')
          : channelStatusLabel(connection?.status || 'idle', t)
  return (
    <span
      data-channel-state={state}
      className={cn(
        'inline-flex min-w-0 items-center gap-1.5 whitespace-nowrap',
        state === 'disconnected' ? 'font-medium text-st-attn' : 'text-content-secondary',
      )}
    >
      {state === 'disconnected' ? (
        <StatusGlyph status="attention" className="size-3.5" />
      ) : state === 'busy' ? (
        <StatusGlyph status="run" className="size-3.5" />
      ) : state === 'live' ? (
        <StatusGlyph status="done" className="size-3.5" />
      ) : null}
      {label}
    </span>
  )
}

type ChannelPatch = Partial<
  Pick<
    ChannelConnection,
    'enabled' | 'accessMode' | 'defaultCwd' | 'replyModel' | 'executionMode' | 'runMode'
  >
>

// 展开的平台：能力与回复模型说明；已连接时是原“{平台}设置”（实时状态、启用、回复模型、审批与执行模式、
// 访问范围、默认工作目录、安全边界）与“解除连接 / 重连”。每项改动立即保存，与原来一致。
function ChannelPlatformDetail({
  platform,
  connection,
  models,
  cwd,
  saving,
  onCwdChange,
  onUpdate,
  onReconnect,
  onRemove,
}: {
  platform: ChannelPlatform
  connection: ChannelConnection | null | undefined
  models: ChannelModel[]
  cwd: string
  saving: boolean
  onCwdChange: (value: string) => void
  onUpdate: (platform: ChannelPlatform, patch: ChannelPatch, success: string) => void
  onReconnect: (platform: ChannelPlatform) => void
  onRemove: (platform: ChannelPlatform) => void
}) {
  const { t, language } = useI18n()
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <dl className="rounded-[var(--r-md)] border border-stroke px-3 py-0.5">
        <div className={KEY_VALUE}>
          <dt>{t('channels:channelsPage.twoWayCapability')}</dt>
          <dd>{providerCapability(platform, t)}</dd>
        </div>
        <div className={KEY_VALUE}>
          <dt>{t('channels:channelsPage.replyModel')}</dt>
          <dd className={connection?.replyModel ? 'font-mono' : undefined}>
            {connection?.replyModel
              ? `${connection.replyModel.provider}/${connection.replyModel.model}`
              : t('channels:channelsPage.useApplicationDefaultModel')}
          </dd>
        </div>
      </dl>
      {connection ? (
        <section
          className="test-panel min-w-0"
          aria-label={t('channels:channelsPage.nameSettings', { name: providerName(platform, t) })}
        >
          <div
            className={cn(
              'channel-live-status flex min-w-0 items-center gap-2.5 py-2',
              connection.status,
            )}
          >
            <ChannelStateLabel connection={connection} />
            <small
              className={cn(
                'min-w-0 truncate text-xs tabular-nums',
                connection.lastError ? 'text-content-secondary' : 'text-content-muted',
              )}
            >
              {connection.lastError ||
                (connection.status === 'connected'
                  ? t('channels:channelsPage.connectedTime', {
                      time: relativeTime(connection.connectedAt, language),
                    })
                  : t('channels:channelsPage.establishingPersistentConnection'))}
            </small>
          </div>
          <div className="modal-toggle-row flex min-w-0 items-center justify-between gap-4 border-t border-stroke-soft py-3">
            <span className="flex min-w-0 flex-col gap-0.5">
              <strong className="text-sm font-medium text-content">
                {t('channels:channelsPage.enableThisChannel')}
              </strong>
              <small className="text-xs text-content-muted">
                {t('channels:channelsPage.turnOffToDisconnectWhileKeepingSignInCredentials')}
              </small>
            </span>
            <Toggle
              value={connection.enabled}
              disabled={saving}
              ariaLabel={t('channels:channelsPage.enableThisChannel')}
              onChange={(enabled) =>
                onUpdate(
                  platform,
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
                connection.replyModel
                  ? `${connection.replyModel.provider}/${connection.replyModel.model}`
                  : ''
              }
              onChange={(event) => {
                const [provider, ...parts] = event.target.value.split('/')
                onUpdate(
                  platform,
                  {
                    replyModel: event.target.value ? { provider, model: parts.join('/') } : null,
                  },
                  t('channels:channelsPage.channelReplyModelUpdated'),
                )
              }}
            >
              <option value="">{t('channels:channelsPage.useApplicationDefaultModel')}</option>
              {models.map((model) => (
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
              value={connection.executionMode}
              onChange={(event) =>
                onUpdate(
                  platform,
                  { executionMode: event.target.value as ChannelConnection['executionMode'] },
                  t('channels:channelsPage.approvalModeUpdated'),
                )
              }
            >
              <option value="approval-required">
                {t('channels:channelsPage.approvalRequired')}
              </option>
              <option value="workspace-write">{t('channels:channelsPage.workspaceWrite')}</option>
              <option value="full-access">{t('channels:channelsPage.fullAccess')}</option>
            </AppSelect>
          </label>
          <label className={FORM_ROW}>
            <span className={FORM_LABEL}>{t('channels:channelsPage.executionMode')}</span>
            <AppSelect
              value={connection.runMode}
              onChange={(event) =>
                onUpdate(
                  platform,
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
              value={connection.accessMode}
              onChange={(event) =>
                onUpdate(
                  platform,
                  { accessMode: event.target.value === 'all' ? 'all' : 'owner' },
                  t('channels:channelsPage.accessScopeUpdated'),
                )
              }
            >
              <option value="owner" disabled={!connection.ownerConfigured}>
                {t('channels:channelsPage.qrCodeOwnerOnly')}
              </option>
              <option value="all">
                {platform === 'feishu'
                  ? t('channels:channelsPage.allMembersInTheCurrentTenant')
                  : platform === 'weixin'
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
                onChange={(event) => onCwdChange(event.target.value)}
              />
              <Button
                variant="secondary"
                size="sm"
                className="max-[650px]:h-9"
                disabled={saving || cwd === connection.defaultCwd}
                onClick={() =>
                  onUpdate(
                    platform,
                    { defaultCwd: cwd },
                    t('channels:channelsPage.defaultWorkingDirectorySaved'),
                  )
                }
              >
                {t('channels:channelsPage.save')}
              </Button>
            </span>
          </label>
          <AppNotice className={cn('mt-1', NOTICE)}>
            <ShieldCheck size={15} />
            <span>
              <strong>{t('channels:channelsPage.localSecurityBoundary')}</strong>
              <small>
                {t('channels:channelsPage.ownerOnlyAccessRespectsToolPermissionsAndExecutionMode')}
              </small>
            </span>
          </AppNotice>
          <div className="mt-3 flex flex-wrap items-center justify-end gap-2 max-[650px]:[&>button]:h-11">
            <Button
              variant="ghost"
              size="sm"
              className="text-danger hover:bg-danger-soft hover:text-danger"
              onClick={() => onRemove(platform)}
            >
              <Unplug />
              {t('channels:channelsPage.disconnect')}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => onReconnect(platform)}
              disabled={saving}
            >
              <RefreshCw />
              {t('channels:channelsPage.reconnect')}
            </Button>
          </div>
        </section>
      ) : (
        <p className="text-xs leading-relaxed text-content-muted">
          {t('channels:channelsPage.notConnectedHint')}
        </p>
      )}
    </div>
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
