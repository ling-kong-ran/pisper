// 快速配置向导：通过 Base URL、API 协议和模型列表完成连接配置。
// 对话与视觉共用流程，但模型类型严格隔离，避免视觉模型进入默认对话配置。
import { useEffect, useRef, useState } from 'react'
import { AlertTriangle, ArrowLeft, ArrowRight, Check, RefreshCw, Server, X } from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { cn } from '@/lib/utils'
import { PROVIDER_APIS } from './provider-constants'
import { SettingsBadge } from './settings-primitives'
import type {
  ConfigData,
  ModelDiscoveryResult,
  ProviderConfig,
  ProviderModel,
  ProviderType,
} from './config-types'

import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'
import { AppCardHeader, AppError, AppNotice } from '@/components/ui/app-primitives'

type QuickSetupWizardProps = {
  config: ConfigData
  providerType?: ProviderType
  // 从连接管理进入时复用已有连接的端点、协议和凭据。
  initialProviderId?: string
  onClose: () => void
  onCompleted: (data: ConfigData) => void
}

function connectionIdentity(baseUrl: string, providerType: ProviderType) {
  try {
    const host = new URL(baseUrl).hostname.replace(/^www\./i, '')
    const suffix = providerType === 'visual' ? 'visual' : 'chat'
    const id = `custom-${host.replace(/[^a-z0-9]+/gi, '-')}-${suffix}`
      .replace(/^-+|-+$/g, '')
      .slice(0, 60)
    return { id: id || 'custom-provider', name: host || 'Custom Provider' }
  } catch {
    return { id: 'custom-provider', name: 'Custom Provider' }
  }
}

function sameBaseUrl(left: string | undefined, right: string) {
  return (
    String(left || '')
      .replace(/\/+$/, '')
      .toLowerCase() === right.replace(/\/+$/, '').toLowerCase()
  )
}

