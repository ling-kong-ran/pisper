// 凭据管理：API 密钥的展示/编辑/删除，密钥脱敏显示。
import { KeyRound, ShieldCheck, Trash2 } from 'lucide-react'
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

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

import { AppCardHeader } from '@/components/ui/app-primitives'

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
      <AppCardHeader>
        <div>
          <h2>{provider.name}</h2>
          <p>
            {provider.id} · {provider.api} ·{' '}
            {t(
              'config:configPage.authenticationAndEndpointsAreStoredIndependentlyForEachConnection',
            )}
          </p>
        </div>
        <div className="provider-header-status flex items-center justify-between gap-[8px] flex-none">
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
            <Button
              variant="destructive"
              size="icon"
              title={t('config:configPage.deleteProvider')}
              onClick={() => onDeleteProvider(provider)}
            >
              <Trash2 size={14} />
            </Button>
          )}
        </div>
      </AppCardHeader>
      <FieldLabel variant="control">
        {t('config:configPage.providerPurpose')}
        <AppSelect
          value={draft.providerType}
          onChange={(event) => onSelectProviderType(event.target.value as ProviderType)}
        >
          <option value="chat">{t('config:configPage.chatProvider')}</option>
          <option value="visual">{t('config:configPage.visualProvider')}</option>
        </AppSelect>
        <small>
          {visualOnly
            ? t(
                'config:configPage.usedOnlyForImageGenerationVideoGenerationAndImageEditingChatModelsAreIgnored',
              )
            : t('config:configPage.usedForAgentChatAndMayAlsoIncludeVisualModels')}
        </small>
      </FieldLabel>
      {codexOAuth ? (
        <div className="oauth-provider-note [&_>_span]:flex [&_>_span]:min-w-0 [&_>_span]:flex-col [&_>_span]:gap-[3px] [&_strong]:text-[var(--text)] [&_strong]:text-[12px] [&_small]:text-[var(--text-muted)] [&_small]:text-[12px] [&_small]:leading-[1.45] flex items-start gap-[9px] [margin-top:14px] [border:1px_solid_var(--star-border)] rounded-[var(--r-sm)] bg-[var(--star-soft)] [padding:10px_11px] text-[var(--star-strong)]">
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
          <FieldLabel variant="control">
            {t('config:configPage.apiProtocol')}
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
          </FieldLabel>
          <FieldLabel variant="control">
            API Key
            <span className="input-wrap [&_button]:absolute [&_button]:right-[3px] [&_button]:top-[3px] [&_button]:grid [&_button]:w-[32px] [&_button]:h-[32px] [&_button]:place-items-center [&_button]:border-0 [&_button]:bg-transparent [&_button]:text-[var(--text-muted)] [&_>_svg]:absolute [&_>_svg]:right-[9px] [&_>_svg]:top-[8px] [&_>_svg]:text-[var(--text-muted)] relative flex">
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
          </FieldLabel>
          <SettingsSectionTitle title={t('config:configPage.endpoint')} />
          <FieldLabel variant="control">
            Provider Base URL
            <input
              value={draft.baseUrl}
              onChange={(event) => onPatchDraft({ baseUrl: event.target.value })}
              placeholder={t('config:configPage.defaultEndpointForModelsInThisConnection')}
            />
          </FieldLabel>
          <FieldLabel variant="control">
            Organization
            <input
              value={draft.organization}
              onChange={(event) => onPatchDraft({ organization: event.target.value })}
              placeholder={t('config:configPage.optionalUsedOnlyForOpenAIOrganization')}
            />
          </FieldLabel>
        </>
      )}
    </SettingsCard>
  )
}
