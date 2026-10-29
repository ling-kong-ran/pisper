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
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'

type ProviderDiscoveryProps = {
  discovery: DiscoveryData
  discovering: boolean
  error: string
  importing: string
  forceVisible?: boolean
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
  forceVisible = false,
  onRefresh,
  onImport,
}: ProviderDiscoveryProps) {
  const { t } = useI18n()
  const [collapsed, setCollapsed] = useState<boolean | null>(null)
  const providers = discovery.providers || []
  const errors = discovery.errors || []
  if (!forceVisible && !providerDiscoveryShouldRender(discovery, discovering, error)) return null
  const isCollapsed =
    collapsed ?? (forceVisible ? false : providerDiscoveryShouldCollapse(discovery, discovering))
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
    <SettingsCard className="mb-3 p-4 py-3.5">
      <div className="flex items-center gap-2.5 max-[650px]:flex-wrap">
        <button
          type="button"
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-3 rounded-sm border-0 bg-transparent p-0 text-left max-[650px]:basis-full"
          aria-expanded={!isCollapsed}
          onClick={() => setCollapsed(!isCollapsed)}
        >
          <span
            aria-hidden="true"
            className="grid size-9.5 shrink-0 place-items-center rounded-lg border border-key-border bg-key text-content-muted shadow-key"
          >
            <Server size={18} />
          </span>
          <span className="flex min-w-0 flex-1 flex-col gap-0.5">
            <span className="text-sm font-medium text-content">
              {t('config:configPage.localProviderConfiguration')}
            </span>
            <span className="text-xs leading-[1.45] text-content-muted">
              {t(
                'config:configPage.readProviderEndpointModelAndAuthenticationFieldsFromCodexConfigTomlAndClaudeSettingsJson',
              )}
            </span>
          </span>
          <ChevronDown
            size={16}
            aria-hidden="true"
            className={cn(
              'flex-none text-content-muted transition-transform',
              !isCollapsed && 'rotate-180',
            )}
          />
        </button>
        <Button
          type="button"
          variant="secondary"
          className="max-[650px]:h-11"
          disabled={discovering || Boolean(importing)}
          onClick={handleRefresh}
        >
          {discovering ? <OrbitSpinner className="text-current" /> : <RefreshCw size={13} />}
          {t('config:configPage.rescan')}
        </Button>
      </div>
      {!isCollapsed && (
        <>
          {discovering && !providers.length ? (
            <div
              role="status"
              className="mt-3 flex min-h-12 items-center justify-center gap-2 rounded-sm bg-surface-subtle text-xs text-content-muted"
            >
              <OrbitSpinner />
              {t('config:configPage.scanningProviderConfigurationFiles')}
            </div>
          ) : providers.length ? (
            <div className="mt-3 grid grid-cols-2 gap-2 max-[650px]:grid-cols-1">
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
                    className={cn(
                      'grid min-h-17 min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2.5 rounded-md border px-3 py-2 max-[650px]:grid-cols-[auto_minmax(0,1fr)]',
                      provider.imported
                        ? 'border-success-border bg-success-soft'
                        : 'border-stroke bg-panel',
                    )}
                    key={provider.id}
                  >
                    <span
                      aria-hidden="true"
                      className="grid size-7.5 place-items-center rounded-md border border-key-border bg-key text-content-muted shadow-key"
                    >
                      <Icon size={15} />
                    </span>
                    <span className="flex min-w-0 flex-col gap-0.5">
                      <span className="truncate text-sm font-medium text-content">
                        {source} · {provider.providerName}
                      </span>
                      <span className="truncate font-mono text-xs text-content-muted">
                        {provider.api} · {modelSummary}
                      </span>
                      <span className="truncate text-xs text-content-muted">
                        {provider.kind === 'authentication'
                          ? t('config:configPage.officialEndpointOnly')
                          : provider.baseUrl || t('config:configPage.noBaseURLSpecified')}{' '}
                        · {discoveryAuthLabel(provider, t)} · {provider.location}
                      </span>
                    </span>
                    <span className="flex min-w-0 flex-none items-center max-[650px]:col-start-2">
                      {provider.imported ? (
                        <SettingsBadge tone="green">{t('config:configPage.loaded')}</SettingsBadge>
                      ) : provider.conflict ? (
                        <SettingsBadge tone="amber">
                          {t('config:configPage.conflictDetected')}
                        </SettingsBadge>
                      ) : provider.importable ? (
                        <Button
                          type="button"
                          size="sm"
                          className="pointer-coarse:h-11"
                          disabled={busy || Boolean(importing)}
                          onClick={() => handleImport(provider)}
                        >
                          {busy ? (
                            <OrbitSpinner className="size-3 text-current" />
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
            <div className="mt-3 flex min-h-12 items-center justify-center gap-2 rounded-sm bg-surface-subtle px-3 text-center text-xs text-content-muted">
              <Server size={15} aria-hidden="true" />
              {t('config:configPage.noImportableCodexOrClaudeProviderConfigurationWasDetected')}
            </div>
          )}
          {(error ||
            errors.length > 0 ||
            providers.some((provider) =>
              provider.warnings?.some((warning) => warning.code !== 'login_auth_not_imported'),
            )) && (
            <div
              className="mt-2 flex flex-col gap-1 text-xs text-danger [&_span]:flex [&_span]:items-center [&_span]:gap-1.5"
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
