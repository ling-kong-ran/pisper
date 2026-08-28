// 快速配置向导：三步完成对话模型配置——
// 选择服务商（已知 Provider 预填协议/端点）→ 填 API Key → 自动拉取模型并推荐，
// 最后一键「保存并设为默认」。自定义/视觉连接仍走高级弹窗。
import { useState } from 'react'
import {
  AlertTriangle,
  ArrowLeft,
  ArrowRight,
  Check,
  CheckCircle2,
  Plus,
  RefreshCw,
  Server,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { cn } from '@/lib/utils'
import { recommendedChatModel } from './config-state'
import { PROVIDER_ICONS } from './provider-constants'
import { SettingsBadge } from './settings-primitives'
import type { ConfigData, ModelDiscoveryResult, ProviderConfig } from './config-types'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

import { AppCardHeader, AppError } from '@/components/ui/app-primitives'

type QuickSetupWizardProps = {
  config: ConfigData
  onClose: () => void
  onCompleted: (data: ConfigData, providerId: string, modelId: string) => void
  onCustomProvider: () => void
}

export function QuickSetupWizard({
  config,
  onClose,
  onCompleted,
  onCustomProvider,
}: QuickSetupWizardProps) {
  const { t } = useI18n()
  // Codex 走本地登录导入、纯视觉 Provider 不参与对话配置，都不进向导。
  const candidates = config.providers.filter(
    (provider) => provider.type !== 'visual' && provider.id !== 'openai-codex',
  )
  const [step, setStep] = useState(1)
  const [provider, setProvider] = useState<ProviderConfig | null>(null)
  const [apiKey, setApiKey] = useState('')
  const [fetchedProvider, setFetchedProvider] = useState<ProviderConfig | null>(null)
  const [modelId, setModelId] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')

  const pickProvider = (item: ProviderConfig) => {
    setProvider(item)
    setError('')
    setStep(2)
  }

  // 第二步 → 拉取模型：Key 以临时参数传给发现接口（不落盘），
  // 成功保存后模型目录会自动同步进配置。
  const fetchModels = async () => {
    if (!provider) return
    if (!apiKey.trim() && !provider.configured) {
      setError(t('config:configPage.enterTheAPIKeyForThisConnection'))
      return
    }
    setBusy(true)
    setError('')
    try {
      const result = await apiJson<ModelDiscoveryResult>(
        `/api/providers/${encodeURIComponent(provider.id)}/models/discover`,
        {
          method: 'POST',
          body: JSON.stringify({ apiKey, providerType: 'chat' }),
        },
      )
      const refreshed = result.config?.providers.find((item) => item.id === provider.id)
      // 目录未同步（synchronized=false）时 config 为 null，用发现结果兑底展示。
      const source =
        refreshed || (result.models?.length ? { ...provider, models: result.models } : null)
      const chatModels = source?.models.filter((item) => item.kind === 'chat') || []
      if (!source || !chatModels.length) {
        setError(t('config:configPage.fetchOrAddAChatModel'))
        return
      }
      setFetchedProvider(source)
      // 预选推荐模型（Provider 默认 > 目录排序第一），用户仍可改选。
      const recommended = refreshed ? recommendedChatModel(refreshed) : chatModels[0]
      setModelId(recommended?.id || '')
      setStep(3)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy(false)
    }
  }

  const saveAndSetDefault = async () => {
    if (!provider || !modelId) return
    setBusy(true)
    setError('')
    try {
      // baseUrl/organization 回传当前生效值：已知 Provider 为官方默认端点，
      // 避免空值误清掉用户已有的自定义端点/组织头。
      const saved = await apiJson<ConfigData>('/api/config', {
        method: 'PUT',
        body: JSON.stringify({
          provider: provider.id,
          providerType: 'chat',
          api: provider.api,
          baseUrl: provider.baseUrl || '',
          organization: provider.organization || '',
          model: modelId,
          apiKey,
          thinkingLevel: config.thinkingLevel,
          toolMode: config.toolMode,
          setAsDefault: true,
          enabled: true,
        }),
      })
      onCompleted(saved, provider.id, modelId)
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
                  type="password"
                  autoComplete="new-password"
                  autoFocus
                  value={apiKey}
                  onChange={(event) => setApiKey(event.target.value)}
                  placeholder={
                    provider.configured
                      ? t('config:configPage.keepExistingKeyBlank')
                      : t('config:configPage.enterTheAPIKeyForThisConnection')
                  }
                />
              </FieldLabel>
            </div>
          </>
        )}

        {step === 3 && fetchedProvider && (
          <>
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
                disabled={busy || !modelId}
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
