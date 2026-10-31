// 全部代理（与手机看板）的交互控制：行动作（批准 / 拒绝 / 重试 / 标记已处理 / 置顶 / 归档 / 复制链接）、
// critical 的处理方式、roving 焦点、单字母快捷键、逐个处理模式，以及从会话返回时的滚动与焦点恢复。
// 桌面全部代理与手机区段共用这一份控制逻辑，只换渲染。
// critical 审批：桌面全部代理只能“打开处理”进会话（规格 3.5，criticalInSession），手机看板仍用确认层（A4 前）。
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
import type { AgentCardHandlers } from './AgentCard'
import type { ApprovalConfirmTarget } from './ApprovalConfirm'
import type { BoardApproval } from './board-api'
import type { BoardItem } from './board-model'
import { clearBoardReturn, peekBoardReturn, rememberBoardReturn } from './board-return'
import { collapseRow, OVERVIEW_ROW_COLLAPSE_MS } from './row-motion'
import { useBoardActions } from './useBoardActions'

// 确认层只记住会话与审批 id：渲染时从最新数据取审批，详情晚到会更新，别处处理掉会关闭。
// snapshot 只在提交途中兜底（乐观更新会先把审批从卡片上拿掉）。
type ConfirmState = {
  sessionId: string
  approvalId: string
  title: string
  cwd: string
  snapshot: BoardApproval | null
  pending: boolean
  error: string
}

type TriageState = { total: number; finished: boolean }

type ControllerOptions = {
  rootRef: RefObject<HTMLElement | null>
  // 页面上看得见的行，自上而下（键盘 J / K、↑ / ↓ 按它移动）。
  order: readonly BoardItem[]
  // 当前筛选下可见的需要你（键盘与逐个处理在它们之间移动）。
  attention: readonly BoardItem[]
  // 筛选前的全部需要你：确认层解析最新审批、逐个处理判断有没有被筛选藏起来的事项。
  attentionAll: readonly BoardItem[]
  // 逐个处理时有事项被筛选藏起来：由页面清除筛选，让它们回到列表里。
  onRevealAttention?: () => void
  // 虚拟列表里不在 DOM 的行：先滚到它再返回元素；没有时直接按 data-board-card 查找。
  reveal?: (sessionId: string) => Promise<HTMLElement | null>
  // critical 只能进会话处理（桌面全部代理，规格 3.5）；false 时打开确认层（手机看板）。
  criticalInSession?: boolean
  ready: boolean
  notify: Notify
  sessionHref: (sessionId: string) => string
  onOpenSession: (sessionId: string) => void
  onToggleGrouping: () => void
}

const TRIAGE_FINISHED_MS = 1_600
// 记住滚动位置的容器：桌面列表与手机区段。
const SCROLLERS = '[data-overview-scroller], [data-board-sections]'

function cardIn(root: HTMLElement | null, sessionId: string) {
  return root?.querySelector<HTMLElement>(`[data-board-card="${CSS.escape(sessionId)}"]`) ?? null
}

