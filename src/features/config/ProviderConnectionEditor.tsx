// 连接与模型分开保存，编辑地址/密钥不覆盖模型能力或全局默认。
import { useState, type FormEvent } from 'react'
import { LoaderCircle } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { AppSelect } from '@/components/AppSelect'
import { PROVIDER_APIS } from './provider-constants'
import { providerApi } from './provider-api'
import type { ConfigData, ProviderConfig } from './config-types'
export type ProviderConnectionDraft = { name: string; api: string; baseUrl: string; apiKey: string }
export function ProviderConnectionEditor({
  provider,
  onSave,
  draft,
  onDraftChange,
  onSavingChange,
}: {
  provider: ProviderConfig
  onSave: (config: ConfigData) => void
  draft: ProviderConnectionDraft
  onDraftChange: (draft: ProviderConnectionDraft | undefined) => void
  onSavingChange: (saving: boolean) => void
}) {
  const { t } = useI18n()
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const dirty =
    draft.name !== provider.name ||
    draft.api !== provider.api ||
    draft.baseUrl !== (provider.baseUrl || '') ||
    Boolean(draft.apiKey)
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
    <form
      onSubmit={(event) => void save(event)}
      className="space-y-4"
      data-provider-connection-editor
    >
      <fieldset disabled={saving} className="grid gap-4">
        <label className="grid gap-1.5 text-sm">
          {t('config:configPage.displayName')}
          <Input
            required
            value={draft.name}
            onChange={(event) => onDraftChange({ ...draft, name: event.target.value })}
          />
        </label>
        <label className="grid gap-1.5 text-sm">
          {t('config:configPage.apiKey')}
          <Input
            type="password"
            autoComplete="new-password"
            value={draft.apiKey}
            placeholder={
              provider.configured
                ? t('config:configPage.leaveBlankToKeepExistingKey')
                : t('config:configPage.enterTheProviderAPIKey')
            }
            onChange={(event) => onDraftChange({ ...draft, apiKey: event.target.value })}
          />
        </label>
        <div className="grid gap-4 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.8fr)]">
          <label className="grid gap-1.5 text-sm">
            {t('config:configPage.apiProtocol')}
            <AppSelect
              value={draft.api}
              onChange={(event) => onDraftChange({ ...draft, api: event.target.value })}
            >
              {PROVIDER_APIS.map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </AppSelect>
          </label>
          <label className="grid gap-1.5 text-sm">
            Base URL
            <Input
              required
              type="url"
              value={draft.baseUrl}
              onChange={(event) => onDraftChange({ ...draft, baseUrl: event.target.value })}
            />
          </label>
        </div>
      </fieldset>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      {dirty && (
        <div className="flex justify-end">
          <Button type="submit" size="sm" disabled={saving}>
            {saving && <LoaderCircle size={14} className="animate-spin" />}
            {saving ? t('config:configPage.saving') : t('config:configPage.saveChanges')}
          </Button>
        </div>
      )}
    </form>
  )
}
