import { useCallback, useEffect, useRef, useState } from 'react'
import {
  AlertTriangle,
  Bot,
  Brain,
  Check,
  ChevronDown,
  Code2,
  Download,
  KeyRound,
  Network,
  Plus,
  RefreshCw,
  Save,
  Server,
  ShieldCheck,
  Sparkles,
  Trash2,
  X,
  Zap,
} from 'lucide-react'
import { APP_NAME } from '@/app/brand'
import { useI18n } from '@/app/use-i18n'
import { Badge, Panel, SectionTitle, Toggle } from '@/components/ui'
import { AppSelect } from '@/components/AppSelect'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { apiJson } from '@/lib/api'
import { NotificationSettings } from './NotificationSettings'
import { UpdateSettings } from './UpdateSettings'
import { LanguageSettings } from './LanguageSettings'
import type { FormEvent } from 'react'
import type { LucideIcon } from 'lucide-react'
import type { I18nValues } from '@/app/i18n'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { EntityRecord } from '@/types/chat'
import type { AppUpdateController } from '@/types/update'
import type { NotificationSettingsData } from '@/types/notifications'

type Translate = (message: string, values?: I18nValues) => string
type ProviderType = 'chat' | 'visual'
type ProviderModel = EntityRecord & {
  id: string
  name: string
  kind: string
  api?: string
  reasoning?: boolean
  contextWindow?: number
  baseUrlOverride?: string
  added?: boolean
}
type ProviderConfig = EntityRecord & {
  id: string
  name: string
  type: ProviderType
  api: string
  models: ProviderModel[]
  defaultModel?: string
  baseUrl?: string
  organization?: string
  enabled: boolean
  configured: boolean
  custom?: boolean
}
type ConfigData = EntityRecord & {
  providers: ProviderConfig[]
  provider: string
  model: string
  thinkingLevel: string
  toolMode: string
  createdProviderId?: string
  addedModelIds?: string[]
  apiKeyUpdated?: boolean
}
type ConfigDraft = {
  provider: string
  providerType: ProviderType
  model: string
  apiKey: string
  baseUrl: string
  modelBaseUrl: string
  organization: string
  thinkingLevel: string
  toolMode: string
}
type DiscoveredProvider = EntityRecord & {
  id: string
  source: string
  providerName: string
  authType?: string
  authVariable?: string
  models?: ProviderModel[]
  warnings?: Array<{ code: string }>
  imported?: boolean
  conflict?: boolean
  importable?: boolean
  api?: string
  baseUrl?: string
  location?: string
}
type DiscoveryError = { source: string; code: string }
type DiscoveryData = { providers: DiscoveredProvider[]; errors: DiscoveryError[] }
type ProviderImportResult = {
  config: ConfigData
  discovery: DiscoveryData
  providerId: string
  selectedModel?: string
}
type ConfigPageProps = {
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
  section: string
  setSection: (section: string) => void
  onBrowserNotificationChange?: (settings: NotificationSettingsData) => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  update: AppUpdateController
}
type DiscoveredProvidersPanelProps = {
  discovery: DiscoveryData
  discovering: boolean
  error: string
  importing: string
  onRefresh: () => void | Promise<void>
  onImport: (provider: DiscoveredProvider) => void | Promise<void>
}
type ProviderConfigModalProps = {
  onClose: () => void
  onCreated: (data: ConfigData) => void
}
type ProviderConnectionDraft = {
  providerType: ProviderType
  api: string
  baseUrl: string
  apiKey: string
  organization: string
}
type ProviderModelModalProps = {
  provider: ProviderConfig
  connectionDraft: ProviderConnectionDraft
  autoDiscover: boolean
  onClose: () => void
  onSynchronized: (data: ConfigData) => void
  onCreated: (data: ConfigData, modelId: string) => void
}
type ModelDiscoveryResult = { models?: ProviderModel[]; config?: ConfigData }

function configDraft(
  data: ConfigData,
  provider?: ProviderConfig,
  preferredModel?: string,
): ConfigDraft {
  const chatModels = provider?.models.filter((item) => item.kind === 'chat') || []
  const model =
    chatModels.find((item) => item.id === preferredModel) ||
    chatModels.find((item) => item.id === provider?.defaultModel) ||
    chatModels[0]
  return {
    provider: provider?.id || 'openai',
    providerType: provider?.type || 'chat',
    model: model?.id || '',
    apiKey: '',
    baseUrl: provider?.baseUrl || '',
    modelBaseUrl: model?.baseUrlOverride || '',
    organization: provider?.organization || '',
    thinkingLevel: data.thinkingLevel || 'medium',
    toolMode: data.toolMode || 'full',
  }
}

