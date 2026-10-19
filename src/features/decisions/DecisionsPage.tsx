// 决策模型页面：远端配置、连通性验证与型号审批状态。
// 该能力面向 Agent（typed_decide 工具），页面只负责配置与连通性验证。
import { useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import { PageFrame } from '@/components/app/PageFrame'
import { PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { RemoteSettingsCard } from './RemoteSettingsCard'
import { DelegationCard } from './DelegationCard'
import { decisionErrorMessage, fetchDecisionsStatus, type DecisionsStatus } from './decisions-api'

export function DecisionsPage({ notify }: { notify: Notify }) {
  const { t } = useI18n()
  const [status, setStatus] = useState<DecisionsStatus | null>(null)
  const [loadError, setLoadError] = useState('')
  const [attempt, setAttempt] = useState(0)

  useEffect(() => {
    const controller = new AbortController()
    setLoadError('')
    fetchDecisionsStatus(controller.signal)
      .then(setStatus)
      .catch((error: unknown) => {
        if (!controller.signal.aborted) {
          setLoadError(decisionErrorMessage(error, t('decisions:page.loadFailed')))
        }
      })
    return () => controller.abort()
    // 挂载时加载一次即可；t 随语言切换变化不应触发重新拉取。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [attempt])

  if (loadError)
    return (
      <PageFrame variant="settings">
        <PageState
          kind="error"
          title={t('decisions:page.loadFailed')}
          description={loadError}
          action={
            <Button variant="outline" onClick={() => setAttempt((value) => value + 1)}>
              {t('common:pageTemplate.retry')}
            </Button>
          }
        />
      </PageFrame>
    )
  if (!status)
    return (
      <PageFrame variant="settings">
        <PageState kind="loading" title={t('common:pageTemplate.loading')} />
      </PageFrame>
    )

  return (
    <PageFrame variant="settings">
      <div className="flex flex-col gap-4">
        <RemoteSettingsCard
          remote={status.config.remote}
          disabled={false}
          notify={notify}
          onSaved={(next) => setStatus((prev) => (prev ? { ...prev, config: next } : prev))}
        />

        <DelegationCard
          delegate={status.config.delegate}
          approvalStatus={status.config.approval?.status}
          hasKey={status.config.remote.hasKey}
          notify={notify}
          onSaved={(next) => setStatus((prev) => (prev ? { ...prev, config: next } : prev))}
        />
      </div>
    </PageFrame>
  )
}
