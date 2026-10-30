// 代理卡片：状态标签 · 时长 · ⋯ 菜单 / 标题 / 元信息 / 按状态区分的正文。
// 整卡参与 roving tabindex：只有当前卡片在 Tab 序列里，卡内按钮也随之进出 Tab 序列。
// 一张卡片最多一个主操作（--star）：标准审批的“允许”、失败的“重试”；琥珀只用于需要你的元素。
// 批准前必须看到完整命令：命令框最多显示 3 行，超出用“展开”，从不截断文本本身。
// 第 2 期 card 摘要有数据才显示（运行中尾巴、失败摘要、结果摘要、改动统计、上下文百分比）；
// 没有 card 的降级形态保持第 1 期：一行活动、“N 条消息”、按需读 /change-summary。
import {
  memo,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from 'react'
import { useQuery } from '@tanstack/react-query'
import {
  ArrowRight,
  Archive,
  Check,
  CornerDownRight,
  Ellipsis,
  FileDiff,
  Link2,
  Pin,
  PinOff,
  RotateCcw,
  ScrollText,
  ShieldAlert,
  SquareTerminal,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { StatusTag } from '@/components/app/StatusTag'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Kbd } from '@/components/ui/kbd'
import { getSessionChangeSummary } from '@/features/chat/public-launch'
import { formatCompactDuration, relativeTime } from '@/lib/format'
import { formatShortcut } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { useShortcutStore } from '@/stores/shortcut-store'
import type { BoardApproval } from './board-api'
import type { BoardItem } from './board-model'
import {
  boardStatusGlyph,
  approvalArgsText,
  boardCardTone,
  boardEdgeWidth,
  boardSubAgent,
  contextPercent,
  elapsedSince,
  guardReasonSummary,
  isGuardReason,
  messageCount,
  modelShortLabel,
  projectLabel,
  tailLineFailed,
  visibleChangeStats,
} from './card-presentation'
import { useRunningMotion } from './useRunningMotion'

export type AgentCardHandlers = {
  onFocusCard: (sessionId: string) => void
  onOpen: (sessionId: string) => void
  onApprove: (item: BoardItem, approval: BoardApproval) => void
  onDeny: (item: BoardItem, approval: BoardApproval) => void
  onRetry: (item: BoardItem) => void
  onMarkHandled: (item: BoardItem) => void
  onPin: (item: BoardItem) => void
  onArchive: (item: BoardItem) => void
  onCopyLink: (item: BoardItem) => void
  isPending: (
    kind: 'approve' | 'deny' | 'retry' | 'archive' | 'pin',
    sessionId: string,
    approvalId?: string,
  ) => boolean
}

type AgentCardProps = {
  item: BoardItem
  now: number
  phone: boolean
  active: boolean
  dimmed?: boolean
  handlers: AgentCardHandlers
}

const EDGE_CLASSES = {
  attn: 'bg-st-attn-fill',
  fail: 'bg-st-fail-fill',
  run: 'bg-st-run-fill',
  done: 'bg-st-done-fill',
  queue: 'bg-st-queue-fill',
} as const

const BORDER_CLASSES = {
  attn: 'border-st-attn-border',
  fail: 'border-st-fail-border',
} as const

function interactiveTarget(target: EventTarget | null) {
  return (
    target instanceof HTMLElement &&
    Boolean(target.closest('button, a, input, textarea, [role="menuitem"], [data-card-stop]'))
  )
}

// 命令框：可换行，最多 3 行；内容超出时出现“展开”。文本始终完整，只是被裁在可视高度里。
// 先在空白处断行，单个词比一行还长时才拆词（overflow-wrap:anywhere），分支名与路径不会被拆在中间。
function CommandWell({
  text,
  prefix,
  phone,
  args = false,
}: {
  text: string
  prefix: string
  phone: boolean
  // 参数清单（没有命令和路径的审批）：读屏名与“展开”文案按参数说。
  args?: boolean
}) {
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
    <div className="min-w-0 rounded-sm border border-well-stroke bg-well">
      {/* 内边距放在外层：折叠到 3 行时，第 4 行不会从内边距里露出半截。 */}
      <div className="px-2.5 py-1.5">
        <pre
          ref={ref}
          aria-label={args ? t('board:confirm.fullArgs') : undefined}
          className={cn(
            'min-w-0 font-mono text-xs leading-5 whitespace-pre-wrap wrap-anywhere text-well-text',
            !expanded && 'line-clamp-3',
            phone && 'text-sm',
          )}
        >
          {prefix && <span className="text-well-muted select-none">{prefix} </span>}
          {text}
        </pre>
      </div>
      {(overflowing || expanded) && (
        <button
          type="button"
          data-card-stop
          className="w-full border-t border-well-stroke px-2.5 py-1 text-left text-xs text-content-secondary hover:text-content focus-visible:outline-2 focus-visible:outline-focus pointer-coarse:min-h-11"
          aria-expanded={expanded}
          onClick={() => setExpanded((value) => !value)}
        >
          {expanded
            ? t('board:card.collapse')
            : args
              ? t('board:card.expandArgs')
              : t('board:card.expand')}
        </button>
      )}
    </div>
  )
}

