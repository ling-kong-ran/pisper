// 对话与视觉连接共用卡片；只有对话连接提供全局默认 Provider 操作。
import { Copy, Plus, Server, Star, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { PROVIDER_ICONS } from './provider-constants'
import { SettingsCard, SettingsSectionTitle, SettingsSwitch } from './settings-primitives'
import type { ProviderConfig } from './config-types'

import { Badge } from '@/components/ui/badge'
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
    <div className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-3">
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
            className="flex min-w-0 flex-col gap-2 rounded-md border border-stroke bg-panel px-3 py-2.5"
          >
            <button
              type="button"
              title={t('config:configPage.configure')}
              className="flex min-h-9 min-w-0 cursor-pointer items-center gap-2.5 rounded-sm border-0 bg-transparent p-0 text-left text-content hover:text-accent-fg"
              onClick={() => onConfigure(provider)}
            >
              <span
                aria-hidden="true"
                className="grid size-7.5 flex-none place-items-center rounded-md border border-key-border bg-key text-content-muted shadow-key"
              >
                <Icon size={15} />
              </span>
              <span className="min-w-0 flex-1 truncate text-sm font-medium">{provider.name}</span>
              {isDefault && <Badge variant="accent">{t('config:configPage.defaultBadge')}</Badge>}
            </button>
            {onSetDefaultModel && provider.models.some((model) => model.kind === 'chat') ? (
              <label className="grid min-w-0 gap-1 text-xs text-content-muted">
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
                className="truncate font-mono text-xs text-content-muted"
                title={provider.defaultModel}
              >
                {t('config:configPage.providerDefaultModel')}: {provider.defaultModel}
              </small>
            ) : null}
            <div className="flex flex-wrap items-center justify-between gap-2">
              <small className="text-xs text-content-muted">{statusText}</small>
              <div className="flex flex-none items-center gap-1.5">
                {onSetDefault && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="size-7 pointer-coarse:size-11"
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
                  className="size-7 pointer-coarse:size-11"
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
                    variant="ghost"
                    size="icon"
                    className="size-7 hover:bg-danger-soft hover:text-danger pointer-coarse:size-11"
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
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <SettingsSectionTitle title={t('config:configPage.connections')} />
        <Button variant="ghost" size="sm" className="pointer-coarse:h-11" onClick={onAddCustom}>
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
