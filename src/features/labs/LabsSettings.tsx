// 设置 › 实验功能（设计稿 docs/design/drafts/labs-*.png，设计见本目录 README）。
// 每个实验功能一行：名称、阶段、说明、“关掉后会怎样”、反馈链接与开关。打开或关闭立即保存到 Runtime，
// 保存中禁用该开关；失败时开关复原，行内写明原因并给出重试（不用会自动消失的提示）。
// 不可用的功能开关禁用并写明原因：是否可用只看连接的 Runtime，与客户端设备无关。
import './labs-messages'
import { useEffect, useState, type MouseEvent } from 'react'
import { Check, ExternalLink, FlaskConical, Info } from 'lucide-react'
import type { BetaFeatureState } from '@shared/beta-features.mjs'
import type { Translate } from '@/app/navigation'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { FeatureAlert, FeatureList, FeatureRow, FeatureSection } from '@/components/app/FeaturePage'
import { InlineEmptyState } from '@/components/app/PageState'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'
import { Switch } from '@/components/ui/switch'
import { openMobileExternalLink } from '@/lib/mobile-external-link'
import { useBetaFeaturesStore } from './beta-features-store'

type FeatureCopy = { title: string; description: string; whenOff: string }

// 每个功能的文案键必须是字面量（i18n 检查逐个核对）：新功能进清单时在这里加一项。
const FEATURE_COPY: Partial<Record<string, (t: Translate) => FeatureCopy>> = {}

function featureCopy(t: Translate, id: string): FeatureCopy {
  return FEATURE_COPY[id]?.(t) ?? { title: id, description: '', whenOff: '' }
}

function stageLabel(t: Translate, stage: BetaFeatureState['stage']) {
  return stage === 'preview' ? t('labs:stage.preview') : t('labs:stage.experimental')
}

export function LabsSettings({ notify }: { notify: Notify }) {
  const { t } = useI18n()
  const features = useBetaFeaturesStore((state) => state.features)
  const status = useBetaFeaturesStore((state) => state.status)
  const error = useBetaFeaturesStore((state) => state.error)
  const pending = useBetaFeaturesStore((state) => state.pending)
  const rowErrors = useBetaFeaturesStore((state) => state.rowErrors)
  const load = useBetaFeaturesStore((state) => state.load)
  const setEnabled = useBetaFeaturesStore((state) => state.setEnabled)
  // 本次进入页面后刚打开的功能：显示“立即生效”的确认；离开页面即忘。
  const [justEnabled, setJustEnabled] = useState<ReadonlySet<string>>(() => new Set())

  // 每次进入都向 Runtime 重读：别的客户端可能刚改过开关。
  useEffect(() => {
    void load()
  }, [load])

  const toggle = async (id: string, enabled: boolean) => {
    await setEnabled(id, enabled)
    const saved = useBetaFeaturesStore.getState()
    const ok =
      !saved.rowErrors[id] && saved.features.some((f) => f.id === id && f.enabled === enabled)
    setJustEnabled((previous) => {
      const next = new Set(previous)
      if (ok && enabled) next.add(id)
      else next.delete(id)
      return next
    })
  }

  const openFeedback = (event: MouseEvent<HTMLAnchorElement>) => {
    const opening = openMobileExternalLink(event, event.currentTarget.href)
    void opening?.catch(() => notify(t('labs:openLinkFailed'), 'error'))
  }

  let list
  if (status === 'loading' && features.length === 0) {
    list = (
      <p role="status" className="flex items-center gap-2 py-6 text-sm text-content-muted">
        <Spinner aria-hidden="true" />
        {t('labs:loading')}
      </p>
    )
  } else if (features.length === 0) {
    list =
      status === 'error' ? null : (
        <InlineEmptyState
          compact
          icon={FlaskConical}
          title={t('labs:emptyTitle')}
          description={t('labs:emptyDescription')}
        />
      )
  } else {
    list = (
      <FeatureList>
        {features.map((feature) => {
          const copy = featureCopy(t, feature.id)
          const saving = pending[feature.id] === true
          const rowError = rowErrors[feature.id]
          return (
            <FeatureRow
              key={feature.id}
              data-beta-feature={feature.id}
              data-enabled={feature.enabled}
              icon={<FlaskConical />}
              name={copy.title}
              status={<Badge variant="outline">{stageLabel(t, feature.stage)}</Badge>}
              description={copy.description || undefined}
              actions={
                <span className="flex items-center gap-2">
                  {saving && (
                    <span
                      role="status"
                      className="flex items-center gap-1.5 text-xs text-content-muted"
                    >
                      <Spinner aria-hidden="true" />
                      {feature.enabled ? t('labs:saving.on') : t('labs:saving.off')}
                    </span>
                  )}
                  <Switch
                    checked={feature.enabled}
                    disabled={saving || !feature.available}
                    aria-busy={saving}
                    aria-label={t('labs:toggleLabel', { name: copy.title })}
                    onCheckedChange={(enabled) => void toggle(feature.id, enabled)}
                  />
                </span>
              }
              detail={
                <div className="flex min-w-0 flex-col gap-1.5 pb-3 text-xs leading-relaxed text-content-muted">
                  {copy.whenOff && (
                    <p>
                      {t('labs:whenOffPrefix')}
                      {copy.whenOff}
                    </p>
                  )}
                  {feature.feedbackUrl && (
                    <a
                      className="inline-flex w-fit items-center gap-1 text-content-secondary underline decoration-stroke-strong underline-offset-3 hover:text-content"
                      href={feature.feedbackUrl}
                      target="_blank"
                      rel="noopener noreferrer"
                      data-web-preview="external"
                      onClick={openFeedback}
                    >
                      {t('labs:feedback')}
                      <ExternalLink className="size-3.5" aria-hidden="true" />
                    </a>
                  )}
                  {!feature.available && (
                    <p className="flex items-start gap-1.5 text-content-secondary">
                      <Info className="mt-[3px] size-3.5 flex-none" aria-hidden="true" />
                      {t('labs:unavailable.runtime')}
                    </p>
                  )}
                  {feature.enabled && justEnabled.has(feature.id) && !saving && (
                    <p role="status" className="flex items-center gap-1.5 text-content-secondary">
                      <Check className="size-3.5 flex-none" aria-hidden="true" />
                      {t('labs:enabledNow')}
                    </p>
                  )}
                  {rowError && (
                    <FeatureAlert className="mt-1">
                      <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
                        {t('labs:saveFailed', { error: rowError.message })}
                        <Button
                          size="sm"
                          variant="outline"
                          onClick={() => void toggle(feature.id, rowError.desired)}
                        >
                          {t('labs:retry')}
                        </Button>
                      </span>
                    </FeatureAlert>
                  )}
                </div>
              }
            />
          )
        })}
      </FeatureList>
    )
  }

  return (
    <div data-labs-settings className="flex min-w-0 flex-col">
      <p
        role="note"
        className="flex items-start gap-2 rounded-[var(--r-md)] bg-surface-subtle px-3 py-2.5 text-[13px] leading-relaxed text-content-secondary"
      >
        <FlaskConical
          className="mt-[3px] size-3.5 flex-none text-content-muted"
          aria-hidden="true"
        />
        {t('labs:notice')}
      </p>
      {status === 'error' && (
        <FeatureAlert>
          <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
            {t('labs:loadFailed', { error })}
            <Button size="sm" variant="outline" onClick={() => void load()}>
              {t('labs:retry')}
            </Button>
          </span>
        </FeatureAlert>
      )}
      <FeatureSection title={t('labs:listTitle')} count={features.length || undefined}>
        {list}
      </FeatureSection>
    </div>
  )
}
