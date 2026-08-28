// Provider 详情卡：凭据、对话模型、视觉模型、高级设置与保存动作合并在一张卡内，
// 替代原先「凭据卡 + 保存条 + 模型目录卡 + 状态卡」的多卡堆叠。
// 普通用户只需要面对 API Key + 模型选择；协议/端点/组织等收进「高级设置」。
import { useState } from 'react'
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  CircleAlert,
  KeyRound,
  Plus,
  RefreshCw,
  Save,
  Server,
  ShieldCheck,
  Trash2,
} from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { PROVIDER_APIS, PROVIDER_ICONS } from './provider-constants'
import { SettingsBadge, SettingsCard, SettingsSwitch } from './settings-primitives'
import type { RefObject } from 'react'
import type { ConfigDraft, ProviderConfig, ProviderType } from './config-types'

import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'

import { FieldLabel } from '@/components/ui/field'

import { AppError, AppNotice } from '@/components/ui/app-primitives'

type ProviderDetailProps = {
  provider: ProviderConfig
  draft: ConfigDraft
  toggling: string
  saving: boolean
  dirty: boolean
  error: string
  isDefault: boolean
  apiKeyInputRef: RefObject<HTMLInputElement | null>
  onPatchDraft: (patch: Partial<ConfigDraft>) => void
  onSelectProviderType: (providerType: ProviderType) => void
  onSelectModel: (modelId: string) => void
  onToggleProvider: (provider: ProviderConfig, enabled: boolean) => void | Promise<void>
  onDeleteProvider: (provider: ProviderConfig) => void | Promise<void>
  onOpenModelDialog: (mode: 'discover' | 'manual') => void
  onSave: (setAsDefault?: boolean) => void | Promise<void>
}

