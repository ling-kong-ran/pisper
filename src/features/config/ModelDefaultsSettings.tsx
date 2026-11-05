// 设置 › 模型的默认值（效果图 desktop-settings-light.png、样稿 settings.html）：页标题下面三行设置——
// 默认模型（跨提供方的对话模型）、思考强度、可用工具。选中即保存，没有单独的“保存”键。
// 辅助模型：起标题、压缩上下文、整理记忆这类后台杂活用的模型，不设置时各自跟随原来的模型。
// 思考强度与可用工具原先在“更多设置 › Agent 运行策略”卡片里，挪到这里后那张卡片删除；搜索“运行策略”直接定位到这几行。
// 备用模型：主模型重试用尽或额度耗尽时，运行中按顺序切换（最多两个）。
import { useEffect, useId, useState, type ReactNode } from 'react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { FeatureAlert } from '@/components/app/FeaturePage'
import { OrbitSpinner } from '@/components/ui/spinner'
import { compactModelName } from '@/lib/model-display'
import { apiJson } from '@/lib/api'
import type { Notify } from '@/app/route-context'
import type { ConfigData, ProviderConfig } from './config-types'
import { providerApi } from './provider-api'

const THINKING_LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'] as const
type ThinkingLevel = (typeof THINKING_LEVELS)[number]
// 选项值里用不会出现在 id 里的分隔符把提供方与模型拼在一起。
const SEPARATOR = '\u0001'

type ModelDefaultsSettingsProps = {
  config: ConfigData
  notify: Notify
  busy: boolean
  onConfigChanged: (data: ConfigData) => void
  onSetDefaultModel: (provider: ProviderConfig, model: string) => void | Promise<void>
}

function fallbackRef(option: { provider: ProviderConfig; model: { id: string } }) {
  return `${option.provider.id}/${option.model.id}`
}

