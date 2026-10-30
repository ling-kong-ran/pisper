// 工具审批：危险工具执行前的确认卡片（允许/拒绝 + 变更预览）。
// critical（命令守卫拦截、全局写入、浏览器提交等）在这里同样不能一步批准：“允许…”只打开
// 看板的同一个确认层（按住 0.6 秒或层内 ↵），与看板、泳道条的批准摩擦一致（规格 3.5）。
import { lazy, Suspense, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Check, ChevronRight, FileDiff, RefreshCw, ShieldAlert, ShieldCheck } from 'lucide-react'
import { approvalSeverity, COMMAND_GUARD_REASON_PREFIX } from '@shared/session-board-status.mjs'
import { ensureBoardMessages } from '@/app/i18n'
import { useI18n } from '@/app/use-i18n'
import { Confirmation } from '@/components/ai-elements/confirmation'
import { usePhoneShell } from '@/hooks/usePhoneShell'
import { cn } from '@/lib/utils'
import type { ConfirmableApproval } from '@/features/board/approval-confirm'
import type { EntityRecord } from '@/types/chat'
import { GitDiffDialog } from './GitDiffViewer'

const ApprovalConfirm = lazy(async () => {
  const [{ ApprovalConfirm }] = await Promise.all([
    import('@/features/board/approval-confirm'),
    ensureBoardMessages(),
  ])
  return { default: ApprovalConfirm }
})

function text(value: unknown) {
  return typeof value === 'string' ? value : ''
}

// 会话审批记录 → 确认层展示用的审批（完整参数原样带上，确认层只按行折叠不截断）。
function confirmableApproval(approval: EntityRecord): ConfirmableApproval {
  const args =
    approval.args && typeof approval.args === 'object' && !Array.isArray(approval.args)
      ? (approval.args as Record<string, unknown>)
      : {}
  return {
    id: text(approval.id),
    toolName: text(approval.toolName),
    toolCallId: text(approval.toolCallId),
    args,
    command: text(args.command),
    path: text(args.path),
    reason: text(approval.reason),
    risk: text(approval.risk),
    severity: 'critical',
    createdAt: text(approval.createdAt) || null,
  }
}

import { Button } from '@/components/ui/button'

// 命令守卫的拦截原因摘要（“命令守卫拦截：改写远端分支。…” → “改写远端分支”），用于危险等级标签。
function guardReasonSummary(reason: string) {
  if (!reason.trimStart().startsWith(COMMAND_GUARD_REASON_PREFIX)) return ''
  const rest = reason
    .trimStart()
    .slice(COMMAND_GUARD_REASON_PREFIX.length)
    .replace(/^[\s:：·-]+/u, '')
  return rest.split(/[\n。；;]/u)[0]?.trim() || ''
}

