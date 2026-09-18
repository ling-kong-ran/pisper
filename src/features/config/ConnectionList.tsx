// 对话与视觉连接共用卡片；只有对话连接提供全局默认 Provider 操作。
import { Copy, Plus, Server, Star, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { PROVIDER_ICONS } from './provider-constants'
import {
  SettingsBadge,
  SettingsCard,
  SettingsSectionTitle,
  SettingsSwitch,
} from './settings-primitives'
import type { ProviderConfig } from './config-types'

import { Button } from '@/components/ui/button'

type ConnectionCardGridProps = {
  providers: ProviderConfig[]
  defaultProviderId?: string
  toggling: string
  settingDefault?: string
  settingModel?: string
  onConfigure: (provider: ProviderConfig) => void
  onClone: (provider: ProviderConfig) => void
  onSetDefault?: (provider: ProviderConfig) => void | Promise<void>
  onSetDefaultModel?: (provider: ProviderConfig, model: string) => void | Promise<void>
  onToggle: (provider: ProviderConfig, enabled: boolean) => void | Promise<void>
  onDelete: (provider: ProviderConfig) => void | Promise<void>
}

export function ConnectionCardGrid({
  providers,
  defaultProviderId = '',
  toggling,
  settingDefault = '',
  settingModel = '',
  onConfigure,
  onClone,
  onSetDefault,
  onSetDefaultModel,
  onToggle,
  onDelete,
}: ConnectionCardGridProps) {
  const { t } = useI18n()
  const visibleProviders = providers.filter((provider) => provider.configured || provider.custom)
  return (
    <div className="grid [grid-template-columns:repeat(auto-fill,minmax(min(230px,100%),1fr))] gap-[8px]">
      {visibleProviders.map((provider) => {
        const Icon = PROVIDER_ICONS[provider.id] || Server
        const isDefault = provider.id === defaultProviderId
        const statusText = !provider.configured
          ? provider.id === 'openai-codex'
            ? t('config:configPage.codexCLILoginRequired')
            : t('config:configPage.apiKeyRequired')
          : provider.enabled
            ? t('config:configPage.authenticationReady')
            : t('config:configPage.disabled2')
        return (
          <div
            key={provider.id}
            className="flex min-w-0 flex-col gap-[8px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[10px_11px]"
          >
            <button
              type="button"
              title={t('config:configPage.configure')}
              className="flex min-w-0 cursor-pointer items-center gap-[8px] rounded-[var(--r-sm)] border-0 bg-transparent p-0 text-left hover:text-[var(--brand-blue)] focus-visible:outline-2 focus-visible:outline-[var(--focus)]"
              onClick={() => onConfigure(provider)}
            >
              <span className="grid w-[30px] h-[30px] flex-none place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)]">
                <Icon size={16} />
              </span>
              <strong className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[13px]">
                {provider.name}
              </strong>
              {isDefault && (
                <SettingsBadge tone="green">{t('config:configPage.defaultBadge')}</SettingsBadge>
              )}
            </button>
            {onSetDefaultModel && provider.models.some((model) => model.kind === 'chat') ? (
              <label className="grid min-w-0 gap-1 text-[12px] text-[var(--text-muted)]">
                {t('config:configPage.providerDefaultModel')}
                <AppSelect
                  aria-label={`${t('config:configPage.providerDefaultModel')}: ${provider.name}`}
                  value={provider.defaultModel || ''}
                  disabled={Boolean(settingModel || settingDefault)}
                  onChange={(event) => void onSetDefaultModel(provider, event.target.value)}
                >
                  {!provider.models.some(
                    (model) => model.kind === 'chat' && model.id === provider.defaultModel,
                  ) && (
                    <option value={provider.defaultModel || ''} disabled>
                      {provider.defaultModel || t('config:configPage.selectModel')}
                    </option>
                  )}
                  {provider.models
                    .filter((model) => model.kind === 'chat')
                    .map((model) => (
                      <option key={model.id} value={model.id}>
                        {model.name}
                      </option>
                    ))}
                </AppSelect>
              </label>
            ) : provider.defaultModel ? (
              <small
                className="truncate text-[12px] text-[var(--text-muted)]"
                title={provider.defaultModel}
              >
                {t('config:configPage.providerDefaultModel')}: {provider.defaultModel}
              </small>
            ) : null}
            <div className="flex flex-wrap items-center justify-between gap-[8px]">
              <small className="text-[12px] text-[var(--text-muted)]">{statusText}</small>
              <div className="flex flex-none items-center gap-[6px]">
                {onSetDefault && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-[26px] w-[26px]"
                    title={t('config:configPage.setAsDefaultProvider')}
                    aria-label={t('config:configPage.setAsDefaultProvider')}
                    aria-pressed={isDefault}
                    disabled={
                      isDefault ||
                      !provider.configured ||
                      !provider.enabled ||
                      !provider.defaultModel ||
                      Boolean(settingDefault || settingModel)
                    }
                    onClick={() => void onSetDefault(provider)}
                  >
                    <Star size={13} />
                  </Button>
                )}
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-[26px] w-[26px]"
                  title={t('config:configPage.cloneProvider')}
                  aria-label={t('config:configPage.cloneProvider')}
                  onClick={() => onClone(provider)}
                >
                  <Copy size={13} />
                </Button>
                <SettingsSwitch
                  ariaLabel={t('config:configPage.providerEnabled', { name: provider.name })}
                  value={provider.configured && provider.enabled}
                  disabled={!provider.configured || toggling === provider.id}
                  onChange={(enabled) => onToggle(provider, enabled)}
                />
                {provider.custom && (
                  <Button
                    type="button"
                    variant="destructive"
                    size="icon"
                    className="h-[26px] w-[26px]"
                    title={t('config:configPage.deleteProvider')}
                    aria-label={t('config:configPage.deleteProvider')}
                    onClick={() => void onDelete(provider)}
                  >
                    <Trash2 size={13} />
                  </Button>
                )}
              </div>
            </div>
          </div>
        )
      })}
    </div>
  )
}

type ConnectionListProps = ConnectionCardGridProps & {
  defaultProviderId: string
  onAddCustom: () => void
}

export function ConnectionList({ onAddCustom, providers, ...gridProps }: ConnectionListProps) {
  const { t } = useI18n()
  return (
    <SettingsCard>
      <div className="flex flex-wrap items-center justify-between gap-[8px] [margin-bottom:8px]">
        <SettingsSectionTitle title={t('config:configPage.connections')} />
        <Button
          variant="ghost"
          size="sm"
          className="h-[26px] px-[8px] text-[12px] text-[var(--text-muted)]"
          onClick={onAddCustom}
        >
          <Plus size={13} />
          {t('config:configPage.addCustomConnection')}
        </Button>
      </div>
      <ConnectionCardGrid
        {...gridProps}
        providers={providers.filter((provider) => provider.type === 'chat')}
      />
    </SettingsCard>
  )
}