export function ProviderDetail({
  provider,
  draft,
  toggling,
  saving,
  dirty,
  error,
  isDefault,
  apiKeyInputRef,
  onPatchDraft,
  onSelectProviderType,
  onSelectModel,
  onToggleProvider,
  onDeleteProvider,
  onOpenModelDialog,
  onSave,
}: ProviderDetailProps) {
  const { t } = useI18n()
  const codexOAuth = provider.id === 'openai-codex'
  const visualOnly = draft.providerType === 'visual'
  // 自定义连接必须填写 Base URL，高级区默认展开；已知 Provider 有官方默认值则收起。
  const [advancedOpen, setAdvancedOpen] = useState(Boolean(provider.custom))
  const Icon = PROVIDER_ICONS[provider.id] || Server
  const chatModels = visualOnly ? [] : provider.models.filter((item) => item.kind === 'chat')
  const visualModels = provider.models.filter((item) => item.kind !== 'chat')
  const selectedModel = provider.models.find((item) => item.id === draft.model)
  const hasModel = Boolean(draft.model)

  return (
    <SettingsCard>
      {/* 头部：图标 + 名称 + 状态徽标 + 启停开关 + 删除（自定义连接） */}
      <div className="flex items-center gap-[10px]">
        <span className="grid w-[34px] h-[34px] flex-none place-items-center rounded-[var(--r-sm)] bg-[var(--accent-soft)] text-[var(--star-strong)]">
          <Icon size={17} />
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-[1px]">
          <strong className="overflow-hidden text-ellipsis whitespace-nowrap text-[15px]">
            {provider.name}
          </strong>
          <span className="text-[11px] text-[var(--text-muted)]">{provider.id}</span>
        </div>
        <SettingsBadge tone={!provider.configured ? 'amber' : provider.enabled ? 'green' : 'gray'}>
          {!provider.configured
            ? codexOAuth
              ? t('config:configPage.codexCLILoginRequired')
              : t('config:configPage.apiKeyRequired')
            : provider.enabled
              ? t('config:configPage.authenticationReady')
              : t('config:configPage.disabled2')}
        </SettingsBadge>
        <SettingsSwitch
          value={provider.configured && provider.enabled}
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

      {codexOAuth && (
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
      )}

      {!codexOAuth && (
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
      )}

      {/* 对话模型：选择 + 获取/添加；视觉 Provider 与无模型时给说明 */}
      {visualOnly ? (
        <AppNotice>
          <AlertTriangle size={16} />
          <span>
            <strong>{t('config:configPage.visualOnlyProvider')}</strong>
            <small>
              {t(
                'config:configPage.usedOnlyForImageGenerationVideoGenerationAndImageEditingAndExcludedFromAgentChatModelSelection',
              )}
            </small>
          </span>
        </AppNotice>
      ) : chatModels.length > 0 ? (
        <div className="[margin-top:12px]">
          <div className="flex items-center justify-between gap-[8px]">
            <span className="text-[12px] font-semibold text-[var(--text-secondary)]">
              {t('config:configPage.defaultChatModel')}
            </span>
            {!codexOAuth && (
              <div className="flex items-center gap-[6px]">
                <Button
                  variant="outline"
                  size="sm"
                  className="h-[26px] bg-surface-subtle px-[8px] text-[12px]"
                  onClick={() => onOpenModelDialog('discover')}
                >
                  <RefreshCw size={12} />
                  {t('config:configPage.fetchModels')}
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  className="h-[26px] bg-surface-subtle px-[8px] text-[12px]"
                  onClick={() => onOpenModelDialog('manual')}
                >
                  <Plus size={12} />
                  {t('config:configPage.addModel')}
                </Button>
              </div>
            )}
          </div>
          <AppSelect
            className="mt-[5px] h-[31px] w-full rounded-[var(--r-xs)] [border:1px_solid_var(--stroke)] bg-[var(--surface-subtle)] px-[9px] text-[12px] font-normal text-[var(--text)] dark:bg-[var(--solid)]"
            value={draft.model}
            onChange={(event) => onSelectModel(event.target.value)}
          >
            {chatModels.map((model) => (
              <option key={model.id} value={model.id}>
                {model.name}
              </option>
            ))}
          </AppSelect>
          <div className="flex flex-wrap items-center gap-[6px] [margin-top:8px]">
            <SettingsBadge>
              {selectedModel?.reasoning
                ? t('config:configPage.reasoningSupported')
                : t('config:configPage.standardModel')}
            </SettingsBadge>
            <SettingsBadge tone="gray">
              {selectedModel?.contextWindow
                ? `${Math.round(selectedModel.contextWindow / 1000)}K context`
                : t('config:configPage.automaticContext')}
            </SettingsBadge>
            {selectedModel?.baseUrlOverride && (
              <SettingsBadge tone="amber">{t('config:configPage.customBaseURL')}</SettingsBadge>
            )}
          </div>
        </div>
      ) : (
        <AppNotice>
          <AlertTriangle size={16} />
          <span>
            <strong>{t('config:configPage.noChatModelAvailable')}</strong>
            <small>{t('config:configPage.fetchOrAddAChatModel')}</small>
            {!codexOAuth && (
              <span className="[margin-top:6px]">
                <Button
                  variant="outline"
                  size="sm"
                  className="h-[26px] bg-surface-subtle px-[8px] text-[12px]"
                  onClick={() => onOpenModelDialog('discover')}
                >
                  <RefreshCw size={12} />
                  {t('config:configPage.fetchModels')}
                </Button>
              </span>
            )}
          </span>
        </AppNotice>
      )}

      {visualModels.length > 0 && (
        <div className="flex flex-wrap items-center gap-[7px] [margin-top:12px]">
          <span className="text-[12px] font-[700] text-[var(--text-muted)]">
            {t('config:configPage.visualModels')}
          </span>
          {visualModels.map((model) => (
            <SettingsBadge tone="blue" key={model.id}>
              {model.name} ·{' '}
              {model.kind === 'video'
                ? t('config:configPage.videoGeneration')
                : t('config:configPage.imageGenerationAndEditing')}
            </SettingsBadge>
          ))}
        </div>
      )}

      {/* 高级设置：协议/端点/组织/模型端点/用途，默认收起 */}
      <Collapsible open={advancedOpen} onOpenChange={setAdvancedOpen} className="[margin-top:12px]">
        <CollapsibleTrigger asChild>
          <button
            type="button"
            className="group flex w-full cursor-pointer items-center gap-[6px] border-0 bg-transparent p-0 text-left text-[12px] font-[600] text-[var(--text-muted)] hover:text-[var(--text)]"
          >
            <ChevronDown
              size={14}
              className="transition-transform group-data-[state=open]:rotate-180"
            />
            <span>{t('config:configPage.advancedSettings')}</span>
            <span className="font-[400] text-[var(--text-tertiary)]">
              {t('config:configPage.advancedSettingsHint')}
            </span>
          </button>
        </CollapsibleTrigger>
        <CollapsibleContent>
          {!codexOAuth && (
            <>
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
              {!visualOnly && (
                <FieldLabel variant="control">
                  {t('config:configPage.modelBaseURL')}
                  <input
                    value={draft.modelBaseUrl}
                    onChange={(event) => onPatchDraft({ modelBaseUrl: event.target.value })}
                    placeholder={t(
                      'config:configPage.optionalOverrideTheProviderBaseURLForThisModel',
                    )}
                  />
                </FieldLabel>
              )}
            </>
          )}
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
        </CollapsibleContent>
      </Collapsible>

      {/* 底部动作行：保存状态 + 设为默认/保存 */}
      <div className="flex flex-wrap items-center justify-between gap-[8px] [margin-top:14px] [border-top:1px_solid_var(--stroke-soft)] [padding-top:10px]">
        <span className="flex items-center gap-[6px] text-[12px]">
          {dirty ? (
            <>
              <CircleAlert size={14} className="text-[var(--warning-strong)]" />
              <span className="text-[var(--warning-strong)]">
                {t('config:configPage.unsavedChanges')}
              </span>
            </>
          ) : (
            <>
              <CheckCircle2 size={14} className="text-[var(--success)]" />
              <span className="text-[var(--text-muted)]">
                {t('config:configPage.allChangesSaved')}
              </span>
            </>
          )}
        </span>
        <div className="flex items-center gap-[7px]">
          {!visualOnly && hasModel && (
            <Button
              variant="outline"
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
      </div>
      {error && (
        <AppError>
          <AlertTriangle size={13} />
          {error}
        </AppError>
      )}
    </SettingsCard>
  )
}
