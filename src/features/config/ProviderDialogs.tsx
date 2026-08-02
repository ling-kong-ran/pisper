import { useCallback, useEffect, useRef, useState } from 'react'
import { AlertTriangle, Check, ChevronDown, Plus, RefreshCw, X } from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { PROVIDER_APIS } from './provider-constants'
import { SettingsBadge, SettingsSwitch } from './settings-primitives'
import type { FormEvent } from 'react'
import type {
  ConfigData,
  ModelDiscoveryResult,
  ProviderConfig,
  ProviderConnectionDraft,
  ProviderModel,
  ProviderType,
} from './config-types'

type ProviderConfigModalProps = {
  onClose: () => void
  onCreated: (data: ConfigData) => void
}

type ProviderModelModalProps = {
  provider: ProviderConfig
  connectionDraft: ProviderConnectionDraft
  autoDiscover: boolean
  onClose: () => void
  onSynchronized: (data: ConfigData) => void
  onCreated: (data: ConfigData, modelId: string) => void
}

export function ProviderConfigModal({ onClose, onCreated }: ProviderConfigModalProps) {
  const { t } = useI18n()
  const [draft, setDraft] = useState({
    name: '',
    id: '',
    providerType: 'chat',
    api: 'openai-responses',
    baseUrl: '',
    apiKey: '',
    model: '',
    modelName: '',
    modelKind: 'chat',
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
      className="modal-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <form className="modal provider-config-modal" onSubmit={submit}>
        <div className="card-head">
          <div>
            <h2>{t('config:configPage.addProviderConnection')}</h2>
            <p>
              {t(
                'config:configPage.youCanCreateMultipleConnectionsUsingTheSameProtocolEachWithItsOwnKeyAndBaseURL',
              )}
            </p>
          </div>
          <button
            type="button"
            className="icon-button"
            aria-label={t('config:configPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </div>
        <div className="form-grid">
          <label className="field-label">
            {t('config:configPage.displayName')}
            <input
              value={draft.name}
              onChange={(event) => updateName(event.target.value)}
              placeholder={t('config:configPage.forExampleOpenAIOfficial')}
            />
          </label>
          <label className="field-label">
            Provider ID
            <input
              value={draft.id}
              onChange={(event) => setDraft({ ...draft, id: event.target.value })}
              placeholder="openai-official"
            />
          </label>
        </div>
        <label className="field-label">
          {t('config:configPage.providerPurpose')}
          <span className="select-wrap">
            <AppSelect
              value={draft.providerType}
              onChange={(event) => updateProviderType(event.target.value as ProviderType)}
            >
              <option value="chat">{t('config:configPage.chatProvider')}</option>
              <option value="visual">{t('config:configPage.visualProvider')}</option>
            </AppSelect>
            <ChevronDown size={13} />
          </span>
          <small>
            {draft.providerType === 'visual'
              ? t(
                  'config:configPage.usedOnlyForImageGenerationVideoGenerationAndImageEditingChatModelsAreIgnored',
                )
              : t('config:configPage.usedForAgentChatAndMayAlsoIncludeVisualModels')}
          </small>
        </label>
        <label className="field-label">
          {t('config:configPage.apiProtocol')}
          <span className="select-wrap">
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
            <ChevronDown size={13} />
          </span>
        </label>
        <label className="field-label">
          Base URL
          <input
            value={draft.baseUrl}
            onChange={(event) => setDraft({ ...draft, baseUrl: event.target.value })}
            placeholder="https://api.openai.com/v1"
          />
        </label>
        <label className="field-label">
          API Key
          <input
            type="password"
            value={draft.apiKey}
            onChange={(event) => setDraft({ ...draft, apiKey: event.target.value })}
            placeholder={t('config:configPage.enterTheAPIKeyForThisConnection')}
          />
        </label>
        <div className="form-grid">
          <label className="field-label">
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
          </label>
          <label className="field-label">
            {t('config:configPage.modelName')}
            <input
              value={draft.modelName}
              onChange={(event) => setDraft({ ...draft, modelName: event.target.value })}
              placeholder={t('config:configPage.leaveBlankToUseTheModelID')}
            />
          </label>
        </div>
        <label className="field-label">
          {t('config:configPage.modelType')}
          <span className="select-wrap">
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
            <ChevronDown size={13} />
          </span>
        </label>
        <div className="modal-toggle-row">
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
          <div className="config-error">
            <AlertTriangle size={13} />
            {error}
          </div>
        )}
        <div className="modal-actions">
          <button type="button" className="button secondary" onClick={onClose}>
            {t('config:configPage.cancel')}
          </button>
          <button className="button primary" disabled={saving}>
            {saving ? <RefreshCw className="spin" size={14} /> : <Plus size={14} />}
            {saving ? t('config:configPage.creating') : t('config:configPage.createConnection')}
          </button>
        </div>
      </form>
    </div>
  )
}

export function ProviderModelModal({
  provider,
  connectionDraft,
  autoDiscover,
  onClose,
  onSynchronized,
  onCreated,
}: ProviderModelModalProps) {
  const { t } = useI18n()
  const [draft, setDraft] = useState({
    id: '',
    name: '',
    api: provider.api || 'openai-responses',
    baseUrl: '',
    kind: provider.type === 'visual' ? 'image' : 'chat',
    reasoning: true,
  })
  const [batchKind, setBatchKind] = useState(provider.type === 'visual' ? 'image' : 'chat')
  const [batchApi, setBatchApi] = useState(
    connectionDraft.api || provider.api || 'openai-responses',
  )
  const [saving, setSaving] = useState(false)
  const [discovering, setDiscovering] = useState(false)
  const [catalog, setCatalog] = useState<ProviderModel[]>([])
  const [selectedIds, setSelectedIds] = useState<string[]>([])
  const [search, setSearch] = useState('')
  const [error, setError] = useState('')
  const onSynchronizedRef = useRef(onSynchronized)
  useEffect(() => {
    onSynchronizedRef.current = onSynchronized
  }, [onSynchronized])
  const connectionApi = connectionDraft.api
  const connectionProviderType = connectionDraft.providerType
  const connectionBaseUrl = connectionDraft.baseUrl
  const connectionApiKey = connectionDraft.apiKey
  const connectionOrganization = connectionDraft.organization
  const discover = useCallback(async () => {
    setDiscovering(true)
    setError('')
    try {
      const result = await apiJson<ModelDiscoveryResult>(
        `/api/providers/${encodeURIComponent(provider.id)}/models/discover`,
        {
          method: 'POST',
          body: JSON.stringify({
            providerType: connectionProviderType,
            api: connectionApi,
            baseUrl: connectionBaseUrl,
            apiKey: connectionApiKey,
            organization: connectionOrganization,
          }),
        },
      )
      setCatalog(result.models || [])
      setSelectedIds([])
      if (result.config) onSynchronizedRef.current(result.config)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setDiscovering(false)
    }
  }, [
    connectionApi,
    connectionApiKey,
    connectionBaseUrl,
    connectionOrganization,
    connectionProviderType,
    provider.id,
  ])
  useEffect(() => {
    if (autoDiscover) void discover()
  }, [autoDiscover, discover])
  const toggleCandidate = (model: ProviderModel) => {
    if (model.added) return
    setSelectedIds((current) =>
      current.includes(model.id) ? current.filter((id) => id !== model.id) : [...current, model.id],
    )
  }
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setSaving(true)
    setError('')
    try {
      if (selectedIds.length) {
        const models = catalog
          .filter((model) => selectedIds.includes(model.id))
          .map((model) => ({
            id: model.id,
            name: model.name,
            kind: batchKind,
            api: batchApi,
            reasoning: batchKind === 'chat',
          }))
        const data = await apiJson<ConfigData>(
          `/api/providers/${encodeURIComponent(provider.id)}/models/batch`,
          { method: 'POST', body: JSON.stringify({ models }) },
        )
        onCreated(data, selectedIds[0])
      } else {
        const data = await apiJson<ConfigData>(
          `/api/providers/${encodeURIComponent(provider.id)}/models`,
          { method: 'POST', body: JSON.stringify(draft) },
        )
        onCreated(data, draft.id)
      }
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }
  const normalizedSearch = search.trim().toLowerCase()
  const visibleCatalog = catalog.filter(
    (model) =>
      !normalizedSearch ||
      model.id.toLowerCase().includes(normalizedSearch) ||
      model.name.toLowerCase().includes(normalizedSearch),
  )
  const canSubmit = selectedIds.length > 0 || draft.id.trim()
  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <form className="modal" onSubmit={submit}>
        <div className="card-head">
          <div>
            <h2>{t('config:configPage.addModel')}</h2>
            <p>
              {t('config:configPage.fetchModelIDsFromProviderOrAddOneManually', {
                provider: provider.name,
              })}
            </p>
          </div>
          <button
            type="button"
            className="icon-button"
            aria-label={t('config:configPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </div>
        <div className="flex items-end gap-2">
          <label className="field-label min-w-0 flex-1">
            {t('config:configPage.remoteModelCatalog')}
            <input
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder={t('config:configPage.searchModelIDOrName')}
              disabled={!catalog.length}
            />
          </label>
          <button
            type="button"
            className="button secondary h-9 shrink-0"
            disabled={discovering}
            onClick={discover}
          >
            {discovering ? <RefreshCw className="spin" size={14} /> : <RefreshCw size={14} />}
            {discovering
              ? t('config:configPage.fetching')
              : catalog.length
                ? t('config:configPage.fetchAgain')
                : t('config:configPage.fetchFromAPI')}
          </button>
        </div>
        {catalog.length > 0 && (
          <div className="form-grid">
            <label className="field-label">
              {t('config:configPage.modelType')}
              <span className="select-wrap">
                <AppSelect value={batchKind} onChange={(event) => setBatchKind(event.target.value)}>
                  {provider.type !== 'visual' && (
                    <option value="chat">{t('config:configPage.chat')}</option>
                  )}
                  <option value="image">{t('config:configPage.imageGenerationAndEditing')}</option>
                  <option value="video">{t('config:configPage.videoGeneration')}</option>
                </AppSelect>
                <ChevronDown size={13} />
              </span>
            </label>
            <label className="field-label">
              {t('config:configPage.apiProtocol')}
              <span className="select-wrap">
                <AppSelect value={batchApi} onChange={(event) => setBatchApi(event.target.value)}>
                  {PROVIDER_APIS.map(([value, label]) => (
                    <option value={value} key={value}>
                      {label}
                    </option>
                  ))}
                </AppSelect>
                <ChevronDown size={13} />
              </span>
            </label>
          </div>
        )}
        {catalog.length > 0 && (
          <div
            className="max-h-64 space-y-1 overflow-y-auto rounded-[var(--r-sm)] border border-[var(--stroke)] bg-[var(--surface-subtle)] p-1"
            role="listbox"
            aria-multiselectable="true"
          >
            {visibleCatalog.map((model) => {
              const selected = selectedIds.includes(model.id)
              return (
                <button
                  type="button"
                  className={`flex min-h-10 w-full items-center gap-2 rounded-[var(--r-xs)] border px-2.5 py-1.5 text-left text-[13px] ${selected ? 'border-[var(--control-selected-border)] bg-[var(--control-selected-bg)] text-[var(--control-selected-text)]' : 'border-transparent bg-transparent text-[var(--text)] hover:bg-[var(--surface-hover)]'} ${model.added ? 'cursor-default opacity-55' : ''}`}
                  role="option"
                  aria-selected={selected}
                  disabled={model.added}
                  onClick={() => toggleCandidate(model)}
                  key={model.id}
                >
                  <span className="min-w-0 flex-1">
                    <strong className="block truncate">{model.id}</strong>
                    {model.name !== model.id && (
                      <small
                        className={`block truncate text-[12px] ${selected ? 'text-inherit opacity-75' : 'text-[var(--text-muted)]'}`}
                      >
                        {model.name}
                      </small>
                    )}
                  </span>
                  {model.added ? (
                    <SettingsBadge tone="gray">{t('config:configPage.added')}</SettingsBadge>
                  ) : selected ? (
                    <Check size={15} />
                  ) : null}
                </button>
              )
            })}
            {!visibleCatalog.length && (
              <div className="px-3 py-6 text-center text-[13px] text-[var(--text-muted)]">
                {t('config:configPage.noMatchingModels')}
              </div>
            )}
          </div>
        )}
        <div className="flex items-center gap-3 text-[12px] text-[var(--text-muted)]">
          <span className="h-px flex-1 bg-[var(--stroke)]" />
          <span>{t('config:configPage.addManually')}</span>
          <span className="h-px flex-1 bg-[var(--stroke)]" />
        </div>
        <div className="form-grid">
          <label className="field-label">
            {t('config:configPage.modelID')}
            <input
              value={draft.id}
              onChange={(event) => setDraft({ ...draft, id: event.target.value })}
              placeholder="gpt-5.4-mini, gpt-image-1, or sora-2"
            />
          </label>
          <label className="field-label">
            {t('config:configPage.displayName')}
            <input
              value={draft.name}
              onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              placeholder={t('config:configPage.leaveBlankToUseTheModelID')}
            />
          </label>
        </div>
        <label className="field-label">
          {t('config:configPage.modelBaseURL')}
          <input
            value={draft.baseUrl}
            onChange={(event) => setDraft({ ...draft, baseUrl: event.target.value })}
            placeholder={t('config:configPage.optionalLeaveBlankToInheritTheProviderBaseURL')}
          />
        </label>
        <label className="field-label">
          {t('config:configPage.apiProtocol')}
          <span className="select-wrap">
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
            <ChevronDown size={13} />
          </span>
        </label>
        <label className="field-label">
          {t('config:configPage.modelType')}
          <span className="select-wrap">
            <AppSelect
              value={draft.kind}
              onChange={(event) => setDraft({ ...draft, kind: event.target.value })}
            >
              {provider.type !== 'visual' && (
                <option value="chat">{t('config:configPage.chat')}</option>
              )}
              <option value="image">{t('config:configPage.imageGenerationAndEditing')}</option>
              <option value="video">{t('config:configPage.videoGeneration')}</option>
            </AppSelect>
            <ChevronDown size={13} />
          </span>
        </label>
        {draft.kind !== 'image' && draft.kind !== 'video' && (
          <div className="modal-toggle-row">
            <span>
              <strong>{t('config:configPage.reasoningModel')}</strong>
              <small>{t('config:configPage.enableReasoningEffortThinkingLevel')}</small>
            </span>
            <SettingsSwitch
              value={draft.reasoning}
              onChange={(reasoning) => setDraft({ ...draft, reasoning })}
            />
          </div>
        )}
        {error && (
          <div className="config-error">
            <AlertTriangle size={13} />
            {error}
          </div>
        )}
        <div className="modal-actions">
          <button type="button" className="button secondary" onClick={onClose}>
            {t('config:configPage.cancel')}
          </button>
          <button className="button primary" disabled={saving || !canSubmit}>
            {saving ? <RefreshCw className="spin" size={14} /> : <Plus size={14} />}
            {saving
              ? t('config:configPage.adding')
              : selectedIds.length
                ? t('config:configPage.addCountModels', { count: selectedIds.length })
                : t('config:configPage.addModel')}
          </button>
        </div>
      </form>
    </div>
  )
}
