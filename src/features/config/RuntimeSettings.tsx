// 运行时设置：服务端口、数据目录、启动行为等运行时参数。
import {
  AlertTriangle,
  CheckCircle2,
  CircleAlert,
  RefreshCw,
  Save,
  ShieldCheck,
} from 'lucide-react'
import { APP_NAME } from '@/app/brand'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { SettingsCard, SettingsSectionTitle } from './settings-primitives'
import type { ConfigDraft, ProviderConfig } from './config-types'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

import { AppError, AppNotice } from '@/components/ui/app-primitives'

type RuntimeSettingsProps = {
  draft: ConfigDraft
  onPatchDraft: (patch: Partial<ConfigDraft>) => void
}

function RuntimeCompactionSettings({ draft, onPatchDraft }: RuntimeSettingsProps) {
  const { t } = useI18n()
  return (
    <FieldLabel variant="control">
      {t('config:configPage.thinkingLevel')}
      <AppSelect
        value={draft.thinkingLevel}
        onChange={(event) => onPatchDraft({ thinkingLevel: event.target.value })}
      >
        {['off', 'minimal', 'low', 'medium', 'high', 'xhigh'].map((level) => (
          <option key={level}>{level}</option>
        ))}
      </AppSelect>
    </FieldLabel>
  )
}

function ToolPermissionSettings({ draft, onPatchDraft }: RuntimeSettingsProps) {
  const { t } = useI18n()
  return (
    <>
      <FieldLabel variant="control">
        {t('config:configPage.availableTools')}
        <AppSelect
          value={draft.toolMode}
          onChange={(event) => onPatchDraft({ toolMode: event.target.value })}
        >
          <option value="read-only">{t('config:configPage.readOnlyInspectionTools')}</option>
          <option value="workspace">{t('config:configPage.fileEditingTools')}</option>
          <option value="full">{t('config:configPage.fullToolAccess')}</option>
          <option value="custom">
            {t('config:configPage.customManageEachToolOnThePluginsPage')}
          </option>
        </AppSelect>
      </FieldLabel>
      <AppNotice>
        <ShieldCheck size={16} />
        <span>
          <strong>{t('config:configPage.permissionsAreEnforcedByTheRuntime')}</strong>
          <small>
            {t(
              'config:configPage.afterSavingExistingRuntimesAreReleasedAndNewChatsUseTheLatestPolicy',
            )}
          </small>
        </span>
      </AppNotice>
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

type ProviderSettingsActionsProps = {
  provider: ProviderConfig
  visualOnly: boolean
  codexOAuth: boolean
  saving: boolean
  dirty: boolean
  error: string
  hasModel: boolean
  isDefault: boolean
  onSave: (setAsDefault?: boolean) => void | Promise<void>
}

export function ProviderSettingsActions({
  provider,
  visualOnly,
  codexOAuth,
  saving,
  dirty,
  error,
  hasModel,
  isDefault,
  onSave,
}: ProviderSettingsActionsProps) {
  const { t } = useI18n()
  return (
    <div
      className="provider-save-bar [&[data-dirty]]:border-[var(--star-border)] max-[650px]:grid-cols-[minmax(0,1fr)] sticky z-[6] top-0 grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-[10px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] bg-[var(--panel)] [padding:9px_10px] shadow-[var(--sh-surface)]"
      data-dirty={dirty || undefined}
    >
      <div className="provider-save-state [.provider-save-bar[data-dirty]_&]:text-[var(--star-strong)] [&_>_span]:flex [&_>_span]:min-w-0 [&_>_span]:flex-col [&_>_span]:gap-[2px] [&_strong]:text-[var(--text)] [&_strong]:text-[12px] [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap flex min-w-0 items-center gap-[8px] text-[var(--success)]">
        {dirty ? <CircleAlert size={17} /> : <CheckCircle2 size={17} />}
        <span>
          <strong>
            {dirty ? t('config:configPage.unsavedChanges') : t('config:configPage.allChangesSaved')}
          </strong>
          <small>{provider.name}</small>
        </span>
      </div>
      <div className="provider-save-actions max-[650px]:[justify-content:stretch] flex min-w-0 items-center justify-end flex-wrap gap-[7px]">
        {!visualOnly && hasModel && (
          <Button
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            disabled={saving || !dirty || isDefault || (codexOAuth && !provider.configured)}
            onClick={() => onSave(true)}
          >
            <ShieldCheck size={14} />
            {isDefault
              ? t('config:configPage.currentDefaultProvider')
              : t('config:configPage.setAsDefaultProvider')}
          </Button>
        )}
        <Button
          size="lg"
          disabled={saving || !dirty || (codexOAuth && !provider.configured)}
          onClick={() => onSave(false)}
        >
          {saving ? <RefreshCw className="animate-spin" size={14} /> : <Save size={14} />}
          {saving
            ? t('config:configPage.saving')
            : codexOAuth && !provider.configured
              ? t('config:configPage.loadAuthenticationToSave')
              : visualOnly
                ? t('config:configPage.saveVisualModelSettings')
                : t('config:configPage.saveProviderSettings')}
        </Button>
      </div>
      {error && (
        <AppError>
          <AlertTriangle size={13} />
          {error}
        </AppError>
      )}
    </div>
  )
}

type RuntimeStatusProps = {
  provider: ProviderConfig
}

export function RuntimeStatus({ provider }: RuntimeStatusProps) {
  const { t } = useI18n()
  return (
    <SettingsCard className="usage-card [&_>_small]:block [&_>_small]:m-[4px_0_12px] [&_>_small]:text-[var(--text-muted)] [&_>_small]:text-[13px]">
      <SettingsSectionTitle title={t('config:configPage.runtimeStatus')} />
      <div className="usage-number [&_span]:text-[var(--text-muted)] [&_span]:text-[12px] [&_strong]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_strong]:text-[15px] flex [align-items:baseline] justify-between [margin-top:10px]">
        <span>Engine</span>
        <strong>{APP_NAME} Runtime</strong>
      </div>
      <div className="usage-number [&_span]:text-[var(--text-muted)] [&_span]:text-[12px] [&_strong]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_strong]:text-[15px] flex [align-items:baseline] justify-between [margin-top:10px]">
        <span>Provider</span>
        <strong>{provider.name}</strong>
      </div>
      <div className="usage-number [&_span]:text-[var(--text-muted)] [&_span]:text-[12px] [&_strong]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_strong]:text-[15px] flex [align-items:baseline] justify-between [margin-top:10px]">
        <span>Models</span>
        <strong>{provider.models.length}</strong>
      </div>
      <div className="usage-number [&_span]:text-[var(--text-muted)] [&_span]:text-[12px] [&_strong]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_strong]:text-[15px] flex [align-items:baseline] justify-between [margin-top:10px]">
        <span>{t('config:configPage.status')}</span>
        <strong>
          {provider.enabled ? t('config:configPage.enabled') : t('config:configPage.disabled')}
        </strong>
      </div>
    </SettingsCard>
  )
}
