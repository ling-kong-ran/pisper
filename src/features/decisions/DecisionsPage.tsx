// 决策模型页面：远端配置、连通性验证与型号审批状态。
// 该能力面向 Agent（typed_decide 工具），页面只负责配置与连通性验证。
import { useEffect, useState } from 'react'
import { Loader2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import { AppCard as Panel } from '@/components/ui/app-primitives'
import { RemoteSettingsCard } from './RemoteSettingsCard'
import { DelegationCard } from './DelegationCard'
import { decisionErrorMessage, fetchDecisionsStatus, type DecisionsStatus } from './decisions-api'

export function DecisionsPage({ notify }: { notify: Notify }) {
  const { t } = useI18n()
  const [status, setStatus] = useState<DecisionsStatus | null>(null)
  const [loadError, setLoadError] = useState('')

  useEffect(() => {
    const controller = new AbortController()
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
  }, [])

  if (loadError) {
    return (
      <div className="mx-auto max-w-3xl p-6">
        <Panel className="p-6 text-sm text-content-muted">{loadError}</Panel>
      </div>
    )
  }
  if (!status) {
    return (
      <div className="flex items-center justify-center p-12 text-content-muted">
        <Loader2 className="size-5 animate-spin" />
      </div>
    )
  }

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-4 p-6 pt-0">
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
  )
}
