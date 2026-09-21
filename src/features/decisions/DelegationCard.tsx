// 审批委派卡片：开关 + 双阈值。开启后，需要审批的工具调用先交给决策模型判断，
// 置信度高于批准阈值自动放行、低于拒绝阈值直接拒绝、中间区间仍回落人工审批。
import { useState } from 'react'
import { Loader2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import {
  AppCard as Panel,
  AppCardHeader,
  AppNotice,
  AppSectionTitle as SectionTitle,
  AppSwitch as Toggle,
} from '@/components/ui/app-primitives'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  decisionErrorMessage,
  updateDecisionsConfig,
  type DecisionConfig,
  type DecisionDelegateConfig,
} from './decisions-api'

export function DelegationCard({
  delegate,
  hasKey,
  notify,
  onSaved,
}: {
  delegate: DecisionDelegateConfig
  hasKey: boolean
  notify: Notify
  onSaved: (config: DecisionConfig) => void
}) {
  const { t } = useI18n()
  const [saving, setSaving] = useState(false)

  const save = async (patch: Partial<DecisionDelegateConfig>) => {
    setSaving(true)
    try {
      const result = await updateDecisionsConfig({ delegate: patch })
      onSaved(result.config)
      notify(t('decisions:delegate.saved'), 'success')
    } catch (caught) {
      notify(decisionErrorMessage(caught, t('decisions:delegate.saveFailed')), 'error')
    } finally {
      setSaving(false)
    }
  }

  const parseThreshold = (value: string) => {
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : null
  }

  return (
    <Panel>
      <AppCardHeader>
        <div className="flex min-w-0 flex-col gap-1">
          <SectionTitle title={t('decisions:delegate.title')} />
          <p>{t('decisions:delegate.subtitle')}</p>
        </div>
      </AppCardHeader>

      <div className="mt-3 flex items-center justify-between gap-3">
        <Label className="flex flex-col items-start gap-1">
          <span>{t('decisions:delegate.enabled')}</span>
          <span className="text-[11px] leading-normal font-normal text-content-muted">
            {t('decisions:delegate.enabledHint')}
          </span>
        </Label>
        <div className="flex items-center gap-2">
          {saving && <Loader2 size={13} className="animate-spin text-content-muted" />}
          <Toggle
            value={delegate.enabled}
            disabled={saving}
            onChange={(checked) => void save({ enabled: checked })}
            ariaLabel={t('decisions:delegate.enabled')}
          />
        </div>
      </div>

      <div className="mt-3 flex items-center justify-between gap-3">
        <Label className="flex flex-col items-start gap-1">
          <span>{t('decisions:delegate.verifyActions')}</span>
          <span className="text-[11px] leading-normal font-normal text-content-muted">
            {t('decisions:delegate.verifyActionsHint')}
          </span>
        </Label>
        <Toggle
          value={delegate.verifyActions}
          disabled={saving}
          onChange={(checked) => void save({ verifyActions: checked })}
          ariaLabel={t('decisions:delegate.verifyActions')}
        />
      </div>

      {delegate.enabled && (
        <div className="mt-3 flex max-w-sm flex-col gap-1.5">
          <Label htmlFor="delegate-allow">{t('decisions:delegate.allowThreshold')}</Label>
          <Input
            id="delegate-allow"
            type="number"
            min={0.5}
            max={1}
            step={0.05}
            disabled={saving}
            defaultValue={delegate.allowThreshold}
            onBlur={(event) => {
              const value = parseThreshold(event.target.value)
              if (value !== null && value !== delegate.allowThreshold) {
                void save({ allowThreshold: value })
              }
            }}
          />
          <p className="text-[11px] text-content-muted">
            {t('decisions:delegate.allowThresholdHint')}
          </p>
        </div>
      )}

      <AppNotice className="mt-3">
        <span>
          <strong>{t('decisions:delegate.safetyTitle')}</strong>
          <small>{t('decisions:delegate.safetyHint')}</small>
          {delegate.enabled && !hasKey && (
            <small className="text-danger">{t('decisions:delegate.noKeyHint')}</small>
          )}
        </span>
      </AppNotice>
    </Panel>
  )
}
