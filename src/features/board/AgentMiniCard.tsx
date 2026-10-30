// 迷你代理卡片：会话页泳道条的悬停 / 聚焦预览、胶囊弹层里的列表行与 ⌘J 代理面板共用。
// 一行状态 + 标题 + 时长，一行细节（命令开头、错误首行、最后一行尾巴或活动）；预览形态再展开完整命令、
// 危险等级与操作。批准只把动作交回调用方（useStripApprovals）：critical 只有“查看”，不直接批准。
import type { ReactNode } from 'react'
import { ArrowRight, RotateCcw, ShieldAlert } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { BoardApproval } from './board-api'
import type { BoardItem } from './board-model'
import {
  BOARD_STATUS_GLYPHS,
  approvalArgsText,
  boardCardTone,
  guardReasonSummary,
  isGuardReason,
} from './card-presentation'
import { sinceLabel, stripDetail, type StripDetail, type StripSince } from './strip-presentation'

export type StripItemHandlers = {
  onOpen: (sessionId: string) => void
  onApprove: (item: BoardItem, approval: BoardApproval) => void
  onDeny: (item: BoardItem, approval: BoardApproval) => void
  onReview: (item: BoardItem, approval: BoardApproval | null) => void
  onRetry: (item: BoardItem) => void
  isPending: (kind: 'approve' | 'deny' | 'retry', sessionId: string, approvalId?: string) => boolean
}

const TONE_TEXT = {
  attn: 'text-st-attn',
  fail: 'text-st-fail',
  run: 'text-st-run',
  done: 'text-st-done',
  queue: 'text-st-queue',
} as const

export function StripItemActions({
  item,
  handlers,
  size = 'chip',
}: {
  item: BoardItem
  handlers: StripItemHandlers
  // chip：泳道条芯片里的 24px 键；row：列表与预览里的常规小键。
  size?: 'chip' | 'row'
}) {
  const { t } = useI18n()
  const approval = item.approvals[0]
  const buttonSize = size === 'chip' ? 'toolbar' : 'default'
  const button =
    size === 'chip'
      ? // 芯片里的键固定 24px（规格 4.2）；触屏上用伪元素把点击区上下各扩 8px，不撑高 36px 单行泳道条。
        "relative h-6 min-w-6 rounded-sm px-1.5 text-xs pointer-coarse:after:absolute pointer-coarse:after:-inset-y-2 pointer-coarse:after:inset-x-0 pointer-coarse:after:content-['']"
      : 'h-7 px-2 text-xs pointer-coarse:h-11'
  if (item.status === 'approval') {
    // critical、截断的预览或详情未到：只有“查看”，打开确认层核对完整参数。
    if (!approval || approval.severity === 'critical' || approval.truncated)
      return (
        <Button
          type="button"
          size={buttonSize}
          variant="secondary"
          className={button}
          data-strip-review
          aria-label={t('board:strip.reviewNamed', { title: item.title })}
          onClick={() => handlers.onReview(item, approval ?? null)}
        >
          {t('board:strip.review')}
        </Button>
      )
    return (
      <>
        <Button
          type="button"
          size={buttonSize}
          variant="secondary"
          className={button}
          data-strip-deny
          disabled={handlers.isPending('deny', item.id, approval.id)}
          aria-label={t('board:strip.denyNamed', { title: item.title })}
          onClick={() => handlers.onDeny(item, approval)}
        >
          {t('board:card.deny')}
        </Button>
        <Button
          type="button"
          size={buttonSize}
          className={button}
          data-strip-approve
          disabled={handlers.isPending('approve', item.id, approval.id)}
          aria-label={t('board:strip.approveNamed', { title: item.title })}
          onClick={() => handlers.onApprove(item, approval)}
        >
          {t('board:card.approve')}
        </Button>
      </>
    )
  }
  if (item.status === 'failed' && item.lane === 'attention' && size === 'row')
    return (
      <Button
        type="button"
        size={buttonSize}
        variant="secondary"
        className={button}
        disabled={handlers.isPending('retry', item.id)}
        onClick={() => handlers.onRetry(item)}
      >
        <RotateCcw aria-hidden="true" />
        {t('board:card.retry')}
      </Button>
    )
  return null
}

// 第 2 行细节：命令、尾巴与错误行用等宽 11px，其余文字（结果摘要、提问、说明）用无衬线 12px。
export function StripDetailText({
  detail,
  className,
}: {
  detail: StripDetail
  className?: string
}) {
  if (!detail.text) return null
  return (
    <span className={cn('min-w-0 truncate', className)}>
      <span className={detail.mono ? 'font-mono text-[11px]' : 'text-xs'}>{detail.text}</span>
      {detail.suffix && <span className="text-xs">{` · ${detail.suffix}`}</span>}
    </span>
  )
}

// 时长：无衬线前缀（“已等”“已运行”）加等宽数值；已完成是无衬线的相对时间。
export function StripSinceText({ since, className }: { since: StripSince; className?: string }) {
  return (
    <span
      className={cn('flex flex-none items-baseline gap-1 text-xs whitespace-nowrap', className)}
    >
      {since.prefix && <span>{since.prefix}</span>}
      <span className={since.mono ? 'font-mono tabular-nums' : undefined}>{since.value}</span>
    </span>
  )
}

