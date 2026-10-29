// 连接与模型分开保存，编辑地址/密钥不覆盖模型能力或全局默认。
import { useId, useState, type FormEvent, type ReactNode } from 'react'
import { useI18n } from '@/app/use-i18n'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { AppSelect } from '@/components/AppSelect'
import { cn } from '@/lib/utils'
import { providerApiOptions } from './provider-constants'
import { providerApi } from './provider-api'
import { SettingsField } from './settings-primitives'
import type { ConfigData, ProviderConfig } from './config-types'
export type ProviderConnectionDraft = { name: string; api: string; baseUrl: string; apiKey: string }
export function isProviderConnectionDirty(
  provider: ProviderConfig,
  draft: ProviderConnectionDraft,
) {
  return (
    draft.name !== provider.name ||
    draft.api !== provider.api ||
    draft.baseUrl !== (provider.baseUrl || '') ||
    Boolean(draft.apiKey)
  )
}
// 手机上字段与下拉统一 44px 触控高度；桌面保持 34px 字段节奏。
const FIELD_CLASS = 'pointer-coarse:min-h-11 max-[650px]:min-h-11'
export function ProviderConnectionEditor({
  provider,
  onSave,
  draft,
  onDraftChange,
  onSavingChange,
  children,
}: {
  provider: ProviderConfig
  onSave: (config: ConfigData) => void
  draft: ProviderConnectionDraft
  onDraftChange: (draft: ProviderConnectionDraft | undefined) => void
  onSavingChange: (saving: boolean) => void
  // 详情卡在表单字段与底部保存栏之间插入的内容（模型表）；保存栏因此落在卡片底边。
  children?: ReactNode
}) {
  const { t } = useI18n()
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const fieldId = useId()
  const dirty = isProviderConnectionDirty(provider, draft)
  const save = async (event: FormEvent) => {
    event.preventDefault()
    if (saving) return
    setSaving(true)
    onSavingChange(true)
    setError('')
    try {
      const result = await providerApi.saveConnection(provider.id, {
        ...draft,
        apiKey: draft.apiKey.trim() || undefined,
      })
      onSave(result)
      onDraftChange(undefined)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
      onSavingChange(false)
    }
  }
  return (
    <form onSubmit={(event) => void save(event)} data-provider-connection-editor>
      <fieldset disabled={saving} className="min-w-0 divide-y divide-stroke-soft px-4 py-1">
        <SettingsField label={t('config:configPage.displayName')} htmlFor={`${fieldId}-name`}>
          <Input
            id={`${fieldId}-name`}
            required
            className={FIELD_CLASS}
            value={draft.name}
            onChange={(event) => onDraftChange({ ...draft, name: event.target.value })}
          />
        </SettingsField>
        <SettingsField label={t('config:configPage.apiKey')} htmlFor={`${fieldId}-key`}>
          <Input
            id={`${fieldId}-key`}
            type="password"
            autoComplete="new-password"
            className={`${FIELD_CLASS} font-mono placeholder:font-sans`}
            value={draft.apiKey}
            placeholder={
              provider.configured
                ? t('config:configPage.leaveBlankToKeepExistingKey')
                : t('config:configPage.enterTheProviderAPIKey')
            }
            onChange={(event) => onDraftChange({ ...draft, apiKey: event.target.value })}
          />
        </SettingsField>
        {/* 协议 ID 以等宽小字放在标签列，读数与下拉里的人类可读名称互相印证。 */}
        <SettingsField
          label={t('config:configPage.apiProtocol')}
          labelId={`${fieldId}-api-label`}
          help={<span className="font-mono">{draft.api}</span>}
        >
          <AppSelect
            aria-labelledby={`${fieldId}-api-label`}
            className={`${FIELD_CLASS} h-8.5`}
            value={draft.api}
            onChange={(event) => onDraftChange({ ...draft, api: event.target.value })}
          >
            {providerApiOptions(draft.api).map(([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </AppSelect>
        </SettingsField>
        <SettingsField label="Base URL" htmlFor={`${fieldId}-url`}>
          <Input
            id={`${fieldId}-url`}
            required
            type="url"
            className={`${FIELD_CLASS} font-mono`}
            value={draft.baseUrl}
            onChange={(event) => onDraftChange({ ...draft, baseUrl: event.target.value })}
          />
        </SettingsField>
      </fieldset>
      {error && (
        <p
          role="alert"
          className="mx-4 mb-3 rounded-sm bg-danger-soft px-2.5 py-2 text-sm text-danger"
        >
          {error}
        </p>
      )}
      {children}
      {/* 保存栏常驻卡片底边：无改动时禁用，避免出现/消失造成布局跳动；
          有改动时吸附在滚动视口底部，模型很多也不必翻过整张模型表才能保存。 */}
      <div
        className={cn(
          'flex flex-wrap items-center justify-end gap-2 border-t border-stroke bg-surface-subtle px-4 py-3',
          dirty && 'sticky bottom-0 z-[1]',
        )}
      >
        {dirty && (
          <span role="status" className="mr-auto text-xs text-content-muted">
            {t('config:configPage.unsavedChanges')}
          </span>
        )}
        <Button type="submit" className="max-[650px]:h-11" disabled={!dirty} loading={saving}>
          {saving ? t('config:configPage.saving') : t('config:configPage.saveChanges')}
        </Button>
      </div>
    </form>
  )
}
