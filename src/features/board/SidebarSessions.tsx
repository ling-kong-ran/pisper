// 侧栏会话分组（规格 v2 第 2.1、2.2、2.8 节）：需要你 → 运行中 → 排队 → 置顶 → 今天完成 → 历史会话。
// 数据只读看板 store（useSidebarModel），与全部代理、手机标签栏同一个所有者，不新开轮询。
// 会话行是带 href 的链接（新标签打开、复制链接照常可用），当前会话行带 aria-current="page"。
// 需要你行悬停或聚焦 80ms 后，右侧时长换成快捷处理：普通批准“拒绝 D · 允许 A”，其余“打开 ↵”；
// 高风险、提问、失败以及审批详情还没到的一律只能打开，进会话处理（规格 1.4、3.5）。
// 处理后行 160ms 收起；新出现的需要你行 160ms 展开、琥珀点外圈扩散一次（600ms），并经 polite 读屏播报。
// 动效只用 transform / opacity，减少动效或页面隐藏时直接跳过，且都不循环。
// 本模块随看板 store 懒加载（入口预算），外壳先画骨架行。
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from 'react'
import { useHref } from 'react-router-dom'
import { ChevronDown, ChevronRight, History, MoreHorizontal, Pin } from 'lucide-react'
import type { BoardLane } from '@shared/session-board-status.mjs'
import type { Notify } from '@/app/route-context'
import { sessionPath } from '@/app/routes'
import { forgetLastSession } from '@/app/session-home'
import { useI18n } from '@/app/use-i18n'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import type { PromptDialogOptions } from '@/hooks/useAppDialog'
import { ariaKeyShortcuts } from '@/lib/shortcut-aria'
import { matchesBoardShortcut } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { useShortcutStore } from '@/stores/shortcut-store'
import { useSidebarStore } from '@/stores/sidebar-store'
import type { BoardActionResult } from './board-actions'
import { useBoardViewStore } from './board-view-store'
import {
  adjacentSidebarSession,
  type SidebarAttentionRow,
  type SidebarGroup,
  type SidebarGroupId,
  type SidebarReason,
  type SidebarRow,
} from './sidebar-model'
import {
  formatSidebarClock,
  formatSidebarDay,
  formatSidebarTime,
  formatSidebarWait,
} from './sidebar-presentation'
import { SIDEBAR_COMMAND_EVENT, type SidebarCommand } from './sidebar-commands'
import { useBoardActions } from './useBoardActions'
import { motionReduced } from './useLaneMotion'
import { useNow } from './useNow'
import { useSidebarModel } from './useSidebarModel'

const ROW_COLLAPSE_MS = 160
const ROW_EXPAND_MS = 160
const RING_MS = 600

function motionAllowed() {
  return !motionReduced() && document.visibilityState !== 'hidden'
}

type Translate = ReturnType<typeof useI18n>['t']

// 原因文字：键写成字面量（i18n 检查要求），组合时按“失败 → 批准 → 问题”的顺序用“ · ”连接。
function reasonText(t: Translate, reasons: readonly SidebarReason[]) {
  return reasons
    .map((reason, index) => {
      if (reason.kind === 'failed') return t('board:sidebar.reason.failed')
      if (reason.kind === 'approval')
        return reason.critical
          ? t('board:sidebar.reason.criticalApproval')
          : t('board:sidebar.reason.approval')
      if (reason.count === null) return t('board:sidebar.reason.question')
      return index === 0
        ? t('board:sidebar.reason.questions', { count: reason.count })
        : t('board:sidebar.reason.questionCount', { count: reason.count })
    })
    .join(' · ')
}

function groupTitle(t: Translate, id: SidebarGroupId) {
  switch (id) {
    case 'attention':
      return t('board:sidebar.group.attention')
    case 'running':
      return t('board:sidebar.group.running')
    case 'queued':
      return t('board:sidebar.group.queued')
    case 'pinned':
      return t('board:sidebar.group.pinned')
    default:
      return t('board:sidebar.group.done')
  }
}

// 复制会话链接：沿用当前地址的协议、主机与路由形式（哈希路由时链接也带 #），与全部代理页一致。
function sessionHref(id: string) {
  const { origin, pathname, search, hash } = window.location
  return hash.startsWith('#/')
    ? `${origin}${pathname}${search}#${sessionPath(id)}`
    : `${origin}${sessionPath(id)}`
}

function sidebarRows(root: ParentNode | null) {
  return root ? [...root.querySelectorAll<HTMLElement>('[data-sidebar-row]')] : []
}