// 列表行（胶囊弹层、代理面板）：整行是打开会话的按钮，操作键在右侧独立可聚焦。
export function AgentMiniRow({
  item,
  now,
  current,
  handlers,
  onOpen,
  word,
  rowRef,
}: {
  item: BoardItem
  now: number
  current: boolean
  handlers: StripItemHandlers
  onOpen: (sessionId: string) => void
  word: string
  rowRef?: (element: HTMLButtonElement | null) => void
}) {
  const { t, language } = useI18n()
  const tone = boardCardTone(item)
  const detail = stripDetail(item, t)
  const time = sinceLabel(item, now, t, language)
  return (
    <div
      data-agent-row={item.id}
      data-lane={item.lane}
      data-current={current || undefined}
      className={cn(
        'relative grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-2 rounded-md px-2 py-1.5 hover:bg-surface-hover',
        current && 'bg-solid ring-1 ring-stroke-strong ring-inset',
      )}
    >
      <button
        ref={rowRef}
        type="button"
        className="absolute inset-0 rounded-[inherit] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus"
        aria-label={t('board:card.label', { title: item.title, status: word })}
        aria-current={current ? 'page' : undefined}
        onClick={() => onOpen(item.id)}
      />
      <span className="pointer-events-none grid min-w-0 gap-0.5">
        <span className="flex min-w-0 items-center gap-1.5 text-[13px]">
          <StatusGlyph
            status={BOARD_STATUS_GLYPHS[item.status]}
            spin={item.status === 'running'}
            className={cn('size-3.5 flex-none', TONE_TEXT[tone])}
          />
          <span className="min-w-0 truncate font-medium text-content">{item.title}</span>
          <span className={cn('flex-none text-xs', TONE_TEXT[tone])}>{word}</span>
          {current && (
            <span className="flex-none rounded-xs bg-surface-muted px-1 text-xs text-content-secondary">
              {t('board:strip.current')}
            </span>
          )}
        </span>
        {/* 时长放到细节行右侧：标题行只留标题与状态词，标题能显示得更完整。 */}
        {(detail.text || time) && (
          <span className="flex min-w-0 items-baseline gap-2 pl-5 text-content-muted">
            <StripDetailText detail={detail} className="flex-1" />
            {time && <StripSinceText since={time} className="ml-auto" />}
          </span>
        )}
      </span>
      <span className="relative z-1 flex flex-none items-center gap-1">
        <StripItemActions item={item} handlers={handlers} size="row" />
      </span>
    </div>
  )
}

// 悬停 / 聚焦预览：完整命令、危险等级、最后一行尾巴、等待时长、操作与“打开 ↵”。
export function AgentMiniPreview({
  item,
  now,
  handlers,
  word,
  footer,
}: {
  item: BoardItem
  now: number
  handlers: StripItemHandlers
  word: string
  footer?: ReactNode
}) {
  const { t, language } = useI18n()
  const tone = boardCardTone(item)
  const approval = item.approvals[0]
  const command = approval ? approvalArgsText(approval) || approval.argsText || '' : ''
  const critical = approval?.severity === 'critical'
  const tail = item.tail.at(-1) || ''
  const time = sinceLabel(item, now, t, language)
  return (
    <div className="grid min-w-0 gap-2 text-xs">
      <div className="flex min-w-0 items-center gap-1.5">
        <StatusGlyph
          status={BOARD_STATUS_GLYPHS[item.status]}
          spin={item.status === 'running'}
          className={cn('size-4 flex-none', TONE_TEXT[tone])}
        />
        <strong className="min-w-0 flex-1 truncate text-[13px] font-semibold text-content">
          {item.title}
        </strong>
        <span className={cn('flex-none', TONE_TEXT[tone])}>{word}</span>
      </div>
      {approval && (
        <p
          className={cn(
            'flex min-w-0 items-center gap-1.5',
            critical ? 'text-st-attn' : 'text-content-muted',
          )}
        >
          {critical && <ShieldAlert className="size-3.5 flex-none" aria-hidden="true" />}
          <span className="min-w-0 truncate">
            {critical
              ? isGuardReason(approval.reason)
                ? t('board:card.criticalWithReason', {
                    reason: guardReasonSummary(approval.reason),
                  })
                : t('board:card.critical')
              : t('board:card.standard')}
            {' · '}
            {approval.toolName}
          </span>
        </p>
      )}
      {command && (
        <pre className="max-h-28 min-w-0 overflow-auto rounded-sm border border-well-stroke bg-well px-2 py-1.5 font-mono text-[11px] leading-[18px] whitespace-pre-wrap wrap-anywhere text-well-text">
          {command}
        </pre>
      )}
      {!command && item.status === 'failed' && item.error && (
        <p className="min-w-0 rounded-sm border border-st-fail-border bg-st-fail-soft px-2 py-1.5 font-mono text-[11px] text-st-fail wrap-anywhere">
          {item.error.split('\n')[0]}
        </p>
      )}
      {tail && item.status !== 'approval' && (
        <p className="min-w-0 truncate font-mono text-[11px] text-content-muted">{tail}</p>
      )}
      <div className="flex min-w-0 items-center gap-1.5">
        {time && <StripSinceText since={time} className="text-content-muted" />}
        <span className="ml-auto flex items-center gap-1">
          <StripItemActions item={item} handlers={handlers} size="row" />
          <Button
            type="button"
            variant="ghost"
            className="h-7 px-2 text-xs pointer-coarse:h-11"
            onClick={() => handlers.onOpen(item.id)}
          >
            {t('board:strip.open')}
            <ArrowRight aria-hidden="true" />
          </Button>
        </span>
      </div>
      {footer}
    </div>
  )
}
