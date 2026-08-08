import { ChevronDown, KeyRound, ShieldCheck, Trash2 } from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { PROVIDER_APIS } from './provider-constants'
import {
  SettingsBadge,
  SettingsCard,
  SettingsSectionTitle,
  SettingsSwitch,
} from './settings-primitives'
import type { RefObject } from 'react'
import type { ConfigDraft, ProviderConfig, ProviderType } from './config-types'

type CredentialSettingsProps = {
  provider: ProviderConfig
  draft: ConfigDraft
  toggling: string
  apiKeyInputRef: RefObject<HTMLInputElement | null>
  onPatchDraft: (patch: Partial<ConfigDraft>) => void
  onSelectProviderType: (providerType: ProviderType) => void
  onToggleProvider: (provider: ProviderConfig, enabled: boolean) => void | Promise<void>
  onDeleteProvider: (provider: ProviderConfig) => void | Promise<void>
}

export function CredentialSettings({
  provider,
  draft,
  toggling,
  apiKeyInputRef,
  onPatchDraft,
  onSelectProviderType,
  onToggleProvider,
  onDeleteProvider,
}: CredentialSettingsProps) {
  const { t } = useI18n()
  const codexOAuth = provider.id === 'openai-codex'
  const visualOnly = draft.providerType === 'visual'

  return (
    <SettingsCard>
      <div className="card-head">
        <div>
          <h2>{provider.name}</h2>
          <p>
            {provider.id} · {provider.api} ·{' '}
            {t(
              'config:configPage.authenticationAndEndpointsAreStoredIndependentlyForEachConnection',
            )}
          </p>
        </div>
        <div className="provider-header-status">
          <SettingsBadge
            tone={!provider.enabled ? 'gray' : provider.configured ? 'green' : 'amber'}
          >
            {!provider.enabled
              ? t('config:configPage.disabled2')
              : provider.configured
                ? t('config:configPage.authenticationReady')
                : codexOAuth
                  ? t('config:configPage.codexCLILoginRequired')
                  : t('config:configPage.apiKeyRequired')}
          </SettingsBadge>
          <SettingsSwitch
            value={provider.enabled}
            disabled={!provider.configured || toggling === provider.id}
            onChange={(enabled) => onToggleProvider(provider, enabled)}
          />
          {provider.custom && (
            <button
              className="icon-button danger"
              title={t('config:configPage.deleteProvider')}
              onClick={() => onDeleteProvider(provider)}
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
            onChange={(event) => onSelectProviderType(event.target.value as ProviderType)}
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
              {provider.configured
                ? t('config:configPage.chatGPTOAuthConnected')
                : t('config:configPage.codexCLILoginRequired')}
            </strong>
            <small>
              {provider.configured
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
          <SettingsSectionTitle title={t('config:configPage.authentication')} />
          <label className="field-label">
            {t('config:configPage.apiProtocol')}
            <span className="select-wrap">
              <AppSelect
                value={draft.api}
                onChange={(event) => onPatchDraft({ api: event.target.value })}
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
            API Key
            <span className="input-wrap">
              <input
                ref={apiKeyInputRef}
                type="password"
                name={`provider-api-key-${provider.id}`}
                autoComplete="new-password"
                value={draft.apiKey}
                onChange={(event) => onPatchDraft({ apiKey: event.currentTarget.value })}
                onInput={(event) => {
                  const apiKey = event.currentTarget.value
                  if (draft.apiKey !== apiKey) onPatchDraft({ apiKey })
                }}
                placeholder={
                  provider.configured
                    ? t('config:configPage.configuredLeaveBlankToKeepTheExistingKey')
                    : t('config:configPage.enterTheProviderAPIKey')
                }
              />
              <KeyRound size={14} />
            </span>
          </label>
          <SettingsSectionTitle title={t('config:configPage.endpoint')} />
          <label className="field-label">
            Provider Base URL
            <input
              value={draft.baseUrl}
              onChange={(event) => onPatchDraft({ baseUrl: event.target.value })}
              placeholder={t('config:configPage.defaultEndpointForModelsInThisConnection')}
            />
          </label>
          <label className="field-label">
            Organization
            <input
              value={draft.organization}
              onChange={(event) => onPatchDraft({ organization: event.target.value })}
              placeholder={t('config:configPage.optionalUsedOnlyForOpenAIOrganization')}
            />
          </label>
        </>
      )}
    </SettingsCard>
  )
}
