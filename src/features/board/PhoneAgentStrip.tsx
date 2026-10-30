// 手机会话页的代理条（规格 7.3）：44px 的着色芯片横向滚动，需要你的排在最前，始终带标题与状态词
// （“修复 CI 失败 · 批准”）；当前会话标出。点芯片打开操作底部表单：完整命令、问题或错误，
// [允许][拒绝]、[重试]、[打开会话]，处理别的代理不用离开当前会话。
// 数据只读看板 store（useBoardItems，与看板、泳道条同一个所有者），不新开轮询或事件流。
// 批准只交给 useStripApprovals：critical、截断的预览或详情未到时，“允许…”只打开同一个按住确认表单。
// 软键盘弹起时整条隐藏，把高度让给转录与输入框。
import { useMemo, useState } from 'react'
import { ArrowRight, RotateCcw, ShieldAlert, X } from 'lucide-react'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { SheetDragHandle } from '@/components/app/SheetDragHandle'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { cn } from '@/lib/utils'
import { ApprovalConfirm } from './ApprovalConfirm'
import type { BoardApproval } from './board-api'
import type { BoardItem } from './board-model'
import {
  boardStatusGlyph,
  approvalArgsText,
  boardCardTone,
  guardReasonSummary,
  isGuardReason,
  projectLabel,
} from './card-presentation'
import { phoneStripItems } from './phone-strip'
import { sinceLabel, useStripWords, type StripSince } from './strip-presentation'
import { useStripApprovals } from './useBoardController'
import { useBoardItems } from './useBoardItems'
import { useNow } from './useNow'

export type PhoneAgentStripProps = {
  currentId: string
  notify: Notify
  // 打开另一个代理的会话：只换路由，看板 store 不变。
  onOpenSession: (sessionId: string) => void
}

const TONE_TEXT = {
  attn: 'text-st-attn',
  fail: 'text-st-fail',
  run: 'text-st-run',
  done: 'text-st-done',
  queue: 'text-st-queue',
} as const

// 芯片底色：需要你琥珀浅底、失败红色浅底、运行中实底强描边，其余是纸面细描边。
const TONE_CHIP = {
  attn: 'border-st-attn-border bg-st-attn-soft',
  fail: 'border-st-fail-border bg-st-fail-soft',
  run: 'border-stroke-strong bg-solid',
  done: 'border-stroke bg-panel',
  queue: 'border-stroke bg-panel',
} as const