// 焦点目标可能还没画出来（临时展开刚触发、分组刚加载）：逐帧找，最多等 20 帧。
// unclaimed：只在焦点无主（落在 body 上，或仍在侧栏里）时才移动，不抢页面自己放好的焦点。
function focusSidebarRow(
  pick: (rows: HTMLElement[]) => HTMLElement | undefined,
  { unclaimed = false }: { unclaimed?: boolean } = {},
) {
  let frames = 0
  const attempt = () => {
    const active = document.activeElement
    if (unclaimed && active && active !== document.body && !active.closest('[data-shell-sidebar]'))
      return
    const target = pick(sidebarRows(document.querySelector('[data-sidebar-groups]')))
    if (target) {
      target.focus()
      target.scrollIntoView?.({ block: 'nearest' })
      return
    }
    if ((frames += 1) < 20) requestAnimationFrame(attempt)
  }
  requestAnimationFrame(attempt)
}

export type SidebarCommandsProps = {
  // 当前会话：读路由器的最新去向（连按 ⌥↓ 时界面还没换过去），不是已渲染的地址。
  resolveCurrentId: () => string
  // 已渲染页面上的会话：会话页（含主页草稿，此时是空串）给出 id，其他页面为 null。
  // 用来在“从全部代理、设置、功能等页面回到原会话”时把焦点还给原来的会话行（规格 2.3）。
  renderedSessionId: string | null
  onOpenSession: (sessionId: string) => void
}

// 无界面的命令处理：⌘J / ⌥A / ⌥↑⌥↓ 由外壳的快捷键层转成事件交到这里。展开与收起时都挂着，
// 同时让看板同步在侧栏收起时也保持存活（窄轨计数、⌥A 目标都要实时），
// 需要你的读屏播报也放在这里：侧栏收成窄轨时分组不挂载，播报照样要有（规格 2.2）。
export function SidebarCommands({
  resolveCurrentId,
  renderedSessionId,
  onOpenSession,
}: SidebarCommandsProps) {
  const { t } = useI18n()
  const { model, isLoading } = useSidebarModel()
  const modelRef = useRef(model)
  modelRef.current = model
  const propsRef = useRef({ resolveCurrentId, onOpenSession })
  propsRef.current = { resolveCurrentId, onOpenSession }
  useEffect(() => {
    const listener = (event: Event) => {
      const command = (event as CustomEvent<SidebarCommand>).detail
      if (!command) return
      const current = modelRef.current
      if (command.kind === 'adjacent') {
        const next = adjacentSidebarSession(
          current.order,
          propsRef.current.resolveCurrentId() || null,
          command.direction,
        )
        if (next) propsRef.current.onOpenSession(next)
        return
      }
      const sidebar = useSidebarStore.getState()
      if (command.kind === 'focus-attention') {
        const target = current.longestWaitingId
        if (!target) return
        if (sidebar.collapsed) sidebar.setPeeking(true)
        focusSidebarRow((rows) => rows.find((row) => row.dataset.sidebarRow === target))
        return
      }
      if (sidebar.collapsed) sidebar.setPeeking(true)
      const currentId = propsRef.current.resolveCurrentId()
      focusSidebarRow((rows) => rows.find((row) => row.dataset.sidebarRow === currentId) ?? rows[0])
    }
    window.addEventListener(SIDEBAR_COMMAND_EVENT, listener)
    return () => window.removeEventListener(SIDEBAR_COMMAND_EVENT, listener)
  }, [])

  // 返回时焦点回到原会话行：记下离开会话页时的会话，回到同一个会话时把焦点交给它的行。
  // 侧栏收起时行不在界面上，不为此临时展开；页面自己已经放好焦点（如输入框）时不抢。
  const previousRef = useRef(renderedSessionId)
  const leftRef = useRef<string | null>(null)
  useEffect(() => {
    const previous = previousRef.current
    previousRef.current = renderedSessionId
    if (renderedSessionId === null) {
      if (previous) leftRef.current = previous
      return
    }
    if (previous !== null) return
    const left = leftRef.current
    leftRef.current = null
    if (!left || left !== renderedSessionId || useSidebarStore.getState().collapsed) return
    focusSidebarRow((rows) => rows.find((row) => row.dataset.sidebarRow === left), {
      unclaimed: true,
    })
  }, [renderedSessionId])

  // 新进入“需要你”的行经 polite 读屏播报（“修复 CI 失败 需要你：等你批准”）；首次载入的既有事项不播。
  const seenRef = useRef<Set<string> | null>(null)
  const [announcement, setAnnouncement] = useState('')
  useEffect(() => {
    if (isLoading) return
    const rows = (model.groups.find((group) => group.id === 'attention')?.rows ??
      []) as SidebarAttentionRow[]
    const previous = seenRef.current
    seenRef.current = new Set(rows.map((row) => row.id))
    if (!previous) return
    const added = rows.filter((row) => !previous.has(row.id))
    if (!added.length) return
    setAnnouncement(
      added
        .map((row) =>
          t('board:sidebar.announce', { title: row.title, reason: reasonText(t, row.reasons) }),
        )
        .join('；'),
    )
  }, [model.groups, isLoading, t])
  return (
    <p data-sidebar-announce aria-live="polite" role="status" className="sr-only">
      {announcement}
    </p>
  )
}

