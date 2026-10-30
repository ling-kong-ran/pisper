// 工具审批：对话里的内联批准卡（规格 v2 第 4.2 / 3.5 节）。与输入框同宽，琥珀描边 + 浅琥珀底；
// 一屏只突出当前要处理的一张，其余折叠成一行“还有 N 项等你处理”。
// critical（命令守卫拦截、全局写入、浏览器提交等）不能一步批准：卡片上是“按住允许”（按住 0.6 秒，
// 鼠标、触控、按住空格或回车都可以）；读屏的直接激活与“减少动态效果”下改为确认层（ApprovalConfirm）。
// 键盘：焦点不在输入框时，A 只把焦点移到允许 / 按住允许键，D 移到拒绝键，不直接批准。
import { lazy, Suspense, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Check, ChevronRight, FileDiff, RefreshCw, ShieldAlert, ShieldCheck } from 'lucide-react'
import { approvalSeverity, COMMAND_GUARD_REASON_PREFIX } from '@shared/session-board-status.mjs'
import { ensureBoardMessages } from '@/app/i18n'
import { useI18n } from '@/app/use-i18n'
import { HoldToConfirmButton } from '@/components/app/HoldToConfirmButton'
import { Confirmation } from '@/components/ai-elements/confirmation'
import { usePhoneShell } from '@/hooks/usePhoneShell'
import { isEditableTarget, matchesBoardShortcut, shortcutEventBlocked } from '@/lib/shortcuts'
import { motionReduced } from '@/lib/motion'
import { cn } from '@/lib/utils'
import { useShortcutStore } from '@/stores/shortcut-store'
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
  // 减少动效时不做按住填充，critical 改为确认层；挂载时读一次（服务端渲染按减少动效处理）。
  const [reducedMotion] = useState(() => motionReduced())
  const resolvingRef = useRef(false)
  const cardRef = useRef<HTMLDivElement>(null)
  const boardBindings = useShortcutStore((state) => state.boardBindings)
  const approval = approvals[0]

  useEffect(() => {
    setDiffOpen(false)
    setConfirming(false)
  }, [approval?.id])

  // A / D（跟随看板绑定）只移动焦点：A 到允许 / 按住允许，D 到拒绝；批准仍要用户自己按下去。
  const approvalId = approval?.id
  useEffect(() => {
    if (!approvalId) return undefined
    const focusAction = (event: KeyboardEvent) => {
      if (shortcutEventBlocked(event) || isEditableTarget(event.target)) return
      // 侧栏里的 A / D 归侧栏行自己（只在需要你行上处理）；从侧栏冒上来的按键不把焦点拽进会话。
      if (event.target instanceof Element && event.target.closest('[data-shell-sidebar]')) return
      const card = cardRef.current
      if (!card || card.getClientRects().length === 0) return
      const target = matchesBoardShortcut(event, boardBindings.boardApprove)
        ? '[data-approval-primary]'
        : matchesBoardShortcut(event, boardBindings.boardDeny)
          ? '[data-approval-deny]'
          : ''
      const button = target ? card.querySelector<HTMLButtonElement>(target) : null
      if (!button || button.disabled) return
      event.preventDefault()
      button.focus()
    }
    window.addEventListener('keydown', focusAction)
    return () => window.removeEventListener('keydown', focusAction)
  }, [approvalId, boardBindings.boardApprove, boardBindings.boardDeny])

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
  // 危险等级标签之后的说明：守卫原因已经写进标签时不再重复；其余待批折叠在卡片下方一行。
  const detail = guardSummary ? '' : text(approval.reason)
  // critical 在动效正常时卡片上直接按住；减少动效时按钮改为打开确认层。
  const holdInline = critical && !reducedMotion

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

  // 内联批准卡：琥珀描边 + 浅琥珀底（琥珀只表示需要你）；主操作在右侧。
  return (
    <div className="flex min-w-0 flex-col gap-1.5" ref={cardRef} data-approval-stack>
      <Confirmation
        approval={{ id: approval.id }}
        state="approval-requested"
        className={`tool-approval grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-2.5 rounded-(--r-lg) border border-st-attn-border bg-st-attn-soft px-3.5 py-3 text-foreground [&.compact]:px-2.5 [&.compact]:py-1.5 max-[470px]:grid-cols-1 ${compact ? 'compact' : ''}`}
        data-pisper-approval-id={approval.id}
        data-approval-current
      >
        <div className="flex min-w-0 items-center gap-2.5 [grid-column:1/-1]">
          <span className="grid size-[18px] flex-none place-items-center text-st-attn">
            {critical ? (
              <ShieldAlert size={16} strokeWidth={1.75} />
            ) : (
              <ShieldCheck size={16} strokeWidth={1.75} />
            )}
          </span>
          <strong className="min-w-0 flex-1 truncate text-[13.5px] font-medium">
            {t('chat:focusSession.toolRequestsApproval', { tool: approval.toolName })}
          </strong>
          {/* 危险等级用琥珀（需要你的颜色），critical 写出守卫给的原因摘要（规格 3.5）。 */}
          <span
            className={cn(
              'min-w-0 flex-none truncate text-xs',
              critical ? 'font-medium text-st-attn' : 'text-content-muted',
            )}
            data-severity={critical ? 'critical' : 'standard'}
          >
            {critical
              ? guardSummary
                ? t('board:card.criticalWithReason', { reason: guardSummary })
                : t('board:card.critical')
              : t('board:card.standard')}
          </span>
        </div>
        {commandText && (
          <div className="min-w-0 [grid-column:1/-1]">
            <ApprovalCommand text={commandText} prefix={args.command ? '$' : ''} />
          </div>
        )}
        {detail && (
          <p className="min-w-0 text-xs text-content-secondary [grid-column:1/-1] [overflow-wrap:anywhere]">
            {detail}
          </p>
        )}
        {!compact && (diff || Object.keys(args).length > 0) && (
          <div className="flex min-w-0 flex-wrap items-start gap-x-3 gap-y-1.5 [grid-column:1/-1]">
            {diff && (
              <button
                type="button"
                className="tool-approval-view-diff inline-flex min-h-7 flex-none cursor-pointer items-center gap-1.5 rounded-sm border border-stroke-strong bg-solid px-2 text-xs font-medium text-content-secondary transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-foreground max-[650px]:min-h-11"
                onClick={() => setDiffOpen(true)}
              >
                <FileDiff size={13} />
                {t('chat:focusSession.gitViewDiff')}
              </button>
            )}
            <details className="group/args min-w-0 flex-1 text-xs text-content-muted">
              <summary className="inline-flex min-h-7 cursor-pointer list-none items-center gap-1 rounded-sm hover:text-foreground focus-visible:outline-2 focus-visible:outline-focus max-[650px]:min-h-11 [&::-webkit-details-marker]:hidden">
                <ChevronRight
                  size={13}
                  className="transition-transform duration-(--d-fast) group-open/args:rotate-90"
                />
                {t('chat:focusSession.viewCallArguments')}
              </summary>
              <pre className="mt-1.5 max-h-[130px] overflow-auto rounded-sm border border-well-stroke bg-well p-[8px_10px] font-mono text-xs leading-[1.6] whitespace-pre-wrap text-well-text">
                {JSON.stringify(approval.args, null, 2)}
              </pre>
            </details>
          </div>
        )}
        <div className="flex min-w-0 flex-wrap items-center justify-between gap-2 [grid-column:1/-1]">
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="bg-solid max-[650px]:h-11 max-[650px]:px-4"
            disabled={resolving}
            data-approval-deny
            onClick={() => resolve(false)}
          >
            {t('chat:focusSession.deny')}
          </Button>
          <span className="flex min-w-0 items-center gap-2.5">
            {holdInline && (
              <span className="min-w-0 truncate text-xs text-content-muted max-[470px]:hidden">
                {phone ? t('board:confirm.hintTouch') : t('board:card.holdHint')}
              </span>
            )}
            {holdInline ? (
              <HoldToConfirmButton
                size="sm"
                className="max-[650px]:h-11 max-[650px]:px-4"
                disabled={resolving}
                data-approval-critical
                data-approval-primary
                data-approval-hold
                aria-label={t('board:card.holdToApprove')}
                onConfirm={() => void resolve(true)}
                // 读屏的直接激活没有按住过程：改为确认层（层内 ↵ 或按住确认）。
                onActivate={() => setConfirming(true)}
              >
                {resolving ? <RefreshCw className="animate-spin" size={12} /> : <Check size={12} />}
                {t('board:card.holdToApprove')}
              </HoldToConfirmButton>
            ) : (
              <Button
                type="button"
                size="sm"
                // 减少动效下的 critical 是默认样式（规格 3.5）：主操作色只留给一步就能完成的批准。
                variant={critical ? 'outline' : 'default'}
                className={cn('max-[650px]:h-11 max-[650px]:px-4', critical && 'bg-solid')}
                disabled={resolving}
                data-approval-critical={critical || undefined}
                data-approval-primary
                aria-haspopup={critical ? 'dialog' : undefined}
                onClick={() => (critical ? setConfirming(true) : resolve(true))}
              >
                {resolving ? <RefreshCw className="animate-spin" size={12} /> : <Check size={12} />}
                {/* 会话路由已注册看板命名空间（确认层文案也在那里）。 */}
                {critical ? t('board:card.approveCritical') : t('chat:focusSession.allow')}
              </Button>
            )}
          </span>
        </div>
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
      {approvals.length > 1 && (
        // 其余等你处理的审批折叠成一行（规格 v2 第 4.2 节）：处理完当前这张后下一张自动顶上来。
        <p
          className="flex min-h-8 min-w-0 items-center gap-2 rounded-(--r-md) border border-st-attn-border px-3 text-xs text-st-attn"
          data-approval-more={approvals.length - 1}
        >
          <span aria-hidden="true" className="size-2 flex-none rounded-full bg-st-attn-fill" />
          {t('board:card.moreWaiting', { count: approvals.length - 1 })}
        </p>
      )}
    </div>
  )
}
