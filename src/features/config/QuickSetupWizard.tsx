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
import { OrbitSpinner } from '@/components/ui/spinner'
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
        className="modal z-[80] block max-h-[calc(100dvh_-_40px)] w-[calc(100%_-_40px)] max-w-lg overflow-y-auto overscroll-contain rounded-lg border border-stroke bg-solid p-[18px] shadow-float sm:max-w-lg max-[650px]:max-h-[calc(100dvh_-_16px)] max-[650px]:w-[calc(100%_-_16px)]"
        overlayClassName="z-[70] bg-[var(--modal-overlay)]"
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
            className="pointer-coarse:size-11"
            aria-label={t('config:configPage.closeDialog')}
            disabled={busy}
            onClick={onClose}
          >
            <X size={17} />
          </Button>
        </AppCardHeader>

        {/* 三段步进刻度：已完成与当前步为强调色，读数与标题里的「第 N 步 / 共 3 步」一致。 */}
        <div aria-hidden="true" className="mt-3 grid grid-cols-3 gap-1">
          {[1, 2, 3].map((index) => (
            <span
              key={index}
              className={cn('h-1 rounded-full', index <= step ? 'bg-star' : 'bg-surface-active')}
            />
          ))}
        </div>
        <div className="mt-3 flex items-center gap-2.5 rounded-md border border-stroke-soft bg-surface-subtle px-2.5 py-2">
          <span className="grid size-7.5 flex-none place-items-center rounded-md border border-key-border bg-key text-content-muted shadow-key">
            <Server size={15} aria-hidden="true" />
          </span>
          <span className="flex min-w-0 flex-1 flex-col gap-0.5">
            <span className="truncate text-sm font-medium text-content">
              {connectionName || identity.name}
            </span>
            <small className="text-xs text-content-muted">{purposeLabel}</small>
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
            <p className="mt-2 text-xs leading-[1.5] text-content-muted">
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
              variant="secondary"
              className="mt-2.5 w-full max-[650px]:h-11"
              disabled={busy}
              onClick={() => void fetchModels()}
            >
              {busy ? <OrbitSpinner className="text-current" /> : <RefreshCw size={14} />}
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
                <p className="mt-3.5 mb-2 text-xs leading-[1.5] text-content-muted">
                  {providerType === 'visual'
                    ? t('config:configPage.selectVisualModelToFinish')
                    : t('config:configPage.selectModelToFinish')}
                </p>
                <div className="flex max-h-[260px] flex-col overflow-y-auto rounded-md border border-stroke">
                  {visibleModels.map((model) => (
                    <button
                      key={`${model.id}-${model.kind}`}
                      type="button"
                      className={cn(
                        'relative flex min-h-10 min-w-0 cursor-pointer items-center gap-2 border-b border-stroke-soft px-2.5 py-2 text-left last:border-b-0 hover:bg-surface-subtle pointer-coarse:min-h-11',
                        modelId === model.id &&
                          'bg-star-soft before:absolute before:inset-y-2 before:left-0 before:w-[3px] before:rounded-r-[3px] before:bg-star hover:bg-star-soft',
                      )}
                      onClick={() => {
                        setModelId(model.id)
                        setModelKind(providerType === 'visual' ? model.kind : 'chat')
                      }}
                    >
                      <span
                        className={cn(
                          'grid size-4 flex-none place-items-center rounded-full border border-control text-transparent',
                          modelId === model.id && 'border-star bg-star text-on-accent',
                        )}
                      >
                        <Check size={11} />
                      </span>
                      <span className="min-w-0 flex-1 truncate text-sm text-content">
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
            <div className="mt-3.5 grid gap-2">
              <FieldLabel variant="control">
                {t('config:configPage.manualModelId')}
                <input
                  value={modelId}
                  onChange={(event) => setModelId(event.target.value)}
                  placeholder={providerType === 'visual' ? 'gpt-image-1' : 'gpt-5.4'}
                />
              </FieldLabel>
              <p className="text-xs text-content-muted">
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

        <div className="mt-[18px] flex justify-between gap-2">
          <Button
            type="button"
            variant="secondary"
            className="max-[650px]:h-11"
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
              className="max-[650px]:h-11"
              disabled={busy || !baseUrl.trim()}
              onClick={nextFromBaseUrl}
            >
              <ArrowRight size={14} />
              {t('config:configPage.nextStep')}
            </Button>
          ) : step === 2 ? (
            <Button
              type="button"
              className="max-[650px]:h-11"
              disabled={busy}
              onClick={nextFromProtocol}
            >
              <ArrowRight size={14} />
              {t('config:configPage.nextStep')}
            </Button>
          ) : (
            <Button
              type="button"
              className="max-[650px]:h-11"
              disabled={busy || (!modelId.trim() && manualIds.length === 0)}
              onClick={() => void save()}
            >
              {busy ? <OrbitSpinner className="text-current" /> : <Check size={14} />}
              {busy ? t('config:configPage.saving') : t('config:configPage.saveChanges')}
            </Button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