export type SidebarRailCountsProps = {
  // 点计数：打开全部代理并筛到对应状态（沿用 /board 的筛选偏好 pisper-board-view）。
  onOpenBoard: () => void
}

// 窄轨上的两个计数（规格 2.1）：琥珀“● N”需要你、“◠ N”运行中；为 0 时变灰，不消失。
export function SidebarRailCounts({ onOpenBoard }: SidebarRailCountsProps) {
  const { t } = useI18n()
  const { model, isLoading } = useSidebarModel()
  const setLaneFilter = useBoardViewStore((state) => state.setLaneFilter)
  const open = (lane: BoardLane) => {
    setLaneFilter([lane])
    onOpenBoard()
  }
  const attention = model.counts.attention
  const running = model.counts.running
  const base =
    'grid size-10 place-items-center rounded-[var(--r-md)] text-xs font-semibold tabular-nums transition-colors duration-(--d-fast) focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus pointer-coarse:size-11'
  return (
    <div className="flex flex-col items-center gap-1" aria-busy={isLoading || undefined}>
      <button
        type="button"
        data-rail-count="attention"
        aria-label={t('board:sidebar.railAttention', { count: attention })}
        title={t('board:sidebar.railAttention', { count: attention })}
        className={cn(
          base,
          attention > 0
            ? 'bg-warning-soft text-warning-strong ring-1 ring-st-attn-border ring-inset hover:brightness-[.98]'
            : 'text-content-muted hover:bg-surface-hover',
        )}
        onClick={() => open('attention')}
      >
        <span className="flex flex-col items-center gap-0.5 leading-none">
          <StatusGlyph
            status="attention"
            className={cn('size-3.5', attention === 0 && 'text-content-faint')}
          />
          <span>{attention}</span>
        </span>
      </button>
      <button
        type="button"
        data-rail-count="running"
        aria-label={t('board:sidebar.railRunning', { count: running })}
        title={t('board:sidebar.railRunning', { count: running })}
        className={cn(
          base,
          'hover:bg-surface-hover',
          running > 0 ? 'text-content-secondary' : 'text-content-muted',
        )}
        onClick={() => open('running')}
      >
        <span className="flex flex-col items-center gap-0.5 leading-none">
          <StatusGlyph
            status="run"
            spin={running > 0}
            className={cn('size-3.5', running === 0 && 'text-content-faint')}
          />
          <span>{running}</span>
        </span>
      </button>
    </div>
  )
}

export type SidebarSessionsProps = {
  currentId: string
  notify: Notify
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  onOpenSession: (sessionId: string) => void
  // 打开全部代理（运行中“还有 N 个 →”会先筛到运行中）。
  onOpenBoard: () => void
  onOpenHistory: () => void
  // 归档成功：归档的是当前会话时由外壳回到主页。
  onArchived: (sessionId: string) => void
}

