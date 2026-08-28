// 当前模型摘要卡：一进配置页就能看到正在生效的对话模型与认证状态；
// 未配置时突出「快速配置」入口，降低新用户的迷路概率。
import { Bot, CheckCircle2, CircleAlert, PencilLine, Wand2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { SettingsBadge, SettingsCard } from './settings-primitives'
import type { ConfigData } from './config-types'

import { Button } from '@/components/ui/button'

type CurrentModelSummaryProps = {
  config: ConfigData
  onQuickSetup: () => void
  onChangeModel: () => void
}

export function CurrentModelSummary({
  config,
  onQuickSetup,
  onChangeModel,
}: CurrentModelSummaryProps) {
  const { t } = useI18n()
  const providerId = config.defaultProvider || config.provider
  const modelId = config.defaultModel || config.model
  const provider = config.providers.find((item) => item.id === providerId)
  const model = provider?.models.find((item) => item.id === modelId)
  const ready = Boolean(provider?.configured && provider?.enabled && modelId)

  return (
    <SettingsCard className="[margin-bottom:12px]">
      <div className="flex flex-wrap items-center gap-[12px]">
        <span className="grid w-[40px] h-[40px] flex-none place-items-center rounded-[11px] bg-[var(--accent-soft)] text-[var(--star-strong)]">
          <Bot size={19} />
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-[3px]">
          <span className="text-[12px] font-[600] text-[var(--text-muted)]">
            {t('config:configPage.currentChatModel')}
          </span>
          {ready ? (
            <span className="flex min-w-0 flex-wrap items-center gap-[7px]">
              <strong className="overflow-hidden text-ellipsis whitespace-nowrap text-[15px]">
                {provider?.name} / {model?.name || modelId}
              </strong>
              <SettingsBadge tone="green">
                <CheckCircle2 size={11} className="mr-[3px] inline" />
                {t('config:configPage.authenticationReady')}
              </SettingsBadge>
            </span>
          ) : (
            <span className="flex min-w-0 flex-col gap-[2px]">
              <strong className="flex items-center gap-[6px] text-[14px] text-[var(--warning-strong)]">
                <CircleAlert size={15} />
                {t('config:configPage.noModelConfiguredYet')}
              </strong>
              <small className="text-[12px] text-[var(--text-muted)]">
                {t('config:configPage.noModelConfiguredHint')}
              </small>
            </span>
          )}
        </div>
        <div className="flex flex-none items-center gap-[7px]">
          {ready && provider && (
            <Button variant="outline" className="bg-surface-subtle" onClick={onChangeModel}>
              <PencilLine size={13} />
              {t('config:configPage.changeModel')}
            </Button>
          )}
          <Button onClick={onQuickSetup}>
            <Wand2 size={13} />
            {t('config:configPage.quickSetup')}
          </Button>
        </div>
      </div>
    </SettingsCard>
  )
}