// 批准前必须能看到完整命令（规格 3.5 / 4.3）：命令井按行折叠到 3 行，超出时给“展开完整命令”，
// 从不截断文字；长行自动换行。
function ApprovalCommand({ text, prefix }: { text: string; prefix: string }) {
  const { t } = useI18n()
  const ref = useRef<HTMLPreElement>(null)
  const [overflowing, setOverflowing] = useState(false)
  const [expanded, setExpanded] = useState(false)
  useLayoutEffect(() => {
    const node = ref.current
    if (!node || expanded) return
    const measure = () => setOverflowing(node.scrollHeight > node.clientHeight + 1)
    measure()
    if (typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(measure)
    observer.observe(node)
    return () => observer.disconnect()
  }, [expanded, text])
  return (
    <div className="min-w-0 rounded-sm border border-well-stroke bg-well" data-approval-command>
      <div className="px-2.5 py-1.5">
        <pre
          ref={ref}
          className={cn(
            'min-w-0 font-mono text-xs leading-5 whitespace-pre-wrap wrap-anywhere text-well-text',
            !expanded && 'line-clamp-3',
          )}
        >
          {prefix && <span className="text-well-muted select-none">{prefix} </span>}
          {text}
        </pre>
      </div>
      {(overflowing || expanded) && (
        <button
          type="button"
          className="w-full border-t border-well-stroke px-2.5 py-1 text-left text-xs text-content-secondary hover:text-content focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus max-[650px]:min-h-11 pointer-coarse:min-h-11"
          aria-expanded={expanded}
          onClick={() => setExpanded((value) => !value)}
        >
          {expanded ? t('board:card.collapse') : t('board:card.expand')}
        </button>
      )}
    </div>
  )
}

export function ToolApproval({
  approvals,
  onResolve,
  compact = false,
  sessionTitle = '',
  cwd = '',
}: {
  approvals: EntityRecord[]
  onResolve: (approvalId: string, approved: boolean) => Promise<void> | void
  compact?: boolean
  // 确认层里核对“哪个会话、在哪个目录”用。
  sessionTitle?: string
  cwd?: string
}) {
  const { t } = useI18n()
  const phone = usePhoneShell()
  const [resolving, setResolving] = useState(false)
  const [diffOpen, setDiffOpen] = useState(false)
  const [confirming, setConfirming] = useState(false)
  const resolvingRef = useRef(false)
  const approval = approvals[0]

  useEffect(() => {
    setDiffOpen(false)
    setConfirming(false)
  }, [approval?.id])

  if (!approval) return null
  const critical = approvalSeverity(approval) === 'critical'
  const fileChange = approval.fileChange
  const diff = typeof fileChange?.diff === 'string' ? fileChange.diff : ''
  const args =
    approval.args && typeof approval.args === 'object' && !Array.isArray(approval.args)
      ? (approval.args as Record<string, unknown>)
      : {}
  // 命令（bash）或路径（edit / write）直接摆在卡片上；其余工具仍在“查看调用参数”里给完整 JSON。
  const commandText = text(args.command) || text(args.path)
  const guardSummary = critical ? guardReasonSummary(text(approval.reason)) : ''
  // 危险等级标签之后的说明：守卫原因已经写进标签时不再重复，多条待批时补上计数。
  const detail = [
    guardSummary ? '' : text(approval.reason),
    approvals.length > 1
      ? t('chat:focusSession.countMoreWaiting', { count: approvals.length - 1 })
      : '',
  ]
    .filter(Boolean)
    .join(' · ')

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
          {critical ? (
            <ShieldAlert size={12} strokeWidth={2} />
          ) : (
            <ShieldCheck size={12} strokeWidth={2} />
          )}
        </span>
        <span className="flex min-w-0 flex-col gap-[1px]">
          <strong className="truncate text-[13px] font-medium">
            {t('chat:focusSession.toolRequestsApproval', { tool: approval.toolName })}
          </strong>
          <small className="flex min-w-0 items-center gap-1.5 text-[12px] text-content-muted">
            {/* 危险等级用琥珀（需要你的颜色），critical 写出守卫给的原因摘要（规格 3.5）。 */}
            <span
              className={cn(
                'flex-none',
                critical &&
                  'rounded-xs border border-st-attn-border bg-st-attn-soft px-1 leading-4 font-semibold text-st-attn',
              )}
              data-severity={critical ? 'critical' : 'standard'}
            >
              {critical
                ? guardSummary
                  ? t('board:card.criticalWithReason', { reason: guardSummary })
                  : t('board:card.critical')
                : t('board:card.standard')}
            </span>
            {detail && (
              <>
                <span aria-hidden="true">·</span>
                <span className="min-w-0 truncate">{detail}</span>
              </>
            )}
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
          // critical 的“允许…”是默认样式（规格 3.5）：主操作色只留给一步就能完成的批准。
          variant={critical ? 'outline' : 'default'}
          className="max-[650px]:h-11 max-[650px]:px-4"
          disabled={resolving}
          data-approval-critical={critical || undefined}
          aria-haspopup={critical ? 'dialog' : undefined}
          onClick={() => (critical ? setConfirming(true) : resolve(true))}
        >
          {resolving ? <RefreshCw className="animate-spin" size={12} /> : <Check size={12} />}
          {/* 会话路由已注册看板命名空间（确认层文案也在那里）。 */}
          {critical ? t('board:card.approveCritical') : t('chat:focusSession.allow')}
        </Button>
      </div>
      {commandText && (
        <div className="min-w-0 [grid-column:1/-1] pl-[28px] max-[470px]:pl-0">
          <ApprovalCommand text={commandText} prefix={args.command ? '$' : ''} />
        </div>
      )}
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
      {critical && confirming && (
        <Suspense fallback={null}>
          <ApprovalConfirm
            target={{
              sessionId: '',
              title: sessionTitle || text(approval.toolName),
              cwd,
              approval: confirmableApproval(approval),
            }}
            phone={phone}
            pending={resolving}
            error=""
            onConfirm={() => {
              // 批准失败时确认层留着，用户可以再按住一次或取消；成功后才关闭。
              void resolve(true).then(
                () => setConfirming(false),
                () => undefined,
              )
            }}
            onOpenChange={(open) => {
              if (!open) setConfirming(false)
            }}
          />
        </Suspense>
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