export function SidebarSessions({
  currentId,
  notify,
  requestText,
  onOpenSession,
  onOpenBoard,
  onOpenHistory,
  onArchived,
}: SidebarSessionsProps) {
  const { t } = useI18n()
  const { model, isLoading, error, refetch } = useSidebarModel()
  const actions = useBoardActions()
  const actionsRef = useRef(actions)
  actionsRef.current = actions
  const doneCollapsed = useSidebarStore((state) => state.doneCollapsed)
  const setDoneCollapsed = useSidebarStore((state) => state.setDoneCollapsed)
  const setLaneFilter = useBoardViewStore((state) => state.setLaneFilter)
  const boardBindings = useShortcutStore((state) => state.boardBindings)
  const root = useHref('/')
  const hrefFor = useCallback((id: string) => root.replace(/\/$/, '') + sessionPath(id), [root])
  const ticking = model.counts.attention + model.counts.running > 0
  const now = useNow(ticking)
  const rootRef = useRef<HTMLDivElement>(null)

  // 处理中（已点允许 / 拒绝）正在收起的行：动作失败时恢复显示。
  const [leaving, setLeaving] = useState<ReadonlySet<string>>(() => new Set())
  const markLeaving = (id: string, on: boolean) =>
    setLeaving((current) => {
      const next = new Set(current)
      if (on) next.add(id)
      else next.delete(id)
      return next
    })

  // 新进入“需要你”的行：首次载入的既有事项不算新进入，只为之后出现的行播展开与外圈扩散动效；
  // 读屏播报在 SidebarCommands（侧栏收成窄轨时也要播）。
  const seenRef = useRef<Set<string> | null>(null)
  const [fresh, setFresh] = useState<ReadonlySet<string>>(() => new Set())
  const attentionRows = useMemo(
    () =>
      (model.groups.find((group) => group.id === 'attention')?.rows ?? []) as SidebarAttentionRow[],
    [model.groups],
  )
  useEffect(() => {
    if (isLoading) return
    const ids = new Set(attentionRows.map((row) => row.id))
    const previous = seenRef.current
    seenRef.current = ids
    if (!previous) return
    const added = attentionRows.filter((row) => !previous.has(row.id))
    if (!added.length) return
    setFresh(new Set(added.map((row) => row.id)))
  }, [attentionRows, isLoading])

  const report = useCallback(
    (result: BoardActionResult) => {
      if (result.ok || result.reason === 'pending') return
      notify(t('board:action.failed'), 'error')
    },
    [notify, t],
  )

  // 普通批准一步处理：先收起（160ms），动作失败时恢复；需要确认（详情变化、变成高风险）时改为打开会话。
  const decide = useCallback(
    async (row: SidebarAttentionRow, approve: boolean, element: HTMLElement | null) => {
      if (row.quickAction.kind !== 'decide') {
        onOpenSession(row.id)
        return
      }
      const approvalId = row.quickAction.approvalId
      // 键盘处理时焦点接到下一行，行收起后不丢焦点。
      const hadFocus = Boolean(element?.contains(document.activeElement))
      const rows = sidebarRows(rootRef.current)
      const index = rows.findIndex((entry) => entry.dataset.sidebarRow === row.id)
      const following = rows[index + 1] ?? rows[index - 1]
      if (hadFocus) following?.focus()
      // 动作与收起同时开始；收起播完才把行藏起来，失败时撤掉收起、行原样回来。
      let failed = false
      const collapsed = collapseRow(element).then(() => {
        if (!failed) markLeaving(row.id, true)
      })
      const result = approve
        ? await actionsRef.current.approve(row.id, approvalId, { confirmCritical: false })
        : await actionsRef.current.deny(row.id, approvalId)
      if (result.ok) return
      failed = true
      await collapsed
      markLeaving(row.id, false)
      element?.getAnimations?.().forEach((animation) => animation.cancel())
      if (result.reason === 'confirmation-required') onOpenSession(row.id)
      else report(result)
    },
    [onOpenSession, report],
  )

  // 行已不在需要你分组（处理成功、别处处理掉）时清掉收起标记。
  useEffect(() => {
    setLeaving((current) => {
      if (!current.size) return current
      const live = new Set(attentionRows.map((row) => row.id))
      const next = new Set([...current].filter((id) => live.has(id)))
      return next.size === current.size ? current : next
    })
  }, [attentionRows])

  const rename = useCallback(
    async (row: SidebarRow) => {
      const name = await requestText({
        title: t('board:sidebar.renameTitle'),
        inputLabel: t('board:sidebar.renameLabel'),
        value: row.title,
        confirmLabel: t('board:sidebar.renameSave'),
        maxLength: 200,
      })
      const trimmed = name?.trim()
      if (!trimmed || trimmed === row.title) return
      const result = await actionsRef.current.rename(row.id, trimmed)
      if (result.ok) notify(t('board:sidebar.renamed'))
      else report(result)
    },
    [notify, report, requestText, t],
  )

  const handlers: RowHandlers = useMemo(
    () => ({
      open: (row) => onOpenSession(row.id),
      decide: (row, approve, element) => void decide(row, approve, element),
      pin: (row) => void actionsRef.current.pin(row.id, !row.pinned).then(report),
      rename: (row) => void rename(row),
      archive: (row) =>
        void actionsRef.current.archive(row.id, true).then((result) => {
          if (!result.ok) {
            report(result)
            return
          }
          notify(t('board:action.archived', { title: row.title }))
          // 归档的是“上次停留的会话”时清掉记录，主页随后显示新任务草稿，不再落回它。
          forgetLastSession(row.id)
          onArchived(row.id)
        }),
      markHandled: (row) => void actionsRef.current.markHandled(row.id).then(report),
      copyLink: (row) => {
        const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
        if (!clipboard) {
          notify(t('board:action.copyFailed'), 'error')
          return
        }
        void clipboard.writeText(sessionHref(row.id)).then(
          () => notify(t('board:action.copied')),
          () => notify(t('board:action.copyFailed'), 'error'),
        )
      },
    }),
    [decide, notify, onArchived, onOpenSession, rename, report, t],
  )

  // 行间键盘：↑ / ↓ 移动，↵ 由链接本身打开；需要你行上 A / D 处理普通批准（跟随全部代理的改绑）。
  const onRowKeyDown = (event: ReactKeyboardEvent<HTMLAnchorElement>, row: SidebarRow) => {
    if (event.altKey || event.metaKey || event.ctrlKey) return
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      const rows = sidebarRows(rootRef.current)
      const index = rows.indexOf(event.currentTarget)
      rows[index + (event.key === 'ArrowDown' ? 1 : -1)]?.focus()
      return
    }
    if (row.group !== 'attention' || event.shiftKey) return
    // 全部代理的单字母键用自己的匹配器（应用快捷键的匹配器不接受无修饰的字母键）。
    const approve = matchesBoardShortcut(event.nativeEvent, boardBindings.boardApprove)
    const deny = !approve && matchesBoardShortcut(event.nativeEvent, boardBindings.boardDeny)
    if (!approve && !deny) return
    event.preventDefault()
    if (row.quickAction.kind === 'decide')
      handlers.decide(row, approve, event.currentTarget.closest('[data-sidebar-item]'))
  }

  const openRunning = () => {
    setLaneFilter(['running'])
    onOpenBoard()
  }

  if (isLoading) return <SidebarSkeleton label={t('board:sidebar.loading')} />

  const empty =
    !error && model.groups.every((group) => group.count === 0) && model.groups.length <= 2
  return (
    <div ref={rootRef} data-sidebar-groups className="flex flex-col gap-3 pb-2">
      {Boolean(error) && model.groups.every((group) => group.count === 0) ? (
        <div className="flex flex-col items-start gap-1.5 px-2.5 py-2 text-xs text-content-muted">
          <p>{t('board:sidebar.error')}</p>
          <button
            type="button"
            className="rounded-sm text-content-secondary underline underline-offset-2 hover:text-content focus-visible:outline-2 focus-visible:outline-focus"
            onClick={refetch}
          >
            {t('board:sidebar.retry')}
          </button>
        </div>
      ) : empty ? (
        <p className="px-2.5 py-2 text-xs leading-5 text-content-muted">
          {t('board:sidebar.empty')}
        </p>
      ) : (
        model.groups.map((group) => (
          <SidebarGroupSection
            key={group.id}
            group={group}
            title={groupTitle(t, group.id)}
            collapsed={group.id === 'done' && doneCollapsed}
            onToggle={group.id === 'done' ? () => setDoneCollapsed(!doneCollapsed) : undefined}
            footer={
              group.id === 'running' && group.hidden > 0 ? (
                <MoreLink onClick={openRunning}>
                  {t('board:sidebar.moreRunning', { count: group.hidden })}
                </MoreLink>
              ) : group.id === 'done' && group.hidden > 0 ? (
                <MoreLink onClick={onOpenHistory}>{t('board:sidebar.allDone')}</MoreLink>
              ) : null
            }
            emptyText={
              group.id === 'attention'
                ? t('board:sidebar.emptyAttention')
                : group.id === 'running'
                  ? t('board:sidebar.emptyRunning')
                  : ''
            }
          >
            {group.rows.map((row) => (
              <SidebarRowItem
                key={row.id}
                row={row}
                href={hrefFor(row.id)}
                current={row.id === currentId}
                now={now}
                hidden={leaving.has(row.id)}
                fresh={fresh.has(row.id)}
                handlers={handlers}
                approveBinding={boardBindings.boardApprove}
                denyBinding={boardBindings.boardDeny}
                onKeyDown={onRowKeyDown}
              />
            ))}
          </SidebarGroupSection>
        ))
      )}
      <a
        href={root.replace(/\/$/, '') + '/chat/history'}
        className="flex h-8 items-center gap-2 rounded-[var(--r-md)] px-2.5 text-[13px] text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content-secondary focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus pointer-coarse:h-11"
        onClick={(event) => {
          if (plainClick(event)) {
            event.preventDefault()
            onOpenHistory()
          }
        }}
      >
        <History className="size-3.5" strokeWidth={1.75} aria-hidden="true" />
        {t('board:sidebar.history')}
      </a>
    </div>
  )
}