type MetaFit = 'all' | 'noModel' | 'projectOnly'

// 元信息行的截断优先级（规格 3.3）：项目标签至少保留 8 个字符；放不下时先整段去掉模型，
// 再去掉消息数，而不是把它们截成半个词。各段的自然宽度在可见时记下，容器变宽时据此恢复。
function useMetaFit(texts: string) {
  const ref = useRef<HTMLParagraphElement>(null)
  const widths = useRef({ project: 0, model: 0, count: 0 })
  const [fit, setFit] = useState<MetaFit>('all')
  const [overflowing, setOverflowing] = useState(false)
  useLayoutEffect(() => {
    // 文本变化后各段宽度作废：先全部显示，下一次测量重新决定。
    widths.current = { project: 0, model: 0, count: 0 }
    setFit('all')
  }, [texts])
  useLayoutEffect(() => {
    const node = ref.current
    if (!node) return
    const measure = () => {
      const part = (name: string) =>
        node.querySelector<HTMLElement>(`[data-meta-part="${name}"]`) ?? null
      const project = part('project')
      const model = part('model')
      const count = part('count')
      const known = widths.current
      if (project) {
        const style = getComputedStyle(project)
        const max = Number.parseFloat(style.maxWidth) || Infinity
        const padding =
          Number.parseFloat(style.paddingLeft) + Number.parseFloat(style.paddingRight) + 2
        known.project = Math.min(project.scrollWidth + padding, max)
      }
      if (model?.offsetWidth) known.model = model.scrollWidth
      if (count?.offsetWidth) known.count = count.scrollWidth
      const gap = Number.parseFloat(getComputedStyle(node).columnGap) || 0
      const minProject = project ? Number.parseFloat(getComputedStyle(project).minWidth) || 0 : 0
      const available = node.clientWidth
      const needed = (...parts: number[]) =>
        parts.filter((width) => width > 0).reduce((sum, width) => sum + width + gap, -gap)
      const next: MetaFit =
        needed(known.project, known.model, known.count) <= available
          ? 'all'
          : needed(Math.min(known.project, minProject || known.project), known.count) <= available
            ? 'noModel'
            : 'projectOnly'
      setFit((current) => (current === next ? current : next))
      setOverflowing(node.scrollWidth > node.clientWidth + 1)
    }
    measure()
    if (typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(measure)
    observer.observe(node)
    return () => observer.disconnect()
  }, [fit, texts])
  return { ref, fit, overflowing }
}

// 改动统计：只为进入可视区的已完成卡片请求 /change-summary，与历史页的指示器共用同一份缓存。
function ChangeStats({
  sessionId,
  completedAt,
}: {
  sessionId: string
  completedAt: string | null
}) {
  const { t } = useI18n()
  const anchor = useRef<HTMLSpanElement>(null)
  const [visible, setVisible] = useState(false)
  useEffect(() => {
    const node = anchor.current
    if (!node || visible) return
    if (typeof IntersectionObserver === 'undefined') {
      setVisible(true)
      return
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setVisible(true)
          observer.disconnect()
        }
      },
      { rootMargin: '120px' },
    )
    observer.observe(node)
    return () => observer.disconnect()
  }, [visible])
  const { data } = useQuery({
    queryKey: ['session-change-summary', sessionId, completedAt],
    queryFn: () => getSessionChangeSummary(sessionId),
    enabled: visible,
    staleTime: 60_000,
    retry: 0,
  })
  if (data?.status !== 'known' || !data.changedFiles) return <span ref={anchor} />
  return (
    <span ref={anchor} className="flex min-w-0 items-center gap-1.5 text-xs text-content-secondary">
      <span>{t('board:card.files', { count: data.changedFiles })}</span>
      <span className="font-mono text-st-done tabular-nums">+{data.added || 0}</span>
      <span className="font-mono text-st-fail tabular-nums">−{data.removed || 0}</span>
    </span>
  )
}

