import { AlertTriangle, ChevronDown, RefreshCw, Save, ShieldCheck } from 'lucide-react'
import { APP_NAME } from '@/app/brand'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { SettingsCard, SettingsSectionTitle } from './settings-primitives'
import type { ConfigDraft, ProviderConfig } from './config-types'

type RuntimeSettingsProps = {
  draft: ConfigDraft
  onPatchDraft: (patch: Partial<ConfigDraft>) => void
}

function RuntimeCompactionSettings({ draft, onPatchDraft }: RuntimeSettingsProps) {
  const { t } = useI18n()
  return (
    <label className="field-label">
      {t('config:configPage.thinkingLevel')}
      <span className="select-wrap">
        <AppSelect
          value={draft.thinkingLevel}
          onChange={(event) => onPatchDraft({ thinkingLevel: event.target.value })}
        >
          {['off', 'minimal', 'low', 'medium', 'high', 'xhigh'].map((level) => (
            <option key={level}>{level}</option>
          ))}
        </AppSelect>
        <ChevronDown size={13} />
      </span>
    </label>
  )
}

function ToolPermissionSettings({ draft, onPatchDraft }: RuntimeSettingsProps) {
  const { t } = useI18n()
  return (
    <>
      <label className="field-label">
        {t('config:configPage.availableTools')}
        <span className="select-wrap">
          <AppSelect
            value={draft.toolMode}
            onChange={(event) => onPatchDraft({ toolMode: event.target.value })}
          >
            <option value="read-only">{t('config:configPage.readOnlyReadGrepFindLs')}</option>
            <option value="workspace">{t('config:configPage.workspaceAllowEditWrite')}</option>
            <option value="full">{t('config:configPage.fullAllowBash')}</option>
            <option value="custom">
              {t('config:configPage.customManageEachToolOnThePluginsPage')}
            </option>
          </AppSelect>
          <ChevronDown size={13} />
        </span>
      </label>
      <div className="permission-note">
        <ShieldCheck size={16} />
        <span>
          <strong>{t('config:configPage.permissionsAreEnforcedByTheServer')}</strong>
          <small>
            {t(
              'config:configPage.afterSavingExistingRuntimesAreReleasedAndNewChatsUseTheLatestPolicy',
            )}
          </small>
        </span>
      </div>
    </>
  )
}

export function RuntimePolicySettings(props: RuntimeSettingsProps) {
  const { t } = useI18n()
  return (
    <SettingsCard>
      <SettingsSectionTitle title={t('config:configPage.agentRuntimePolicy')} />
      <RuntimeCompactionSettings {...props} />
      <ToolPermissionSettings {...props} />
    </SettingsCard>
  )
}

type RuntimeStatusProps = {
  provider: ProviderConfig
  visualOnly: boolean
  codexOAuth: boolean
  saving: boolean
  dirty: boolean
  error: string
  hasModel: boolean
  onSave: () => void | Promise<void>
}

export function RuntimeStatus({
  provider,
  visualOnly,
  codexOAuth,
  saving,
  dirty,
  error,
  hasModel,
  onSave,
}: RuntimeStatusProps) {
  const { t } = useI18n()
  return (
    <SettingsCard className="usage-card" data-dirty={dirty || undefined}>
      <SettingsSectionTitle title={t('config:configPage.runtimeStatus')} />
      <div className="usage-number">
        <span>Engine</span>
        <strong>{APP_NAME} Runtime</strong>
      </div>
      <div className="usage-number">
        <span>Provider</span>
        <strong>{provider.name}</strong>
      </div>
      <div className="usage-number">
        <span>Models</span>
        <strong>{provider.models.length}</strong>
      </div>
      <div className="usage-number">
        <span>{t('config:configPage.status')}</span>
        <strong>
          {provider.enabled ? t('config:configPage.enabled') : t('config:configPage.disabled')}
        </strong>
      </div>
      {error && (
        <div className="config-error">
          <AlertTriangle size={13} />
          {error}
        </div>
      )}
      <button
        className="button primary wide"
        disabled={saving || !provider.enabled || (codexOAuth && !provider.configured)}
        onClick={onSave}
      >
        {saving ? <RefreshCw className="spin" size={14} /> : <Save size={14} />}
        {saving
          ? t('config:configPage.saving')
          : codexOAuth && !provider.configured
            ? t('config:configPage.loadAuthenticationToSave')
            : provider.enabled
              ? visualOnly
                ? t('config:configPage.saveVisualModelSettings')
                : hasModel
                  ? t('config:configPage.saveAndSetAsDefaultProvider')
                  : t('config:configPage.saveProviderSettings')
              : t('config:configPage.enableToSave')}
      </button>
    </SettingsCard>
  )
}
