import {
  AlertTriangle,
  Bot,
  Brain,
  Code2,
  ChevronDown,
  Network,
  Plus,
  RefreshCw,
  Server,
  Sparkles,
  Zap,
} from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import {
  SettingsBadge,
  SettingsCard,
  SettingsSectionTitle,
  SettingsSwitch,
} from './settings-primitives'
import type { LucideIcon } from 'lucide-react'
import type { ConfigDraft, ProviderConfig } from './config-types'

const PROVIDER_ICONS: Record<string, LucideIcon> = {
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

type ProviderConnectionsProps = {
  providers: ProviderConfig[]
  selectedProviderId: string
  toggling: string
  onAdd: () => void
  onSelect: (provider: ProviderConfig) => void
  onToggle: (provider: ProviderConfig, enabled: boolean) => void | Promise<void>
}

export function ProviderConnections({
  providers,
  selectedProviderId,
  toggling,
  onAdd,
  onSelect,
  onToggle,
}: ProviderConnectionsProps) {
  const { t } = useI18n()
  return (
    <SettingsCard className="selection-list">
      <div className="provider-list-heading">
        <SettingsSectionTitle title={t('config:configPage.providerConnections')} />
        <button className="icon-button" title={t('config:configPage.addProvider')} onClick={onAdd}>
          <Plus size={15} />
        </button>
      </div>
      {providers.map((provider) => {
        const Icon = PROVIDER_ICONS[provider.id] || Server
        return (
          <div
            className={`provider-list-item ${selectedProviderId === provider.id ? 'active' : ''} ${provider.enabled ? '' : 'disabled-provider'}`}
            key={provider.id}
          >
            <button className="provider-select-main" onClick={() => onSelect(provider)}>
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
              <SettingsBadge tone={provider.enabled && provider.configured ? 'green' : 'gray'}>
                {!provider.enabled
                  ? t('config:configPage.disabled2')
                  : provider.configured
                    ? t('config:configPage.configured')
                    : t('config:configPage.notAuthenticated')}
              </SettingsBadge>
              <SettingsSwitch
                value={provider.enabled}
                disabled={!provider.configured || toggling === provider.id}
                onChange={(enabled) => onToggle(provider, enabled)}
              />
            </div>
          </div>
        )
      })}
    </SettingsCard>
  )
}

type ProviderModelCatalogProps = {
  provider: ProviderConfig
  draft: ConfigDraft
  onPatchDraft: (patch: Partial<ConfigDraft>) => void
  onSelectModel: (modelId: string) => void
  onOpenModelDialog: (mode: 'discover' | 'manual') => void
}

export function ProviderModelCatalog({
  provider,
  draft,
  onPatchDraft,
  onSelectModel,
  onOpenModelDialog,
}: ProviderModelCatalogProps) {
  const { t } = useI18n()
  const visualOnly = draft.providerType === 'visual'
  const codexOAuth = provider.id === 'openai-codex'
  const chatModels = visualOnly ? [] : provider.models.filter((item) => item.kind === 'chat')
  const visualModels = provider.models.filter((item) => item.kind !== 'chat')
  const selectedModel = provider.models.find((item) => item.id === draft.model)

  return (
    <>
      <div className="model-config-heading">
        <SettingsSectionTitle title={t('config:configPage.models')} />
        {!codexOAuth && (
          <div className="flex items-center gap-2">
            <button className="button secondary tiny" onClick={() => onOpenModelDialog('discover')}>
              <RefreshCw size={13} />
              {t('config:configPage.fetchModels')}
            </button>
            <button className="button secondary tiny" onClick={() => onOpenModelDialog('manual')}>
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
                onChange={(event) => onSelectModel(event.target.value)}
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
                onChange={(event) => onPatchDraft({ modelBaseUrl: event.target.value })}
                placeholder={t('config:configPage.optionalOverrideTheProviderBaseURLForThisModel')}
              />
            </label>
          )}
          <div className="tag-field">
            <SettingsBadge>{draft.provider}</SettingsBadge>
            <SettingsBadge>
              {selectedModel?.reasoning
                ? t('config:configPage.reasoningSupported')
                : t('config:configPage.standardModel')}
            </SettingsBadge>
            <SettingsBadge tone="gray">
              {selectedModel?.contextWindow
                ? `${Math.round(selectedModel.contextWindow / 1000)}K context`
                : t('config:configPage.automaticContext')}
            </SettingsBadge>
            {selectedModel?.baseUrlOverride && (
              <SettingsBadge tone="amber">{t('config:configPage.customBaseURL')}</SettingsBadge>
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
            <SettingsBadge tone="blue" key={model.id}>
              {model.name} ·{' '}
              {model.kind === 'video'
                ? t('config:configPage.videoGeneration')
                : t('config:configPage.imageGenerationAndEditing')}
            </SettingsBadge>
          ))}
        </div>
      )}
    </>
  )
}