function scrollKey(element: HTMLElement) {
  return element.hasAttribute('data-overview-scroller') ? 'overview' : 'sections'
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
  criticalInSession = false,
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
  const [confirm, setConfirm] = useState<ConfirmState | null>(null)
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
    async (item: BoardItem, approval: BoardApproval, confirmCritical: boolean) => {
      const followUp = nextAttentionId(item.id)
      const leaving = item.approvals.length <= 1
      if (leaving) await collapse(item.id)
      const result = await actionsRef.current.approve(item.id, approval.id, { confirmCritical })
      if (!result.ok) {
        restore(item.id)
        return result
      }
      if (leaving && followUp) void focusCard(followUp)
      return result
    },
    [collapse, focusCard, nextAttentionId, restore],
  )

  const openConfirm = useCallback((item: BoardItem, approval: BoardApproval | null) => {
    setConfirm({
      sessionId: item.id,
      approvalId: approval?.id ?? '',
      title: item.title,
      cwd: item.cwd,
      snapshot: approval,
      pending: false,
      error: '',
    })
  }, [])

  // critical（或拿不到完整详情）的去处：桌面进会话处理，手机打开确认层。
  const review = useCallback(
    (item: BoardItem, approval: BoardApproval | null) => {
      if (criticalInSession) openSession(item.id)
      else openConfirm(item, approval)
    },
    [criticalInSession, openConfirm, openSession],
  )

  // 确认层展示的审批：按 id 取最新数据；打开时详情还没到（id 为空）就取卡片上的第一条。
  const confirmItem = confirm
    ? attentionAll.find((entry) => entry.id === confirm.sessionId)
    : undefined
  const liveApproval = confirm
    ? ((confirm.approvalId
        ? confirmItem?.approvals.find((entry) => entry.id === confirm.approvalId)
        : confirmItem?.approvals[0]) ?? null)
    : null
  const shownApproval = liveApproval ?? (confirm?.pending ? confirm.snapshot : null)
  const confirmTitle = confirmItem?.title || confirm?.title || ''
  const confirmCwd = confirmItem?.cwd ?? confirm?.cwd ?? ''
  const confirmSession = confirm?.sessionId ?? ''
  const confirmTarget = useMemo<ApprovalConfirmTarget | null>(
    () =>
      confirmSession
        ? {
            sessionId: confirmSession,
            title: confirmTitle,
            cwd: confirmCwd,
            // 截断的预览不能拿来批准，确认层显示“正在读取审批详情”并禁用确认，
            // 等 /live 给出完整参数（读取失败时一直保持禁用）。
            approval: shownApproval && !shownApproval.truncated ? shownApproval : null,
          }
        : null,
    [confirmCwd, confirmSession, confirmTitle, shownApproval],
  )
  // 审批在别处（会话页、另一台设备）被处理掉：关闭确认层并说明，而不是停在一条已失效的审批上。
  const confirmGone =
    Boolean(confirm) &&
    !confirm?.pending &&
    (!confirmItem ||
      confirmItem.status !== 'approval' ||
      (confirmItem.approvalsState === 'ready' && !liveApproval))
  useEffect(() => {
    if (!confirmGone) return
    setConfirm(null)
    notify(t('board:confirm.resolvedElsewhere'))
  }, [confirmGone, notify, t])

  const handlers = useMemo<AgentCardHandlers>(
    () => ({
      onFocusCard: setActiveId,
      onOpen: openSession,
      onApprove: (item, approval) => {
        // critical 不一步允许：桌面进会话处理，手机打开确认层；标准审批一步允许。
        if (approval.severity === 'critical') {
          review(item, approval)
          return
        }
        void approveNow(item, approval, false).then((result) => {
          if (!result.ok && result.reason === 'confirmation-required') review(item, approval)
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
      review,
      sessionHref,
      t,
    ],
  )

  const confirmApproval = useCallback(async () => {
    const current = confirm
    const approval = confirmTarget?.approval
    if (!current || !approval || current.pending) return
    setConfirm({
      ...current,
      approvalId: approval.id,
      snapshot: approval,
      pending: true,
      error: '',
    })
    const item = [...attentionAllRef.current].find((entry) => entry.id === current.sessionId)
    const result = item
      ? await approveNow(item, approval, true)
      : await actionsRef.current.approve(current.sessionId, approval.id, {
          confirmCritical: true,
        })
    if (result.ok) setConfirm(null)
    else if (result.reason !== 'pending')
      setConfirm((state) =>
        state ? { ...state, pending: false, error: t('board:action.failed') } : state,
      )
  }, [approveNow, confirm, confirmTarget?.approval, t])

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
        const approval = item.approvals[0]
        // critical 或详情未到：桌面什么也不做（行上只有“打开处理”，↵ 进会话），手机只打开确认层。
        if (!approval || approval.truncated || approval.severity === 'critical') {
          if (!criticalInSession) openConfirm(item, approval ?? null)
        } else handlers.onApprove(item, approval)
      } else if (
        acting &&
        matchesBoardShortcut(event, bindings.boardDeny) &&
        item?.status === 'approval' &&
        item.approvals[0]
      )
        handlers.onDeny(item, item.approvals[0])
      else if (
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
    criticalInSession,
    effectiveActiveId,
    exitTriage,
    handlers,
    move,
    onToggleGrouping,
    openConfirm,
    openSession,
    rootRef,
    startTriage,
    triage,
  ])

  // —— 从会话返回：恢复滚动位置，焦点回到来源行 ——
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
      requestAnimationFrame(() =>
        cardIn(rootRef.current, state.focusId)?.focus({ preventScroll: true }),
      )
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
    confirm: confirm
      ? {
          target: confirmTarget as ApprovalConfirmTarget,
          pending: confirm.pending,
          error: confirm.error,
        }
      : null,
    setConfirmOpen: (open: boolean) => {
      if (!open) setConfirm(null)
    },
    confirmApproval,
    triage: triage ? { ...triage, index: triageIndex } : null,
    startTriage,
    exitTriage,
    bindings,
  }
}