// card 自带的改动统计（Runtime 在写入类工具结束时缓存）：有它就不再单独读 /change-summary。
function CardChangeStats({
  files,
  added,
  removed,
}: {
  files: number
  added: number
  removed: number
}) {
  const { t } = useI18n()
  return (
    <span className="flex min-w-0 items-center gap-1.5 text-xs text-content-secondary">
      <span>{t('board:card.files', { count: files })}</span>
      <span className="font-mono text-st-done tabular-nums">+{added}</span>
      <span className="font-mono text-st-fail tabular-nums">−{removed}</span>
    </span>
  )
}

// 等宽尾巴：最后 2–3 行输出，失败行标红；运行中最后一行带闪烁光标（离屏时静止）。
// 不用 role="log"：尾巴每 250ms 更新一次，持续播报会淹没读屏，需要时由用户打开会话查看。
function TailWell({ lines, label, cursor }: { lines: string[]; label: string; cursor: boolean }) {
  return (
    <div
      role="group"
      aria-label={label}
      data-card-tail
      className="min-w-0 rounded-sm border border-well-stroke bg-well px-2.5 py-1.5 font-mono text-xs leading-5"
    >
      {lines.map((line, index) => (
        <p
          key={index}
          data-tail-fail={tailLineFailed(line) || undefined}
          className={cn('truncate', tailLineFailed(line) ? 'text-st-fail' : 'text-well-text')}
        >
          {line}
          {cursor && index === lines.length - 1 && (
            <span
              aria-hidden="true"
              data-tail-cursor
              className="ml-0.5 inline-block h-3 w-1.5 translate-y-0.5 bg-well-muted animate-[caret-blink_1s_steps(1,end)_infinite]"
            />
          )}
        </p>
      ))}
    </div>
  )
}

function Duration({ label, value }: { label: string; value: string }) {
  return (
    <span className="flex flex-none items-baseline gap-1 text-xs whitespace-nowrap text-content-muted">
      {label && <span>{label}</span>}
      <span className="font-mono tabular-nums">{value}</span>
    </span>
  )
}

