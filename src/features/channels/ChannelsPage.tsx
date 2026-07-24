import { useCallback, useEffect, useState } from 'react'
import {
  AlertTriangle,
  Bot,
  CheckCircle2,
  ChevronDown,
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
import { Badge, Panel, SectionTitle, Toggle } from '@/components/ui'
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
      <Panel className="empty-state">
        <RefreshCw className="spin" size={23} />
        <h2>{t('channels:channelsPage.loadingChannels')}</h2>
      </Panel>
    )
  const selectedConnection = data.connections?.[selectedPlatform]
  return (
    <div className="channel-page">
      {error && (
        <div className="config-error">
          <AlertTriangle size={13} />
          {error}
        </div>
      )}
      <div className="channel-cards">
        {PROVIDER_ENTRIES.map(([platform, provider]) => {
          const connection = data.connections?.[platform]
          const status = connection?.status || 'idle'
          const tone = channelStatusTone(status)
          const Icon = provider.Icon
          return (
            <Panel
              className={`provider-card channel-platform-card ${selectedPlatform === platform ? 'selected' : ''}`}
              key={platform}
              onClick={() => {
                setSelectedPlatform(platform)
                setCwd(connection?.defaultCwd || '')
              }}
            >
              <div className="provider-title">
                <span className={`provider-icon ${provider.tone}`}>
                  <Icon />
                </span>
                <div>
                  <h2>{providerTitle(platform, t)}</h2>
                  <p>{providerTransport(platform, t)}</p>
                </div>
                <Badge tone={tone}>{channelStatusLabel(status, t)}</Badge>
              </div>
              <label className="field-label">
                {t('channels:channelsPage.twoWayCapability')}
                <span className="channel-summary-field">{providerCapability(platform, t)}</span>
              </label>
              <label className="field-label">
                {t('channels:channelsPage.replyModel')}
                <span className="channel-summary-field">
                  {connection?.replyModel
                    ? `${connection.replyModel.provider}/${connection.replyModel.model}`
                    : t('channels:channelsPage.useApplicationDefaultModel')}
                </span>
              </label>
              <button
                className={`button wide channel-provider-connect ${connection ? 'secondary' : 'primary'}`}
                disabled={starting === platform}
                onClick={(event) => {
                  event.stopPropagation()
                  beginOnboarding(platform)
                }}
              >
                {starting === platform ? (
                  <RefreshCw className="spin" size={14} />
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
              </button>
            </Panel>
          )
        })}
      </div>

      <div className="two-one-grid">
        <Panel>
          <div className="channel-section-head">
            <SectionTitle title={t('channels:channelsPage.channelChats')} />
            <span>{t('channels:channelsPage.countLinked', { count: data.scopes.length })}</span>
          </div>
          {data.scopes.length ? (
            data.scopes.map((scope) => (
              <div className="route-row" key={scope.key}>
                <span className="route-icon">
                  {scope.platform === 'feishu' ? (
                    <MessageSquare size={14} />
                  ) : (
                    <MessageCircle size={14} />
                  )}
                </span>
                <div className="channel-route-copy">
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
                <div className="channel-route-controls">
                  <Badge tone="green">{t('channels:channelsPage.twoWay')}</Badge>
                  <button
                    className="icon-button danger"
                    title={t('channels:channelsPage.resetChat')}
                    onClick={() => resetScope(scope)}
                  >
                    <Trash2 size={13} />
                  </button>
                </div>
              </div>
            ))
          ) : (
            <div className="channel-route-empty">
              <StarOrbit size={38} />
              <strong>{t('channels:channelsPage.waitingForTheFirstReplyFromAfar')}</strong>
              <span>
                {t(
                  'channels:channelsPage.onceConnectedMessagesFromFeishuOrWeixinWillArriveHereInVesper',
                )}
              </span>
            </div>
          )}
        </Panel>
        <Panel className="test-panel">
          <div className="channel-section-head">
            <SectionTitle
              title={t('channels:channelsPage.nameSettings', {
                name: providerName(selectedPlatform, t),
              })}
            />
            <span>{providerTransport(selectedPlatform, t)}</span>
          </div>
          {selectedConnection ? (
            <>
              <div className={`channel-live-status ${selectedConnection.status}`}>
                <span className="channel-status-dot" />
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
              <div className="modal-toggle-row">
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
              <label className="field-label">
                {t('channels:channelsPage.replyModel')}
                <span className="select-wrap">
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
                  <ChevronDown size={13} />
                </span>
              </label>
              <label className="field-label">
                {t('channels:channelsPage.accessScope')}
                <span className="select-wrap">
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
                  <ChevronDown size={13} />
                </span>
              </label>
              <label className="field-label">
                {t('channels:channelsPage.defaultWorkingDirectoryForNewChats')}
                <span className="channel-setting-input">
                  <FolderOpen size={13} />
                  <input value={cwd} onChange={(event) => setCwd(event.target.value)} />
                  <button
                    className="button tiny"
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
                  </button>
                </span>
              </label>
              <div className="permission-note">
                <ShieldCheck size={15} />
                <span>
                  <strong>{t('channels:channelsPage.localSecurityBoundary')}</strong>
                  <small>
                    {t(
                      'channels:channelsPage.onlyThePersonWhoScannedTheCodeIsAllowedByDefaultAgentToolsStillFollowPluginPermissionsAndChatWor',
                    )}
                  </small>
                </span>
              </div>
              <div className="button-row">
                <button
                  className="button secondary"
                  onClick={() => reconnect(selectedPlatform)}
                  disabled={saving}
                >
                  <RefreshCw size={14} />
                  {t('channels:channelsPage.reconnect')}
                </button>
                <button className="button danger" onClick={() => remove(selectedPlatform)}>
                  <Unplug size={14} />
                  {t('channels:channelsPage.disconnect')}
                </button>
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
              <div className="test-summary">
                <CheckCircle2 size={14} />
                {t('channels:channelsPage.trueTwoWayMessagingNoNotificationWebhookRequired')}
              </div>
              <div className="test-summary">
                <CheckCircle2 size={14} />
                {t('channels:channelsPage.eachContactOrChatMapsToAnIndependentAgentSession')}
              </div>
              <button
                className="button primary wide"
                onClick={() => beginOnboarding(selectedPlatform)}
              >
                <Zap size={15} />
                {t('channels:channelsPage.connectNameByQRCode', {
                  name: providerName(selectedPlatform, t),
                })}
              </button>
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
      className="modal-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <section className="modal feishu-onboard-modal">
        <div className="card-head">
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
          <button
            className="icon-button"
            aria-label={t('channels:channelsPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </div>
        <div className={`feishu-qr-stage ${job.status}`}>
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
            <RefreshCw className="spin" size={32} />
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
          <div className="weixin-verify-code">
            <input
              value={code}
              onChange={(event) => setCode(event.target.value.replace(/\D/g, '').slice(0, 8))}
              placeholder={t('channels:channelsPage.enterTheNumberShownOnYourPhone')}
            />
            <button className="button primary" disabled={!code} onClick={submitCode}>
              {t('channels:channelsPage.submitPairingCode')}
            </button>
          </div>
        )}
        {job.qrUrl && !terminal && (
          <a
            className="button secondary wide feishu-open-link"
            href={job.qrUrl}
            target="_blank"
            rel="noreferrer"
          >
            <ExternalLink size={14} />
            {t('channels:channelsPage.cannotScanOpenTheSignInLink')}
          </a>
        )}
        <div className="permission-note">
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
        </div>
        <div className="modal-actions">
          <button className="button secondary" onClick={onClose}>
            {terminal ? t('channels:channelsPage.off') : t('channels:channelsPage.cancel')}
          </button>
          {job.status === 'failed' && (
            <button className="button primary" onClick={onRetry}>
              <RefreshCw size={14} />
              {t('channels:channelsPage.generateANewQRCode')}
            </button>
          )}
        </div>
      </section>
    </div>
  )
}