// —— 会话页泳道条与代理面板的审批 ——
// 与看板控制器同一套规则：标准审批一步允许；critical（或拿不到完整详情）只打开确认层，
// 只有确认层（按住 0.6 秒或层内 ↵）回调 confirmStripApproval 才带 confirmCritical。
// 放在本文件里，是为了让“能批准 critical 的代码只在控制器文件里”这条源码守卫继续成立。
type StripConfirmState = {
  sessionId: string
  approvalId: string
  title: string
  cwd: string
  snapshot: BoardApproval | null
  pending: boolean
  error: string
}

export function useStripApprovals({
  attentionAll,
  notify,
}: {
  attentionAll: readonly BoardItem[]
  notify: Notify
}) {
  const { t } = useI18n()
  const actions = useBoardActions()
  const actionsRef = useRef(actions)
  actionsRef.current = actions
  const [confirm, setConfirm] = useState<StripConfirmState | null>(null)

  const report = useCallback(
    (result: Awaited<ReturnType<typeof actions.approve>>) => {
      if (result.ok || result.reason === 'pending') return
      notify(t('board:action.failed'), 'error')
    },
    [notify, t],
  )

  const review = useCallback((item: BoardItem, approval: BoardApproval | null) => {
    setConfirm({
      sessionId: item.id,
      approvalId: approval?.id ?? '',
      title: item.title,
      cwd: item.cwd,
      snapshot: approval,
      pending: false,
      error: '',
    })
  }, [])

  const approve = useCallback(
    (item: BoardItem, approval: BoardApproval) => {
      if (approval.severity === 'critical' || approval.truncated) {
        review(item, approval)
        return
      }
      void actionsRef.current
        .approve(item.id, approval.id, { confirmCritical: false })
        .then((result) => {
          if (!result.ok && result.reason === 'confirmation-required') review(item, approval)
          else report(result)
        })
    },
    [report, review],
  )

  const deny = useCallback(
    (item: BoardItem, approval: BoardApproval) => {
      void actionsRef.current.deny(item.id, approval.id).then(report)
    },
    [report],
  )

  const retry = useCallback(
    (item: BoardItem) => {
      void actionsRef.current.retry(item.id).then(report)
    },
    [report],
  )

  // 确认层展示的审批按 id 从最新数据解析；别处处理掉时关闭并说明。
  const item = confirm ? attentionAll.find((entry) => entry.id === confirm.sessionId) : undefined
  const live = confirm
    ? ((confirm.approvalId
        ? item?.approvals.find((entry) => entry.id === confirm.approvalId)
        : item?.approvals[0]) ?? null)
    : null
  const shown = live ?? (confirm?.pending ? confirm.snapshot : null)
  const target = useMemo<ApprovalConfirmTarget | null>(
    () =>
      confirm
        ? {
            sessionId: confirm.sessionId,
            title: item?.title || confirm.title,
            cwd: item?.cwd ?? confirm.cwd,
            // 截断的预览不能用来批准：等 /live 给出完整参数。
            approval: shown && !shown.truncated ? shown : null,
          }
        : null,
    [confirm, item?.cwd, item?.title, shown],
  )
  const gone =
    Boolean(confirm) &&
    !confirm?.pending &&
    (!item || item.status !== 'approval' || (item.approvalsState === 'ready' && !live))
  useEffect(() => {
    if (!gone) return
    setConfirm(null)
    notify(t('board:confirm.resolvedElsewhere'))
  }, [gone, notify, t])

  const confirmStripApproval = useCallback(async () => {
    const current = confirm
    const approval = target?.approval
    if (!current || !approval || current.pending) return
    setConfirm({ ...current, approvalId: approval.id, snapshot: approval, pending: true })
    const result = await actionsRef.current.approve(current.sessionId, approval.id, {
      confirmCritical: true,
    })
    if (result.ok) setConfirm(null)
    else if (result.reason !== 'pending')
      setConfirm((state) =>
        state ? { ...state, pending: false, error: t('board:action.failed') } : state,
      )
  }, [confirm, t, target?.approval])

  return {
    approve,
    deny,
    retry,
    review,
    isPending: actions.isPending,
    confirm: confirm ? { target, pending: confirm.pending, error: confirm.error } : null,
    closeConfirm: () => setConfirm(null),
    confirmStripApproval,
  }
}
