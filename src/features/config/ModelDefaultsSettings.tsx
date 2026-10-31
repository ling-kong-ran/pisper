// 设置 › 模型的默认值（效果图 desktop-settings-light.png、样稿 settings.html）：页标题下面三行设置——
// 默认模型（跨提供方的对话模型）、思考强度、可用工具。选中即保存，没有单独的“保存”键。
// 效果图里的“辅助模型”（生成标题、压缩上下文）Runtime 没有对应字段，不做假控件（见 decisions）。
// 思考强度与可用工具原先在“更多设置 › Agent 运行策略”卡片里，挪到这里后那张卡片删除；搜索“运行策略”直接定位到这几行。
import { useEffect, useId, useState, type ReactNode } from 'react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { FeatureAlert } from '@/components/app/FeaturePage'
import { OrbitSpinner } from '@/components/ui/spinner'
import { apiJson } from '@/lib/api'
import type { Notify } from '@/app/route-context'
import type { ConfigData, ProviderConfig } from './config-types'

const THINKING_LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh'] as const
// 选项值里用不会出现在 id 里的分隔符把提供方与模型拼在一起。
const SEPARATOR = '\u0001'

type ModelDefaultsSettingsProps = {
  config: ConfigData
  notify: Notify
  busy: boolean
  onConfigChanged: (data: ConfigData) => void
  onSetDefaultModel: (provider: ProviderConfig, model: string) => void | Promise<void>
}

// 一行设置：左边名称 + 一句说明，右边控件；窄屏上下堆叠（手机控件 44 高、占满宽度）。
function DefaultsRow({
  labelId,
  label,
  hint,
  children,
}: {
  labelId: string
  label: string
  hint: string
  children: ReactNode
}) {
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-6 gap-y-2 border-b border-stroke py-4 max-[650px]:py-3">
      <div className="min-w-[min(240px,100%)] flex-1">
        <div id={labelId} className="text-sm font-medium text-content">
          {label}
        </div>
        <p id={`${labelId}-hint`} className="mt-0.5 text-xs text-content-muted">
          {hint}
        </p>
      </div>
      <div className="w-[min(280px,100%)] min-w-0 max-[650px]:w-full">{children}</div>
    </div>
  )
}

