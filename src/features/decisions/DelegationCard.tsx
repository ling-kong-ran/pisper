// 审批委派：阈值达标才批准；模型或端点改变后，需要重新确认阈值。
import { useState } from 'react'
import { Loader2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import {
  AppCard as Panel,
  AppCardHeader,
  AppSectionTitle as SectionTitle,
  AppSwitch as Toggle,
} from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  decisionErrorMessage,
  updateDecisionsConfig,
  type DecisionConfig,
  type DecisionApprovalStatus,
  type DecisionDelegateConfig,
} from './decisions-api'

export function DelegationCard({
  delegate,
  hasKey,
  approvalStatus = 'ready',
  notify,
  onSaved,
}: {
  delegate: DecisionDelegateConfig
  hasKey: boolean
  approvalStatus?: DecisionApprovalStatus
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
        </div>
      </AppCardHeader>

      <div className="mt-4 flex items-center justify-between gap-4">
        <Label htmlFor="decision-delegation" className="flex flex-col items-start gap-1">
          <span>{t('decisions:delegate.enabled')}</span>
          <span className="text-[11px] leading-normal font-normal text-content-muted">
            {t('decisions:delegate.enabledHint')}
          </span>
        </Label>
        <div className="flex items-center gap-2">
          {saving && <Loader2 size={13} className="animate-spin text-content-muted" />}
          <Toggle
            id="decision-delegation"
            value={delegate.enabled}
            disabled={saving}
            onChange={(checked) => void save({ enabled: checked })}
            ariaLabel={t('decisions:delegate.enabled')}
          />
        </div>
      </div>

      <div className="mt-4 flex items-center justify-between gap-4">
        <Label htmlFor="decision-verification" className="flex flex-col items-start gap-1">
          <span>{t('decisions:delegate.verifyActions')}</span>
          <span className="text-[11px] leading-normal font-normal text-content-muted">
            {t('decisions:delegate.verifyActionsHint')}
          </span>
        </Label>
        <Toggle
          id="decision-verification"
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
            key={delegate.allowThreshold}
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

      {delegate.enabled && approvalStatus !== 'ready' && (
        <div className="mt-3 rounded-md bg-muted/40 p-3 text-xs text-content-muted" role="status">
          <div className="flex flex-col items-start gap-2">
            <p>
              {approvalStatus === 'model_unverified'
                ? t('decisions:delegate.modelUnverified')
                : t('decisions:delegate.thresholdRequired')}
            </p>
            {approvalStatus === 'threshold_required' && (
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={saving}
                onClick={() => void save({ allowThreshold: delegate.allowThreshold })}
              >
                {t('decisions:delegate.confirmThreshold')}
              </Button>
            )}
          </div>
        </div>
      )}

      {(delegate.enabled || delegate.verifyActions) && !hasKey && (
        <p className="mt-3 text-xs text-content-muted" role="status">
          {t('decisions:delegate.noKeyHint')}
        </p>
      )}
    </Panel>
  )
}
