// 运行时策略设置：思考等级、工具权限等全局 Agent 运行参数。
import { ShieldCheck } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import { SettingsCard, SettingsSectionTitle } from './settings-primitives'
import type { ConfigDraft } from './config-types'

import { FieldLabel } from '@/components/ui/field'

import { AppNotice } from '@/components/ui/app-primitives'

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
