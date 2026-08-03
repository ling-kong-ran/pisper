import { useRef, useState } from 'react'
import { Check, RefreshCw, ShieldCheck } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Confirmation } from '@/components/ai-elements/confirmation'
import type { EntityRecord } from '@/types/chat'

export function ToolApproval({
  approvals,
  onResolve,
  compact = false,
}: {
  approvals: EntityRecord[]
  onResolve: (approvalId: string, approved: boolean) => Promise<void> | void
  compact?: boolean
}) {
  const { t } = useI18n()
  const [resolving, setResolving] = useState(false)
  const resolvingRef = useRef(false)
  const approval = approvals[0]
  if (!approval) return null

  const resolve = async (approved: boolean) => {
    if (resolvingRef.current) return
    resolvingRef.current = true
    setResolving(true)
    try {
      await onResolve(approval.id, approved)
    } finally {
      resolvingRef.current = false
      setResolving(false)
    }
  }

  return (
    <Confirmation
      approval={{ id: approval.id }}
      state="approval-requested"
      className={`tool-approval ${compact ? 'compact' : ''}`}
      data-pisper-approval-id={approval.id}
    >
      <div>
        <ShieldCheck size={compact ? 12 : 15} />
        <span>
          <strong>
            {t('chat:focusSession.toolRequestsApproval', { tool: approval.toolName })}
          </strong>
          <small>
            {approval.reason}
            {approvals.length > 1
              ? ` · ${t('chat:focusSession.countMoreWaiting', { count: approvals.length - 1 })}`
              : ''}
          </small>
        </span>
      </div>
      {!compact && (
        <details>
          <summary>{t('chat:focusSession.viewCallArguments')}</summary>
          <pre>{JSON.stringify(approval.args, null, 2)}</pre>
        </details>
      )}
      <div className="tool-approval-actions">
        <button
          type="button"
          className="button secondary"
          disabled={resolving}
          onClick={() => resolve(false)}
        >
          {t('chat:focusSession.deny')}
        </button>
        <button
          type="button"
          className="button primary"
          disabled={resolving}
          onClick={() => resolve(true)}
        >
          {resolving ? <RefreshCw className="spin" size={12} /> : <Check size={12} />}
          {t('chat:focusSession.allow')}
        </button>
      </div>
    </Confirmation>
  )
}
