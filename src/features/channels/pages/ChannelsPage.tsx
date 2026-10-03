// 通道页：管理飞书/微信/QQ/Telegram 的双向连接和会话路由。
// 仅负责状态编排和数据请求，引导流程委托给 OnboardingModal。
import { useCallback, useEffect, useState } from 'react'
import {
  AlertTriangle,
  Bot,
  MessageCircle,
  MessageSquare,
  Plus,
  RefreshCw,
  Send,
  Settings,
} from 'lucide-react'
import {
  AppCard as Panel,
  AppSectionTitle as SectionTitle,
  StatusBadge as Badge,
  AppError,
  AppEmptyState,
} from '@/components/ui/app-primitives'
import { useI18n } from '@/app/i18n/use-i18n'
import { StarOrbit } from '@/components/common/StarOrbit'
import { apiJson } from '@/lib/http/api'
import { relativeTime } from '@/lib/format/format'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'
import { OnboardingModal } from '@/features/channels/components/OnboardingModal'
import {
  PROVIDER_ENTRIES,
  channelStatusLabel,
  channelStatusTone,
  connectActionLabel,
  isManualPlatform,
  providerCapability,
  providerName,
  providerTitle,
  providerTransport,
} from '@/features/channels/model/channel-providers'
import type {
  ChannelConnection,
  ChannelPlatform,
  ChannelsData,
  ManualCredentials,
  ManualOnboardingDescriptor,
  OnboardingJob,
} from '@/features/channels/model/channel-types'
import type { Notify } from '@/app/routes/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

type ChannelsPageProps = {
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}

function errorMessage(caught: unknown) {
  return caught instanceof Error ? caught.message : String(caught)
}

export function ChannelsPage({ notify, registerPrimaryAction, requestConfirm }: ChannelsPageProps) {
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

  // 开始渠道对接（onboarding）：已存在连接时先确认覆盖。
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
        { method: 'POST', body: '{}' },
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

  // 更新渠道连接（启用/访问模式/工作目录/回复模型）。
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
      notify(success)
    } catch (caught) {
      setError(errorMessage(caught))
    } finally {
      setSaving(false)
    }
  }

  // 提交手动凭据（Telegram 等）。
  const submitManualCredentials = async (credentials: ManualCredentials) => {
    if (!onboarding) return
    try {
      await apiJson(
        `/api/channels/${onboarding.platform}/onboarding/manual`,
        { method: 'POST', body: JSON.stringify(credentials) },
      )
      notify(t('channels:channelsPage.connecting'))
    } catch (caught) {
      setOnboarding({ ...onboarding, status: 'failed', error: errorMessage(caught) })
    }
  }

  if (loading)
    return (
      <AppEmptyState>
        <RefreshCw className="animate-spin" size={23} />
        <h2>{t('channels:channelsPage.loadingChannels')}</h2>
      </AppEmptyState>
    )

  const connection = data.connections?.[selectedPlatform] || null
  const selectedConnection = data.connections?.[selectedPlatform]

  return (
    <>
      {error && (
        <AppError>
          <AlertTriangle size={13} />
          {error}
        </AppError>
      )}
      <div className="channels-grid grid grid-cols-[repeat(4,minmax(0,1fr))] gap-[12px] max-[1150px]:grid-cols-[repeat(2,minmax(0,1fr))] max-[650px]:grid-cols-[1fr]">
        {PROVIDER_ENTRIES.map(([platform, provider]) => {
          const status = data.connections?.[platform]?.status || ('idle' as const)
          const tone = channelStatusTone(status)
          const Icon = provider.Icon
          return (
            <Panel
              className={`provider-card channel-platform-card cursor-pointer [transition:border-color_var(--d1)_var(--ease-out),box-shadow_var(--d1)_var(--ease-out)] hover:[transform:translateY(-2px)] hover:shadow-[var(--sh-2)] hover:border-[var(--star-border)] [&.selected]:border-[var(--accent-border)] [&.selected]:shadow-[0_10px_30px_-24px_var(--blue),0_0_0_2px_var(--selection-ring)] ${selectedPlatform === platform ? 'selected' : ''}`}
              key={platform}
              onClick={() => {
                setSelectedPlatform(platform)
                setCwd(data.connections?.[platform]?.defaultCwd || '')
              }}
            >
              <div className="provider-title [&_h2]:text-[14px] [&_p]:mt-[3px] [&_p]:text-[var(--text-secondary)] [&_p]:text-[12px] grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[10px] [margin-bottom:10px]">
                <span
                  className={`provider-icon [&_svg]:w-[19px] [&.green]:bg-[var(--success-soft)] [&.green]:text-[var(--success)] [&.blue]:bg-[var(--brand-blue-soft)] [&.blue]:text-[var(--brand-blue-strong)] grid w-[38px] h-[38px] place-items-center rounded-[var(--r-md)] ${provider.tone}`}
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
                {connectActionLabel(platform, Boolean(connection), t)}
              </Button>
            </Panel>
          )
        })}
      </div>
      <div
        className={`two-one-grid max-[900px]:grid-cols-[1fr] grid gap-[12px] ${
          selectedConnection ? 'grid-cols-[minmax(0,2fr)_minmax(260px,1fr)]' : 'grid-cols-[1fr]'
        }`}
      >
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
                  ) : scope.platform === 'telegram' ? (
                    <Send size={14} />
                  ) : scope.platform === 'weixin' ? (
                    <MessageCircle size={14} />
                  ) : (
                    <Bot size={14} />
                  )}
                </span>
                <div>
                  <strong>{scope.title}</strong>
                  <small>
                    {scope.lastMessage
                      ? scope.lastMessage.slice(0, 48)
                      : t('channels:channelsPage.noMessages')}
                  </small>
                </div>
                <em>
                  {scope.updatedAt ? relativeTime(scope.updatedAt, language) : ''}
                </em>
              </div>
            ))
          ) : (
            <div className="channel-route-empty grid min-h-[130px] place-content-center justify-items-center text-[var(--text-muted)] text-center">
              <StarOrbit size={30} />
              <strong>{t('channels:channelsPage.noRoutedChats')}</strong>
            </div>
          )}
        </Panel>
        {selectedConnection && (
          <Panel>
            <SectionTitle title={t('channels:channelsPage.connectionSettings')} />
            <FieldLabel variant="control">
              {t('channels:channelsPage.workingDirectory')}
              <input value={cwd} onChange={(event) => setCwd(event.target.value)} />
            </FieldLabel>
            <Button
              size="lg"
              disabled={saving || cwd === selectedConnection.defaultCwd}
              onClick={() =>
                update(
                  selectedPlatform,
                  { defaultCwd: cwd },
                  t('channels:channelsPage.connectionSaved'),
                )
              }
            >
              <Settings size={14} />
              {saving ? t('channels:channelsPage.saving') : t('channels:channelsPage.save')}
            </Button>
          </Panel>
        )}
      </div>
      {onboarding && (
        <OnboardingModal
          job={onboarding}
          onClose={closeOnboarding}
          onRetry={() => beginOnboarding(onboarding.platform)}
          onSubmitCredentials={submitManualCredentials}
          notify={notify}
        />
      )}
    </>
  )
}
