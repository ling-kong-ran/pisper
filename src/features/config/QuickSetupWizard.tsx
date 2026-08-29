// 快速配置向导：三步完成对话模型配置——
// 选择服务商（已知 Provider 预填协议/端点）→ 填 API Key → 自动拉取模型并推荐，
// 最后一键「保存并设为默认」。自定义/视觉连接仍走高级弹窗。
// 支持 initialProviderId 预选（连接列表「配置」入口）：直接进入 Key 或选模型步骤；
// 第 2 步的高级折叠区覆盖中转站场景（Base URL/协议/Organization）；
// 第 3 步支持手动输入模型 ID（发现接口不可用时兜底）。
import { useRef, useState } from 'react'
import {
  AlertTriangle,
  ArrowLeft,
  ArrowRight,
  Check,
  CheckCircle2,
  ChevronDown,
  Plus,
  RefreshCw,
  Server,
  X,
} from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { cn } from '@/lib/utils'
import { recommendedChatModel } from './model-recommendation'
import { PROVIDER_APIS, PROVIDER_ICONS } from './provider-constants'
import { SettingsBadge } from './settings-primitives'
import type { ConfigData, ModelDiscoveryResult, ProviderConfig } from './config-types'

import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'

import { FieldLabel } from '@/components/ui/field'

import { AppCardHeader, AppError, AppNotice } from '@/components/ui/app-primitives'

type QuickSetupWizardProps = {
  config: ConfigData
  // 预选 Provider（连接列表「配置」入口）：跳过第 1 步。
  initialProviderId?: string
  // 已有可用模型时直接进入第 3 步选模型（「更改模型」入口）。
  startAtModels?: boolean
  onClose: () => void
  onCompleted: (data: ConfigData) => void
  onCustomProvider: () => void
}

