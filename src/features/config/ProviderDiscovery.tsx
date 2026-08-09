import { useState } from 'react'
import { AlertTriangle, Bot, Brain, ChevronDown, Download, RefreshCw, Server } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  providerDiscoveryShouldCollapse,
  providerDiscoveryShouldRender,
} from './provider-discovery-state'
import { SettingsBadge, SettingsCard } from './settings-primitives'
import type { DiscoveredProvider, DiscoveryData, DiscoveryError, Translate } from './config-types'

type ProviderDiscoveryProps = {
  discovery: DiscoveryData
  discovering: boolean
  error: string
  importing: string
  onRefresh: () => void | Promise<void>
  onImport: (provider: DiscoveredProvider) => void | Promise<void>
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
        : item.code === 'file_too_large'
          ? t('config:configPage.configurationFileIsTooLarge')
          : t('config:configPage.unableToReadTheConfigurationFile')

  return (
    <SettingsCard className="provider-discovery-panel">
      <div className="provider-discovery-head">
        <button
          type="button"
          className="provider-discovery-toggle"
          aria-expanded={!isCollapsed}
          onClick={() => setCollapsed(!isCollapsed)}
        >
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
          <ChevronDown
            size={15}
            className={`provider-discovery-caret ${isCollapsed ? '' : 'open'}`}
          />
        </button>
        <button
          type="button"
          className="button secondary tiny"
          disabled={discovering || Boolean(importing)}
          onClick={handleRefresh}
        >
          {discovering ? <RefreshCw className="spin" size={13} /> : <RefreshCw size={13} />}
          {t('config:configPage.rescan')}
        </button>
      </div>
      {!isCollapsed && (
        <>
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
                        <SettingsBadge tone="green">{t('config:configPage.loaded')}</SettingsBadge>
                      ) : provider.conflict ? (
                        <SettingsBadge tone="amber">
                          {t('config:configPage.conflictDetected')}
                        </SettingsBadge>
                      ) : provider.importable ? (
                        <button
                          type="button"
                          className="button primary tiny"
                          disabled={busy || Boolean(importing)}
                          onClick={() => handleImport(provider)}
                        >
                          {busy ? <RefreshCw className="spin" size={12} /> : <Download size={12} />}
                          {busy
                            ? t('config:configPage.loading')
                            : t('config:configPage.loadConfiguration')}
                        </button>
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
        </>
      )}
    </SettingsCard>
  )
}
