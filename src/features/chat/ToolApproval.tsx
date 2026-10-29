// 工具审批：危险工具执行前的确认卡片（允许/拒绝 + 变更预览）。
import { useEffect, useRef, useState } from 'react'
import { Check, ChevronRight, FileDiff, RefreshCw, ShieldCheck } from 'lucide-react'
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

  // 审批卡：中性卡片 + 琥珀色盾形状态方块；主操作「允许」是唯一的强调色实心键。
  return (
    <Confirmation
      approval={{ id: approval.id }}
      state="approval-requested"
      className={`tool-approval grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-[12px] gap-y-[8px] rounded-md border border-stroke bg-panel px-[12px] py-[10px] text-foreground shadow-raise [&.compact]:px-[10px] [&.compact]:py-[6px] max-[470px]:grid-cols-1 ${compact ? 'compact' : ''}`}
      data-pisper-approval-id={approval.id}
    >
      <div className="flex min-w-0 items-center gap-[10px]">
        <span className="grid size-[18px] flex-none place-items-center rounded-xs bg-warning-soft text-warning-strong">
          <ShieldCheck size={12} strokeWidth={2} />
        </span>
        <span className="flex min-w-0 flex-col gap-[1px]">
          <strong className="truncate text-[13px] font-medium">
            {t('chat:focusSession.toolRequestsApproval', { tool: approval.toolName })}
          </strong>
          <small className="truncate text-[12px] text-content-muted">
            {approval.reason}
            {approvals.length > 1
              ? ` · ${t('chat:focusSession.countMoreWaiting', { count: approvals.length - 1 })}`
              : ''}
          </small>
        </span>
      </div>
      <div className="flex flex-none justify-end gap-[6px]">
        <Button
          type="button"
          variant="secondary"
          size="sm"
          className="max-[650px]:h-11 max-[650px]:px-4"
          disabled={resolving}
          onClick={() => resolve(false)}
        >
          {t('chat:focusSession.deny')}
        </Button>
        <Button
          type="button"
          size="sm"
          className="max-[650px]:h-11 max-[650px]:px-4"
          disabled={resolving}
          onClick={() => resolve(true)}
        >
          {resolving ? <RefreshCw className="animate-spin" size={12} /> : <Check size={12} />}
          {t('chat:focusSession.allow')}
        </Button>
      </div>
      {!compact && (
        <div className="flex min-w-0 flex-wrap items-start gap-x-[12px] gap-y-[6px] [grid-column:1/-1] pl-[28px] max-[470px]:pl-0">
          {diff && (
            <button
              type="button"
              className="tool-approval-view-diff inline-flex min-h-[28px] flex-none cursor-pointer items-center gap-[6px] rounded-sm border border-stroke-strong bg-key px-[8px] text-[12px] font-medium text-content-secondary shadow-key transition-colors duration-(--d-fast) hover:bg-surface-subtle hover:text-foreground max-[650px]:min-h-11"
              onClick={() => setDiffOpen(true)}
            >
              <FileDiff size={13} />
              {t('chat:focusSession.gitViewDiff')}
            </button>
          )}
          <details className="group/args min-w-0 flex-1 text-[12px] text-content-muted">
            <summary className="inline-flex min-h-[28px] cursor-pointer list-none items-center gap-[4px] rounded-sm hover:text-foreground max-[650px]:min-h-11 [&::-webkit-details-marker]:hidden">
              <ChevronRight
                size={13}
                className="transition-transform duration-(--d-fast) group-open/args:rotate-90"
              />
              {t('chat:focusSession.viewCallArguments')}
            </summary>
            <pre className="mt-[6px] max-h-[130px] overflow-auto rounded-sm border border-stroke-soft bg-code p-[8px_10px] font-mono text-[12px] leading-[1.6] whitespace-pre-wrap text-content-soft">
              {JSON.stringify(approval.args, null, 2)}
            </pre>
          </details>
        </div>
      )}
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