export function PhoneAgentStrip({ currentId, notify, onOpenSession }: PhoneAgentStripProps) {
  const { t } = useI18n()
  const word = useStripWords()
  const { items, capabilities, isLoading } = useBoardItems('today')
  const shown = useMemo(
    () => phoneStripItems(items, currentId, { queue: capabilities.sessionQueue }),
    [capabilities.sessionQueue, currentId, items],
  )
  const attentionAll = useMemo(() => items.filter((item) => item.lane === 'attention'), [items])
  const approvals = useStripApprovals({ attentionAll, notify })
  const [selectedId, setSelectedId] = useState<string | null>(null)
  // 表单展示的条目按 id 从最新数据解析：别处处理掉或会话消失时，表单随之关闭。
  const selected = selectedId ? (items.find((item) => item.id === selectedId) ?? null) : null
  const [lastSelected, setLastSelected] = useState<BoardItem | null>(null)
  if (selected && selected !== lastSelected) setLastSelected(selected)
  const sheetItem = selected ?? lastSelected
  // 芯片上不显示时长：只有打开操作表单（显示“已等 / 已运行”）时才每秒推进。
  const now = useNow(Boolean(selected))

  if (!shown.length && !isLoading) return null

  const close = () => setSelectedId(null)

  return (
    <nav
      aria-label={t('board:phone.stripLabel')}
      data-agent-strip
      className="relative z-10 flex-none border-b border-stroke-soft bg-chrome [[data-mobile-keyboard='open']_&]:hidden"
    >
      <div
        data-agent-strip-row
        className="flex h-[52px] items-center gap-2 overflow-x-auto overflow-y-hidden px-3 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
      >
        {isLoading && !shown.length ? (
          <span role="status" className="px-1 text-xs text-content-muted">
            {t('board:phone.stripLoading')}
          </span>
        ) : (
          shown.map((item) => {
            const tone = boardCardTone(item)
            const current = item.id === currentId
            const status = word(item)
            return (
              <button
                key={item.id}
                type="button"
                data-agent-chip={item.id}
                data-lane={item.lane}
                data-status={item.status}
                data-severity={item.approvals[0]?.severity}
                data-current={current || undefined}
                aria-current={current ? 'page' : undefined}
                aria-haspopup="dialog"
                aria-label={
                  current
                    ? t('board:phone.chipCurrent', { title: item.title, status })
                    : t('board:card.label', { title: item.title, status })
                }
                className={cn(
                  'flex h-11 max-w-[240px] flex-none items-center gap-2 rounded-card border pr-3 pl-2.5 text-[13px] font-medium text-content transition-colors duration-(--d-fast) focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus active:bg-surface-hover',
                  TONE_CHIP[tone],
                  current && 'ring-2 ring-stroke-strong',
                )}
                onClick={() => setSelectedId(item.id)}
              >
                <StatusGlyph
                  status={boardStatusGlyph(item)}
                  spin={item.status === 'running'}
                  className={cn('size-4 flex-none', TONE_TEXT[tone])}
                />
                <span className="min-w-0 truncate">{item.title}</span>
                <span className={cn('flex-none text-xs font-semibold', TONE_TEXT[tone])}>
                  {status}
                </span>
                {current && (
                  <span className="flex-none rounded-xs bg-surface-muted px-1 text-xs font-normal text-content-secondary">
                    {t('board:strip.current')}
                  </span>
                )}
              </button>
            )
          })
        )}
      </div>
      <Sheet open={Boolean(selected)} onOpenChange={(open) => !open && close()}>
        <SheetContent
          side="bottom"
          showCloseButton={false}
          aria-describedby="phone-agent-sheet-description"
          data-agent-sheet={sheetItem?.id}
          className="max-h-[85dvh] gap-3 overflow-y-auto rounded-t-(--r-sheet) px-4 pt-0 pb-[calc(16px+var(--pisper-safe-area-bottom))]"
        >
          <SheetDragHandle onDismiss={close} className="-mb-2" />
          {sheetItem && (
            <AgentActionSheetBody
              item={sheetItem}
              current={sheetItem.id === currentId}
              now={now}
              word={word(sheetItem)}
              pending={approvals.isPending}
              onClose={close}
              onApprove={(item, approval) => {
                close()
                approvals.approve(item, approval)
              }}
              onDeny={(item, approval) => {
                close()
                approvals.deny(item, approval)
              }}
              onReview={(item, approval) => {
                // 先收起操作表单再打开确认表单：两层底部表单不叠放，确认层是唯一的批准入口。
                close()
                approvals.review(item, approval)
              }}
              onRetry={(item) => {
                close()
                approvals.retry(item)
              }}
              onOpen={(id) => {
                close()
                onOpenSession(id)
              }}
            />
          )}
        </SheetContent>
      </Sheet>
      <ApprovalConfirm
        target={approvals.confirm?.target ?? null}
        phone
        pending={approvals.confirm?.pending ?? false}
        error={approvals.confirm?.error ?? ''}
        onConfirm={() => void approvals.confirmStripApproval()}
        onOpenChange={(value) => {
          if (!value) approvals.closeConfirm()
        }}
      />
    </nav>
  )
}

