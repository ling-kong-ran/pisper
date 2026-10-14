// 快速配置向导：通过 Base URL、API 协议和模型列表完成连接配置。
// 对话与视觉共用流程，但模型类型严格隔离，避免视觉模型进入默认对话配置。
import { useRef, useState } from 'react'
import { AlertTriangle, ArrowLeft, ArrowRight, Check, RefreshCw, Server, X } from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { cn } from '@/lib/utils'
import { ApiKeyInput } from './ApiKeyInput'
import { ManualModelIds } from './ManualModelIds'
import { PROVIDER_APIS } from './provider-constants'
import { createProviderConnectionId } from './provider-connection-id'
import { SettingsBadge } from './settings-primitives'
import type {
  ConfigData,
  ModelDiscoveryResult,
  ProviderConfig,
  ProviderModel,
  ProviderType,
} from './config-types'

import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { FieldLabel } from '@/components/ui/field'
import { AppCardHeader, AppError, AppNotice } from '@/components/ui/app-primitives'

type QuickSetupWizardProps = {
  config: ConfigData
  providerType?: ProviderType
  // 从连接管理进入时复用已有连接的端点、协议和凭据。
  initialProviderId?: string
  onClose: () => void
  onConfigChanged?: (data: ConfigData) => void
  onCompleted: (data: ConfigData) => void
}

function connectionIdentity(baseUrl: string) {
  try {
    const host = new URL(baseUrl).hostname.replace(/^www\./i, '')
    return { name: host || 'Custom Provider' }
  } catch {
    return { name: 'Custom Provider' }
  }
}