function plainClick(event: ReactMouseEvent) {
  return event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey
}

// 处理后的收起：scaleY + 透明度 160ms；行随后隐藏，下一行顶上来。
function collapseRow(element: HTMLElement | null) {
  if (!element || !motionAllowed() || typeof element.animate !== 'function')
    return Promise.resolve()
  element.style.transformOrigin = 'top center'
  return element
    .animate(
      [
        { transform: 'scaleY(1)', opacity: 1 },
        { transform: 'scaleY(.6)', opacity: 0 },
      ],
      { duration: ROW_COLLAPSE_MS, easing: 'ease-in', fill: 'forwards' },
    )
    .finished.then(
      () => undefined,
      () => undefined,
    )
}

export function SidebarSkeleton({ label }: { label: string }) {
  return (
    <div
      role="status"
      aria-busy="true"
      aria-label={label}
      className="flex flex-col gap-1 px-1 pt-1"
    >
      {[64, 48, 72, 56].map((width) => (
        <div key={width} className="flex h-9 items-center gap-2.5 px-1.5" aria-hidden="true">
          <span className="size-4 flex-none rounded-full bg-surface-hover" />
          <span className="h-3 rounded-sm bg-surface-hover" style={{ width: `${width}%` }} />
        </div>
      ))}
    </div>
  )
}

