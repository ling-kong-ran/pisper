// Provider 目录：按类别展示可用模型 Provider（Anthropic/OpenAI/本地等），
// 支持搜索与快捷配置入口。
import { Plus, Server } from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { cn } from '@/lib/utils'
import { PROVIDER_ICONS } from './provider-constants'
import { SettingsCard, SettingsSectionTitle, SettingsSwitch } from './settings-primitives'
import type { ProviderConfig } from './config-types'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

type ProviderConnectionsProps = {
  className?: string
  providers: ProviderConfig[]
  selectedProviderId: string
  toggling: string
  onAdd: () => void
  onSelect: (provider: ProviderConfig) => void
  onToggle: (provider: ProviderConfig, enabled: boolean) => void | Promise<void>
}

type ProviderMobilePickerProps = Pick<
  ProviderConnectionsProps,
  'providers' | 'selectedProviderId' | 'onAdd' | 'onSelect'
>

export function ProviderMobilePicker({
  providers,
  selectedProviderId,
  onAdd,
  onSelect,
}: ProviderMobilePickerProps) {
  const { t } = useI18n()

  return (
    <SettingsCard className="hidden max-[900px]:block">
      <div className="flex items-center justify-between gap-2">
        <SettingsSectionTitle title={t('config:configPage.modelService')} />
        <Button
          variant="ghost"
          size="icon"
          title={t('config:configPage.addProvider')}
          onClick={onAdd}
        >
          <Plus size={15} />
        </Button>
      </div>
      <FieldLabel variant="control" className="mt-2">
        {t('config:configPage.currentModelService')}
        <AppSelect
          className="!h-11 rounded-[var(--r-sm)] px-3 text-sm"
          value={selectedProviderId}
          onChange={(event) => {
            const provider = providers.find((item) => item.id === event.target.value)
            if (provider) onSelect(provider)
          }}
        >
          {providers.map((provider) => (
            <option value={provider.id} key={provider.id}>
              {provider.name} ·{' '}
              {provider.configured
                ? t('config:configPage.configured')
                : provider.id === 'openai-codex'
                  ? t('config:configPage.codexCLILoginRequired')
                  : t('config:configPage.apiKeyRequired')}
            </option>
          ))}
        </AppSelect>
      </FieldLabel>
    </SettingsCard>
  )
}

export function ProviderConnections({
  className,
  providers,
  selectedProviderId,
  toggling,
  onAdd,
  onSelect,
  onToggle,
}: ProviderConnectionsProps) {
  const { t } = useI18n()
  return (
    <SettingsCard
      className={cn(
        'selection-list [.config-layout_>_&]:max-h-[calc(100dvh_-_280px)] [.config-layout_>_&]:overflow-y-auto max-[900px]:[.config-layout_>_&]:max-h-[300px] max-[650px]:[.config-layout_>_&]:max-h-[240px] min-h-0 overflow-auto',
        className,
      )}
    >
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
                value={provider.configured && provider.enabled}
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