type ActionSheetBodyProps = {
  item: BoardItem
  current: boolean
  now: number
  word: string
  pending: ReturnType<typeof useStripApprovals>['isPending']
  onClose: () => void
  onApprove: (item: BoardItem, approval: BoardApproval) => void
  onDeny: (item: BoardItem, approval: BoardApproval) => void
  onReview: (item: BoardItem, approval: BoardApproval | null) => void
  onRetry: (item: BoardItem) => void
  onOpen: (sessionId: string) => void
}

function AgentActionSheetBody({
  item,
  current,
  now,
  word,
  pending,
  onClose,
  onApprove,
  onDeny,
  onReview,
  onRetry,
  onOpen,
}: ActionSheetBodyProps) {
  const { t, language } = useI18n()
  const tone = boardCardTone(item)
  const approval = item.status === 'approval' ? (item.approvals[0] ?? null) : null
  const critical = approval?.severity === 'critical'
  const command = approval ? approvalArgsText(approval) || approval.argsText || '' : ''
  // 截断的预览或详情未到：只能查看，不能在这里批准（确认表单等到完整参数才放行）。
  const reviewOnly = item.status === 'approval' && (!approval || approval.truncated)
  const since = sinceLabel(item, now, t, language)
  const project = projectLabel(item.cwd)
  const big = 'h-11 text-sm'
  return (
    <>
      <SheetHeader className="flex-row items-start gap-2 p-0">
        <StatusGlyph
          status={boardStatusGlyph(item)}
          spin={item.status === 'running'}
          className={cn('mt-0.5 size-5 flex-none', TONE_TEXT[tone])}
        />
        <div className="grid min-w-0 flex-1 gap-0.5">
          <SheetTitle className="flex min-w-0 items-baseline gap-2 text-base font-semibold">
            <span className="min-w-0 truncate">{item.title}</span>
            <span className={cn('flex-none text-sm font-semibold', TONE_TEXT[tone])}>{word}</span>
          </SheetTitle>
          <SheetDescription
            id="phone-agent-sheet-description"
            className="flex min-w-0 items-center gap-2 text-xs text-content-muted"
          >
            {project && <span className="min-w-0 truncate font-mono">{project}</span>}
            {since && <StripSinceText since={since} />}
            {current && (
              <span className="flex-none rounded-xs bg-surface-muted px-1 text-content-secondary">
                {t('board:strip.current')}
              </span>
            )}
          </SheetDescription>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="-mt-1 -mr-2 size-11 flex-none"
          aria-label={t('board:phone.closeSheet')}
          onClick={onClose}
        >
          <X aria-hidden="true" />
        </Button>
      </SheetHeader>
      <div className="grid min-w-0 gap-2 text-sm" data-agent-sheet-body>
        {item.status === 'approval' && (
          <>
            {approval && (
              <p
                className={cn(
                  'flex min-w-0 items-center gap-1.5 text-xs',
                  critical ? 'font-medium text-st-attn' : 'text-content-muted',
                )}
                data-agent-sheet-severity={approval.severity}
              >
                {critical && <ShieldAlert className="size-4 flex-none" aria-hidden="true" />}
                <span className="min-w-0 break-words">
                  {critical
                    ? isGuardReason(approval.reason)
                      ? t('board:card.criticalWithReason', {
                          reason: guardReasonSummary(approval.reason),
                        })
                      : t('board:card.critical')
                    : t('board:card.standard')}
                  {' · '}
                  <span className="font-mono">{approval.toolName}</span>
                </span>
              </p>
            )}
            {command && (
              <pre
                aria-label={t('board:confirm.fullArgs')}
                data-agent-sheet-command
                className="max-h-[40dvh] min-w-0 overflow-auto rounded-sm border border-well-stroke bg-well px-3 py-2 font-mono text-xs leading-5 whitespace-pre-wrap wrap-anywhere text-well-text"
              >
                {command}
              </pre>
            )}
            {reviewOnly && (
              <p role="status" className="text-xs text-content-muted" data-agent-sheet-review-only>
                {approval ? t('board:phone.truncated') : t('board:confirm.loading')}
              </p>
            )}
          </>
        )}
        {item.status === 'question' && item.preview && (
          <blockquote className="border-l-2 border-st-attn-border pl-3 text-content">
            {item.preview}
          </blockquote>
        )}
        {item.status === 'failed' && (item.error || item.tail.length > 0) && (
          <pre className="max-h-[30dvh] min-w-0 overflow-auto rounded-sm border border-st-fail-border bg-st-fail-soft px-3 py-2 font-mono text-xs leading-5 whitespace-pre-wrap wrap-anywhere text-st-fail">
            {item.error || item.tail.join('\n')}
          </pre>
        )}
        {(item.status === 'running' || item.status === 'queued') && item.tail.length > 0 && (
          <pre className="max-h-[30dvh] min-w-0 overflow-auto rounded-sm border border-well-stroke bg-well px-3 py-2 font-mono text-xs leading-5 whitespace-pre-wrap wrap-anywhere text-well-text">
            {item.tail.join('\n')}
          </pre>
        )}
        {(item.status === 'done' || item.status === 'stopped') && item.preview && (
          <p className="min-w-0 break-words text-content-secondary">{item.preview}</p>
        )}
      </div>
      <div className="grid gap-2" data-agent-sheet-actions>
        {item.status === 'approval' && (
          <div className="grid grid-cols-2 gap-2">
            <Button
              type="button"
              variant="secondary"
              className={big}
              data-agent-sheet-deny
              disabled={!approval || pending('deny', item.id, approval.id)}
              onClick={() => approval && onDeny(item, approval)}
            >
              {t('board:card.deny')}
            </Button>
            {reviewOnly ? (
              <Button
                type="button"
                variant="secondary"
                className={big}
                data-agent-sheet-review
                onClick={() => onReview(item, approval)}
              >
                {t('board:phone.reviewFull')}
              </Button>
            ) : critical && approval ? (
              // critical：默认样式的“允许…”，只打开按住 0.6 秒的确认表单，绝不一步批准。
              <Button
                type="button"
                variant="secondary"
                className={big}
                data-agent-sheet-approve="critical"
                onClick={() => onReview(item, approval)}
              >
                {t('board:card.approveCritical')}
              </Button>
            ) : (
              approval && (
                <Button
                  type="button"
                  className={big}
                  data-agent-sheet-approve="standard"
                  disabled={pending('approve', item.id, approval.id)}
                  onClick={() => onApprove(item, approval)}
                >
                  {t('board:card.approve')}
                </Button>
              )
            )}
          </div>
        )}
        {item.status === 'failed' && item.lane === 'attention' && (
          // 失败卡片唯一的主操作是重试（规格 3.4），与手机看板卡片一致用 --star 主按钮。
          <Button
            type="button"
            className={big}
            data-agent-sheet-retry
            disabled={pending('retry', item.id)}
            onClick={() => onRetry(item)}
          >
            <RotateCcw aria-hidden="true" />
            {t('board:card.retry')}
          </Button>
        )}
        {!current && (
          <Button
            type="button"
            variant="ghost"
            className={big}
            data-agent-sheet-open
            onClick={() => onOpen(item.id)}
          >
            {t('board:phone.openSession')}
            <ArrowRight aria-hidden="true" />
          </Button>
        )}
      </div>
    </>
  )
}

// 时长：无衬线前缀（“已等”“已运行”）加等宽数值；已完成是无衬线的相对时间。
function StripSinceText({ since, className }: { since: StripSince; className?: string }) {
  return (
    <span
      className={cn('flex flex-none items-baseline gap-1 text-xs whitespace-nowrap', className)}
    >
      {since.prefix && <span>{since.prefix}</span>}
      <span className={since.mono ? 'font-mono tabular-nums' : undefined}>{since.value}</span>
    </span>
  )
}