function MoreLink({ children, onClick }: { children: ReactNode; onClick: () => void }) {
  return (
    <button
      type="button"
      className="flex h-7 w-full items-center gap-1 rounded-[var(--r-md)] px-2.5 pl-[34px] text-left text-xs text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content-secondary focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus pointer-coarse:h-11"
      onClick={onClick}
    >
      {children}
    </button>
  )
}

function SidebarGroupSection({
  group,
  title,
  collapsed,
  onToggle,
  footer,
  emptyText,
  children,
}: {
  group: SidebarGroup
  title: string
  collapsed: boolean
  onToggle?: () => void
  footer: ReactNode
  emptyText: string
  children: ReactNode
}) {
  const headingId = `sidebar-group-${group.id}`
  const attention = group.id === 'attention'
  const count = (
    <span
      data-sidebar-count
      className={cn(
        'tabular-nums',
        attention &&
          group.count > 0 &&
          'inline-grid h-[18px] min-w-[18px] place-items-center rounded-full bg-warning-soft px-1.5 text-[11.5px] font-semibold ring-1 ring-st-attn-border ring-inset',
      )}
    >
      {group.count}
    </span>
  )
  const headingClass = cn(
    'flex h-7 items-center gap-1.5 px-2.5 text-xs font-medium',
    attention && group.count > 0 ? 'text-warning-strong' : 'text-content-muted',
  )
  return (
    <section aria-labelledby={headingId} data-sidebar-group={group.id}>
      <h2 id={headingId} className="m-0">
        {onToggle ? (
          <button
            type="button"
            aria-expanded={!collapsed}
            className={cn(
              headingClass,
              'w-full rounded-[var(--r-md)] text-left hover:text-content-secondary focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus pointer-coarse:h-11',
            )}
            onClick={onToggle}
          >
            {title}
            {count}
            {collapsed ? (
              <ChevronRight className="size-3" aria-hidden="true" />
            ) : (
              <ChevronDown className="size-3" aria-hidden="true" />
            )}
          </button>
        ) : (
          <span className={headingClass}>
            {title}
            {count}
          </span>
        )}
      </h2>
      {!collapsed &&
        (group.rows.length ? (
          <ul className="m-0 flex list-none flex-col gap-px p-0">{children}</ul>
        ) : emptyText ? (
          <p className="px-2.5 pl-[34px] text-xs leading-7 text-content-faint">{emptyText}</p>
        ) : null)}
      {!collapsed && footer}
    </section>
  )
}

type RowHandlers = {
  open: (row: SidebarRow) => void
  decide: (row: SidebarAttentionRow, approve: boolean, element: HTMLElement | null) => void
  pin: (row: SidebarRow) => void
  rename: (row: SidebarRow) => void
  archive: (row: SidebarRow) => void
  markHandled: (row: SidebarRow) => void
  copyLink: (row: SidebarRow) => void
}

