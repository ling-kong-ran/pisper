// Provider 配置对话框：新增/编辑 Provider（密钥、端点、模型列表），
// 提交前校验必填项与端点格式。
import { useEffect, useState } from 'react'
import { AlertTriangle, Check, Plus, RefreshCw, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { PROVIDER_APIS } from './provider-constants'
import { ManualModelIds } from './ManualModelIds'
import { SettingsSwitch } from './settings-primitives'
import type { FormEvent } from 'react'
import type { ConfigData, ProviderConfig, ProviderType } from './config-types'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

import { AppCardHeader, AppError } from '@/components/ui/app-primitives'

type ProviderConfigModalProps = {
  onClose: () => void
  onCreated: (data: ConfigData) => void
  // 初始用途：从「新建视觉连接」等入口打开时预选 visual，减少手动切换。
  initialProviderType?: ProviderType
  // 传入已有连接时进入编辑模式，允许更新 Key、URL 和模型定义。
  initialProvider?: ProviderConfig
}

function editableModel(provider: ProviderConfig | undefined, providerType: ProviderType) {
  if (providerType === 'visual') {
    return provider?.models.find((model) => model.kind === 'image' || model.kind === 'video')
  }
  return provider?.models.find((model) => model.kind === 'chat') || provider?.models[0]
}

export function ProviderConfigModal({
  onClose,
  onCreated,
  initialProviderType = 'chat',
  initialProvider,
}: ProviderConfigModalProps) {
  const { t } = useI18n()
  const providerType = initialProvider?.type || initialProviderType
  const existingModel = editableModel(initialProvider, providerType)
  const editing = Boolean(initialProvider)
  const [draft, setDraft] = useState({
    name: initialProvider?.name || '',
    id: initialProvider?.id || '',
    providerType,
    api: initialProvider?.api || 'openai-responses',
    baseUrl: initialProvider?.baseUrl || '',
    apiKey: '',
    model: existingModel?.id || '',
    modelName: existingModel?.name || '',
    modelKind:
      existingModel?.kind === 'image' || existingModel?.kind === 'video'
        ? existingModel.kind
        : providerType === 'visual'
          ? 'image'
          : 'chat',
    reasoning: existingModel?.reasoning !== false,
    // 预填该模型当前有效的思考等级（服务端按 thinkingLevelMap 计算）；
    // 未配置过的模型为空，保存时不写映射，保持默认行为。
    thinkingLevels: existingModel?.thinkingLevels || [],
    enabled: initialProvider?.enabled ?? true,
  })
  // 思考等级选项与 Composer 下拉一致（off 恒可选，其余等级取决于模型能力）。
  const thinkingLevelOptions = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']
  // 只有用户实际操作过勾选才发送 thinkingLevels，避免把「未触碰」误写成 off-only 映射。
  const [thinkingLevelsTouched, setThinkingLevelsTouched] = useState(false)
  const toggleThinkingLevel = (level: string) => {
    setThinkingLevelsTouched(true)
    setDraft((current) => ({
      ...current,
      thinkingLevels: current.thinkingLevels.includes(level)
        ? current.thinkingLevels.filter((item) => item !== level)
        : [...current.thinkingLevels, level],
    }))
  }
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  // 手动追加的额外模型 ID：保存时随主模型批量写入该连接（对话/视觉均支持）。
  const [extraModelIds, setExtraModelIds] = useState<string[]>([])
  const updateName = (name: string) =>
    setDraft((current) => ({
      ...current,
      name,
      id:
        current.id ||
        name
          .toLowerCase()
          .replace(/[^a-z0-9._-]+/g, '-')
          .replace(/^-+|-+$/g, ''),
    }))
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
  // 新建使用专用接口，编辑复用统一配置保存接口以原子更新连接和模型定义。
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setSaving(true)
    setError('')
    try {
      let data = editing
        ? await apiJson<ConfigData>('/api/config', {
            method: 'PUT',
            body: JSON.stringify({
              ...draft,
              // 未触碰勾选时省略字段，保留服务端已有映射/默认行为。
              thinkingLevels: thinkingLevelsTouched ? draft.thinkingLevels : undefined,
              provider: draft.id,
              providerName: draft.name,
              setAsDefault: false,
            }),
          })
        : await apiJson<ConfigData>('/api/providers', {
            method: 'POST',
            body: JSON.stringify({
              ...draft,
              thinkingLevels: thinkingLevelsTouched ? draft.thinkingLevels : undefined,
            }),
          })
      // 主模型保存成功后批量写入追加的模型；失败时保留对话框，修正后可安全重试
      //（服务端会跳过已存在的模型）。
      const existingIds = new Set((initialProvider?.models || []).map((model) => model.id))
      const extraIds = extraModelIds.filter(
        (id) => id !== draft.model.trim() && !existingIds.has(id),
      )
      if (extraIds.length) {
        // 新建连接时以服务端返回的真实 ID 为准（服务端会对空/非法 ID 做归一化）。
        const targetProviderId =
          (data as ConfigData & { createdProviderId?: string }).createdProviderId || draft.id
        try {
          data = await apiJson<ConfigData>(
            `/api/providers/${encodeURIComponent(targetProviderId)}/models/batch`,
            {
              method: 'POST',
              body: JSON.stringify({
                models: extraIds.map((id) => ({
                  id,
                  kind: draft.providerType === 'visual' ? draft.modelKind : 'chat',
                })),
              }),
            },
          )
        } catch (caught) {
          setError(caught instanceof Error ? caught.message : String(caught))
          return
        }
      }
      onCreated(data)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }
  return (
    <div
      className="modal-backdrop max-[650px]:p-[8px] fixed z-[70] inset-0 grid place-items-center overflow-y-auto bg-[var(--modal-overlay)] [backdrop-filter:blur(3px)] [padding:20px] [overscroll-behavior:contain] [animation:fade-in_var(--d1)_var(--ease-out)]"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <form
        role="dialog"
        aria-modal="true"
        className="modal !w-[min(430px,100%)] max-h-[calc(100dvh_-_40px)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)] provider-config-modal !w-[min(620px,100%)]"
        onSubmit={submit}
      >
        <AppCardHeader>
          <div>
            <h2>
              {editing
                ? t('config:configPage.editProviderConnection')
                : t('config:configPage.addProviderConnection')}
            </h2>
            <p>
              {editing
                ? t('config:configPage.updateProviderKeyURLAndModelSettings')
                : t(
                    'config:configPage.youCanCreateMultipleConnectionsUsingTheSameProtocolEachWithItsOwnKeyAndBaseURL',
                  )}
            </p>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={t('config:configPage.closeDialog')}
            onClick={(event) => {
              event.preventDefault()
              event.stopPropagation()
              onClose()
            }}
          >
            <X size={17} />
          </Button>
        </AppCardHeader>
        <div className="form-grid grid gap-[9px]">
          <FieldLabel variant="control">
            {t('config:configPage.displayName')}
            <input
              value={draft.name}
              onChange={(event) => updateName(event.target.value)}
              placeholder={t('config:configPage.forExampleOpenAIOfficial')}
            />
          </FieldLabel>
          <FieldLabel variant="control">
            Provider ID
            <input
              value={draft.id}
              disabled={editing}
              onChange={(event) => setDraft({ ...draft, id: event.target.value })}
              placeholder="openai-official"
            />
          </FieldLabel>
        </div>
        <div className="flex items-center justify-between gap-[8px] [margin-top:10px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[8px_10px]">
          <span className="text-[12px] text-[var(--text-muted)]">
            {t('config:configPage.providerPurpose')}
          </span>
          <strong className="text-[12px]">
            {draft.providerType === 'visual'
              ? t('config:configPage.visualProvider')
              : t('config:configPage.chatProvider')}
          </strong>
        </div>
        <FieldLabel variant="control">
          {t('config:configPage.apiProtocol')}
          <AppSelect
            value={draft.api}
            onChange={(event) => setDraft({ ...draft, api: event.target.value })}
          >
            {PROVIDER_APIS.map(([value, label]) => (
              <option value={value} key={value}>
                {label}
              </option>
            ))}
          </AppSelect>
        </FieldLabel>
        <FieldLabel variant="control">
          Base URL
          <input
            value={draft.baseUrl}
            onChange={(event) => setDraft({ ...draft, baseUrl: event.target.value })}
            placeholder="https://api.openai.com/v1"
          />
        </FieldLabel>
        <FieldLabel variant="control">
          API Key
          <input
            type="password"
            value={draft.apiKey}
            onChange={(event) => setDraft({ ...draft, apiKey: event.target.value })}
            placeholder={
              editing
                ? t('config:configPage.keepExistingKeyBlank')
                : t('config:configPage.enterTheAPIKeyForThisConnection')
            }
          />
        </FieldLabel>
        <div className="form-grid grid gap-[9px]">
          <FieldLabel variant="control">
            {t('config:configPage.initialModelID')}
            <input
              value={draft.model}
              onChange={(event) => setDraft({ ...draft, model: event.target.value })}
              placeholder={
                draft.providerType === 'visual'
                  ? 'gpt-image-2 or grok-imagine-video'
                  : 'gpt-5.4 or gpt-image-1'
              }
            />
          </FieldLabel>
          <FieldLabel variant="control">
            {t('config:configPage.modelName')}
            <input
              value={draft.modelName}
              onChange={(event) => setDraft({ ...draft, modelName: event.target.value })}
              placeholder={t('config:configPage.leaveBlankToUseTheModelID')}
            />
          </FieldLabel>
        </div>
        <FieldLabel variant="control">
          {t('config:configPage.modelType')}
          {draft.providerType === 'visual' ? (
            <AppSelect
              value={draft.modelKind}
              onChange={(event) => setDraft({ ...draft, modelKind: event.target.value })}
            >
              <option value="image">{t('config:configPage.imageGenerationAndEditing')}</option>
              <option value="video">{t('config:configPage.videoGeneration')}</option>
            </AppSelect>
          ) : (
            <div className="rounded-[var(--r-xs)] bg-[var(--surface-subtle)] px-[10px] py-[8px] text-[13px] text-[var(--text-muted)]">
              {t('config:configPage.chat')}
            </div>
          )}
        </FieldLabel>
        <div className="grid gap-[9px]">
          <ManualModelIds
            ids={extraModelIds}
            onChange={setExtraModelIds}
            primaryId={draft.model}
            placeholder={draft.providerType === 'visual' ? 'gpt-image-1' : 'gpt-5.5'}
          />
        </div>
        {draft.modelKind === 'chat' && (
          <FieldLabel variant="control">
            {t('config:configPage.modelThinkingLevels')}
            <div className="flex flex-wrap gap-[6px]">
              {thinkingLevelOptions.map((level) => {
                // off 是应用级「不思考」开关，恒可用，不参与勾选；
                // 其余等级未勾选即从 Composer 下拉隐藏。
                const isOff = level === 'off'
                const checked = isOff || draft.thinkingLevels.includes(level)
                // 非思考模型只有 off 有意义；其余等级置灰避免误解。
                const disabled = isOff || (!draft.reasoning && level !== 'off')
                return (
                  <button
                    key={level}
                    type="button"
                    aria-pressed={checked}
                    disabled={disabled}
                    onClick={() => !isOff && toggleThinkingLevel(level)}
                    className={cn(
                      'inline-flex h-[27px] cursor-pointer items-center gap-[4px] rounded-full border px-[10px] text-[12px] font-medium transition-colors',
                      checked
                        ? 'border-[var(--control-selected-border)] bg-[var(--control-selected-bg)] text-[var(--control-selected-text)]'
                        : 'border-[var(--stroke)] bg-[var(--surface-subtle)] text-[var(--text)] hover:bg-[var(--surface-highlight)]',
                      // off 恒选中，仅表现为不可取消；其余禁用项明显弱化。
                      isOff ? 'cursor-default' : disabled && 'cursor-not-allowed opacity-40',
                    )}
                  >
                    {checked && <Check size={12} />}
                    {level}
                  </button>
                )
              })}
            </div>
            <small className="text-[var(--text-muted)]">
              {t('config:configPage.modelThinkingLevelsHint')}
            </small>
          </FieldLabel>
        )}
        <div className="modal-toggle-row [&_>_span]:flex [&_>_span]:flex-col [&_>_span]:gap-[3px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] dark:bg-[var(--surface-subtle)] flex min-h-[45px] items-center justify-between gap-[12px] [margin-top:10px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] [padding:8px_10px]">
          <span>
            <strong>{t('config:configPage.enableAfterCreation')}</strong>
            <small>
              {t(
                'config:configPage.visualModelsAreSelectedByTheVisualGenerationToolAndDoNotAppearInTheChatModelList',
              )}
            </small>
          </span>
          <SettingsSwitch
            value={draft.enabled}
            onChange={(enabled) => setDraft({ ...draft, enabled })}
          />
        </div>
        {error && (
          <AppError>
            <AlertTriangle size={13} />
            {error}
          </AppError>
        )}
        <div className="flex justify-end gap-[8px] [margin-top:18px]">
          <Button
            type="button"
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            onClick={onClose}
          >
            {t('config:configPage.cancel')}
          </Button>
          <Button size="lg" disabled={saving}>
            {saving ? (
              <RefreshCw className="animate-spin" size={14} />
            ) : editing ? (
              <Check size={14} />
            ) : (
              <Plus size={14} />
            )}
            {saving
              ? t('config:configPage.saving')
              : editing
                ? t('config:configPage.saveChanges')
                : t('config:configPage.createConnection')}
          </Button>
        </div>
      </form>
    </div>
  )
}
