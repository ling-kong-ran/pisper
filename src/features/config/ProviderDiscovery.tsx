// Provider 发现：扫描本地可导入的 Provider（Ollama 等）并展示导入入口。
import { useState } from 'react'
import { AlertTriangle, Bot, Brain, ChevronDown, Download, RefreshCw, Server } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  providerDiscoveryShouldCollapse,
  providerDiscoveryShouldRender,
} from './provider-discovery-state'
import { SettingsBadge, SettingsCard } from './settings-primitives'
import type { DiscoveredProvider, DiscoveryData, DiscoveryError, Translate } from './config-types'

import { Button } from '@/components/ui/button'

type ProviderDiscoveryProps = {
  discovery: DiscoveryData
  discovering: boolean
  error: string
  importing: string
  onRefresh: () => void | Promise<void>
  onImport: (provider: DiscoveredProvider) => void | Promise<void>
}

function discoverySourceLabel(provider: Pick<DiscoveredProvider, 'source'>) {
  if (provider.source === 'codex-config') return 'Codex config.toml'
  if (provider.source === 'claude-config') return 'Claude settings.json'
  if (provider.source === 'codex-auth') return 'Codex login'
  return 'Claude login'
}

function discoveryAuthLabel(provider: DiscoveredProvider, t: Translate) {
  if (provider.authType === 'environment')
    return t('config:configPage.keyVariableName', { name: provider.authVariable })
  if (provider.authType === 'bearer' || provider.authType === 'api_key')
    return t('config:configPage.authenticationIncludedInConfiguration')
  if (provider.authType === 'oauth') return t('config:configPage.loginStateFileDetected')
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

export function ProviderDiscovery({
  discovery,
  discovering,
  error,
  importing,
  onRefresh,
  onImport,
}: ProviderDiscoveryProps) {
  const { t } = useI18n()
  const [collapsed, setCollapsed] = useState<boolean | null>(null)
  const providers = discovery.providers || []
  const errors = discovery.errors || []
  if (!providerDiscoveryShouldRender(discovery, discovering, error)) return null
  const isCollapsed = collapsed ?? providerDiscoveryShouldCollapse(discovery, discovering)
  const handleRefresh = () => {
    setCollapsed(null)
    return onRefresh()
  }
  const handleImport = (provider: DiscoveredProvider) => {
    setCollapsed(null)
    return onImport(provider)
  }
  const errorLabel = (item: DiscoveryError) =>
    ['invalid_json', 'invalid_toml'].includes(item.code)
      ? t('config:configPage.invalidConfigurationFileFormat')
      : item.code === 'unsupported_config'
        ? t('config:configPage.noImportableProviderConfigurationWasFound')
        : item.code === 'invalid_login_state'
          ? t('config:configPage.incompleteLoginState')
          : item.code === 'file_too_large'
            ? t('config:configPage.configurationFileIsTooLarge')
            : t('config:configPage.unableToReadTheConfigurationFile')

  return (
    <SettingsCard className="[margin-bottom:12px] [padding:12px_14px]">
      <div className="provider-discovery-head [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[12px] [&_small]:leading-[1.45] max-[650px]:flex-wrap flex items-center gap-[10px]">
        <button
          type="button"
          className="provider-discovery-toggle [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-1 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[3px] flex min-w-0 flex-1 items-center gap-[10px] border-0 bg-transparent p-0 text-inherit text-left cursor-pointer"
          aria-expanded={!isCollapsed}
          onClick={() => setCollapsed(!isCollapsed)}
        >
          <span className="grid w-[38px] h-[38px] [flex:0_0_auto] place-items-center rounded-[11px] bg-[var(--star-soft)] text-[var(--star-strong)]">
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
          <ChevronDown
            size={15}
            className={`provider-discovery-caret [&.open]:[transform:rotate(180deg)] flex-none text-[var(--text-muted)] [transition:transform_var(--d1)_var(--ease-out)] ${isCollapsed ? '' : 'open'}`}
          />
        </button>
        <Button
          type="button"
          variant="outline"
          className="bg-surface-subtle"
          disabled={discovering || Boolean(importing)}
          onClick={handleRefresh}
        >
          {discovering ? <RefreshCw className="animate-spin" size={13} /> : <RefreshCw size={13} />}
          {t('config:configPage.rescan')}
        </Button>
      </div>
      {!isCollapsed && (
        <>
          {discovering && !providers.length ? (
            <div className="flex min-h-[48px] items-center justify-center gap-[7px] [margin-top:9px] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] text-[var(--text-muted)] text-[12px]">
              <RefreshCw className="animate-spin" size={15} />
              {t('config:configPage.scanningProviderConfigurationFiles')}
            </div>
          ) : providers.length ? (
            <div className="provider-discovery-list max-[650px]:grid-cols-[1fr] grid grid-cols-[repeat(2,minmax(0,1fr))] gap-[8px] [margin-top:11px]">
              {providers.map((provider) => {
                const source = discoverySourceLabel(provider)
                const busy = importing === provider.id
                const Icon = provider.source.startsWith('claude-') ? Brain : Bot
                const modelSummary =
                  provider.kind === 'authentication'
                    ? t('config:configPage.officialProviderAuthenticationOnly')
                    : provider.models?.length
                      ? provider.models.map((model) => model.id).join(', ')
                      : t('config:configPage.noModelSpecified')
                return (
                  <div
                    className={`provider-discovery-card grid min-w-0 min-h-[68px] grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[9px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] [padding:8px_9px] ${provider.imported ? 'configured [.provider-discovery-card&]:border-[var(--success-border)] [.provider-discovery-card&]:bg-[var(--success-soft)]' : ''}`}
                    key={provider.id}
                  >
                    <span
                      className={`provider-discovery-icon [&.source-claude-config]:bg-[var(--brand-blue-soft)] [&.source-claude-config]:text-[var(--brand-blue-strong)] grid w-[32px] h-[32px] place-items-center rounded-[var(--r-sm)] bg-[var(--star-soft)] text-[var(--star-strong)] source-${provider.source}`}
                    >
                      <Icon size={17} />
                    </span>
                    <span className="provider-discovery-copy [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_strong]:text-[12px] [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] flex min-w-0 flex-col gap-[3px]">
                      <strong>
                        {source} · {provider.providerName}
                      </strong>
                      <small>
                        {provider.api} · {modelSummary}
                      </small>
                      <small>
                        {provider.kind === 'authentication'
                          ? t('config:configPage.officialEndpointOnly')
                          : provider.baseUrl || t('config:configPage.noBaseURLSpecified')}{' '}
                        · {discoveryAuthLabel(provider, t)} · {provider.location}
                      </small>
                    </span>
                    <span className="flex flex-none items-center">
                      {provider.imported ? (
                        <SettingsBadge tone="green">{t('config:configPage.loaded')}</SettingsBadge>
                      ) : provider.conflict ? (
                        <SettingsBadge tone="amber">
                          {t('config:configPage.conflictDetected')}
                        </SettingsBadge>
                      ) : provider.importable ? (
                        <Button
                          type="button"
                          disabled={busy || Boolean(importing)}
                          onClick={() => handleImport(provider)}
                        >
                          {busy ? (
                            <RefreshCw className="animate-spin" size={12} />
                          ) : (
                            <Download size={12} />
                          )}
                          {busy
                            ? t('config:configPage.loading')
                            : provider.kind === 'authentication'
                              ? t('config:configPage.loadLoginState')
                              : t('config:configPage.loadConfiguration')}
                        </Button>
                      ) : (
                        <SettingsBadge tone="gray">
                          {t('config:configPage.cannotLoad')}
                        </SettingsBadge>
                      )}
                    </span>
                  </div>
                )
              })}
            </div>
          ) : (
            <div className="flex min-h-[48px] items-center justify-center gap-[7px] [margin-top:9px] rounded-[var(--r-sm)] bg-[var(--surface-subtle)] text-[var(--text-muted)] text-[12px]">
              <Server size={15} />
              {t('config:configPage.noImportableCodexOrClaudeProviderConfigurationWasDetected')}
            </div>
          )}
          {(error ||
            errors.length > 0 ||
            providers.some((provider) =>
              provider.warnings?.some((warning) => warning.code !== 'login_auth_not_imported'),
            )) && (
            <div
              className="provider-discovery-errors [&_span]:flex [&_span]:items-center [&_span]:gap-[5px] flex flex-col gap-[4px] [margin-top:8px] text-[var(--danger)] text-[11px]"
              aria-live="polite"
            >
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
        </>
      )}
    </SettingsCard>
  )
}