function AgentCardComponent({ item, now, phone, active, dimmed, handlers }: AgentCardProps) {
  const { t, language } = useI18n()
  const tone = boardCardTone(item)
  const edge = boardEdgeWidth(item)
  const tabIndex = active ? 0 : -1
  // 卡片上的键帽与 aria-keyshortcuts 跟随“设置 → 快捷键”里看板单字母键的当前绑定。
  const approveKey = formatShortcut(useShortcutStore((state) => state.boardBindings.boardApprove))
  const denyKey = formatShortcut(useShortcutStore((state) => state.boardBindings.boardDeny))
  const retryKey = formatShortcut(useShortcutStore((state) => state.boardBindings.boardRetry))
  const approval = item.approvals[0] ?? null
  // 标签、按钮样式与实际动作都看同一条审批（第一条）；其余审批的最高等级只在“还有 N 条”里提示。
  const critical = approval?.severity === 'critical'
  const restCritical = item.approvals.slice(1).some((entry) => entry.severity === 'critical')
  const subAgent = boardSubAgent(item)
  const agentCounts = item.agents
  const model = modelShortLabel(item.session.model)
  const messages = messageCount(item)
  const context = contextPercent(item)
  // 第 N 轮 / 上下文：Runtime 给出上下文用量时显示百分比，否则如实写消息条数，不从条数估算轮次。
  const countLabel =
    context !== null
      ? t('board:card.context', { pct: context })
      : messages > 0
        ? t('board:card.messages', { count: messages })
        : ''
  const project = item.cwd ? projectLabel(item.cwd) : ''
  const meta = useMetaFit(`${project}\u0000${model}\u0000${countLabel}`)
  const cardRef = useRef<HTMLElement>(null)
  const running = item.status === 'running'
  const motion = useRunningMotion(cardRef, running)
  const elapsed = elapsedSince(item.since, now)
  const statusLabel =
    item.status === 'failed' && item.handled
      ? t('board:status.failedHandled')
      : {
          approval: t('board:status.approval'),
          question: t('board:status.question'),
          failed: t('board:status.failed'),
          running: t('board:status.running'),
          queued: t('board:status.queued'),
          done: t('board:status.done'),
          stopped: t('board:status.stopped'),
        }[item.status]
  const title = item.title || t('navigation:shell.untitledSession')
  const buttonSize = phone ? 'lg' : 'sm'

  let duration: ReactNode = null
  if (item.lane === 'attention' && elapsed !== null)
    duration = <Duration label={t('board:card.waited')} value={formatCompactDuration(elapsed)} />
  else if (item.lane === 'running' && item.startedAt !== null)
    duration = (
      <Duration
        label={t('board:card.ran')}
        value={formatCompactDuration(Math.max(0, now - item.startedAt))}
      />
    )
  else if (item.lane === 'done' && item.since !== null)
    duration = (
      <span className="flex-none text-xs whitespace-nowrap text-content-muted">
        {relativeTime(item.since, language)}
      </span>
    )

  const open = () => handlers.onOpen(item.id)
  // 按下落在卡片里的按钮等控件上时，这一次点击不算“打开卡片”：按下到松开之间版面若移动了
  // （例如泳道的溢出摘要收起），浏览器会把 click 派发给两者的共同祖先，不能因此误开会话。
  const pressOnControlRef = useRef(false)
  const clickCard = (event: MouseEvent<HTMLElement>) => {
    const pressedControl = pressOnControlRef.current
    pressOnControlRef.current = false
    if (pressedControl || interactiveTarget(event.target)) return
    open()
  }
  const keyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.target !== event.currentTarget) return
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault()
      open()
    }
  }

  // 降级形态：只有确知的活动才写一行（工具名 + 摘要 + 工具耗时，或思考中 / 回复中）；
  // 不知道时不重复“运行中”，状态标签已经说明，卡片保持矮小。
  const activityLine = () => {
    const activity = item.activity
    if (activity?.type === 'tool') {
      const toolStart = activity.startedAt ? Date.parse(activity.startedAt) : NaN
      return (
        <p className="flex min-w-0 items-center gap-2 text-xs">
          <span className="flex-none rounded-xs bg-st-run-soft px-1.5 font-mono leading-5 font-medium text-st-run">
            {activity.name}
          </span>
          <span className="min-w-0 flex-1 truncate font-mono text-content-secondary">
            {activity.summary}
          </span>
          {Number.isFinite(toolStart) && (
            <span className="flex-none font-mono text-content-muted tabular-nums">
              {formatCompactDuration(Math.max(0, now - toolStart))}
            </span>
          )}
        </p>
      )
    }
    if (activity?.type !== 'model') return null
    const stage =
      (
        {
          starting: t('board:activity.starting'),
          thinking: t('board:activity.thinking'),
          responding: t('board:activity.responding'),
          compacting: t('board:activity.compacting'),
        } as Record<string, string>
      )[activity.stage] || t('board:activity.working')
    return <p className="truncate text-xs text-content-secondary">{stage}</p>
  }

  // 运行中：有尾巴时先放等宽尾巴，活动行作为脚注；降级形态只有一行活动，卡片保持矮小。
  const runningBody = () => {
    const activity = activityLine()
    if (!item.tail.length) return activity
    return (
      <div className="grid min-w-0 gap-1.5">
        <TailWell lines={item.tail} label={t('board:card.tail')} cursor={motion.spin} />
        {activity}
      </div>
    )
  }

  const approvalBody = () => {
    // 详情未到（加载中、读取失败或刚被别处处理）时不给可点的“允许”，只能去会话里看上下文。
    if (item.approvalsState !== 'ready' || !approval)
      return (
        <div className="grid gap-2">
          <p role="status" className="text-xs text-content-muted">
            {item.approvalsState === 'error'
              ? t('board:card.approvalUnavailable')
              : t('board:card.approvalLoading')}
          </p>
          <div className="flex items-center justify-end">
            <Button variant="ghost" size={buttonSize} tabIndex={tabIndex} onClick={open}>
              {t('board:card.viewContext')}
              <ArrowRight aria-hidden="true" />
            </Button>
          </div>
        </div>
      )
    const guardReason = isGuardReason(approval.reason)
    const reason = critical && guardReason ? guardReasonSummary(approval.reason) : ''
    const isBash = approval.toolName === 'bash' && approval.command
    // 命令或路径优先；两者都没有时（插件安装、浏览器交互、MCP 工具）展示完整参数和请求原因。
    const commandText = approval.command || approval.path
    const argsText = commandText ? '' : approvalArgsText(approval)
    const reasonLine = !commandText && approval.reason && !guardReason ? approval.reason : ''
    const approving = handlers.isPending('approve', item.id, approval.id)
    const denying = handlers.isPending('deny', item.id, approval.id)
    return (
      <div className="grid min-w-0 gap-2">
        <div className="flex min-w-0 items-center gap-2 text-xs">
          <SquareTerminal className="size-3.5 flex-none text-content-muted" aria-hidden="true" />
          <span className="min-w-0 truncate text-content-secondary">
            {t('board:card.requests')}{' '}
            <b className="font-mono font-semibold text-content">{approval.toolName || '—'}</b>
          </span>
          {/* 危险等级用琥珀（需要你的颜色）而不是失败红：红色只表示失败。 */}
          <span
            className={cn(
              'ml-auto flex min-w-0 flex-none items-center gap-1',
              critical
                ? 'max-w-[60%] rounded-xs border border-st-attn-border bg-st-attn-soft px-1.5 leading-4 font-semibold text-st-attn'
                : 'font-medium text-content-muted',
            )}
            data-severity={critical ? 'critical' : 'standard'}
          >
            {critical && <ShieldAlert className="size-3.5 flex-none" aria-hidden="true" />}
            <span className="min-w-0 truncate">
              {critical
                ? reason
                  ? t('board:card.criticalWithReason', { reason })
                  : t('board:card.critical')
                : t('board:card.standard')}
            </span>
          </span>
        </div>
        {commandText ? (
          <CommandWell text={commandText} prefix={isBash ? '$' : ''} phone={phone} />
        ) : argsText ? (
          <CommandWell text={argsText} prefix="" phone={phone} args />
        ) : null}
        {reasonLine && (
          <p className="min-w-0 text-xs break-words text-content-secondary">{reasonLine}</p>
        )}
        {item.approvals.length > 1 && (
          <p className="text-xs text-content-muted">
            {restCritical
              ? t('board:card.moreApprovalsCritical', { count: item.approvals.length - 1 })
              : t('board:card.moreApprovals', { count: item.approvals.length - 1 })}
          </p>
        )}
        <div className={cn('flex min-w-0 items-center gap-2', phone && 'grid grid-cols-2')}>
          <Button
            variant="outline"
            size={buttonSize}
            tabIndex={tabIndex}
            disabled={approving || denying}
            loading={denying}
            aria-keyshortcuts={denyKey || undefined}
            className={cn(phone && 'min-h-11')}
            onClick={() => handlers.onDeny(item, approval)}
          >
            <X aria-hidden="true" />
            {t('board:card.deny')}
            {!phone && denyKey && <Kbd className="ml-0.5">{denyKey}</Kbd>}
          </Button>
          <Button
            variant={critical ? 'outline' : 'default'}
            size={buttonSize}
            tabIndex={tabIndex}
            disabled={approving || denying}
            loading={approving}
            aria-keyshortcuts={approveKey || undefined}
            aria-haspopup={critical ? 'dialog' : undefined}
            className={cn(phone && 'min-h-11')}
            onClick={() => handlers.onApprove(item, approval)}
          >
            <Check aria-hidden="true" />
            {critical ? t('board:card.approveCritical') : t('board:card.approve')}
            {!phone && approveKey && (
              <Kbd
                className={cn(
                  'ml-0.5',
                  !critical && 'border-transparent bg-on-accent/15 text-on-accent',
                )}
              >
                {approveKey}
              </Kbd>
            )}
          </Button>
          {!phone && (
            <Button
              variant="ghost"
              size={buttonSize}
              tabIndex={tabIndex}
              className="ml-auto min-w-0"
              onClick={open}
            >
              <span className="truncate">{t('board:card.viewContext')}</span>
              <ArrowRight aria-hidden="true" />
            </Button>
          )}
        </div>
      </div>
    )
  }

  const failureBody = () => {
    const retrying = handlers.isPending('retry', item.id)
    const lines = item.error
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .slice(0, 3)
    // card 的错误摘要只有一行；再补上失败前最后的输出（最多 2 行），失败行标红，帮助判断。
    const tail = item.tail.slice(-Math.max(0, 3 - lines.length))
    return (
      <div className="grid min-w-0 gap-2">
        {(lines.length > 0 || tail.length > 0) && (
          <div
            data-card-error
            className="min-w-0 rounded-sm border border-well-stroke bg-well px-2.5 py-1.5 font-mono text-xs leading-5"
          >
            {lines.length > 0 && (
              <p className="truncate text-st-fail">
                <span aria-hidden="true">× </span>
                {lines[0]}
              </p>
            )}
            {lines.slice(1).map((line, index) => (
              <p key={index} className="truncate text-well-muted">
                {line}
              </p>
            ))}
            {tail.map((line, index) => (
              <p
                key={`tail-${index}`}
                className={cn(
                  'truncate',
                  tailLineFailed(line) ? 'text-st-fail' : 'text-well-muted',
                )}
              >
                {line}
              </p>
            ))}
          </div>
        )}
        <div className={cn('flex min-w-0 items-center gap-2', phone && 'grid grid-cols-2')}>
          <Button
            variant="outline"
            size={buttonSize}
            tabIndex={tabIndex}
            className={cn(phone && 'min-h-11')}
            onClick={open}
          >
            <ScrollText aria-hidden="true" />
            {t('board:card.viewLog')}
          </Button>
          {item.lane === 'attention' ? (
            <Button
              size={buttonSize}
              tabIndex={tabIndex}
              loading={retrying}
              disabled={retrying}
              aria-keyshortcuts={retryKey || undefined}
              className={cn(phone && 'min-h-11')}
              onClick={() => handlers.onRetry(item)}
            >
              <RotateCcw aria-hidden="true" />
              {t('board:card.retry')}
              {!phone && retryKey && (
                <Kbd className="ml-0.5 border-transparent bg-on-accent/15 text-on-accent">
                  {retryKey}
                </Kbd>
              )}
            </Button>
          ) : (
            <Button
              variant="ghost"
              size={buttonSize}
              tabIndex={tabIndex}
              className={cn(phone && 'min-h-11')}
              disabled={handlers.isPending('archive', item.id)}
              onClick={() => handlers.onArchive(item)}
            >
              <Archive aria-hidden="true" />
              {t('board:card.archive')}
            </Button>
          )}
        </div>
      </div>
    )
  }

  const cardStats = visibleChangeStats(item)
  // card 带了改动统计（哪怕是 unavailable）就不再单独读 /change-summary；没有 card 时按需懒加载。
  const lazyStats = !item.changeStats
  const doneBody = () => (
    <div className="grid min-w-0 gap-2">
      {item.preview && (
        <p
          data-card-preview
          className="line-clamp-2 text-xs leading-5 break-words text-content-secondary"
        >
          {item.preview}
        </p>
      )}
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        {cardStats ? (
          <CardChangeStats {...cardStats} />
        ) : lazyStats ? (
          <ChangeStats
            sessionId={item.id}
            completedAt={
              typeof item.session.lastCompletedAt === 'string' ? item.session.lastCompletedAt : null
            }
          />
        ) : null}
        <div className="ml-auto flex items-center gap-1">
          <Button variant="outline" size={buttonSize} tabIndex={tabIndex} onClick={open}>
            <FileDiff aria-hidden="true" />
            {t('board:card.viewChanges')}
          </Button>
          <Button
            variant="ghost"
            size={buttonSize}
            tabIndex={tabIndex}
            disabled={handlers.isPending('archive', item.id)}
            onClick={() => handlers.onArchive(item)}
          >
            {t('board:card.archive')}
          </Button>
        </div>
      </div>
    </div>
  )

  const body =
    item.status === 'approval'
      ? approvalBody()
      : item.status === 'failed'
        ? failureBody()
        : item.status === 'running'
          ? runningBody()
          : item.lane === 'done'
            ? doneBody()
            : null

  return (
    <article
      ref={cardRef}
      data-board-card={item.id}
      data-lane={item.lane}
      data-status={item.status}
      data-severity={item.severity ?? undefined}
      tabIndex={tabIndex}
      aria-label={t('board:card.label', { title, status: statusLabel })}
      aria-keyshortcuts="Enter"
      onFocus={(event) => {
        if (event.target === event.currentTarget || event.currentTarget.contains(event.target))
          handlers.onFocusCard(item.id)
      }}
      onPointerDownCapture={(event) => {
        pressOnControlRef.current = interactiveTarget(event.target)
      }}
      onClick={clickCard}
      onKeyDown={keyDown}
      // 不写 outline-none：Tailwind v4 的 outline-none 会把 --tw-outline-style 置为 none，
      // 连 focus-visible 的 2px 焦点环也一起吃掉；浏览器本身只在 :focus-visible 时画轮廓。
      className={cn(
        '@container relative flex min-w-0 cursor-pointer flex-col gap-1.5 overflow-hidden rounded-card border bg-solid py-3 pr-3.5 pl-4 shadow-card transition-[opacity,border-color] duration-(--d-fast) hover:border-stroke-strong focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus',
        item.lane === 'attention'
          ? BORDER_CLASSES[tone === 'fail' ? 'fail' : 'attn']
          : 'border-stroke',
        dimmed && 'opacity-50',
        phone && 'gap-2 py-3.5 pr-4 pl-5',
      )}
    >
      <span
        aria-hidden="true"
        data-card-edge
        className={cn('absolute inset-y-0 left-0 overflow-hidden', EDGE_CLASSES[tone])}
        style={{ width: edge }}
      >
        {/* 运行中沿边扫光：只动 transform；离屏、超过 6 张或减少动效时不挂载。 */}
        {motion.sweep && (
          <span
            data-card-sweep
            className="absolute inset-x-0 top-0 h-1/3 -translate-y-full bg-linear-to-b from-transparent via-solid/80 to-transparent animate-[edge-sweep_2.4s_linear_infinite]"
          />
        )}
      </span>
      <div className="flex min-w-0 items-center gap-2">
        <StatusTag
          status={boardStatusGlyph(item)}
          tone={boardCardTone(item)}
          // 运行中图标只在卡片可见时旋转。
          spin={running ? motion.spin : undefined}
        >
          {statusLabel}
        </StatusTag>
        {item.lane === 'done' && item.unread && (
          <span className="flex flex-none items-center gap-1 text-xs font-medium text-content">
            <span aria-hidden="true" className="size-1.5 rounded-full bg-content" />
            {t('board:card.unread')}
          </span>
        )}
        <span className="min-w-0 flex-1" />
        {duration}
        {item.pinned && (
          <Pin
            className="size-3.5 flex-none rotate-45 text-content-muted"
            role="img"
            aria-label={t('board:card.pinned')}
          />
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-xs"
              tabIndex={tabIndex}
              className="-my-1 -mr-1 flex-none pointer-coarse:size-11"
              aria-label={t('board:card.more', { title })}
            >
              <Ellipsis aria-hidden="true" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-48">
            <DropdownMenuItem onSelect={open}>
              <ArrowRight aria-hidden="true" />
              {t('board:menu.open')}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => handlers.onPin(item)}>
              {item.pinned ? <PinOff aria-hidden="true" /> : <Pin aria-hidden="true" />}
              {item.pinned ? t('board:menu.unpin') : t('board:menu.pin')}
            </DropdownMenuItem>
            {item.status === 'failed' && !item.handled && (
              <DropdownMenuItem onSelect={() => handlers.onMarkHandled(item)}>
                <Check aria-hidden="true" />
                {t('board:menu.markHandled')}
              </DropdownMenuItem>
            )}
            <DropdownMenuItem onSelect={() => handlers.onCopyLink(item)}>
              <Link2 aria-hidden="true" />
              {t('board:menu.copyLink')}
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => handlers.onArchive(item)}>
              <Archive aria-hidden="true" />
              {t('board:menu.archive')}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <h3
        className={cn(
          'line-clamp-2 text-base leading-5 font-semibold break-words text-content',
          phone && 'text-[calc(var(--text-base)+1px)] leading-[22px]',
        )}
      >
        {title}
      </h3>
      {(project || model || countLabel) && (
        <p
          ref={meta.ref}
          className="flex min-w-0 items-center gap-1.5 overflow-hidden text-xs whitespace-nowrap text-content-muted data-overflowing:[mask-image:linear-gradient(to_right,black_calc(100%-20px),transparent)]"
          data-card-meta
          data-meta-fit={meta.fit}
          data-overflowing={meta.overflowing || undefined}
        >
          {project && (
            <span
              data-meta-part="project"
              className="max-w-[24ch] min-w-[8ch] shrink truncate rounded-xs border border-stroke-soft bg-surface-subtle px-1.5 font-mono leading-5 text-content-secondary"
              title={item.cwd}
            >
              {project}
            </span>
          )}
          {model && meta.fit === 'all' && (
            <span data-meta-part="model" className="flex flex-none items-center gap-1.5">
              <span aria-hidden="true">·</span>
              <span>{model}</span>
            </span>
          )}
          {countLabel && meta.fit !== 'projectOnly' && (
            <span
              data-meta-part="count"
              data-meta-context={context !== null || undefined}
              className="flex flex-none items-center gap-1.5"
            >
              <span aria-hidden="true">·</span>
              <span className={cn(context !== null && 'tabular-nums')}>{countLabel}</span>
            </span>
          )}
        </p>
      )}
      {body && <div className="mt-1 min-w-0">{body}</div>}
      {(subAgent || (agentCounts && agentCounts.running > 0)) && (
        <p
          data-card-agents
          className="flex min-w-0 items-center gap-1.5 border-t border-stroke-soft pt-2 text-xs text-content-secondary"
        >
          <CornerDownRight className="size-3.5 flex-none text-content-muted" aria-hidden="true" />
          <span className="flex-none">{t('board:card.subAgent')}</span>
          <span aria-hidden="true">·</span>
          <span className="min-w-0 truncate">
            {subAgent?.name ||
              t('board:card.subAgentsRunning', { count: agentCounts?.running ?? 0 })}
          </span>
          {agentCounts ? (
            // card 的汇总包含已结束的子代理：显示“运行中 / 总数”，比名字后的 +N 更完整。
            <span
              className="ml-auto flex-none font-mono tabular-nums text-content-muted"
              title={t('board:card.subAgentProgress', agentCounts)}
            >
              <span aria-hidden="true">
                {agentCounts.running}/{agentCounts.total}
              </span>
              <span className="sr-only">{t('board:card.subAgentProgress', agentCounts)}</span>
            </span>
          ) : (
            subAgent &&
            subAgent.extra > 0 && (
              <span className="flex-none font-mono tabular-nums text-content-muted">
                +{subAgent.extra}
              </span>
            )
          )}
        </p>
      )}
    </article>
  )
}

export const AgentCard = memo(AgentCardComponent)
