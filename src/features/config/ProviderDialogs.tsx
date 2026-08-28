// Provider 配置对话框：新增/编辑 Provider（密钥、端点、模型列表），
// 提交前校验必填项与端点格式。
import { useState } from 'react'
import { AlertTriangle, Plus, RefreshCw, X } from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { PROVIDER_APIS } from './provider-constants'
import { SettingsSwitch } from './settings-primitives'
import type { FormEvent } from 'react'
import type { ConfigData, ProviderType } from './config-types'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

import { AppCardHeader, AppError } from '@/components/ui/app-primitives'

type ProviderConfigModalProps = {
  onClose: () => void
  onCreated: (data: ConfigData) => void
  // 初始用途：从「新建视觉连接」等入口打开时预选 visual，减少手动切换。
  initialProviderType?: ProviderType
}

export function ProviderConfigModal({
  onClose,
  onCreated,
  initialProviderType = 'chat',
}: ProviderConfigModalProps) {
  const { t } = useI18n()
  const [draft, setDraft] = useState({
    name: '',
    id: '',
    providerType: initialProviderType,
    api: 'openai-responses',
    baseUrl: '',
    apiKey: '',
    model: '',
    modelName: '',
    modelKind: initialProviderType === 'visual' ? 'image' : 'chat',
    reasoning: true,
    enabled: true,
  })
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
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
  const updateProviderType = (providerType: ProviderType) =>
    setDraft((current) => ({
      ...current,
      providerType,
      modelKind:
        providerType === 'visual'
          ? current.modelKind === 'image' || current.modelKind === 'video'
            ? current.modelKind
            : 'image'
          : current.modelKind === 'chat'
            ? current.modelKind
            : 'chat',
    }))
  // 提交新增 Provider：POST 到运行时并回调创建结果；失败展示错误。
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setSaving(true)
    setError('')
    try {
      onCreated(
        await apiJson<ConfigData>('/api/providers', {
          method: 'POST',
          body: JSON.stringify(draft),
        }),
      )
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
        className="modal !w-[min(430px,100%)] max-h-[calc(100dvh_-_40px)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)] provider-config-modal !w-[min(620px,100%)]"
        onSubmit={submit}
      >
        <AppCardHeader>
          <div>
            <h2>{t('config:configPage.addProviderConnection')}</h2>
            <p>
              {t(
                'config:configPage.youCanCreateMultipleConnectionsUsingTheSameProtocolEachWithItsOwnKeyAndBaseURL',
              )}
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
              onChange={(event) => setDraft({ ...draft, id: event.target.value })}
              placeholder="openai-official"
            />
          </FieldLabel>
        </div>
        <FieldLabel variant="control">
          {t('config:configPage.providerPurpose')}
          <AppSelect
            value={draft.providerType}
            onChange={(event) => updateProviderType(event.target.value as ProviderType)}
          >
            <option value="chat">{t('config:configPage.chatProvider')}</option>
            <option value="visual">{t('config:configPage.visualProvider')}</option>
          </AppSelect>
          <small>
            {draft.providerType === 'visual'
              ? t(
                  'config:configPage.usedOnlyForImageGenerationVideoGenerationAndImageEditingChatModelsAreIgnored',
                )
              : t('config:configPage.usedForAgentChatAndMayAlsoIncludeVisualModels')}
          </small>
        </FieldLabel>
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
            placeholder={t('config:configPage.enterTheAPIKeyForThisConnection')}
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
          <AppSelect
            value={draft.modelKind}
            onChange={(event) => setDraft({ ...draft, modelKind: event.target.value })}
          >
            {draft.providerType !== 'visual' && (
              <option value="chat">{t('config:configPage.chat')}</option>
            )}
            <option value="image">{t('config:configPage.imageGenerationAndEditing')}</option>
            <option value="video">{t('config:configPage.videoGeneration')}</option>
          </AppSelect>
        </FieldLabel>
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
            {saving ? <RefreshCw className="animate-spin" size={14} /> : <Plus size={14} />}
            {saving ? t('config:configPage.creating') : t('config:configPage.createConnection')}
          </Button>
        </div>
      </form>
    </div>
  )
}