export function QuickSetupWizard({
  config,
  providerType = 'chat',
  initialProviderId,
  onClose,
  onCompleted,
}: QuickSetupWizardProps) {
  const { t } = useI18n()
  const initialProvider = initialProviderId
    ? config.providers.find((item) => item.id === initialProviderId) || null
    : null
  const [step, setStep] = useState(1)
  const [provider, setProvider] = useState<ProviderConfig | null>(initialProvider)
  const [baseUrl, setBaseUrl] = useState(initialProvider?.baseUrl || '')
  const [api, setApi] = useState(initialProvider?.api || 'openai-responses')
  const [apiKey, setApiKey] = useState('')
  const [organization, setOrganization] = useState(initialProvider?.organization || '')
  const [connectionName, setConnectionName] = useState(initialProvider?.name || '')
  const [models, setModels] = useState<ProviderModel[]>([])
  const [modelId, setModelId] = useState('')
  const [modelKind, setModelKind] = useState<ProviderModel['kind']>(
    providerType === 'visual' ? 'image' : 'chat',
  )
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [discoverWarning, setDiscoverWarning] = useState('')
  const apiKeyInputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [onClose])

  const readApiKey = () => apiKeyInputRef.current?.value ?? apiKey
  const identity = connectionIdentity(baseUrl, providerType)
  const purposeLabel =
    providerType === 'visual'
      ? t('config:configPage.visualProvider')
      : t('config:configPage.chatProvider')
  const stepLabel =
    step === 1
      ? t('config:configPage.quickSetupStepBaseUrl')
      : step === 2
        ? t('config:configPage.quickSetupStepProtocol')
        : t('config:configPage.quickSetupStepModel')
  const visibleModels = models.filter((model) =>
    providerType === 'visual'
      ? model.kind === 'image' || model.kind === 'video'
      : model.kind === 'chat',
  )

  const nextFromBaseUrl = () => {
    const value = baseUrl.trim()
    try {
      const parsed = new URL(value)
      if (!['http:', 'https:'].includes(parsed.protocol)) throw new Error()
    } catch {
      setError(t('config:configPage.providerBaseURLMustBeHTTPOrHTTPS'))
      return
    }
    setError('')
    const existing = config.providers.find(
      (item) => item.type === providerType && sameBaseUrl(item.baseUrl, value) && item.api === api,
    )
    setProvider(existing || null)
    if (existing) {
      setConnectionName(existing.name)
      setOrganization(existing.organization || '')
    } else if (!connectionName.trim()) {
      setConnectionName(identity.name)
    }
    setStep(2)
  }

  const nextFromProtocol = () => {
    if (!api) {
      setError(t('config:configPage.selectAPIProtocol'))
      return
    }
    setError('')
    setStep(3)
  }

  // 第三步才访问 Provider：临时参数只用于发现模型，成功选择后才写入配置文件。
  const fetchModels = async () => {
    const liveKey = readApiKey().trim()
    if (liveKey !== apiKey) setApiKey(liveKey)
    const existing = config.providers.find(
      (item) =>
        item.type === providerType && sameBaseUrl(item.baseUrl, baseUrl) && item.api === api,
    )
    setProvider(existing || null)
    if (!liveKey && !existing?.configured) {
      setError(t('config:configPage.enterTheAPIKeyForThisConnection'))
      return
    }
    setBusy(true)
    setError('')
    setDiscoverWarning('')
    try {
      const result = await apiJson<ModelDiscoveryResult>(
        '/api/providers/models/discover-connection',
        {
          method: 'POST',
          body: JSON.stringify({
            providerId: existing?.id || '',
            providerType,
            api,
            baseUrl,
            organization,
            apiKey: liveKey,
          }),
        },
      )
      const discovered = (result.models || []).filter((model) =>
        providerType === 'visual'
          ? model.kind === 'image' || model.kind === 'video'
          : model.kind === 'chat',
      )
      if (!discovered.length) {
        setError(
          providerType === 'visual'
            ? t('config:configPage.fetchOrAddAVisualModel')
            : t('config:configPage.fetchOrAddAChatModel'),
        )
        return
      }
      setModels(discovered)
      const selectedModel = discovered.find((item) => item.id === modelId) || discovered[0]
      setModelId(selectedModel.id)
      setModelKind(providerType === 'visual' ? selectedModel.kind : 'chat')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      const existingModels =
        existing?.models.filter((model) =>
          providerType === 'visual'
            ? model.kind === 'image' || model.kind === 'video'
            : model.kind === 'chat',
        ) || []
      const authFailure = /\(401\)|\(403\)|鉴权|认证|密钥|API Key/i.test(message)
      if (!authFailure && existingModels.length) {
        setModels(existingModels)
        const selectedModel =
          existingModels.find((item) => item.id === modelId) || existingModels[0]
        setModelId(selectedModel.id)
        setModelKind(providerType === 'visual' ? selectedModel.kind : 'chat')
        setDiscoverWarning(message)
      } else {
        setError(message)
      }
    } finally {
      setBusy(false)
    }
  }

  const save = async () => {
    const model = modelId.trim()
    if (!model) {
      setError(t('config:configPage.selectModelToFinish'))
      return
    }
    setBusy(true)
    setError('')
    try {
      const data = provider
        ? await apiJson<ConfigData>('/api/config', {
            method: 'PUT',
            body: JSON.stringify({
              provider: provider.id,
              providerType,
              api,
              baseUrl,
              organization,
              model,
              modelKind: providerType === 'visual' ? modelKind : 'chat',
              apiKey: readApiKey(),
              thinkingLevel: config.thinkingLevel,
              toolMode: config.toolMode,
              setAsDefault: true,
              enabled: true,
            }),
          })
        : await apiJson<ConfigData>('/api/providers', {
            method: 'POST',
            body: JSON.stringify({
              id: identity.id,
              name: connectionName.trim() || identity.name,
              providerType,
              api,
              baseUrl,
              organization,
              apiKey: readApiKey(),
              model,
              modelKind: providerType === 'visual' ? modelKind : 'chat',
              enabled: true,
            }),
          })
      onCompleted(data)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      className="modal-backdrop max-[650px]:p-[8px] fixed z-[70] inset-0 grid place-items-center overflow-y-auto bg-[var(--modal-overlay)] [backdrop-filter:blur(3px)] [padding:20px] [overscroll-behavior:contain] [animation:fade-in_var(--d1)_var(--ease-out)]"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <div className="modal max-h-[calc(100dvh_-_40px)] w-[min(520px,100%)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)]">
        <AppCardHeader>
          <div>
            <h2>
              {providerType === 'visual'
                ? t('config:configPage.visualQuickSetupTitle')
                : t('config:configPage.quickSetupTitle')}
            </h2>
            <p>
              {t('config:configPage.stepIndicator', { current: step, total: 3 })} · {stepLabel}
            </p>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={t('config:configPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </Button>
        </AppCardHeader>

        <div className="flex items-center gap-[8px] [margin-top:12px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[9px_10px]">
          <span className="grid w-[30px] h-[30px] flex-none place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)]">
            <Server size={16} />
          </span>
          <span className="flex min-w-0 flex-1 flex-col gap-[2px]">
            <strong className="overflow-hidden text-ellipsis whitespace-nowrap text-[13px]">
              {connectionName || identity.name}
            </strong>
            <small className="text-[11px] text-[var(--text-muted)]">{purposeLabel}</small>
          </span>
          <SettingsBadge tone="gray">{purposeLabel}</SettingsBadge>
        </div>

        {step === 1 && (
          <div className="[margin-top:14px]">
            <FieldLabel variant="control">
              Base URL
              <input
                autoFocus
                value={baseUrl}
                onChange={(event) => setBaseUrl(event.target.value)}
                placeholder="https://api.example.com/v1"
                inputMode="url"
              />
            </FieldLabel>
            <p className="[margin:8px_0_0] text-[12px] leading-[1.5] text-[var(--text-muted)]">
              {t('config:configPage.quickSetupBaseUrlHint')}
            </p>
          </div>
        )}

        {step === 2 && (
          <div className="[margin-top:14px]">
            <FieldLabel variant="control">
              {t('config:configPage.apiProtocol')}
              <AppSelect value={api} onChange={(event) => setApi(event.target.value)}>
                {PROVIDER_APIS.map(([value, label]) => (
                  <option value={value} key={value}>
                    {label}
                  </option>
                ))}
              </AppSelect>
            </FieldLabel>
            <FieldLabel variant="control">
              Organization
              <input
                value={organization}
                onChange={(event) => setOrganization(event.target.value)}
                placeholder={t('config:configPage.optionalUsedOnlyForOpenAIOrganization')}
              />
            </FieldLabel>
          </div>
        )}

        {step === 3 && (
          <div className="[margin-top:14px]">
            {!provider && (
              <FieldLabel variant="control">
                {t('config:configPage.displayName')}
                <input
                  value={connectionName}
                  onChange={(event) => setConnectionName(event.target.value)}
                  placeholder={identity.name}
                />
              </FieldLabel>
            )}
            <FieldLabel variant="control">
              API Key
              <input
                ref={apiKeyInputRef}
                type="password"
                autoComplete="new-password"
                autoFocus
                value={apiKey}
                onChange={(event) => setApiKey(event.target.value)}
                onInput={(event) => setApiKey(event.currentTarget.value)}
                placeholder={
                  provider?.configured
                    ? t('config:configPage.keepExistingKeyBlank')
                    : t('config:configPage.enterTheAPIKeyForThisConnection')
                }
              />
            </FieldLabel>
            <Button
              type="button"
              size="lg"
              className="[margin-top:10px] w-full"
              disabled={busy}
              onClick={() => void fetchModels()}
            >
              {busy ? <RefreshCw className="animate-spin" size={14} /> : <RefreshCw size={14} />}
              {busy ? t('config:configPage.fetchingModels') : t('config:configPage.fetchModels')}
            </Button>
            {discoverWarning && (
              <AppNotice className="[margin-top:10px]">
                <AlertTriangle size={15} />
                <small>
                  {t('config:configPage.discoverFailedUsingExisting', { message: discoverWarning })}
                </small>
              </AppNotice>
            )}
            {visibleModels.length > 0 && (
              <>
                <p className="[margin:14px_0_8px] text-[12px] leading-[1.5] text-[var(--text-muted)]">
                  {providerType === 'visual'
                    ? t('config:configPage.selectVisualModelToFinish')
                    : t('config:configPage.selectModelToFinish')}
                </p>
                <div className="flex max-h-[260px] flex-col gap-[6px] overflow-y-auto">
                  {visibleModels.map((model) => (
                    <button
                      key={`${model.id}-${model.kind}`}
                      type="button"
                      className={cn(
                        'flex min-w-0 cursor-pointer items-center gap-[8px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[8px_10px] text-left hover:border-[var(--accent-border)]',
                        modelId === model.id &&
                          '[border-color:var(--accent-border)] bg-[var(--accent-soft)]',
                      )}
                      onClick={() => {
                        setModelId(model.id)
                        setModelKind(providerType === 'visual' ? model.kind : 'chat')
                      }}
                    >
                      <span
                        className={cn(
                          'grid w-[16px] h-[16px] flex-none place-items-center rounded-full [border:1px_solid_var(--stroke)] text-transparent',
                          modelId === model.id &&
                            '[border-color:var(--brand-blue)] bg-[var(--brand-blue)] text-white',
                        )}
                      >
                        <Check size={11} />
                      </span>
                      <span className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[13px]">
                        {model.name}
                      </span>
                      {providerType === 'visual' && (
                        <SettingsBadge tone="gray">
                          {model.kind === 'video'
                            ? t('config:configPage.visualVideoModel')
                            : t('config:configPage.visualImageModel')}
                        </SettingsBadge>
                      )}
                    </button>
                  ))}
                </div>
              </>
            )}
          </div>
        )}

        {error && (
          <AppError>
            <AlertTriangle size={13} />
            {error}
          </AppError>
        )}

        <div className="flex justify-between gap-[8px] [margin-top:18px]">
          <Button
            type="button"
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            disabled={busy || step === 1}
            onClick={() => {
              setError('')
              setStep(step - 1)
            }}
          >
            <ArrowLeft size={14} />
            {t('config:configPage.previousStep')}
          </Button>
          {step === 1 ? (
            <Button
              type="button"
              size="lg"
              disabled={busy || !baseUrl.trim()}
              onClick={nextFromBaseUrl}
            >
              <ArrowRight size={14} />
              {t('config:configPage.nextStep')}
            </Button>
          ) : step === 2 ? (
            <Button type="button" size="lg" disabled={busy} onClick={nextFromProtocol}>
              <ArrowRight size={14} />
              {t('config:configPage.nextStep')}
            </Button>
          ) : (
            <Button
              type="button"
              size="lg"
              disabled={busy || !modelId.trim() || visibleModels.length === 0}
              onClick={() => void save()}
            >
              {busy ? <RefreshCw className="animate-spin" size={14} /> : <Check size={14} />}
              {busy ? t('config:configPage.saving') : t('config:configPage.saveAndSetDefault')}
            </Button>
          )}
        </div>
      </div>
    </div>
  )
}