export function ModelDefaultsSettings({
  config,
  notify,
  busy,
  onConfigChanged,
  onSetDefaultModel,
}: ModelDefaultsSettingsProps) {
  const { t } = useI18n()
  const id = useId()
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  // 乐观显示刚选的值；保存失败时回到服务端的值。
  const [thinkingLevel, setThinkingLevel] = useState(config.thinkingLevel)
  const [toolMode, setToolMode] = useState(config.toolMode)
  useEffect(() => {
    setThinkingLevel(config.thinkingLevel)
    setToolMode(config.toolMode)
  }, [config.thinkingLevel, config.toolMode])

  const defaultProviderId = config.defaultProvider || config.provider
  const defaultModel = config.defaultModel || config.model
  const defaultProvider = config.providers.find((item) => item.id === defaultProviderId)
  // 运行策略的保存要借一个已配置提供方的上下文（Runtime 校验认证）；还没有时三行都禁用并写明原因。
  const canSave = Boolean(defaultProvider?.configured && defaultModel)
  const chatProviders = config.providers.filter(
    (provider) => provider.type === 'chat' && provider.configured && provider.enabled,
  )
  const chatOptions = chatProviders.flatMap((provider) =>
    provider.models
      .filter((model) => model.kind === 'chat')
      .map((model) => ({ provider, model, value: `${provider.id}${SEPARATOR}${model.id}` })),
  )
  const currentValue =
    defaultProvider && defaultModel ? `${defaultProvider.id}${SEPARATOR}${defaultModel}` : ''

  const savePolicy = async (patch: { thinkingLevel?: string; toolMode?: string }) => {
    if (!defaultProvider || !defaultModel) return
    setSaving(true)
    setError('')
    try {
      const saved = await apiJson<ConfigData>('/api/config', {
        method: 'PUT',
        body: JSON.stringify({
          provider: defaultProvider.id,
          providerType: defaultProvider.type,
          api: defaultProvider.api,
          baseUrl: defaultProvider.baseUrl || '',
          organization: defaultProvider.organization || '',
          model: defaultModel,
          thinkingLevel: patch.thinkingLevel ?? config.thinkingLevel,
          toolMode: patch.toolMode ?? config.toolMode,
          // 只更新策略字段：默认提供方 / 模型保持不变，也不改动启用状态。
          setAsDefault: false,
        }),
      })
      onConfigChanged(saved)
      notify(t('config:configPage.runtimePolicySaved'))
    } catch (caught) {
      setThinkingLevel(config.thinkingLevel)
      setToolMode(config.toolMode)
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  const disabled = !canSave || saving || busy
  return (
    <section
      data-config-card="models-current-model"
      aria-labelledby={`${id}-heading`}
      aria-busy={saving || undefined}
      className="mt-6 min-w-0"
    >
      <h2 id={`${id}-heading`} className="sr-only">
        {t('config:configPage.currentChatModel')}
      </h2>
      <div data-config-target="models-runtime-policy" className="border-t border-stroke">
        <DefaultsRow
          labelId={`${id}-model`}
          label={t('config:providerWorkbench.defaultModel')}
          hint={t('config:providerWorkbench.defaultModelHint')}
        >
          <AppSelect
            aria-labelledby={`${id}-model`}
            className="w-full max-[650px]:min-h-11 pointer-coarse:min-h-11"
            disabled={!chatOptions.length || busy || saving}
            value={chatOptions.some((option) => option.value === currentValue) ? currentValue : ''}
            onChange={(event) => {
              const [providerId, modelId] = event.target.value.split(SEPARATOR)
              const provider = chatProviders.find((item) => item.id === providerId)
              if (provider && modelId) void onSetDefaultModel(provider, modelId)
            }}
          >
            {!chatOptions.length && (
              <option value="">{t('config:providerWorkbench.noChatModel')}</option>
            )}
            {chatOptions.map(({ provider, model, value }) => (
              <option key={value} value={value}>
                {`${model.name || model.id} · ${provider.name}`}
              </option>
            ))}
          </AppSelect>
        </DefaultsRow>
        <DefaultsRow
          labelId={`${id}-thinking`}
          label={t('config:configPage.thinkingLevel')}
          hint={t('config:providerWorkbench.thinkingHint')}
        >
          <AppSelect
            aria-labelledby={`${id}-thinking`}
            className="w-full font-mono max-[650px]:min-h-11 pointer-coarse:min-h-11"
            disabled={disabled}
            value={thinkingLevel}
            onChange={(event) => {
              setThinkingLevel(event.target.value)
              void savePolicy({ thinkingLevel: event.target.value })
            }}
          >
            {THINKING_LEVELS.map((level) => (
              <option key={level}>{level}</option>
            ))}
          </AppSelect>
        </DefaultsRow>
        <DefaultsRow
          labelId={`${id}-tools`}
          label={t('config:configPage.availableTools')}
          hint={t('config:providerWorkbench.toolsHint')}
        >
          <AppSelect
            aria-labelledby={`${id}-tools`}
            className="w-full max-[650px]:min-h-11 pointer-coarse:min-h-11"
            disabled={disabled}
            value={toolMode}
            onChange={(event) => {
              setToolMode(event.target.value)
              void savePolicy({ toolMode: event.target.value })
            }}
          >
            <option value="full">{t('config:configPage.fullToolAccess')}</option>
            <option value="custom">
              {t('config:configPage.customManageEachToolOnThePluginsPage')}
            </option>
          </AppSelect>
        </DefaultsRow>
      </div>
      {(!canSave || saving) && (
        <p className="mt-2 flex items-center gap-2 text-xs text-content-muted" role="status">
          {saving && <OrbitSpinner className="size-3" />}
          {saving
            ? t('config:configPage.saving')
            : t('config:providerWorkbench.defaultsNeedProvider')}
        </p>
      )}
      {error && <FeatureAlert>{error}</FeatureAlert>}
    </section>
  )
}
