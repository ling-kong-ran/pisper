// 连接列表：只展示已添加或已配置的对话连接；点击卡片进入三步连接向导。
// 视觉供应商不在对话连接里重复展示，统一在视觉生成专区的「视觉连接」里管理。
import { Plus, Server, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { PROVIDER_ICONS } from './provider-constants'
import {
  SettingsBadge,
  SettingsCard,
  SettingsSectionTitle,
  SettingsSwitch,
} from './settings-primitives'
import type { ProviderConfig } from './config-types'

import { Button } from '@/components/ui/button'

type ConnectionListProps = {
  providers: ProviderConfig[]
  defaultProviderId: string
  toggling: string
  onConfigure: (provider: ProviderConfig) => void
  onToggle: (provider: ProviderConfig, enabled: boolean) => void | Promise<void>
  onDelete: (provider: ProviderConfig) => void | Promise<void>
  onAddCustom: () => void
}

export function ConnectionList({
  providers,
  defaultProviderId,
  toggling,
  onConfigure,
  onToggle,
  onDelete,
  onAddCustom,
}: ConnectionListProps) {
  const { t } = useI18n()
  const visibleProviders = providers.filter((provider) => provider.configured || provider.custom)
  return (
    <SettingsCard>
      <div className="flex items-center justify-between gap-[8px] [margin-bottom:8px]">
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
      <div className="grid [grid-template-columns:repeat(auto-fill,minmax(230px,1fr))] gap-[8px]">
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
              role="button"
              tabIndex={0}
              title={t('config:configPage.configure')}
              className="flex cursor-pointer flex-col gap-[8px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] p-[10px_11px] hover:border-[var(--accent-border)] hover:bg-[var(--accent-soft)] focus-visible:outline-2 focus-visible:outline-[var(--focus)]"
              onClick={() => onConfigure(provider)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault()
                  onConfigure(provider)
                }
              }}
            >
              <div className="flex min-w-0 items-center gap-[8px]">
                <span className="grid w-[30px] h-[30px] flex-none place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)]">
                  <Icon size={16} />
                </span>
                <strong className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[13px]">
                  {provider.name}
                </strong>
                {isDefault && (
                  <SettingsBadge tone="green">{t('config:configPage.defaultBadge')}</SettingsBadge>
                )}
              </div>
              <div className="flex items-center justify-between gap-[8px]">
                <small className="text-[12px] text-[var(--text-muted)]">{statusText}</small>
                {/* 开关/删除是卡片内的独立控件，不触发卡片点击 */}
                <div
                  className="flex flex-none items-center gap-[6px]"
                  onClick={(event) => event.stopPropagation()}
                  onKeyDown={(event) => event.stopPropagation()}
                >
                  <SettingsSwitch
                    value={provider.configured && provider.enabled}
                    disabled={!provider.configured || toggling === provider.id}
                    onChange={(enabled) => onToggle(provider, enabled)}
                  />
                  {provider.custom && (
                    <Button
                      variant="destructive"
                      size="icon"
                      className="h-[26px] w-[26px]"
                      title={t('config:configPage.deleteProvider')}
                      onClick={() => onDelete(provider)}
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
    </SettingsCard>
  )
}
