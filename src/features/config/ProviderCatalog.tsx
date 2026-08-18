// Provider 目录：按类别展示可用模型 Provider（Anthropic/OpenAI/本地等），
// 支持搜索与快捷配置入口。
import {
  AlertTriangle,
  Bot,
  Brain,
  Code2,
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

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

import { AppNotice } from '@/components/ui/app-primitives'

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
    <SettingsCard className="selection-list [.config-layout_>_&]:max-h-[calc(100dvh_-_280px)] [.config-layout_>_&]:overflow-y-auto max-[900px]:max-h-[300px] min-h-0 overflow-auto">
      <div className="provider-list-heading flex items-center justify-between gap-[8px] [margin-bottom:4px]">
        <SettingsSectionTitle title={t('config:configPage.providerConnections')} />
        <Button
          variant="ghost"
          size="icon"
          title={t('config:configPage.addProvider')}
          onClick={onAdd}
        >
          <Plus size={15} />
        </Button>
      </div>
      {providers.map((provider) => {
        const Icon = PROVIDER_ICONS[provider.id] || Server
        return (
          <div
            className={`provider-list-item grid w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[9px] border-0 [border-top:1px_solid_var(--stroke-soft)] bg-transparent p-[10px_8px] text-left hover:rounded-[var(--r-sm)] hover:bg-[var(--accent-soft)] [&.active]:rounded-[var(--r-sm)] [&.active]:bg-[var(--accent-soft)] [&.active]:relative [&.active::before]:[content:''] [&.active::before]:absolute [&.active::before]:left-0 [&.active::before]:top-[9px] [&.active::before]:bottom-[9px] [&.active::before]:w-[3px] [&.active::before]:rounded-[var(--r-pill)] [&.active::before]:bg-[var(--brand-blue)] [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[3px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&.disabled-provider:not(.active)]:opacity-[.65] grid-cols-[minmax(0,1fr)_auto] gap-[5px] [padding:4px] ${selectedProviderId === provider.id ? 'active' : ''}    ${provider.enabled ? '' : 'disabled-provider'}`}
            key={provider.id}
          >
            <button
              className="provider-select-main [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[3px] [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap grid min-w-0 h-[48px] grid-cols-[auto_minmax(0,1fr)] items-center gap-[9px] border-0 bg-transparent [padding:4px] text-left"
              onClick={() => onSelect(provider)}
            >
              <span className="list-icon [.chat-resource-list_&]:grid [.chat-resource-list_&]:w-[28px] [.chat-resource-list_&]:h-[28px] [.chat-resource-list_&]:place-items-center [.chat-resource-list_&]:rounded-[var(--r-sm)] [.chat-resource-list_&]:bg-[var(--surface-subtle)] [.chat-resource-list_&]:text-[var(--star-strong)] [.session-workflow-summary_&]:grid [.session-workflow-summary_&]:w-[28px] [.session-workflow-summary_&]:h-[28px] [.session-workflow-summary_&]:place-items-center [.session-workflow-summary_&]:rounded-[var(--r-sm)] [.session-workflow-summary_&]:bg-[var(--surface-subtle)] [.session-workflow-summary_&]:text-[var(--star-strong)] grid w-[27px] h-[27px] place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)] [.workflow-template-gallery_&]:grid [.workflow-template-gallery_&]:w-[32px] [.workflow-template-gallery_&]:h-[32px] [.workflow-template-gallery_&]:place-items-center [.workflow-template-gallery_&]:rounded-[var(--r-sm)] [.workflow-template-gallery_&]:bg-[var(--surface-subtle)] [.workflow-template-gallery_&]:text-[var(--star-strong)]">
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
            <div className="flex items-center gap-[6px] [padding-right:3px]">
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
      <div className="model-config-heading flex items-center justify-between gap-[8px] [margin-top:13px] [border-top:1px_solid_var(--stroke-soft)] [padding-top:12px]">
        <SettingsSectionTitle title={t('config:configPage.models')} />
        {!codexOAuth && (
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              className="bg-surface-subtle"
              onClick={() => onOpenModelDialog('discover')}
            >
              <RefreshCw size={13} />
              {t('config:configPage.fetchModels')}
            </Button>
            <Button
              variant="outline"
              className="bg-surface-subtle"
              onClick={() => onOpenModelDialog('manual')}
            >
              <Plus size={13} />
              {t('config:configPage.addModel')}
            </Button>
          </div>
        )}
      </div>
      {!visualOnly && chatModels.length > 0 ? (
        <>
          <FieldLabel variant="control">
            {t('config:configPage.defaultChatModel')}
            <AppSelect value={draft.model} onChange={(event) => onSelectModel(event.target.value)}>
              {chatModels.map((model) => (
                <option key={model.id} value={model.id}>
                  {model.name}
                </option>
              ))}
            </AppSelect>
          </FieldLabel>
          {!codexOAuth && (
            <FieldLabel variant="control">
              {t('config:configPage.modelBaseURL')}
              <input
                value={draft.modelBaseUrl}
                onChange={(event) => onPatchDraft({ modelBaseUrl: event.target.value })}
                placeholder={t('config:configPage.optionalOverrideTheProviderBaseURLForThisModel')}
              />
            </FieldLabel>
          )}
          <div className="tag-field [&_>_span:first-child]:mr-[3px] [&_>_span:first-child]:text-[var(--text-secondary)] [&_>_span:first-child]:text-[12px] [&_>_span:first-child]:font-[600] flex items-center flex-wrap gap-[6px] [margin-top:10px]">
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
        <AppNotice>
          <Sparkles size={16} />
          <span>
            <strong>{t('config:configPage.visualOnlyProvider')}</strong>
            <small>
              {t(
                'config:configPage.usedOnlyForImageGenerationVideoGenerationAndImageEditingAndExcludedFromAgentChatModelSelection',
              )}
            </small>
          </span>
        </AppNotice>
      ) : (
        <AppNotice>
          <AlertTriangle size={16} />
          <span>
            <strong>{t('config:configPage.noChatModelAvailable')}</strong>
            <small>{t('config:configPage.fetchOrAddAChatModel')}</small>
          </span>
        </AppNotice>
      )}
      {visualModels.length > 0 && (
        <div className="visual-model-list [&_>_span]:mr-[3px] [&_>_span]:text-[var(--text-muted)] [&_>_span]:text-[12px] [&_>_span]:font-[700] flex flex-wrap items-center gap-[7px] [margin-top:12px] [border-top:1px_solid_var(--stroke-soft)] [padding-top:12px]">
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
