// 工具审批：危险工具执行前的确认卡片（允许/拒绝 + 变更预览）。
import { useEffect, useRef, useState } from 'react'
import { Check, FileDiff, RefreshCw, ShieldCheck } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Confirmation } from '@/components/ai-elements/confirmation'
import type { EntityRecord } from '@/types/chat'
import { GitDiffDialog } from './GitDiffViewer'

import { Button } from '@/components/ui/button'

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
  const [diffOpen, setDiffOpen] = useState(false)
  const resolvingRef = useRef(false)
  const approval = approvals[0]

  useEffect(() => setDiffOpen(false), [approval?.id])

  if (!approval) return null
  const fileChange = approval.fileChange
  const diff = typeof fileChange?.diff === 'string' ? fileChange.diff : ''

  // 审批处理：把允许/拒绝回传给运行时；防重入（resolvingRef），
  // 避免用户在提交期间重复点击产生重复调用。
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
      className={`tool-approval [&_>_div:first-child]:flex [&_>_div:first-child]:min-w-0 [&_>_div:first-child]:items-center [&_>_div:first-child]:gap-[8px] [&_>_div:first-child_>_span]:flex [&_>_div:first-child_>_span]:min-w-0 [&_>_div:first-child_>_span]:flex-col [&_>_div:first-child_>_span]:gap-[2px] [&_strong]:text-[13px] [&_small]:overflow-hidden [&_small]:text-[13px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_details]:min-w-0 [&_details]:text-[13px] [&_summary]:cursor-pointer [&_pre]:max-h-[130px] [&_pre]:overflow-auto [&_pre]:mt-[6px] [&_pre]:rounded-[var(--r-xs)] [&_pre]:bg-[var(--warning-code-bg)] [&_pre]:text-[var(--warning-soft)] [&_pre]:p-[8px] [&_pre]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_pre]:text-[13px] [&_pre]:whitespace-pre-wrap [&.compact]:grid-cols-[minmax(0,1fr)_auto] [&.compact]:p-[6px_7px] [&.compact_strong]:text-[13px] [&.compact_small]:text-[13px] grid grid-cols-[minmax(0,1fr)_auto] items-center gap-[8px] [border:1px_solid_var(--warning-border)] rounded-[var(--r-sm)] bg-[var(--warning-soft)] text-[var(--warning-strong)] [padding:9px_10px] ${compact ? 'compact' : ''}`}
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
        <div className="flex [grid-column:1/-1] items-center gap-[10px]">
          {diff && (
            <button
              type="button"
              className="tool-approval-view-diff hover:border-[var(--accent-border)] hover:bg-[var(--accent-soft)] hover:text-[var(--star-strong)] inline-flex min-h-[28px] flex-none items-center gap-[6px] [border:1px_solid_var(--warning-border)] rounded-[var(--r-sm)] bg-[var(--surface-muted)] [padding:4px_7px] text-[var(--text-secondary)] text-[12px] font-[600] cursor-pointer"
              onClick={() => setDiffOpen(true)}
            >
              <FileDiff size={13} />
              {t('chat:focusSession.gitViewDiff')}
            </button>
          )}
          <details>
            <summary>{t('chat:focusSession.viewCallArguments')}</summary>
            <pre>{JSON.stringify(approval.args, null, 2)}</pre>
          </details>
        </div>
      )}
      <div className="flex gap-[5px]">
        <Button
          type="button"
          variant="outline"
          size="lg"
          className="bg-surface-subtle"
          disabled={resolving}
          onClick={() => resolve(false)}
        >
          {t('chat:focusSession.deny')}
        </Button>
        <Button type="button" size="lg" disabled={resolving} onClick={() => resolve(true)}>
          {resolving ? <RefreshCw className="animate-spin" size={12} /> : <Check size={12} />}
          {t('chat:focusSession.allow')}
        </Button>
      </div>
      {diffOpen && (
        <GitDiffDialog
          diff={diff}
          truncated={Boolean(fileChange?.truncated)}
          onClose={() => setDiffOpen(false)}
        />
      )}
    </Confirmation>
  )
}