function SidebarRowItem({
  row,
  href,
  current,
  now,
  hidden,
  fresh,
  handlers,
  approveBinding,
  denyBinding,
  onKeyDown,
}: {
  row: SidebarRow
  href: string
  current: boolean
  now: number
  hidden: boolean
  fresh: boolean
  handlers: RowHandlers
  approveBinding: string | null
  denyBinding: string | null
  onKeyDown: (event: ReactKeyboardEvent<HTMLAnchorElement>, row: SidebarRow) => void
}) {
  const { t } = useI18n()
  const itemRef = useRef<HTMLLIElement>(null)
  const ringRef = useRef<HTMLSpanElement>(null)
  const [menuOpen, setMenuOpen] = useState(false)
  const attention = row.group === 'attention'

  // 新出现的需要你行：160ms 展开，圆点外圈扩散一次（600ms，不循环）。
  useLayoutEffect(() => {
    if (!fresh || !motionAllowed()) return
    const item = itemRef.current
    const ring = ringRef.current
    if (item && typeof item.animate === 'function') {
      item.style.transformOrigin = 'top center'
      item.animate(
        [
          { transform: 'scaleY(.6)', opacity: 0 },
          { transform: 'scaleY(1)', opacity: 1 },
        ],
        { duration: ROW_EXPAND_MS, easing: 'ease-out' },
      )
    }
    if (ring && typeof ring.animate === 'function')
      ring.animate(
        [
          { transform: 'scale(1)', opacity: 0.55 },
          { transform: 'scale(2.6)', opacity: 0 },
        ],
        { duration: RING_MS, easing: 'ease-out' },
      )
  }, [fresh])

  let meta = ''
  let reason = ''
  if (row.group === 'attention') {
    meta = formatSidebarWait(row.waitingSince === null ? null : now - row.waitingSince)
    reason = reasonText(t, row.reasons)
  } else if (row.group === 'running')
    meta = formatSidebarClock(row.startedAt === null ? null : now - row.startedAt)
  else if (row.group === 'queued') meta = `#${row.position}`
  else if (row.group === 'pinned') meta = formatSidebarDay(row.at, now)
  else meta = formatSidebarTime(row.finishedAt)

  const label =
    row.group === 'attention'
      ? meta
        ? t('board:sidebar.attentionRow', { title: row.title, reason, wait: meta })
        : `${row.title}，${reason}`
      : undefined

  const reveal =
    'invisible opacity-0 transition-[opacity,visibility] delay-[80ms] duration-[120ms] group-hover/row:visible group-hover/row:opacity-100 group-focus-within/row:visible group-focus-within/row:opacity-100 group-data-[menu-open]/row:visible group-data-[menu-open]/row:opacity-100 pointer-coarse:hidden'
  const metaFade =
    'transition-opacity delay-[80ms] duration-[120ms] group-hover/row:opacity-0 group-focus-within/row:opacity-0 group-data-[menu-open]/row:opacity-0 pointer-coarse:group-hover/row:opacity-100'
  const quickButton =
    'flex h-[26px] flex-none items-center gap-1 rounded-[var(--r-sm)] px-2 text-[12.5px] leading-none font-medium transition-colors duration-(--d-fast) focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus'

  return (
    <li
      ref={itemRef}
      data-sidebar-item={row.id}
      data-menu-open={menuOpen || undefined}
      hidden={hidden}
      className="group/row relative"
    >
      <a
        href={href}
        data-sidebar-row={row.id}
        data-sidebar-group-row={row.group}
        aria-current={current ? 'page' : undefined}
        aria-label={label}
        className={cn(
          'flex min-w-0 items-center gap-2.5 rounded-[var(--r-md)] px-2.5 text-base text-content transition-colors duration-(--d-fast) hover:bg-surface-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus',
          attention ? 'h-12 pointer-coarse:h-14' : 'h-9 pointer-coarse:h-11',
          current && 'bg-surface-active hover:bg-surface-active',
        )}
        onClick={(event) => {
          if (!plainClick(event)) return
          event.preventDefault()
          handlers.open(row)
        }}
        onKeyDown={(event) => onKeyDown(event, row)}
      >
        <span
          className={cn(
            'relative grid size-4 flex-none place-items-center',
            attention && 'self-start mt-[7px]',
          )}
        >
          {row.group === 'pinned' ? (
            <Pin
              className="size-3.5 text-content-muted"
              strokeWidth={1.75}
              role="img"
              aria-label={t('board:sidebar.group.pinned')}
            />
          ) : (
            <StatusGlyph
              status={row.glyph}
              phase={row.group === 'running' ? row.phase : undefined}
              label={attention ? undefined : glyphLabel(t, row)}
            />
          )}
          {attention && (
            <span
              ref={ringRef}
              aria-hidden="true"
              className="pointer-events-none absolute inset-1 rounded-full bg-st-attn-fill opacity-0"
            />
          )}
        </span>
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="flex min-w-0 items-baseline gap-1.5">
            <span
              className={cn(
                'min-w-0 truncate leading-5',
                row.group === 'done' && 'text-content-secondary',
              )}
            >
              {row.title}
            </span>
            {row.group === 'done' && row.unread && (
              <span
                role="img"
                aria-label={t('board:sidebar.unread')}
                className="size-1.5 flex-none translate-y-[-1px] self-center rounded-full bg-content-secondary"
              />
            )}
          </span>
          {attention && (
            <span className="truncate text-xs leading-[17px] text-warning-strong">{reason}</span>
          )}
        </span>
        {meta && (
          <span
            aria-hidden={attention || undefined}
            className={cn(
              'flex-none text-xs tabular-nums',
              row.group === 'running'
                ? 'font-mono text-content-secondary'
                : current
                  ? 'text-content-secondary'
                  : 'text-content-muted',
              attention && 'self-start mt-[7px]',
              metaFade,
            )}
          >
            {meta}
          </span>
        )}
      </a>
      <div
        className={cn(
          'absolute top-1/2 right-1.5 flex -translate-y-1/2 items-center gap-1',
          reveal,
        )}
      >
        {row.group === 'attention' &&
          (row.quickAction.kind === 'decide' ? (
            <>
              <button
                type="button"
                className={cn(
                  quickButton,
                  'bg-solid text-content ring-1 ring-stroke-strong ring-inset hover:bg-surface-hover',
                )}
                aria-label={t('board:sidebar.denyNamed', { title: row.title })}
                aria-keyshortcuts={ariaKeyShortcuts(denyBinding)}
                onClick={(event) =>
                  handlers.decide(row, false, event.currentTarget.closest('[data-sidebar-item]'))
                }
              >
                {t('board:sidebar.deny')}
                <KeyHint binding={denyBinding} />
              </button>
              <button
                type="button"
                className={cn(quickButton, 'bg-star text-on-accent hover:bg-star-hover')}
                aria-label={t('board:sidebar.approveNamed', { title: row.title })}
                aria-keyshortcuts={ariaKeyShortcuts(approveBinding)}
                onClick={(event) =>
                  handlers.decide(row, true, event.currentTarget.closest('[data-sidebar-item]'))
                }
              >
                {t('board:sidebar.approve')}
                <KeyHint binding={approveBinding} />
              </button>
            </>
          ) : (
            <button
              type="button"
              className={cn(
                quickButton,
                'bg-solid text-content ring-1 ring-stroke-strong ring-inset hover:bg-surface-hover',
              )}
              aria-label={t('board:sidebar.openNamed', { title: row.title })}
              onClick={() => handlers.open(row)}
            >
              {t('board:sidebar.open')}
              <span aria-hidden="true" className="text-content-faint">
                ↵
              </span>
            </button>
          ))}
        <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label={t('board:sidebar.rowMenu', { title: row.title })}
              className="grid size-[26px] flex-none place-items-center rounded-[var(--r-sm)] text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-active hover:text-content focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus"
            >
              <MoreHorizontal className="size-4" aria-hidden="true" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" side="right" className="min-w-44">
            <DropdownMenuItem onSelect={() => handlers.open(row)}>
              {t('board:menu.open')}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => handlers.pin(row)}>
              {row.pinned ? t('board:menu.unpin') : t('board:menu.pin')}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => handlers.rename(row)}>
              {t('board:menu.rename')}
            </DropdownMenuItem>
            {row.group === 'attention' && row.status === 'failed' && (
              <DropdownMenuItem onSelect={() => handlers.markHandled(row)}>
                {t('board:menu.markHandled')}
              </DropdownMenuItem>
            )}
            <DropdownMenuItem onSelect={() => handlers.copyLink(row)}>
              {t('board:menu.copyLink')}
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => handlers.archive(row)}>
              {t('board:menu.archive')}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </li>
  )
}

function KeyHint({ binding }: { binding: string | null }) {
  // 单字母键（KeyA → A）；改绑成组合键时只靠 aria-keyshortcuts 与速查说明，不在窄按钮里挤长键名。
  const letter = binding && /^Key[A-Z]$/.test(binding) ? binding.slice(3) : ''
  if (!letter) return null
  return (
    <span aria-hidden="true" className="font-normal opacity-60">
      {letter}
    </span>
  )
}

function glyphLabel(t: Translate, row: SidebarRow) {
  switch (row.glyph) {
    case 'run':
      return t('common:statusGlyph.run')
    case 'queued':
      return t('common:statusGlyph.queued')
    case 'failed':
      return t('common:statusGlyph.failed')
    case 'stopped':
      return t('common:statusGlyph.stopped')
    case 'attention':
      return t('common:statusGlyph.attention')
    default:
      return t('common:statusGlyph.done')
  }
}
