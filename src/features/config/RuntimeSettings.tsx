// 运行时策略设置：思考等级、工具权限等全局 Agent 运行参数。
// 自包含保存：复用 PUT /api/config（携带当前默认 Provider/模型不变，
// setAsDefault=false），不再需要页面级的 Provider 草稿。
import { useEffect, useState } from 'react'
import { AlertTriangle, RefreshCw, Save, ShieldCheck } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { AppSelect } from '@/components/AppSelect'
import { SettingsCard, SettingsSectionTitle } from './settings-primitives'
import type { Notify } from '@/app/route-context'
import type { ConfigData } from './config-types'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

import { AppError, AppNotice } from '@/components/ui/app-primitives'

type RuntimePolicySettingsProps = {
  config: ConfigData
  notify: Notify
  onConfigChanged: (data: ConfigData) => void
}

export function RuntimePolicySettings({
  config,
  notify,
  onConfigChanged,
}: RuntimePolicySettingsProps) {
  const { t } = useI18n()
  const [thinkingLevel, setThinkingLevel] = useState(config.thinkingLevel)
  const [toolMode, setToolMode] = useState(config.toolMode)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')

  // 配置重载（向导/导入完成后）时同步表单。
  useEffect(() => {
    setThinkingLevel(config.thinkingLevel)
    setToolMode(config.toolMode)
  }, [config.thinkingLevel, config.toolMode])

  const defaultProviderId = config.defaultProvider || config.provider
  const defaultModel = config.defaultModel || config.model
  const defaultProvider = config.providers.find((item) => item.id === defaultProviderId)
  // 保存需要借助一个已配置 Provider 的上下文（saveConfig 校验认证）；
  // 尚未配置任何 Provider 时禁用保存。
  const canSave = Boolean(defaultProvider?.configured && defaultModel)
  const dirty = thinkingLevel !== config.thinkingLevel || toolMode !== config.toolMode

  const save = async () => {
    if (!defaultProvider || !defaultModel) return
    setSaving(true)
    setError('')
    try {
      const saved = await apiJson<ConfigData>('/api/config', {
        method: 'PUT',
        body: JSON.stringify({
          provider: defaultProvider.id,
          providerType: defaultProvider.type,
          api: defaultProvider.api,
          baseUrl: defaultProvider.baseUrl || '',
          organization: defaultProvider.organization || '',
          model: defaultModel,
          thinkingLevel,
          toolMode,
          // 仅更新策略字段：默认 Provider/模型保持不变，也不改动启用状态。
          setAsDefault: false,
        }),
      })
      onConfigChanged(saved)
      notify(t('config:configPage.runtimePolicySaved'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  return (
    <SettingsCard>
      <div className="flex items-center justify-between gap-[8px]">
        <SettingsSectionTitle title={t('config:configPage.agentRuntimePolicy')} />
        <Button
          size="sm"
          className="h-[27px] px-[10px] text-[12px]"
          disabled={saving || !dirty || !canSave}
          onClick={() => void save()}
        >
          {saving ? <RefreshCw className="animate-spin" size={13} /> : <Save size={13} />}
          {saving ? t('config:configPage.saving') : t('config:configPage.saveSettings')}
        </Button>
      </div>
      <FieldLabel variant="control">
        {t('config:configPage.thinkingLevel')}
        <AppSelect value={thinkingLevel} onChange={(event) => setThinkingLevel(event.target.value)}>
          {['off', 'minimal', 'low', 'medium', 'high', 'xhigh'].map((level) => (
            <option key={level}>{level}</option>
          ))}
        </AppSelect>
      </FieldLabel>
      <FieldLabel variant="control">
        {t('config:configPage.availableTools')}
        <AppSelect value={toolMode} onChange={(event) => setToolMode(event.target.value)}>
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
      {error && (
        <AppError>
          <AlertTriangle size={13} />
          {error}
        </AppError>
      )}
    </SettingsCard>
  )
}