export function QuickSetupWizard({
  config,
  initialProviderId,
  startAtModels,
  onClose,
  onCompleted,
  onCustomProvider,
}: QuickSetupWizardProps) {
  const { t } = useI18n()
  // Codex 走本地登录导入、纯视觉 Provider 不参与对话配置，都不进向导。
  const candidates = config.providers.filter(
    (provider) => provider.type !== 'visual' && provider.id !== 'openai-codex',
  )
  const initial = initialProviderId
    ? candidates.find((item) => item.id === initialProviderId) || null
    : null
  const startWithModels = Boolean(
    initial && startAtModels && initial.configured && initial.models.some((m) => m.kind === 'chat'),
  )
  const [step, setStep] = useState(initial ? (startWithModels ? 3 : 2) : 1)
  const [provider, setProvider] = useState<ProviderConfig | null>(initial)
  const [apiKey, setApiKey] = useState('')
  const [api, setApi] = useState(initial?.api || 'openai-responses')
  const [baseUrl, setBaseUrl] = useState(initial?.baseUrl || '')
  const [organization, setOrganization] = useState(initial?.organization || '')
  const [fetchedProvider, setFetchedProvider] = useState<ProviderConfig | null>(
    startWithModels ? initial : null,
  )
  const [modelId, setModelId] = useState(() =>
    startWithModels ? recommendedChatModel(initial)?.id || '' : '',
  )
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  // 发现接口失败但目录已有模型时的非阻塞提示（第 3 步展示）。
  const [discoverWarning, setDiscoverWarning] = useState('')
  // 中转站/自定义连接依赖端点字段：高级区默认展开。
  const [advancedOpen, setAdvancedOpen] = useState(Boolean(initial?.custom))
  const apiKeyInputRef = useRef<HTMLInputElement>(null)
  // 密码管理器/自动填充可能不触发 React onChange，提交时读实时值兜底。
  const readApiKey = () => apiKeyInputRef.current?.value ?? apiKey

  const pickProvider = (item: ProviderConfig) => {
    setProvider(item)
    setApi(item.api || 'openai-responses')
    setBaseUrl(item.baseUrl || '')
    setOrganization(item.organization || '')
    setAdvancedOpen(Boolean(item.custom))
    setError('')
    setStep(2)
  }

  // 第二步 → 拉取模型：连接参数（含高级区覆盖）传给发现接口，
  // Key 以临时参数传递（不落盘），保存成功后才写入凭据。
  const fetchModels = async () => {
    if (!provider) return
    const liveKey = readApiKey().trim()
    if (liveKey !== apiKey) setApiKey(liveKey)
    if (!liveKey && !provider.configured) {
      setError(t('config:configPage.enterTheAPIKeyForThisConnection'))
      return
    }
    if (provider.custom && !baseUrl.trim()) {
      setError(t('config:configPage.enterProviderBaseURLBeforeSaving'))
      return
    }
    setBusy(true)
    setError('')
    try {
      const result = await apiJson<ModelDiscoveryResult>(
        `/api/providers/${encodeURIComponent(provider.id)}/models/discover`,
        {
          method: 'POST',
          body: JSON.stringify({
            apiKey: liveKey,
            api,
            baseUrl,
            organization,
            providerType: 'chat',
          }),
        },
      )
      const refreshed = result.config?.providers.find((item) => item.id === provider.id)
      // 目录未同步（synchronized=false，如自定义端点）时 config 为 null，用发现结果兜底展示。
      const source =
        refreshed || (result.models?.length ? { ...provider, models: result.models } : null)
      const chatModels = source?.models.filter((item) => item.kind === 'chat') || []
      if (!source || !chatModels.length) {
        setError(t('config:configPage.fetchOrAddAChatModel'))
        return
      }
      setFetchedProvider(source)
      // 预选推荐模型（Provider 默认 > 目录排序第一），用户仍可改选或手动输入。
      const recommended = refreshed ? recommendedChatModel(refreshed) : chatModels[0]
      setModelId(recommended?.id || '')
      setStep(3)
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      // 发现接口不可用（端点无 /models、网络失败等）且目录里已有模型时，
      // 允许直接用已有模型继续，不再卡死在第 2 步；鉴权类错误仍然拦截。
      const authFailure = /\(401\)|\(403\)|鉴权|认证|密钥|API Key/i.test(message)
      const existing = provider.models.filter((item) => item.kind === 'chat')
      if (!authFailure && existing.length) {
        setFetchedProvider(provider)
        setModelId(recommendedChatModel(provider)?.id || '')
        setDiscoverWarning(message)
        setStep(3)
        return
      }
      setError(message)
    } finally {
      setBusy(false)
    }
  }

  const saveAndSetDefault = async () => {
    const model = modelId.trim()
    if (!provider || !model) return
    setBusy(true)
    setError('')
    try {
      // baseUrl/organization/api 回传当前表单值：已知 Provider 默认预填官方端点，
      // 清空则回退官方默认；自定义连接必须非空（第 2 步已校验）。
      const saved = await apiJson<ConfigData>('/api/config', {
        method: 'PUT',
        body: JSON.stringify({
          provider: provider.id,
          providerType: 'chat',
          api,
          baseUrl,
          organization,
          model,
          apiKey: readApiKey(),
          thinkingLevel: config.thinkingLevel,
          toolMode: config.toolMode,
          setAsDefault: true,
          enabled: true,
        }),
      })
      onCompleted(saved)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }

  const chatModels = fetchedProvider?.models.filter((item) => item.kind === 'chat') || []
  const recommendedId = fetchedProvider ? recommendedChatModel(fetchedProvider)?.id : ''
  const StepProviderIcon = (provider && PROVIDER_ICONS[provider.id]) || Server

  return (
    <div
      className="modal-backdrop max-[650px]:p-[8px] fixed z-[70] inset-0 grid place-items-center overflow-y-auto bg-[var(--modal-overlay)] [backdrop-filter:blur(3px)] [padding:20px] [overscroll-behavior:contain] [animation:fade-in_var(--d1)_var(--ease-out)]"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <div className="modal max-h-[calc(100dvh_-_40px)] w-[min(520px,100%)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)]">
        <AppCardHeader>
          <div>
            <h2>{t('config:configPage.quickSetupTitle')}</h2>
            <p>
              {t('config:configPage.stepIndicator', { current: step, total: 3 })} ·{' '}
              {step === 1
                ? t('config:configPage.quickSetupStepProvider')
                : step === 2
                  ? t('config:configPage.quickSetupStepKey')
                  : t('config:configPage.quickSetupStepModel')}
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

        {step === 1 && (
          <>
            <div className="grid grid-cols-2 gap-[8px] max-[520px]:grid-cols-1 [margin-top:12px]">
              {candidates.map((item) => {
                const Icon = PROVIDER_ICONS[item.id] || Server
                return (
                  <button
                    key={item.id}
                    type="button"
                    className="flex min-w-0 cursor-pointer items-center gap-[9px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[10px_11px] text-left hover:border-[var(--accent-border)] hover:bg-[var(--accent-soft)]"
                    onClick={() => pickProvider(item)}
                  >
                    <span className="grid w-[30px] h-[30px] flex-none place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)]">
                      <Icon size={16} />
                    </span>
                    <span className="flex min-w-0 flex-1 flex-col gap-[2px]">
                      <strong className="overflow-hidden text-ellipsis whitespace-nowrap text-[13px]">
                        {item.name}
                      </strong>
                      <small className="text-[11px] text-[var(--text-muted)]">
                        {item.configured
                          ? t('config:configPage.configured')
                          : t('config:configPage.apiKeyRequired')}
                      </small>
                    </span>
                    {item.configured && (
                      <CheckCircle2 size={14} className="flex-none text-[var(--success)]" />
                    )}
                  </button>
                )
              })}
            </div>
            <div className="flex justify-center [margin-top:14px]">
              <Button
                variant="ghost"
                className="text-[var(--text-muted)]"
                onClick={onCustomProvider}
              >
                <Plus size={14} />
                {t('config:configPage.addCustomConnection')}
              </Button>
            </div>
          </>
        )}

        {step === 2 && provider && (
          <>
            <div className="flex items-center gap-[9px] [margin-top:12px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[10px_11px]">
              <span className="grid w-[30px] h-[30px] flex-none place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)]">
                <StepProviderIcon size={16} />
              </span>
              <strong className="text-[13px]">{provider.name}</strong>
              {provider.configured && (
                <SettingsBadge tone="green">{t('config:configPage.configured')}</SettingsBadge>
              )}
            </div>
            <div className="[margin-top:12px]">
              <FieldLabel variant="control">
                API Key
                <input
                  ref={apiKeyInputRef}
                  type="password"
                  autoComplete="new-password"
                  autoFocus
                  value={apiKey}
                  onChange={(event) => setApiKey(event.target.value)}
                  onInput={(event) => {
                    const value = event.currentTarget.value
                    if (apiKey !== value) setApiKey(value)
                  }}
                  placeholder={
                    provider.configured
                      ? t('config:configPage.keepExistingKeyBlank')
                      : t('config:configPage.enterTheAPIKeyForThisConnection')
                  }
                />
              </FieldLabel>
            </div>
            <Collapsible
              open={advancedOpen}
              onOpenChange={setAdvancedOpen}
              className="[margin-top:10px]"
            >
              <CollapsibleTrigger asChild>
                <button
                  type="button"
                  className="group flex w-full cursor-pointer items-center gap-[6px] border-0 bg-transparent p-0 text-left text-[12px] font-[600] text-[var(--text-muted)] hover:text-[var(--text)]"
                >
                  <ChevronDown
                    size={14}
                    className="transition-transform group-data-[state=open]:rotate-180"
                  />
                  <span>{t('config:configPage.advancedSettings')}</span>
                  <span className="font-[400] text-[var(--text-tertiary)]">
                    {t('config:configPage.advancedSettingsHint')}
                  </span>
                </button>
              </CollapsibleTrigger>
              <CollapsibleContent>
                <FieldLabel variant="control">
                  Base URL
                  <input
                    value={baseUrl}
                    onChange={(event) => setBaseUrl(event.target.value)}
                    placeholder={t('config:configPage.defaultEndpointForModelsInThisConnection')}
                  />
                </FieldLabel>
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
              </CollapsibleContent>
            </Collapsible>
          </>
        )}

        {step === 3 && fetchedProvider && (
          <>
            {discoverWarning && (
              <AppNotice className="[margin-top:12px]">
                <AlertTriangle size={15} />
                <span>
                  <small>
                    {t('config:configPage.discoverFailedUsingExisting', {
                      message: discoverWarning,
                    })}
                  </small>
                </span>
              </AppNotice>
            )}
            <p className="[margin:12px_0_8px] text-[12px] leading-[1.5] text-[var(--text-muted)]">
              {t('config:configPage.selectModelToFinish')}
            </p>
            <div className="flex max-h-[260px] flex-col gap-[6px] overflow-y-auto">
              {chatModels.map((model) => (
                <button
                  key={model.id}
                  type="button"
                  className={cn(
                    'flex min-w-0 cursor-pointer items-center gap-[8px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[8px_10px] text-left hover:border-[var(--accent-border)]',
                    modelId === model.id &&
                      '[border-color:var(--accent-border)] bg-[var(--accent-soft)]',
                  )}
                  onClick={() => setModelId(model.id)}
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
                  {model.id === recommendedId && (
                    <SettingsBadge tone="blue">{t('config:configPage.recommended')}</SettingsBadge>
                  )}
                </button>
              ))}
            </div>
            <FieldLabel variant="control">
              {t('config:configPage.manualModelId')}
              <input
                value={modelId}
                onChange={(event) => setModelId(event.target.value)}
                placeholder={t('config:configPage.manualModelIdHint')}
              />
            </FieldLabel>
          </>
        )}

        {error && (
          <AppError>
            <AlertTriangle size={13} />
            {error}
          </AppError>
        )}

        {step > 1 && (
          <div className="flex justify-between gap-[8px] [margin-top:18px]">
            <Button
              variant="outline"
              size="lg"
              className="bg-surface-subtle"
              disabled={busy}
              onClick={() => {
                setError('')
                setStep(step - 1)
              }}
            >
              <ArrowLeft size={14} />
              {t('config:configPage.previousStep')}
            </Button>
            {step === 2 ? (
              <Button size="lg" disabled={busy} onClick={() => void fetchModels()}>
                {busy ? <RefreshCw className="animate-spin" size={14} /> : <ArrowRight size={14} />}
                {busy ? t('config:configPage.fetchingModels') : t('config:configPage.nextStep')}
              </Button>
            ) : (
              <Button
                size="lg"
                disabled={busy || !modelId.trim()}
                onClick={() => void saveAndSetDefault()}
              >
                {busy ? <RefreshCw className="animate-spin" size={14} /> : <Check size={14} />}
                {busy ? t('config:configPage.saving') : t('config:configPage.saveAndSetDefault')}
              </Button>
            )}
          </div>
        )}
      </div>
    </div>
  )
}
