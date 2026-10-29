// 运行时策略设置：思考等级、工具权限等全局 Agent 运行参数。
// 自包含保存：复用 PUT /api/config（携带当前默认 Provider/模型不变，
// setAsDefault=false），不再需要页面级的 Provider 草稿。
import { useEffect, useId, useState } from 'react'
import { AlertTriangle, Save, ShieldCheck } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { AppSelect } from '@/components/AppSelect'
import { SettingsCard, SettingsField, SettingsSectionTitle } from './settings-primitives'
import type { Notify } from '@/app/route-context'
import type { ConfigData } from './config-types'

import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'

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
  const fieldId = useId()

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
    <SettingsCard className="@container/card p-0 py-0">
      <div className="flex items-center justify-between gap-2 border-b border-stroke-soft px-4 py-3">
        <SettingsSectionTitle
          title={t('config:configPage.agentRuntimePolicy')}
          className="text-[15px] leading-6 font-medium text-content"
        />
        <Button
          size="sm"
          className="pointer-coarse:h-11"
          disabled={saving || !dirty || !canSave}
          onClick={() => void save()}
        >
          {saving ? <OrbitSpinner className="size-3.5 text-current" /> : <Save size={13} />}
          {saving ? t('config:configPage.saving') : t('config:configPage.saveSettings')}
        </Button>
      </div>
      <div className="divide-y divide-stroke-soft px-4 py-1">
        <SettingsField label={t('config:configPage.thinkingLevel')} labelId={`${fieldId}-thinking`}>
          <AppSelect
            aria-labelledby={`${fieldId}-thinking`}
            className="h-8.5 font-mono pointer-coarse:min-h-11 max-[650px]:min-h-11"
            value={thinkingLevel}
            onChange={(event) => setThinkingLevel(event.target.value)}
          >
            {['off', 'minimal', 'low', 'medium', 'high', 'xhigh'].map((level) => (
              <option key={level}>{level}</option>
            ))}
          </AppSelect>
        </SettingsField>
        <SettingsField label={t('config:configPage.availableTools')} labelId={`${fieldId}-tools`}>
          <AppSelect
            aria-labelledby={`${fieldId}-tools`}
            className="h-8.5 pointer-coarse:min-h-11 max-[650px]:min-h-11"
            value={toolMode}
            onChange={(event) => setToolMode(event.target.value)}
          >
            <option value="full">{t('config:configPage.fullToolAccess')}</option>
            <option value="custom">
              {t('config:configPage.customManageEachToolOnThePluginsPage')}
            </option>
          </AppSelect>
        </SettingsField>
      </div>
      <div className="px-4 pb-4">
        <AppNotice className="mt-1">
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
      </div>
    </SettingsCard>
  )
}