export function QuickSetupWizard({
  config,
  providerType = 'chat',
  initialProviderId,
  onClose,
  onConfigChanged,
  onCompleted,
}: QuickSetupWizardProps) {
  const { t } = useI18n()
  const initialProvider = initialProviderId
    ? config.providers.find((item) => item.id === initialProviderId) || null
    : null
  const [step, setStep] = useState(1)
  const [provider, setProvider] = useState<ProviderConfig | null>(initialProvider)
  const [connectionId] = useState(createProviderConnectionId)
  // 创建连接与追加模型分别提交；即使后续追加失败，也必须沿用已落盘的真实 ID。
  const [createdProviderId, setCreatedProviderId] = useState('')
  const returnFocusRef = useRef<HTMLElement | null>(null)
  const baseUrlInputRef = useRef<HTMLInputElement | null>(null)
  const [baseUrl, setBaseUrl] = useState(initialProvider?.baseUrl || '')
  const [api, setApi] = useState(initialProvider?.api || 'openai-responses')
  const [apiKeyDraft, setApiKeyDraft] = useState('')
  const apiKey = apiKeyDraft.trim() || undefined
  const [organization, setOrganization] = useState(initialProvider?.organization || '')
  const [connectionName, setConnectionName] = useState(initialProvider?.name || '')
  const [models, setModels] = useState<ProviderModel[]>(initialProvider?.models || [])
  const [modelId, setModelId] = useState(initialProvider?.defaultModel || '')
  // 手动追加的额外模型 ID 列表：点 + 或回车逐个追加，保存时随主模型一并写入连接。
  const [manualIds, setManualIds] = useState<string[]>([])
  const [modelKind, setModelKind] = useState<ProviderModel['kind']>(
    initialProvider?.models.find((item) => item.id === initialProvider.defaultModel)?.kind ||
      (providerType === 'visual' ? 'image' : 'chat'),
  )
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [discoverWarning, setDiscoverWarning] = useState('')

  const identity = connectionIdentity(baseUrl)
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
    if (!connectionName.trim()) {
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
    if (busy) return
    const existing = provider
    if (!apiKey && !existing?.configured && !createdProviderId) {
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
            providerId: existing?.id || createdProviderId,
            providerType,
            api,
            baseUrl,
            organization,
            apiKey,
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
    if (busy) return
    // 主模型取列表选中项/手动输入；都为空时退回第一个追加项。
    const model = modelId.trim() || manualIds[0] || ''
    if (!model) {
      setError(t('config:configPage.selectModelToFinish'))
      return
    }
    if (!apiKey && !provider?.configured && !createdProviderId) {
      setError(t('config:configPage.enterTheAPIKeyForThisConnection'))
      return
    }
    // 已在连接里的模型跳过（服务端也会跳过已存在项，这里提前过滤避免整批被判为重复）。
    const existingIds = new Set((provider?.models || []).map((item) => item.id))
    const extraIds = manualIds.filter((id) => id !== model && !existingIds.has(id))
    setBusy(true)
    setError('')
    try {
      const existingProviderId = provider?.id || createdProviderId
      let data = existingProviderId
        ? await apiJson<ConfigData>('/api/config', {
            method: 'PUT',
            body: JSON.stringify({
              provider: existingProviderId,
              providerType,
              api,
              baseUrl,
              organization,
              model,
              modelKind: providerType === 'visual' ? modelKind : 'chat',
              apiKey,
              setAsDefault: false,
              enabled: true,
            }),
          })
        : await apiJson<ConfigData>('/api/providers', {
            method: 'POST',
            body: JSON.stringify({
              id: connectionId,
              name: connectionName.trim() || identity.name,
              providerType,
              api,
              baseUrl,
              organization,
              apiKey,
              model,
              modelKind: providerType === 'visual' ? modelKind : 'chat',
              enabled: true,
            }),
          })
      const targetProviderId = existingProviderId || data.createdProviderId || connectionId
      const committedCreatedId = initialProvider ? '' : targetProviderId
      if (committedCreatedId) setCreatedProviderId(committedCreatedId)
      const savedProvider = data.providers.find((item) => item.id === targetProviderId)
      if (savedProvider) setProvider(savedProvider)
      // 先同步已提交的连接，让追加失败后关闭向导仍能看见并继续编辑它。
      onConfigChanged?.(
        committedCreatedId ? { ...data, createdProviderId: committedCreatedId } : data,
      )
      // 主模型保存成功后再批量添加其余手动输入的模型；失败时保留向导以便修正重试
      //（重试安全：服务端会跳过已存在的模型）。
      if (extraIds.length) {
        const kind = providerType === 'visual' ? modelKind : 'chat'
        try {
          data = await apiJson<ConfigData>(
            `/api/providers/${encodeURIComponent(targetProviderId)}/models/batch`,
            {
              method: 'POST',
              body: JSON.stringify({ models: extraIds.map((id) => ({ id, kind })) }),
            },
          )
        } catch (caught) {
          setError(caught instanceof Error ? caught.message : String(caught))
          return
        }
      }
      onCompleted(committedCreatedId ? { ...data, createdProviderId: committedCreatedId } : data)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose()
      }}
    >
      <DialogContent
        showCloseButton={false}
        className="modal z-[80] block max-h-[calc(100dvh_-_40px)] w-[calc(100%_-_40px)] max-w-lg overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] sm:max-w-lg max-[650px]:max-h-[calc(100dvh_-_16px)] max-[650px]:w-[calc(100%_-_16px)]"
        overlayClassName="z-[70] bg-[var(--modal-overlay)] [backdrop-filter:blur(3px)]"
        aria-busy={busy}
        onEscapeKeyDown={(event) => {
          if (busy) event.preventDefault()
        }}
        onInteractOutside={(event) => {
          if (busy) event.preventDefault()
        }}
        onOpenAutoFocus={(event) => {
          event.preventDefault()
          returnFocusRef.current =
            document.activeElement instanceof HTMLElement ? document.activeElement : null
          baseUrlInputRef.current?.focus()
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault()
          returnFocusRef.current?.focus()
        }}
      >
        <AppCardHeader>
          <div>
            <DialogTitle asChild>
              <h2>
                {providerType === 'visual'
                  ? t('config:configPage.visualQuickSetupTitle')
                  : t('config:configPage.quickSetupTitle')}
              </h2>
            </DialogTitle>
            <DialogDescription asChild>
              <p>
                {t('config:configPage.stepIndicator', { current: step, total: 3 })} · {stepLabel}
              </p>
            </DialogDescription>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={t('config:configPage.closeDialog')}
            disabled={busy}
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
                ref={baseUrlInputRef}
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
            <ApiKeyInput
              value={apiKeyDraft}
              onChange={setApiKeyDraft}
              configured={provider?.configured}
            />
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
            {/* 手动输入模型 ID：站点不提供 /models 接口或列表缺模型时的兜底入口，
                与上面获取到的列表双向联动（点列表填入，可直接改）。 */}
            <div className="[margin-top:14px] grid gap-[9px]">
              <FieldLabel variant="control">
                {t('config:configPage.manualModelId')}
                <input
                  value={modelId}
                  onChange={(event) => setModelId(event.target.value)}
                  placeholder={providerType === 'visual' ? 'gpt-image-1' : 'gpt-5.4'}
                />
              </FieldLabel>
              <p className="[margin:0] text-[11px] text-[var(--text-tertiary)]">
                {t('config:configPage.manualModelIdHint')}
              </p>
              {providerType === 'visual' && (
                <FieldLabel variant="control">
                  {t('config:configPage.modelType')}
                  <AppSelect
                    value={modelKind}
                    onChange={(event) => setModelKind(event.target.value as ProviderModel['kind'])}
                  >
                    <option value="image">
                      {t('config:configPage.imageGenerationAndEditing')}
                    </option>
                    <option value="video">{t('config:configPage.videoGeneration')}</option>
                  </AppSelect>
                </FieldLabel>
              )}
              <ManualModelIds
                ids={manualIds}
                onChange={setManualIds}
                primaryId={modelId}
                placeholder={providerType === 'visual' ? 'gpt-image-1' : 'gpt-5.5'}
              />
            </div>
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
              disabled={busy || (!modelId.trim() && manualIds.length === 0)}
              onClick={() => void save()}
            >
              {busy ? <RefreshCw className="animate-spin" size={14} /> : <Check size={14} />}
              {busy ? t('config:configPage.saving') : t('config:configPage.saveChanges')}
            </Button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
