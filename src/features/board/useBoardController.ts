// 全部代理的交互控制：行动作（批准 / 拒绝 / 重试 / 标记已处理 / 置顶 / 归档 / 复制链接）、
// critical 的处理方式、roving 焦点、单字母快捷键、逐个处理模式，以及从会话返回时的滚动与焦点恢复。
// 桌面行与手机卡片（规格 v2 第 7.3 节）共用这一份控制逻辑，只换渲染。
// critical 审批（或 Runtime 要求确认）一律“打开处理”进会话，在会话里按住确认（规格 3.5）：
// 这一页没有确认层，也从不带 confirmCritical。
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from 'react'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { isEditableTarget, matchesBoardShortcut, shortcutEventBlocked } from '@/lib/shortcuts'
import { useShortcutStore } from '@/stores/shortcut-store'
import type { BoardApproval } from './board-api'
import type { BoardItem } from './board-model'
import { clearBoardReturn, peekBoardReturn, rememberBoardReturn } from './board-return'
import { collapseRow, OVERVIEW_ROW_COLLAPSE_MS } from './row-motion'
import { sidebarQuickAction } from './sidebar-model'
import { useBoardActions } from './useBoardActions'

// 全部代理行（桌面行与手机卡片）交给控制器的动作。
export type BoardRowHandlers = {
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

type TriageState = { total: number; finished: boolean }

type ControllerOptions = {
  rootRef: RefObject<HTMLElement | null>
  // 页面上看得见的行，自上而下（键盘 J / K、↑ / ↓ 按它移动）。
  order: readonly BoardItem[]
  // 当前筛选下可见的需要你（键盘与逐个处理在它们之间移动）。
  attention: readonly BoardItem[]
  // 筛选前的全部需要你：逐个处理判断有没有被筛选藏起来的事项。
  attentionAll: readonly BoardItem[]
  // 逐个处理时有事项被筛选藏起来：由页面清除筛选，让它们回到列表里。
  onRevealAttention?: () => void
  // 虚拟列表里不在 DOM 的行：先滚到它再返回元素；没有时直接按 data-board-card 查找。
  reveal?: (sessionId: string) => Promise<HTMLElement | null>
  ready: boolean
  notify: Notify
  sessionHref: (sessionId: string) => string
  onOpenSession: (sessionId: string) => void
  onToggleGrouping: () => void
}

const TRIAGE_FINISHED_MS = 1_600
// 记住滚动位置的容器：全部代理列表（桌面与手机同一个滚动区）。
const SCROLLERS = '[data-overview-scroller]'

function cardIn(root: HTMLElement | null, sessionId: string) {
  return root?.querySelector<HTMLElement>(`[data-board-card="${CSS.escape(sessionId)}"]`) ?? null
}

function scrollKey(element: HTMLElement) {
  return element.dataset.overviewScroller !== undefined ? 'overview' : 'other'
}

// 键盘 A / D 能一步处理的那条审批：sidebarQuickAction 判为 decide 时取它指定的审批，否则没有。
function decidableApproval(item: BoardItem): BoardApproval | null {
  const quick = sidebarQuickAction(item)
  if (quick.kind !== 'decide') return null
  return item.approvals.find((entry) => entry.id === quick.approvalId) ?? null
}

function nextFrame() {
  return new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
}

export function useBoardController({
  rootRef,
  order,
  attention,
  attentionAll,
  onRevealAttention,
  reveal,
  ready,
  notify,
  sessionHref,
  onOpenSession,
  onToggleGrouping,
}: ControllerOptions) {
  const { t } = useI18n()
  const actions = useBoardActions()
  const bindings = useShortcutStore((state) => state.boardBindings)
  const [activeId, setActiveId] = useState('')
  const [triage, setTriage] = useState<TriageState | null>(null)
  const actionsRef = useRef(actions)
  actionsRef.current = actions
  const orderRef = useRef(order)
  orderRef.current = order
  const attentionRef = useRef(attention)
  attentionRef.current = attention
  const attentionAllRef = useRef(attentionAll)
  attentionAllRef.current = attentionAll
  const revealRef = useRef(reveal)
  revealRef.current = reveal
  const [triageRequested, setTriageRequested] = useState(false)

  const allIds = useMemo(() => order.map((item) => item.id), [order])
  // 当前行消失（换分组后被筛掉、归档）时退回第一行，保证 Tab 总能进入列表。
  const effectiveActiveId = allIds.includes(activeId) ? activeId : (allIds[0] ?? '')

  const focusCard = useCallback(
    async (sessionId: string) => {
      if (!sessionId) return
      setActiveId(sessionId)
      await nextFrame()
      const revealed = revealRef.current ? await revealRef.current(sessionId) : null
      const target = revealed ?? cardIn(rootRef.current, sessionId)
      target?.scrollIntoView({ block: 'nearest' })
      target?.focus({ preventScroll: true })
    },
    [rootRef],
  )

  // 下一条需要你：优先排在它后面的那条，已是最后一条时退回前一条。
  const nextAttentionId = useCallback((sessionId: string) => {
    const list = attentionRef.current
    const index = list.findIndex((item) => item.id === sessionId)
    if (index < 0) return list[0]?.id ?? ''
    return list[index + 1]?.id ?? list[index - 1]?.id ?? ''
  }, [])

  const openSession = useCallback(
    (sessionId: string) => {
      const scroll: Record<string, number> = {}
      rootRef.current?.querySelectorAll<HTMLElement>(SCROLLERS).forEach((element) => {
        scroll[scrollKey(element)] = element.scrollTop
      })
      rememberBoardReturn({ focusId: sessionId, scroll })
      onOpenSession(sessionId)
    },
    [onOpenSession, rootRef],
  )

  const reportFailure = useCallback(
    (result: Awaited<ReturnType<typeof actions.approve>>) => {
      if (result.ok || result.reason === 'pending') return
      notify(t('board:action.failed'), 'error')
    },
    [notify, t],
  )

  // 处理后行以 scaleY + 透明度收起（180ms），焦点落到下一条需要你；失败时撤掉收起，行原样回来。
  const collapse = useCallback(
    (sessionId: string) =>
      collapseRow(cardIn(rootRef.current, sessionId), {
        duration: OVERVIEW_ROW_COLLAPSE_MS,
        scale: 0.9,
      }),
    [rootRef],
  )
  const restore = useCallback(
    (sessionId: string) =>
      cardIn(rootRef.current, sessionId)
        ?.getAnimations()
        .forEach((animation) => animation.cancel()),
    [rootRef],
  )

  const approveNow = useCallback(
    async (item: BoardItem, approval: BoardApproval) => {
      const followUp = nextAttentionId(item.id)
      const leaving = item.approvals.length <= 1
      if (leaving) await collapse(item.id)
      const result = await actionsRef.current.approve(item.id, approval.id, {
        confirmCritical: false,
      })
      if (!result.ok) {
        restore(item.id)
        return result
      }
      if (leaving && followUp) void focusCard(followUp)
      return result
    },
    [collapse, focusCard, nextAttentionId, restore],
  )

  const handlers = useMemo<BoardRowHandlers>(
    () => ({
      onFocusCard: setActiveId,
      onOpen: openSession,
      onApprove: (item, approval) => {
        // critical 不一步允许，进会话按住确认；Runtime 回 confirmation-required（详情变了）时同样进会话。
        if (approval.severity === 'critical') {
          openSession(item.id)
          return
        }
        void approveNow(item, approval).then((result) => {
          if (!result.ok && result.reason === 'confirmation-required') openSession(item.id)
          else reportFailure(result)
        })
      },
      onDeny: (item, approval) => {
        const followUp = nextAttentionId(item.id)
        const leaving = item.approvals.length <= 1
        void (async () => {
          if (leaving) await collapse(item.id)
          const result = await actionsRef.current.deny(item.id, approval.id)
          if (!result.ok) restore(item.id)
          reportFailure(result)
          if (result.ok && leaving && followUp) void focusCard(followUp)
        })()
      },
      onRetry: (item) => {
        const followUp = nextAttentionId(item.id)
        void (async () => {
          await collapse(item.id)
          const result = await actionsRef.current.retry(item.id)
          if (!result.ok) restore(item.id)
          reportFailure(result)
          if (result.ok && followUp) void focusCard(followUp)
        })()
      },
      onMarkHandled: (item) => void actionsRef.current.markHandled(item.id).then(reportFailure),
      onPin: (item) => void actionsRef.current.pin(item.id, !item.pinned).then(reportFailure),
      onArchive: (item) =>
        void actionsRef.current.archive(item.id, true).then((result) => {
          if (result.ok) notify(t('board:action.archived', { title: item.title }))
          else reportFailure(result)
        }),
      onCopyLink: (item) => {
        const href = sessionHref(item.id)
        const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
        if (!clipboard) {
          notify(t('board:action.copyFailed'), 'error')
          return
        }
        void clipboard.writeText(href).then(
          () => notify(t('board:action.copied')),
          () => notify(t('board:action.copyFailed'), 'error'),
        )
      },
      isPending: (kind, sessionId, approvalId) => actions.isPending(kind, sessionId, approvalId),
    }),
    [
      actions,
      approveNow,
      collapse,
      focusCard,
      nextAttentionId,
      notify,
      openSession,
      reportFailure,
      restore,
      sessionHref,
      t,
    ],
  )

  // —— 逐个处理 ——
  // 范围是全部需要你事项：有事项被当前筛选藏起来时先请页面清除筛选，等它们回到列表再开始。
  const startTriage = useCallback(() => {
    const hidden = attentionAllRef.current.length - attentionRef.current.length
    if (hidden > 0 && onRevealAttention) {
      setTriageRequested(true)
      onRevealAttention()
      return
    }
    const first = attentionRef.current[0]
    if (!first) {
      notify(t('board:triage.nothing'))
      return
    }
    setTriage({ total: attentionRef.current.length, finished: false })
    void focusCard(first.id)
  }, [focusCard, notify, onRevealAttention, t])

  useEffect(() => {
    if (!triageRequested || attention.length < attentionAll.length) return
    setTriageRequested(false)
    startTriage()
  }, [attention.length, attentionAll.length, startTriage, triageRequested])

  const exitTriage = useCallback(() => setTriage(null), [])

  useEffect(() => {
    if (!triage || triage.finished) return
    if (attention.length === 0) {
      setTriage({ ...triage, finished: true })
      return
    }
    // 新事项进来时总数随之增加，进度不倒退。
    const handled = triage.total - Math.min(triage.total, attention.length)
    if (handled + attention.length > triage.total)
      setTriage({ ...triage, total: handled + attention.length })
  }, [attention.length, triage])

  useEffect(() => {
    if (!triage?.finished) return
    const timer = setTimeout(() => setTriage(null), TRIAGE_FINISHED_MS)
    return () => clearTimeout(timer)
  }, [triage?.finished])

  // —— 键盘 ——
  // 逐个处理时只在需要你之间走；平时在看得见的全部行之间走（规格 3.3）。
  const move = useCallback(
    (delta: number) => {
      const list = triage ? attentionRef.current : orderRef.current
      if (!list.length) return
      const index = list.findIndex((item) => item.id === effectiveActiveId)
      const next = index < 0 ? 0 : Math.max(0, Math.min(list.length - 1, index + delta))
      void focusCard(list[next].id)
    },
    [effectiveActiveId, focusCard, triage],
  )

  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (shortcutEventBlocked(event) || isEditableTarget(event.target)) return
      const root = rootRef.current
      const target = event.target instanceof HTMLElement ? event.target : null
      const onCard = Boolean(target?.closest('[data-board-card]'))
      const inBoard = Boolean(target && root?.contains(target)) || target === document.body
      if (!inBoard) return
      const plain = !event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey
      // 允许 / 拒绝 / 重试只作用于真正聚焦的行（或逐个处理中程序聚焦的那条）：
      // 焦点在空白处时按 A、D、R 不能落到“第一行”上，避免误批准或误重试。
      const acting = onCard || Boolean(triage)
      const item = orderRef.current.find((entry) => entry.id === effectiveActiveId)
      let handled = true
      if (event.key === 'Escape' && triage) exitTriage()
      else if (
        matchesBoardShortcut(event, bindings.boardNext) ||
        (onCard && plain && event.key === 'ArrowDown')
      )
        move(1)
      else if (
        matchesBoardShortcut(event, bindings.boardPrevious) ||
        (onCard && plain && event.key === 'ArrowUp')
      )
        move(-1)
      else if (matchesBoardShortcut(event, bindings.boardToggleGrouping)) onToggleGrouping()
      else if (matchesBoardShortcut(event, bindings.boardTriage)) {
        if (triage) exitTriage()
        else startTriage()
      } else if (
        acting &&
        matchesBoardShortcut(event, bindings.boardApprove) &&
        item?.status === 'approval'
      ) {
        // 与行上按钮、侧栏同一条规则（sidebarQuickAction）：只有详情已到、全部是普通批准、没有同时在等的问题
        // 才一步允许，且允许的是行上显示的那条；其余什么也不做（行上只有“打开处理”，↵ 进会话）。
        // 只看 approvals[0] 会在后面还排着 critical 时把整行当成普通批准。
        const approval = decidableApproval(item)
        if (approval) handlers.onApprove(item, approval)
      } else if (
        acting &&
        matchesBoardShortcut(event, bindings.boardDeny) &&
        item?.status === 'approval'
      ) {
        // 拒绝同样只作用于看得见完整命令的普通批准：高风险只能在会话里处理（规格 3.5），
        // 截断或还在加载的审批不能在列表上被拒掉。
        const approval = decidableApproval(item)
        if (approval) handlers.onDeny(item, approval)
      } else if (
        acting &&
        matchesBoardShortcut(event, bindings.boardRetry) &&
        item?.status === 'failed' &&
        item.lane === 'attention'
      )
        handlers.onRetry(item)
      else if (event.key === 'Enter' && plain && !onCard && item && triage) openSession(item.id)
      else handled = false
      if (handled) event.preventDefault()
    }
    window.addEventListener('keydown', keydown)
    return () => window.removeEventListener('keydown', keydown)
  }, [
    bindings,
    effectiveActiveId,
    exitTriage,
    handlers,
    move,
    onToggleGrouping,
    openSession,
    rootRef,
    startTriage,
    triage,
  ])

  // —— 离开这一页时记下位置 ——
  // 去设置、功能或别的会话（⌘,、⌘2、侧栏入口）也要能回到原处（规格 2.3“返回时焦点回到原来的会话行”）：
  // 卸载时焦点还在某一行里就记下那一行，滚动位置总是记下。从行打开会话时 openSession 已经记过
  // （来源行就是被打开的那行），不覆盖。React 卸载时先跑这里的清理、后摘 DOM，此刻焦点与 scrollTop 仍可读。
  useLayoutEffect(() => {
    const root = rootRef.current
    return () => {
      if (!root || peekBoardReturn()) return
      const active = document.activeElement
      const row =
        active instanceof HTMLElement && root.contains(active)
          ? active.closest<HTMLElement>('[data-board-card]')
          : null
      const scroll: Record<string, number> = {}
      root.querySelectorAll<HTMLElement>(SCROLLERS).forEach((element) => {
        scroll[scrollKey(element)] = element.scrollTop
      })
      rememberBoardReturn({ focusId: row?.dataset.boardCard ?? '', scroll })
    }
  }, [rootRef])

  // —— 回到这一页：恢复滚动位置，焦点回到来源行 ——
  useLayoutEffect(() => {
    const state = peekBoardReturn()
    if (!state || !ready || !allIds.length) return
    clearBoardReturn()
    rootRef.current?.querySelectorAll<HTMLElement>(SCROLLERS).forEach((element) => {
      const key = scrollKey(element)
      if (key in state.scroll) element.scrollTop = state.scroll[key]
    })
    if (allIds.includes(state.focusId)) {
      setActiveId(state.focusId)
      // 虚拟列表里行可能还没按新的滚动位置画出来：经 reveal 取（必要时滚到它），不直接查 DOM。
      requestAnimationFrame(() => {
        void (async () => {
          const revealed = revealRef.current ? await revealRef.current(state.focusId) : null
          ;(revealed ?? cardIn(rootRef.current, state.focusId))?.focus({ preventScroll: true })
        })()
      })
    }
  }, [allIds, ready, rootRef])

  const triageIndex = triage
    ? Math.min(triage.total, triage.total - Math.min(triage.total, attention.length) + 1)
    : 0

  return {
    actions,
    handlers,
    activeId: effectiveActiveId,
    focusCard,
    triage: triage ? { ...triage, index: triageIndex } : null,
    startTriage,
    exitTriage,
    bindings,
  }
}