function refreshedConfigDraft(data: ConfigData, current: ConfigDraft | null): ConfigDraft {
  if (!current) {
    const provider = data.providers.find((item) => item.id === data.provider) || data.providers[0]
    return configDraft(data, provider, data.model)
  }
  const provider =
    data.providers.find((item) => item.id === current.provider) ||
    data.providers.find((item) => item.id === data.provider) ||
    data.providers[0]
  const chatModels = provider?.models.filter((model) => model.kind === 'chat') || []
  const model = chatModels.find((item) => item.id === current.model) || chatModels[0]
  return {
    ...current,
    provider: provider?.id || current.provider,
    providerType: current.providerType || provider?.type || 'chat',
    model: model?.id || '',
    modelBaseUrl: model?.baseUrlOverride || '',
  }
}

export function ConfigPage({
  notify,
  registerPrimaryAction,
  section,
  setSection,
  onBrowserNotificationChange,
  requestConfirm,
  update,
}: ConfigPageProps) {
  const { t } = useI18n()
  const [config, setConfig] = useState<ConfigData | null>(null)
  const [draft, setDraft] = useState<ConfigDraft | null>(null)
  const [saving, setSaving] = useState(false)
  const [toggling, setToggling] = useState('')
  const [error, setError] = useState('')
  const [providerModal, setProviderModal] = useState(false)
  const [modelModal, setModelModal] = useState('')
  const [discovery, setDiscovery] = useState<DiscoveryData>({ providers: [], errors: [] })
  const [discovering, setDiscovering] = useState(true)
  const [discoveryError, setDiscoveryError] = useState('')
  const [importingProvider, setImportingProvider] = useState('')
  const apiKeyInputRef = useRef<HTMLInputElement>(null)
  usePagePrimaryAction(registerPrimaryAction, () => setProviderModal(true))

  const refreshDiscovery = async () => {
    setDiscovering(true)
    setDiscoveryError('')
    try {
      setDiscovery(await apiJson<DiscoveryData>('/api/providers/discovery'))
    } catch (caught) {
      setDiscoveryError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setDiscovering(false)
    }
  }

  useEffect(() => {
    let active = true
    apiJson<ConfigData>('/api/config')
      .then((data) => {
        if (!active) return
        setConfig(data)
        const provider =
          data.providers.find((item) => item.id === data.provider) || data.providers[0]
        setDraft(configDraft(data, provider, data.model))
        return apiJson<{ config?: ConfigData }>('/api/providers/models/refresh', {
          method: 'POST',
          body: '{}',
        })
      })
      .then((result) => {
        if (!active || !result?.config) return
        const refreshed = result.config
        setConfig(refreshed)
        setDraft((current) => refreshedConfigDraft(refreshed, current))
      })
      .catch(
        (caught: unknown) =>
          active && setError(caught instanceof Error ? caught.message : String(caught)),
      )
    void refreshDiscovery()
    return () => {
      active = false
    }
  }, [])

  const importDiscoveredProvider = async (provider: DiscoveredProvider) => {
    const source = provider.source === 'codex-config' ? 'Codex config.toml' : 'Claude settings.json'
    const approved = await requestConfirm({
      title: t('config:configPage.loadProviderConfiguration'),
      message: t('config:configPage.loadThisProviderConfigurationFromSource', { source }),
      confirmLabel: t('config:configPage.loadConfiguration'),
    })
    if (!approved) return
    setImportingProvider(provider.id)
    setError('')
    try {
      const result = await apiJson<ProviderImportResult>(
        `/api/providers/${encodeURIComponent(provider.id)}/import`,
        { method: 'POST', body: '{}' },
      )
      setConfig(result.config)
      setDiscovery(result.discovery)
      const imported =
        result.config.providers.find((item) => item.id === result.providerId) ||
        result.config.providers[0]
      setDraft((current) => ({
        ...configDraft(result.config, imported, result.selectedModel),
        thinkingLevel: current?.thinkingLevel || result.config.thinkingLevel,
        toolMode: current?.toolMode || result.config.toolMode,
      }))
      notify(
        t('config:configPage.nameConfigurationHasBeenLoadedIntoVesper', { name: imported.name }),
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setImportingProvider('')
    }
  }

  const selectProvider = (provider: ProviderConfig) => {
    if (!config) return
    setDraft((current) => ({
      ...configDraft(config, provider),
      thinkingLevel: current?.thinkingLevel || config?.thinkingLevel || 'medium',
      toolMode: current?.toolMode || config?.toolMode || 'full',
    }))
  }

  const selectModel = (modelId: string) => {
    if (!config || !draft) return
    const provider = config.providers.find((item) => item.id === draft.provider)
    const selectedModel = provider?.models.find((item) => item.id === modelId)
    setDraft((current) =>
      current
        ? { ...current, model: modelId, modelBaseUrl: selectedModel?.baseUrlOverride || '' }
        : current,
    )
  }

  const selectProviderType = (providerType: ProviderType) => {
    const firstChatModel = selectedProvider.models.find((model) => model.kind === 'chat')
    setDraft((current) =>
      current
        ? {
            ...current,
            providerType,
            model: providerType === 'visual' ? '' : current.model || firstChatModel?.id || '',
            modelBaseUrl:
              providerType === 'visual'
                ? ''
                : current.modelBaseUrl || firstChatModel?.baseUrlOverride || '',
          }
        : current,
    )
  }

  const toggleProvider = async (provider: ProviderConfig, enabled: boolean) => {
    setToggling(provider.id)
    setError('')
    try {
      const updated = await apiJson<ConfigData>(
        `/api/providers/${encodeURIComponent(provider.id)}/enabled`,
        { method: 'PUT', body: JSON.stringify({ enabled }) },
      )
      setConfig(updated)
      notify(
        t('config:configPage.nameState', {
          name: provider.name,
          state: enabled ? t('config:configPage.enabled') : t('config:configPage.disabled'),
        }),
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setToggling('')
    }
  }

  const deleteProvider = async (provider: ProviderConfig) => {
    const approved = await requestConfirm({
      title: t('config:configPage.deleteProviderConnection'),
      message: t(
        'config:configPage.deleteNameItsModelSettingsAndAuthenticationDetailsWillAlsoBeRemoved',
        {
          name: provider.name,
        },
      ),
      confirmLabel: t('config:configPage.delete'),
    })
    if (!approved) return
    setError('')
    try {
      const updated = await apiJson<ConfigData>(
        `/api/providers/${encodeURIComponent(provider.id)}`,
        {
          method: 'DELETE',
        },
      )
      setConfig(updated)
      const nextProvider =
        updated.providers.find((item) => item.id === updated.provider) || updated.providers[0]
      setDraft(configDraft(updated, nextProvider, updated.model))
      notify(t('config:configPage.providerConnectionDeleted'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }

  const save = async () => {
    if (!draft) return
    setSaving(true)
    setError('')
    try {
      const apiKey = apiKeyInputRef.current?.value || draft.apiKey
      const saved = await apiJson<ConfigData>('/api/config', {
        method: 'PUT',
        body: JSON.stringify({ ...draft, apiKey }),
      })
      if (apiKey.trim() && !saved.apiKeyUpdated) {
        throw new Error(t('config:configPage.apiKeyCouldNotBeUpdatedPleaseRetry'))
      }
      setConfig(saved)
      const provider =
        saved.providers.find((item) => item.id === draft.provider) || saved.providers[0]
      setDraft((current) =>
        current
          ? {
              ...configDraft(saved, provider, current.model),
              thinkingLevel: current.thinkingLevel,
              toolMode: current.toolMode,
            }
          : current,
      )
      notify(
        provider.type === 'visual'
          ? t('config:configPage.visualModelSettingsSaved')
          : draft.model
            ? t('config:configPage.agentSettingsSavedNewChatsWillUseThisModel')
            : t('config:configPage.providerSettingsSaved'),
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  const subnav = (
    <div className="config-subnav">
      <button className={section === 'models' ? 'active' : ''} onClick={() => setSection('models')}>
        {t('config:configPage.models')}
      </button>
      <button
        className={section === 'notifications' ? 'active' : ''}
        onClick={() => setSection('notifications')}
      >
        {t('config:configPage.notifications')}
      </button>
      <button
        className={section === 'interface' ? 'active' : ''}
        onClick={() => setSection('interface')}
      >
        {t('config:configPage.interface')}
      </button>
      <button
        className={section === 'updates' ? 'active' : ''}
        onClick={() => setSection('updates')}
      >
        {t('config:configPage.appUpdates')}
      </button>
    </div>
  )
  if (section === 'interface')
    return (
      <>
        {subnav}
        <LanguageSettings notify={notify} />
      </>
    )
  if (section === 'updates')
    return (
      <>
        {subnav}
        <UpdateSettings notify={notify} update={update} />
      </>
    )
  if (!config || !draft)
    return (
      <>
        {subnav}
        <Panel className="empty-state">
          <RefreshCw className="spin" size={24} />
          <h2>{t('config:configPage.loadingModelCatalog')}</h2>
          <p>{t('config:configPage.readingProvidersAndAuthenticationStatus')}</p>
        </Panel>
      </>
    )
  const selectedProvider =
    config.providers.find((item) => item.id === draft.provider) || config.providers[0]
  const selectedModel = selectedProvider.models.find((item) => item.id === draft.model)
  const visualOnly = draft.providerType === 'visual'
  const chatModels = visualOnly
    ? []
    : selectedProvider.models.filter((item) => item.kind === 'chat')
  const visualModels = selectedProvider.models.filter((item) => item.kind !== 'chat')
  const providerIcons: Record<string, LucideIcon> = {
    openai: Bot,
    'openai-codex': Bot,
    anthropic: Brain,
    google: Sparkles,
    deepseek: Code2,
    xai: Zap,
    openrouter: Network,
    'kimi-coding': Sparkles,
    'zai-coding-cn': Brain,
  }
  const codexOAuth = selectedProvider.id === 'openai-codex'
  return (
    <>
      {subnav}
      {section === 'notifications' ? (
        <NotificationSettings
          notify={notify}
          onBrowserNotificationChange={onBrowserNotificationChange}
        />
      ) : (
        <>
          <DiscoveredProvidersPanel
            discovery={discovery}
            discovering={discovering}
            error={discoveryError}
            importing={importingProvider}
            onRefresh={refreshDiscovery}
            onImport={importDiscoveredProvider}
          />
          <div className="split-list-detail config-layout">
            <Panel className="selection-list">
              <div className="provider-list-heading">
                <SectionTitle title={t('config:configPage.providerConnections')} />
                <button
                  className="icon-button"
                  title={t('config:configPage.addProvider')}
                  onClick={() => setProviderModal(true)}
                >
                  <Plus size={15} />
                </button>
              </div>
              {config.providers.map((provider) => {
                const Icon = providerIcons[provider.id] || Server
                return (
                  <div
                    className={`provider-list-item ${draft.provider === provider.id ? 'active' : ''} ${provider.enabled ? '' : 'disabled-provider'}`}
                    key={provider.id}
                  >
                    <button
                      className="provider-select-main"
                      onClick={() => selectProvider(provider)}
                    >
                      <span className="list-icon">
                        <Icon size={16} />
                      </span>
                      <span>
                        <strong>{provider.name}</strong>
                        <small>
                          {provider.id} ·{' '}
                          {t('config:configPage.countModels', { count: provider.models.length })}
                        </small>
                      </span>
                    </button>
                    <div className="provider-list-control">
                      <Badge
                        tone={!provider.enabled ? 'gray' : provider.configured ? 'green' : 'amber'}
                      >
                        {!provider.enabled
                          ? t('config:configPage.disabled2')
                          : provider.configured
                            ? t('config:configPage.configured')
                            : t('config:configPage.notAuthenticated')}
                      </Badge>
                      <Toggle
                        value={provider.enabled}
                        disabled={!provider.configured || toggling === provider.id}
                        onChange={(enabled) => toggleProvider(provider, enabled)}
                      />
                    </div>
                  </div>
                )
              })}
            </Panel>
            <div className="detail-stack">
              <Panel>
                <div className="card-head">
                  <div>
                    <h2>{selectedProvider.name}</h2>
                    <p>
                      {selectedProvider.id} · {selectedProvider.api} ·{' '}
                      {t(
                        'config:configPage.authenticationAndEndpointsAreStoredIndependentlyForEachConnection',
                      )}
                    </p>
                  </div>
                  <div className="provider-header-status">
                    <Badge
                      tone={
                        !selectedProvider.enabled
                          ? 'gray'
                          : selectedProvider.configured
                            ? 'green'
                            : 'amber'
                      }
                    >
                      {!selectedProvider.enabled
                        ? t('config:configPage.disabled2')
                        : selectedProvider.configured
                          ? t('config:configPage.authenticationReady')
                          : codexOAuth
                            ? t('config:configPage.codexCLILoginRequired')
                            : t('config:configPage.apiKeyRequired')}
                    </Badge>
                    <Toggle
                      value={selectedProvider.enabled}
                      disabled={!selectedProvider.configured || toggling === selectedProvider.id}
                      onChange={(enabled) => toggleProvider(selectedProvider, enabled)}
                    />
                    {selectedProvider.custom && (
                      <button
                        className="icon-button danger"
                        title={t('config:configPage.deleteProvider')}
                        onClick={() => deleteProvider(selectedProvider)}
                      >
                        <Trash2 size={14} />
                      </button>
                    )}
                  </div>
                </div>
                <label className="field-label">
                  {t('config:configPage.providerPurpose')}
                  <span className="select-wrap">
                    <AppSelect
                      value={draft.providerType}
                      onChange={(event) => selectProviderType(event.target.value as ProviderType)}
                    >
                      <option value="chat">{t('config:configPage.chatProvider')}</option>
                      <option value="visual">{t('config:configPage.visualProvider')}</option>
                    </AppSelect>
                    <ChevronDown size={13} />
                  </span>
                  <small>
                    {visualOnly
                      ? t(
                          'config:configPage.usedOnlyForImageGenerationVideoGenerationAndImageEditingChatModelsAreIgnored',
                        )
                      : t('config:configPage.usedForAgentChatAndMayAlsoIncludeVisualModels')}
                  </small>
                </label>
                {codexOAuth ? (
                  <div className="oauth-provider-note">
                    <ShieldCheck size={17} />
                    <span>
                      <strong>
                        {selectedProvider.configured
                          ? t('config:configPage.chatGPTOAuthConnected')
                          : t('config:configPage.codexCLILoginRequired')}
                      </strong>
                      <small>
                        {selectedProvider.configured
                          ? t(
                              'config:configPage.openAICodexUsesChatGPTPlusProOAuthAndDoesNotAcceptARegularAPIKey',
                            )
                          : t(
                              'config:configPage.signInWithCodexCLIThenLoadItFromTheLocalProvidersSectionAbove',
                            )}
                      </small>
                    </span>
                  </div>
                ) : (
                  <>
                    <label className="field-label">
                      API Key
                      <span className="input-wrap">
                        <input
                          ref={apiKeyInputRef}
                          type="password"
                          name={`provider-api-key-${selectedProvider.id}`}
                          autoComplete="new-password"
                          value={draft.apiKey}
                          onChange={(event) => {
                            const apiKey = event.currentTarget.value
                            setDraft((current) => (current ? { ...current, apiKey } : current))
                          }}
                          onInput={(event) => {
                            const apiKey = event.currentTarget.value
                            setDraft((current) =>
                              current && current.apiKey !== apiKey
                                ? { ...current, apiKey }
                                : current,
                            )
                          }}
                          placeholder={
                            selectedProvider.configured
                              ? t('config:configPage.configuredLeaveBlankToKeepTheExistingKey')
                              : t('config:configPage.enterTheProviderAPIKey')
                          }
                        />
                        <KeyRound size={14} />
                      </span>
                    </label>
                    <label className="field-label">
                      Provider Base URL
                      <input
                        value={draft.baseUrl}
                        onChange={(event) => setDraft({ ...draft, baseUrl: event.target.value })}
                        placeholder={t(
                          'config:configPage.defaultEndpointForModelsInThisConnection',
                        )}
                      />
                    </label>
                    <label className="field-label">
                      Organization
                      <input
                        value={draft.organization}
                        onChange={(event) =>
                          setDraft({ ...draft, organization: event.target.value })
                        }
                        placeholder={t('config:configPage.optionalUsedOnlyForOpenAIOrganization')}
                      />
                    </label>
                  </>
                )}
                <div className="model-config-heading">
                  <SectionTitle title={t('config:configPage.models')} />
                  {!codexOAuth && (
                    <div className="flex items-center gap-2">
                      <button
                        className="button secondary tiny"
                        onClick={() => setModelModal('discover')}
                      >
                        <RefreshCw size={13} />
                        {t('config:configPage.fetchModels')}
                      </button>
                      <button
                        className="button secondary tiny"
                        onClick={() => setModelModal('manual')}
                      >
                        <Plus size={13} />
                        {t('config:configPage.addModel')}
                      </button>
                    </div>
                  )}
                </div>
                {!visualOnly && chatModels.length > 0 ? (
                  <>
                    <label className="field-label">
                      {t('config:configPage.defaultChatModel')}
                      <span className="select-wrap">
                        <AppSelect
                          value={draft.model}
                          onChange={(event) => selectModel(event.target.value)}
                        >
                          {chatModels.map((model) => (
                            <option key={model.id} value={model.id}>
                              {model.name}
                            </option>
                          ))}
                        </AppSelect>
                        <ChevronDown size={13} />
                      </span>
                    </label>
                    {!codexOAuth && (
                      <label className="field-label">
                        {t('config:configPage.modelBaseURL')}
                        <input
                          value={draft.modelBaseUrl}
                          onChange={(event) =>
                            setDraft({ ...draft, modelBaseUrl: event.target.value })
                          }
                          placeholder={t(
                            'config:configPage.optionalOverrideTheProviderBaseURLForThisModel',
                          )}
                        />
                      </label>
                    )}
                    <div className="tag-field">
                      <Badge>{draft.provider}</Badge>
                      <Badge>
                        {selectedModel?.reasoning
                          ? t('config:configPage.reasoningSupported')
                          : t('config:configPage.standardModel')}
                      </Badge>
                      <Badge tone="gray">
                        {selectedModel?.contextWindow
                          ? `${Math.round(selectedModel.contextWindow / 1000)}K context`
                          : t('config:configPage.automaticContext')}
                      </Badge>
                      {selectedModel?.baseUrlOverride && (
                        <Badge tone="amber">{t('config:configPage.customBaseURL')}</Badge>
                      )}
                    </div>
                  </>
                ) : visualOnly ? (
                  <div className="permission-note">
                    <Sparkles size={16} />
                    <span>
                      <strong>{t('config:configPage.visualOnlyProvider')}</strong>
                      <small>
                        {t(
                          'config:configPage.usedOnlyForImageGenerationVideoGenerationAndImageEditingAndExcludedFromAgentChatModelSelection',
                        )}
                      </small>
                    </span>
                  </div>
                ) : (
                  <div className="permission-note">
                    <AlertTriangle size={16} />
                    <span>
                      <strong>{t('config:configPage.noChatModelAvailable')}</strong>
                      <small>{t('config:configPage.fetchOrAddAChatModel')}</small>
                    </span>
                  </div>
                )}
                {visualModels.length > 0 && (
                  <div className="visual-model-list">
                    <span>{t('config:configPage.visualModels')}</span>
                    {visualModels.map((model) => (
                      <Badge tone="blue" key={model.id}>
                        {model.name} ·{' '}
                        {model.kind === 'video'
                          ? t('config:configPage.videoGeneration')
                          : t('config:configPage.imageGenerationAndEditing')}
                      </Badge>
                    ))}
                  </div>
                )}
              </Panel>
              <div className="config-bottom">
                <Panel>
                  <SectionTitle title={t('config:configPage.agentRuntimePolicy')} />
                  <label className="field-label">
                    {t('config:configPage.thinkingLevel')}
                    <span className="select-wrap">
                      <AppSelect
                        value={draft.thinkingLevel}
                        onChange={(event) =>
                          setDraft({ ...draft, thinkingLevel: event.target.value })
                        }
                      >
                        {['off', 'minimal', 'low', 'medium', 'high', 'xhigh'].map((level) => (
                          <option key={level}>{level}</option>
                        ))}
                      </AppSelect>
                      <ChevronDown size={13} />
                    </span>
                  </label>
                  <label className="field-label">
                    {t('config:configPage.availableTools')}
                    <span className="select-wrap">
                      <AppSelect
                        value={draft.toolMode}
                        onChange={(event) => setDraft({ ...draft, toolMode: event.target.value })}
                      >
                        <option value="read-only">
                          {t('config:configPage.readOnlyReadGrepFindLs')}
                        </option>
                        <option value="workspace">
                          {t('config:configPage.workspaceAllowEditWrite')}
                        </option>
                        <option value="full">{t('config:configPage.fullAllowBash')}</option>
                        <option value="custom">
                          {t('config:configPage.customManageEachToolOnThePluginsPage')}
                        </option>
                      </AppSelect>
                      <ChevronDown size={13} />
                    </span>
                  </label>
                  <div className="permission-note">
                    <ShieldCheck size={16} />
                    <span>
                      <strong>{t('config:configPage.permissionsAreEnforcedByTheServer')}</strong>
                      <small>
                        {t(
                          'config:configPage.afterSavingExistingRuntimesAreReleasedAndNewChatsUseTheLatestPolicy',
                        )}
                      </small>
                    </span>
                  </div>
                </Panel>
                <Panel className="usage-card">
                  <SectionTitle title={t('config:configPage.runtimeStatus')} />
                  <div className="usage-number">
                    <span>Engine</span>
                    <strong>{APP_NAME} Runtime</strong>
                  </div>
                  <div className="usage-number">
                    <span>Provider</span>
                    <strong>{selectedProvider.name}</strong>
                  </div>
                  <div className="usage-number">
                    <span>Models</span>
                    <strong>{selectedProvider.models.length}</strong>
                  </div>
                  <div className="usage-number">
                    <span>{t('config:configPage.status')}</span>
                    <strong>
                      {selectedProvider.enabled
                        ? t('config:configPage.enabled')
                        : t('config:configPage.disabled')}
                    </strong>
                  </div>
                  {error && (
                    <div className="config-error">
                      <AlertTriangle size={13} />
                      {error}
                    </div>
                  )}
                  <button
                    className="button primary wide"
                    disabled={
                      saving ||
                      !selectedProvider.enabled ||
                      (codexOAuth && !selectedProvider.configured)
                    }
                    onClick={save}
                  >
                    {saving ? <RefreshCw className="spin" size={14} /> : <Save size={14} />}
                    {saving
                      ? t('config:configPage.saving')
                      : codexOAuth && !selectedProvider.configured
                        ? t('config:configPage.loadAuthenticationToSave')
                        : selectedProvider.enabled
                          ? visualOnly
                            ? t('config:configPage.saveVisualModelSettings')
                            : draft.model
                              ? t('config:configPage.saveAndSetAsDefaultProvider')
                              : t('config:configPage.saveProviderSettings')
                          : t('config:configPage.enableToSave')}
                  </button>
                </Panel>
              </div>
            </div>
          </div>
          {providerModal && (
            <ProviderConfigModal
              onClose={() => setProviderModal(false)}
              onCreated={(data) => {
                const provider = data.providers.find((item) => item.id === data.createdProviderId)
                setConfig(data)
                setDraft(configDraft(data, provider))
                setProviderModal(false)
                notify(t('config:configPage.providerConnectionCreated'))
              }}
            />
          )}
          {modelModal && (
            <ProviderModelModal
              provider={{ ...selectedProvider, type: draft.providerType }}
              connectionDraft={{
                providerType: draft.providerType,
                api: selectedProvider.api,
                baseUrl: draft.baseUrl,
                apiKey: draft.apiKey,
                organization: draft.organization,
              }}
              autoDiscover={modelModal === 'discover'}
              onClose={() => setModelModal('')}
              onSynchronized={(data) => {
                setConfig(data)
                setDraft((current) => refreshedConfigDraft(data, current))
              }}
              onCreated={(data, modelId) => {
                const provider = data.providers.find((item) => item.id === selectedProvider.id)
                setConfig(data)
                setDraft((current) => ({
                  ...configDraft(data, provider, modelId),
                  providerType: current?.providerType || provider?.type || 'chat',
                  thinkingLevel: current?.thinkingLevel || data.thinkingLevel,
                  toolMode: current?.toolMode || data.toolMode,
                }))
                setModelModal('')
                notify(
                  t('config:configPage.countModelsAdded', {
                    count: data.addedModelIds?.length || 1,
                  }),
                )
              }}
            />
          )}
        </>
      )}
    </>
  )
}

function discoverySourceLabel(provider: Pick<DiscoveredProvider, 'source'>) {
  return provider.source === 'codex-config' ? 'Codex config.toml' : 'Claude settings.json'
}

function discoveryAuthLabel(provider: DiscoveredProvider, t: Translate) {
  if (provider.authType === 'environment')
    return t('config:configPage.keyVariableName', { name: provider.authVariable })
  if (provider.authType === 'bearer' || provider.authType === 'api_key')
    return t('config:configPage.authenticationIncludedInConfiguration')
  if (provider.authType === 'external-login') return t('config:configPage.authenticationRequired')
  return t('config:configPage.noAuthenticationInConfiguration')
}

function discoveryWarningLabel(code: string, t: Translate) {
  if (code === 'multiple_auth_values')
    return t(
      'config:configPage.multipleAuthenticationFieldsWereDetectedTheAuthorizationFieldTakesPrecedence',
    )
  if (code === 'invalid_url') return t('config:configPage.theProviderEndpointIsInvalid')
  if (code === 'unsupported_api')
    return t('config:configPage.theAPIProtocolInThisConfigurationIsNotSupportedYet')
  if (code === 'invalid_env_name')
    return t('config:configPage.theKeyEnvironmentVariableNameIsInvalid')
  return t('config:configPage.someConfigurationFieldsCannotBeImported')
}

function DiscoveredProvidersPanel({
  discovery,
  discovering,
  error,
  importing,
  onRefresh,
  onImport,
}: DiscoveredProvidersPanelProps) {
  const { t } = useI18n()
  const providers = discovery.providers || []
  const errors = discovery.errors || []
  const errorLabel = (item: DiscoveryError) =>
    ['invalid_json', 'invalid_toml'].includes(item.code)
      ? t('config:configPage.invalidConfigurationFileFormat')
      : item.code === 'unsupported_config'
        ? t('config:configPage.noImportableProviderConfigurationWasFound')
        : item.code === 'file_too_large'
          ? t('config:configPage.configurationFileIsTooLarge')
          : t('config:configPage.unableToReadTheConfigurationFile')
  return (
    <Panel className="provider-discovery-panel">
      <div className="provider-discovery-head">
        <span className="language-settings-icon">
          <Server size={18} />
        </span>
        <span>
          <strong>{t('config:configPage.localProviderConfiguration')}</strong>
          <small>
            {t(
              'config:configPage.readProviderEndpointModelAndAuthenticationFieldsFromCodexConfigTomlAndClaudeSettingsJson',
            )}
          </small>
        </span>
        <button
          type="button"
          className="button secondary tiny"
          disabled={discovering || Boolean(importing)}
          onClick={onRefresh}
        >
          {discovering ? <RefreshCw className="spin" size={13} /> : <RefreshCw size={13} />}
          {t('config:configPage.rescan')}
        </button>
      </div>
      {discovering && !providers.length ? (
        <div className="provider-discovery-empty">
          <RefreshCw className="spin" size={15} />
          {t('config:configPage.scanningProviderConfigurationFiles')}
        </div>
      ) : providers.length ? (
        <div className="provider-discovery-list">
          {providers.map((provider) => {
            const source = discoverySourceLabel(provider)
            const busy = importing === provider.id
            const Icon = provider.source === 'claude-config' ? Brain : Bot
            const modelSummary = provider.models?.length
              ? provider.models.map((model) => model.id).join(', ')
              : t('config:configPage.noModelSpecified')
            return (
              <div
                className={`provider-discovery-card ${provider.imported ? 'configured' : ''}`}
                key={provider.id}
              >
                <span className={`provider-discovery-icon source-${provider.source}`}>
                  <Icon size={17} />
                </span>
                <span className="provider-discovery-copy">
                  <strong>
                    {source} · {provider.providerName}
                  </strong>
                  <small>
                    {provider.api} · {modelSummary}
                  </small>
                  <small>
                    {provider.baseUrl || t('config:configPage.noBaseURLSpecified')} ·{' '}
                    {discoveryAuthLabel(provider, t)} · {provider.location}
                  </small>
                </span>
                <span className="provider-discovery-actions">
                  {provider.imported ? (
                    <Badge tone="green">{t('config:configPage.loaded')}</Badge>
                  ) : provider.conflict ? (
                    <Badge tone="amber">{t('config:configPage.conflictDetected')}</Badge>
                  ) : provider.importable ? (
                    <button
                      type="button"
                      className="button primary tiny"
                      disabled={busy || Boolean(importing)}
                      onClick={() => onImport(provider)}
                    >
                      {busy ? <RefreshCw className="spin" size={12} /> : <Download size={12} />}
                      {busy
                        ? t('config:configPage.loading')
                        : t('config:configPage.loadConfiguration')}
                    </button>
                  ) : (
                    <Badge tone="gray">{t('config:configPage.cannotLoad')}</Badge>
                  )}
                </span>
              </div>
            )
          })}
        </div>
      ) : (
        <div className="provider-discovery-empty">
          <Server size={15} />
          {t('config:configPage.noImportableCodexOrClaudeProviderConfigurationWasDetected')}
        </div>
      )}
      {(error ||
        errors.length > 0 ||
        providers.some((provider) =>
          provider.warnings?.some((warning) => warning.code !== 'login_auth_not_imported'),
        )) && (
        <div className="provider-discovery-errors" aria-live="polite">
          {error && (
            <span>
              <AlertTriangle size={13} />
              {error}
            </span>
          )}
          {errors.map((item, index) => (
            <span key={`${item.source}-${item.code}-${index}`}>
              <AlertTriangle size={13} />
              {discoverySourceLabel(item)} · {errorLabel(item)}
            </span>
          ))}
          {providers.flatMap((provider) =>
            (provider.warnings || [])
              .filter((warning) => warning.code !== 'login_auth_not_imported')
              .map((warning, index) => (
                <span key={`${provider.id}-${warning.code}-${index}`}>
                  <AlertTriangle size={13} />
                  {discoverySourceLabel(provider)} · {discoveryWarningLabel(warning.code, t)}
                </span>
              )),
          )}
        </div>
      )}
    </Panel>
  )
}

const PROVIDER_APIS: Array<[string, string]> = [
  ['openai-responses', 'OpenAI Responses'],
  ['openai-completions', 'OpenAI Chat Completions'],
  ['anthropic-messages', 'Anthropic Messages'],
  ['google-generative-ai', 'Google Generative AI'],
]

function ProviderConfigModal({ onClose, onCreated }: ProviderConfigModalProps) {
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
    modelKind: 'auto',
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
        providerType === 'visual' && (current.modelKind === 'auto' || current.modelKind === 'chat')
          ? 'image'
          : current.modelKind,
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
                <option value="auto">{t('config:configPage.autoDetect')}</option>
              )}
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
          <Toggle value={draft.enabled} onChange={(enabled) => setDraft({ ...draft, enabled })} />
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

function ProviderModelModal({
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
    kind: provider.type === 'visual' ? 'image' : 'auto',
    reasoning: true,
  })
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
            kind: model.kind,
            api: connectionDraft.api,
            reasoning: model.kind === 'chat',
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
      (provider.type !== 'visual' || model.kind !== 'chat') &&
      (!normalizedSearch ||
        model.id.toLowerCase().includes(normalizedSearch) ||
        model.name.toLowerCase().includes(normalizedSearch)),
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
                  <Badge tone={model.kind === 'image' || model.kind === 'video' ? 'blue' : 'gray'}>
                    {model.kind === 'video'
                      ? t('config:configPage.video')
                      : model.kind === 'image'
                        ? t('config:configPage.image')
                        : t('config:configPage.chat')}
                  </Badge>
                  {model.added ? (
                    <Badge tone="gray">{t('config:configPage.added')}</Badge>
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
                <option value="auto">{t('config:configPage.autoDetect')}</option>
              )}
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
            <Toggle
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