// 一行设置。桌面：左边名称 + 一句说明，右边控件。手机：名称与控件同一行（控件 44 高），说明在下面占满宽度，
// 五行设置不再把提供方挤出第一屏。
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
    <div className="grid min-w-0 grid-cols-[minmax(0,1fr)_minmax(0,280px)] items-center gap-x-6 gap-y-0.5 border-b border-stroke py-4 max-[650px]:grid-cols-[auto_minmax(0,1fr)] max-[650px]:gap-x-3 max-[650px]:gap-y-1 max-[650px]:py-3">
      <div
        id={labelId}
        className="self-end text-sm font-medium text-content max-[650px]:self-center"
      >
        {label}
      </div>
      <div className="row-span-2 min-w-0 max-[650px]:row-span-1">{children}</div>
      <p
        id={`${labelId}-hint`}
        className="col-start-1 self-start text-xs text-content-muted max-[650px]:col-span-2"
      >
        {hint}
      </p>
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
  // 和输入框的模型芯片同一套叫法，不把 medium 这样的内部值露给用户。
  const thinkingLabels: Record<ThinkingLevel, string> = {
    off: t('chat:focusSession.thinkingLabel.off'),
    minimal: t('chat:focusSession.thinkingLabel.minimal'),
    low: t('chat:focusSession.thinkingLabel.low'),
    medium: t('chat:focusSession.thinkingLabel.medium'),
    high: t('chat:focusSession.thinkingLabel.high'),
    xhigh: t('chat:focusSession.thinkingLabel.xhigh'),
    max: t('chat:focusSession.thinkingLabel.max'),
  }
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  // 乐观显示刚选的值；保存失败时回到服务端的值。
  const [thinkingLevel, setThinkingLevel] = useState(config.thinkingLevel)
  const [toolMode, setToolMode] = useState(config.toolMode)
  useEffect(() => {
    setThinkingLevel(config.thinkingLevel)
    setToolMode(config.toolMode)
  }, [config.thinkingLevel, config.toolMode])
  const savedFallbacks = (config.fallbackModels || []).join('\n')
  const [fallbacks, setFallbacks] = useState<string[]>(config.fallbackModels || [])
  useEffect(() => {
    setFallbacks(savedFallbacks ? savedFallbacks.split('\n') : [])
  }, [savedFallbacks])
  const [auxiliary, setAuxiliary] = useState(config.auxiliaryModel || '')
  useEffect(() => {
    setAuxiliary(config.auxiliaryModel || '')
  }, [config.auxiliaryModel])

  const defaultProviderId = config.defaultProvider || config.provider
  const defaultModel = config.defaultModel || config.model
  const defaultProvider = config.providers.find((item) => item.id === defaultProviderId)
  // 档位跟默认模型走，和会话页的滑条同一份数据：不列它没有的档，也不漏掉它有的 max。
  // 之前保存的档位这个模型不支持时仍然列出并写明，不让下拉框空着或悄悄改掉。
  const modelLevels = defaultProvider?.models.find(
    (item) => item.id === defaultModel,
  )?.thinkingLevels
  const thinkingOptions = THINKING_LEVELS.filter(
    (level) => !modelLevels?.length || modelLevels.includes(level) || level === thinkingLevel,
  )
  const thinkingOptionLabel = (level: ThinkingLevel) =>
    modelLevels?.length && !modelLevels.includes(level)
      ? t('config:providerWorkbench.thinkingUnsupportedByModel', { level: thinkingLabels[level] })
      : thinkingLabels[level]
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

  // 备用模型的选项值直接用 "provider/modelId"，与 Runtime 保存的格式一致。
  const saveFallbacks = async (slot: number, ref: string) => {
    const next = [...fallbacks]
    next[slot] = ref
    // 清空第一个时第二个顺位前移；重复项只留第一次出现。
    const models = next.filter((item, index) => item && next.indexOf(item) === index)
    const previous = fallbacks
    setFallbacks(models)
    setSaving(true)
    setError('')
    try {
      onConfigChanged(await providerApi.saveFallbackModels(models))
      notify(t('config:providerWorkbench.fallbackModelsSaved'))
    } catch (caught) {
      setFallbacks(previous)
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }
  const saveAuxiliary = async (ref: string) => {
    const previous = auxiliary
    setAuxiliary(ref)
    setSaving(true)
    setError('')
    try {
      onConfigChanged(await providerApi.saveAuxiliaryModel(ref))
      notify(t('config:providerWorkbench.auxiliaryModelSaved'))
    } catch (caught) {
      setAuxiliary(previous)
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }
  const fallbackSelect = (slot: number, labelId: string) => (
    <AppSelect
      aria-labelledby={labelId}
      className="w-full"
      disabled={!chatOptions.length || busy || saving || (slot > 0 && !fallbacks[0])}
      value={fallbacks[slot] || ''}
      onChange={(event) => void saveFallbacks(slot, event.target.value)}
    >
      <option value="">{t('config:providerWorkbench.fallbackNone')}</option>
      {/* 已保存但现在不可用的备用模型也显示出来，便于用户看见并换掉。 */}
      {fallbacks[slot] &&
        !chatOptions.some((option) => fallbackRef(option) === fallbacks[slot]) && (
          <option value={fallbacks[slot]}>{fallbacks[slot]}</option>
        )}
      {chatOptions.map((option) => (
        <option key={option.value} value={fallbackRef(option)}>
          {`${option.model.name || option.model.id} · ${option.provider.name}`}
        </option>
      ))}
    </AppSelect>
  )
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
            className="w-full"
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
                {/* 下拉框按末尾截断：带路径前缀的模型 ID 只显示能区分的最后几段。 */}
                {`${compactModelName(
                  model.name || model.id,
                  chatOptions.map((option) => option.model.name || option.model.id),
                )} · ${provider.name}`}
              </option>
            ))}
          </AppSelect>
        </DefaultsRow>
        <DefaultsRow
          labelId={`${id}-fallback`}
          label={t('config:providerWorkbench.fallbackModels')}
          hint={t('config:providerWorkbench.fallbackModelsHint')}
        >
          <div className="flex min-w-0 flex-col gap-2">
            <span id={`${id}-fallback-1`} className="sr-only">
              {t('config:providerWorkbench.fallbackFirst')}
            </span>
            {fallbackSelect(0, `${id}-fallback ${id}-fallback-1`)}
            {/* 第二个备用只有选了第一个之后才有意义：之前不放一个禁用的空下拉。 */}
            {fallbacks[0] && (
              <>
                <span id={`${id}-fallback-2`} className="sr-only">
                  {t('config:providerWorkbench.fallbackSecond')}
                </span>
                {fallbackSelect(1, `${id}-fallback ${id}-fallback-2`)}
              </>
            )}
          </div>
        </DefaultsRow>
        <DefaultsRow
          labelId={`${id}-auxiliary`}
          label={t('config:providerWorkbench.auxiliaryModel')}
          hint={t('config:providerWorkbench.auxiliaryModelHint')}
        >
          <AppSelect
            aria-labelledby={`${id}-auxiliary`}
            aria-describedby={`${id}-auxiliary-hint`}
            className="w-full"
            disabled={!chatOptions.length || busy || saving}
            value={auxiliary}
            onChange={(event) => void saveAuxiliary(event.target.value)}
          >
            <option value="">{t('config:providerWorkbench.auxiliaryFollowSession')}</option>
            {/* 已保存但现在不可用的辅助模型也显示出来，便于用户看见并换掉。 */}
            {auxiliary && !chatOptions.some((option) => fallbackRef(option) === auxiliary) && (
              <option value={auxiliary}>{auxiliary}</option>
            )}
            {chatOptions.map((option) => (
              <option key={option.value} value={fallbackRef(option)}>
                {`${option.model.name || option.model.id} · ${option.provider.name}`}
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
            className="w-full"
            disabled={disabled}
            value={thinkingLevel}
            onChange={(event) => {
              setThinkingLevel(event.target.value)
              void savePolicy({ thinkingLevel: event.target.value })
            }}
          >
            {thinkingOptions.map((level) => (
              <option key={level} value={level}>
                {thinkingOptionLabel(level)}
              </option>
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
            className="w-full"
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
