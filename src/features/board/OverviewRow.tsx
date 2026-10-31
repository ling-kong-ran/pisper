// 全部代理的一行（规格 v2 第 3.3 节）：状态图形 · 标题 / 状态 / 项目 · 按状态区分的第 2 行 · 行内操作 · 时间。
// 每个代理一行（不是卡片墙），行高随内容 52–96；整行参与 roving tabindex，↵ 打开会话，⋯ 菜单同侧栏行。
// 一行最多一个主操作（石墨）：普通批准的“允许”、失败的“重试”、高风险 / 提问 / 详情未到的“打开处理”。
// 批准前必须看到完整命令：命令最多显示 3 行，超出用“展开”，从不截断文本本身（CommandWell）。
// 琥珀只用于需要你：圆点、原因文字、高风险说明；diff 统计用红绿（规格 6.1 例外）。
import {
  memo,
  useLayoutEffect,
  useRef,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from 'react'
import { CornerDownRight, Pin } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { Button } from '@/components/ui/button'
import { formatShortcut } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import type { AgentCardHandlers } from './AgentCard'
import type { BoardItem } from './board-model'
import {
  approvalArgsText,
  boardStatusGlyph,
  guardReasonSummary,
  isGuardReason,
  tailLineFailed,
  visibleChangeStats,
} from './card-presentation'
import { CardChangeStats, ChangeStats } from './ChangeStats'
import { CommandWell } from './CommandWell'
import { SessionRowMenu } from './SessionRowMenu'
import { sidebarQuickAction, sidebarReasons } from './sidebar-model'
import {
  formatSidebarClock,
  formatSidebarTime,
  formatSidebarWait,
  reasonText,
} from './sidebar-presentation'
import type { SessionRowActions } from './useSessionRowActions'

export type OverviewRowProps = {
  item: BoardItem
  now: number
  // 按状态分组时行里写项目名；按项目分组时项目已在分区标题里。
  project: string
  active: boolean
  // 逐个处理时非需要你的行降到 50% 不透明度。
  dimmed: boolean
  // 运行中弧的相位（0–1），同组多行错开。
  phase?: number
  bindings: { approve: string | null; deny: string | null; retry: string | null }
  handlers: AgentCardHandlers
  menu: SessionRowActions
  // 排队行的“立即开始”：sessionQueue（A9）提供之前为空，不显示按钮。
  onStartNow?: (item: BoardItem) => void
}

// 用 Element 而不是 HTMLElement：按钮里的图标是 SVG（lucide 的 circle / path），点在图标上时
// 事件目标是 SVGElement，按 HTMLElement 判断会漏掉，点 ⋯ 或“允许”的图标就会顺带打开会话。
function interactiveTarget(target: EventTarget | null) {
  return (
    target instanceof Element &&
    Boolean(target.closest('button, a, input, textarea, [role="menuitem"], [data-card-stop]'))
  )
}

// 单字母键帽（KeyA → A）；改绑成组合键时只靠 aria-keyshortcuts 与速查说明，不在小按钮里挤长键名。
function KeyHint({ binding }: { binding: string | null }) {
  const label = binding && /^Key[A-Z]$/.test(binding) ? binding.slice(3) : ''
  if (!label) return null
  return (
    <span aria-hidden="true" className="font-normal opacity-60">
      {label}
    </span>
  )
}

function firstLine(text: string) {
  return (
    text
      .split('\n')
      .map((line) => line.trim())
      .find(Boolean) ?? ''
  )
}

function OverviewRowComponent({
  item,
  now,
  project,
  active,
  dimmed,
  phase,
  bindings,
  handlers,
  menu,
  onStartNow,
}: OverviewRowProps) {
  const { t } = useI18n()
  const rowRef = useRef<HTMLElement>(null)
  const tabIndex = active ? 0 : -1
  const attention = item.lane === 'attention'
  const approval = item.approvals[0] ?? null
  const critical = approval?.severity === 'critical'
  const quick = sidebarQuickAction(item)
  const title = item.title || t('navigation:shell.untitledSession')
  const open = () => handlers.onOpen(item.id)

  // 换了状态的行（允许后进运行中、重试后开始运行）在按项目分组时仍是同一个 DOM 元素：
  // 处理前的收起动画停在透明，这里撤掉它，行在新位置原样出现。首次挂载不动。
  const statusKey = `${item.lane}:${item.status}`
  const statusRef = useRef(statusKey)
  useLayoutEffect(() => {
    if (statusRef.current === statusKey) return
    statusRef.current = statusKey
    rowRef.current?.getAnimations?.().forEach((animation) => animation.cancel())
  }, [statusKey])

  // 按下落在行里的按钮等控件上时，这一次点击不算“打开”：按下到松开之间版面若移动了，
  // 浏览器会把 click 派发给两者的共同祖先（行本身），不能因此误开会话。
  const pressOnControlRef = useRef(false)
  const clickRow = (event: MouseEvent<HTMLElement>) => {
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

  // —— 第 1 行：状态文字 ——
  let statusText = ''
  if (attention) statusText = reasonText(t, sidebarReasons(item))
  else if (item.status === 'failed') statusText = t('board:status.failedHandled')
  else if (item.status === 'stopped') statusText = t('board:status.stopped')
  else if (item.lane === 'running') statusText = t('board:status.running')
  else if (item.lane === 'queued') statusText = t('board:status.queued')
  else statusText = item.unread ? t('board:overview.doneUnread') : t('board:status.done')

  // —— 右侧时间：需要你等待时长、运行中计时、今天完成的完成时刻 ——
  let time = ''
  let timeLabel = ''
  if (attention && item.since !== null) {
    time = formatSidebarWait(now - item.since)
    timeLabel = t('board:overview.waited', { wait: time })
  } else if (item.lane === 'running') {
    const start = item.startedAt ?? item.since
    time = formatSidebarClock(start === null ? null : now - start)
    timeLabel = time ? t('board:overview.ran', { time }) : ''
  } else if (item.lane === 'done') {
    time = formatSidebarTime(item.since)
    timeLabel = time ? t('board:overview.finishedAt', { time }) : ''
  }

  // —— 第 2 行与行内操作 ——
  let detail: ReactNode = null
  let actions: ReactNode = null
  const openButton = (
    <Button size="sm" tabIndex={tabIndex} onClick={open}>
      {t('board:overview.openToHandle')}
      <span aria-hidden="true" className="font-normal opacity-60">
        ↵
      </span>
    </Button>
  )

  if (item.status === 'approval') {
    if (item.approvalsState !== 'ready' || !approval) {
      detail = (
        <p role="status" className="text-xs leading-5 text-content-muted">
          {item.approvalsState === 'error'
            ? t('board:card.approvalUnavailable')
            : t('board:card.approvalLoading')}
        </p>
      )
    } else {
      const guardReason = isGuardReason(approval.reason)
      const riskReason = critical && guardReason ? guardReasonSummary(approval.reason) : ''
      const isBash = approval.toolName === 'bash' && approval.command
      // 命令或路径优先；两者都没有时（插件安装、浏览器交互、MCP 工具）展示完整参数和请求原因。
      const commandText = approval.command || approval.path
      const argsText = commandText ? '' : approvalArgsText(approval)
      const reasonLine = !commandText && approval.reason && !guardReason ? approval.reason : ''
      const rest = item.approvals.length - 1
      const restCritical = item.approvals.slice(1).some((entry) => entry.severity === 'critical')
      detail = (
        <div className="grid min-w-0 gap-0.5">
          <p className="min-w-0 truncate text-xs leading-5 text-content-muted">
            {critical && (
              <span data-severity="critical" className="font-medium text-warning-strong">
                {riskReason
                  ? t('board:card.criticalWithReason', { reason: riskReason })
                  : t('board:card.critical')}
                <span aria-hidden="true"> · </span>
              </span>
            )}
            {t('board:card.requests')}{' '}
            <b className="font-mono font-medium text-content-secondary">
              {approval.toolName || '—'}
            </b>
          </p>
          {commandText ? (
            <CommandWell
              text={commandText}
              prefix={isBash ? '$' : ''}
              variant="plain"
              tabIndex={tabIndex}
            />
          ) : argsText ? (
            <CommandWell text={argsText} prefix="" variant="plain" args tabIndex={tabIndex} />
          ) : null}
          {reasonLine && (
            <p className="min-w-0 text-xs break-words text-content-secondary">{reasonLine}</p>
          )}
          {rest > 0 && (
            <p className="text-xs leading-5 text-content-muted">
              {restCritical
                ? t('board:card.moreApprovalsCritical', { count: rest })
                : t('board:card.moreApprovals', { count: rest })}
            </p>
          )}
        </div>
      )
    }
    if (quick.kind === 'decide' && approval) {
      const approving = handlers.isPending('approve', item.id, approval.id)
      const denying = handlers.isPending('deny', item.id, approval.id)
      actions = (
        <>
          <Button
            variant="outline"
            size="sm"
            tabIndex={tabIndex}
            disabled={approving || denying}
            loading={denying}
            aria-label={t('board:sidebar.denyNamed', { title })}
            aria-keyshortcuts={formatShortcut(bindings.deny) || undefined}
            onClick={() => handlers.onDeny(item, approval)}
          >
            {t('board:card.deny')}
            <KeyHint binding={bindings.deny} />
          </Button>
          <Button
            size="sm"
            tabIndex={tabIndex}
            disabled={approving || denying}
            loading={approving}
            aria-label={t('board:sidebar.approveNamed', { title })}
            aria-keyshortcuts={formatShortcut(bindings.approve) || undefined}
            onClick={() => handlers.onApprove(item, approval)}
          >
            {t('board:card.approve')}
            <KeyHint binding={bindings.approve} />
          </Button>
        </>
      )
    } else actions = openButton
  } else if (item.status === 'question') {
    // 问题正文与选项胶囊在 A5（askUser）接入；在那之前只能进会话回答。
    actions = openButton
  } else if (item.status === 'failed' && attention) {
    const summary =
      firstLine(item.error) || [...item.tail].reverse().find(tailLineFailed)?.trim() || ''
    const retrying = handlers.isPending('retry', item.id)
    detail = summary ? (
      <p
        data-card-error
        className="min-w-0 truncate font-mono text-xs leading-5 text-content-secondary"
      >
        {summary}
      </p>
    ) : null
    actions = (
      <>
        <Button variant="outline" size="sm" tabIndex={tabIndex} onClick={open}>
          {t('board:overview.log')}
        </Button>
        <Button
          size="sm"
          tabIndex={tabIndex}
          loading={retrying}
          disabled={retrying}
          aria-label={t('board:overview.retryNamed', { title })}
          aria-keyshortcuts={formatShortcut(bindings.retry) || undefined}
          onClick={() => handlers.onRetry(item)}
        >
          {t('board:card.retry')}
          <KeyHint binding={bindings.retry} />
        </Button>
      </>
    )
  } else if (item.lane === 'running') {
    const activity = item.activity
    const agents = item.agents
    let line: ReactNode = null
    if (activity?.type === 'tool') {
      const toolStart = activity.startedAt ? Date.parse(activity.startedAt) : NaN
      const parts = [activity.name, activity.summary].filter(Boolean).join(' · ')
      const elapsed = Number.isFinite(toolStart)
        ? formatSidebarWait(Math.max(0, now - toolStart))
        : ''
      line = (
        <span className="min-w-0 truncate">
          {parts}
          {elapsed && <span className="text-content-muted"> · {elapsed}</span>}
        </span>
      )
    } else if (item.tail.length) {
      line = <span className="min-w-0 truncate">{item.tail.at(-1)}</span>
    } else if (activity?.type === 'model') {
      const stage =
        (
          {
            starting: t('board:activity.starting'),
            thinking: t('board:activity.thinking'),
            responding: t('board:activity.responding'),
            compacting: t('board:activity.compacting'),
          } as Record<string, string>
        )[activity.stage] || t('board:activity.working')
      line = <span className="min-w-0 truncate font-sans">{stage}</span>
    }
    if (line || (agents && agents.total > 0))
      detail = (
        <p
          data-card-activity
          className="flex min-w-0 items-center gap-2 font-mono text-xs leading-5 text-content-secondary"
        >
          {line}
          {agents && agents.total > 0 && (
            <span
              data-card-agents
              className="flex flex-none items-center gap-1 font-sans text-content-muted"
              title={t('board:card.subAgentProgress', agents)}
            >
              <CornerDownRight className="size-3" aria-hidden="true" />
              <span aria-hidden="true">
                {t('board:overview.subAgents', { running: agents.running, total: agents.total })}
              </span>
              <span className="sr-only">{t('board:card.subAgentProgress', agents)}</span>
            </span>
          )}
        </p>
      )
  } else if (item.lane === 'queued') {
    if (onStartNow)
      actions = (
        <Button variant="outline" size="sm" tabIndex={tabIndex} onClick={() => onStartNow(item)}>
          {t('board:overview.startNow')}
        </Button>
      )
  } else if (item.lane === 'done') {
    const preview = firstLine(item.preview)
    detail = preview ? (
      <p data-card-preview className="min-w-0 truncate text-xs leading-5 text-content-secondary">
        {preview}
      </p>
    ) : null
    const stats = visibleChangeStats(item)
    // card 带了改动统计（哪怕是 unavailable）就不再单独读 /change-summary；没有 card 时按需懒加载。
    actions = stats ? (
      <CardChangeStats {...stats} />
    ) : !item.changeStats ? (
      <ChangeStats
        sessionId={item.id}
        completedAt={
          typeof item.session.lastCompletedAt === 'string' ? item.session.lastCompletedAt : null
        }
      />
    ) : null
  }

  return (
    <article
      ref={rowRef}
      data-board-card={item.id}
      data-overview-row
      data-lane={item.lane}
      data-status={item.status}
      data-severity={item.severity ?? undefined}
      tabIndex={tabIndex}
      aria-label={t('board:card.label', { title, status: statusText })}
      aria-keyshortcuts="Enter"
      onFocus={(event) => {
        if (event.target === event.currentTarget || event.currentTarget.contains(event.target))
          handlers.onFocusCard(item.id)
      }}
      onPointerDownCapture={(event) => {
        pressOnControlRef.current = interactiveTarget(event.target)
      }}
      onClick={clickRow}
      onKeyDown={keyDown}
      // 不写 outline-none：Tailwind v4 的 outline-none 会把 --tw-outline-style 置为 none，
      // 连 focus-visible 的 2px 焦点环也一起吃掉；浏览器本身只在 :focus-visible 时画轮廓。
      className={cn(
        'group/row relative grid min-h-[52px] min-w-0 cursor-pointer grid-cols-[16px_minmax(0,1fr)_auto] items-start gap-x-3 rounded-[var(--r-md)] px-3 py-[11px] transition-[opacity,background-color] duration-(--d-fast) hover:bg-surface-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus',
        dimmed && 'opacity-50',
      )}
    >
      <span className="mt-0.5 grid size-4 place-items-center">
        <StatusGlyph status={boardStatusGlyph(item)} phase={phase} />
      </span>
      <div className="flex min-w-0 flex-col gap-0.5">
        <p className="flex min-w-0 items-baseline gap-2.5 leading-5">
          <span
            className={cn(
              'min-w-0 truncate text-base font-medium',
              item.lane === 'done' ? 'text-content-secondary' : 'text-content',
            )}
          >
            {title}
          </span>
          <span
            className={cn(
              'flex-none text-xs',
              attention ? 'text-warning-strong' : 'text-content-muted',
            )}
          >
            {statusText}
          </span>
          {project && (
            <span
              data-row-project
              className="min-w-0 shrink truncate font-mono text-xs text-content-muted"
              title={item.cwd || undefined}
            >
              {project}
            </span>
          )}
          {item.pinned && (
            <Pin
              className="size-3 flex-none self-center text-content-muted"
              strokeWidth={1.75}
              role="img"
              aria-label={t('board:card.pinned')}
            />
          )}
        </p>
        {detail}
      </div>
      <div className="flex min-h-[30px] items-center gap-2 self-start">
        {actions && <div className="flex items-center gap-1.5">{actions}</div>}
        {time && (
          <span
            className={cn(
              'min-w-[44px] text-right text-xs text-content-muted tabular-nums',
              item.lane === 'running' && 'font-mono text-content-secondary',
            )}
          >
            <span aria-hidden="true">{time}</span>
            <span className="sr-only">{timeLabel}</span>
          </span>
        )}
        <SessionRowMenu
          row={{ id: item.id, title, pinned: item.pinned }}
          actions={menu}
          canMarkHandled={attention && item.status === 'failed'}
          align="end"
          tabIndex={tabIndex}
          className="opacity-0 group-hover/row:opacity-100 group-focus-within/row:opacity-100 data-[state=open]:opacity-100 pointer-coarse:opacity-100"
        />
      </div>
    </article>
  )
}

export const OverviewRow = memo(OverviewRowComponent)
