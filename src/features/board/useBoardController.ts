// 看板交互控制：卡片动作（批准 / 拒绝 / 重试 / 标记已处理 / 置顶 / 归档 / 复制链接）、
// critical 确认层、roving 焦点、单字母快捷键、逐个处理模式，以及从会话返回时的滚动与焦点恢复。
// 桌面泳道、项目矩阵与手机区段共用这一份控制逻辑，只换渲染。
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from 'react'
import type { BoardLane } from '@shared/session-board-status.mjs'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { isEditableTarget, matchesBoardShortcut, shortcutEventBlocked } from '@/lib/shortcuts'
import { useShortcutStore } from '@/stores/shortcut-store'
import type { AgentCardHandlers } from './AgentCard'
import type { ApprovalConfirmTarget } from './ApprovalConfirm'
import type { BoardApproval } from './board-api'
import type { BoardItem } from './board-model'
import {
  boardReturnRect,
  clearBoardReturn,
  flipFrom,
  freshReturnRect,
  peekBoardReturn,
  rememberBoardReturn,
} from './board-return'
import type { LaneHandle } from './LaneColumn'
import { collapseCard, motionReduced } from './useLaneMotion'
import { useBoardActions } from './useBoardActions'

export type BoardNavLane = { lane: BoardLane; items: BoardItem[] }

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
  navLanes: readonly BoardNavLane[]
  // 当前筛选下可见的需要你卡片（键盘与逐个处理在它们之间移动）。
  attention: readonly BoardItem[]
  // 筛选前的全部需要你卡片：确认层解析最新审批、逐个处理判断有没有被筛选藏起来的事项。
  attentionAll: readonly BoardItem[]
  // 逐个处理时有事项被筛选藏起来：由页面清除筛选，让它们回到泳道里。
  onRevealAttention?: () => void
  laneHandles: RefObject<Map<BoardLane, LaneHandle>>
  ready: boolean
  notify: Notify
  sessionHref: (sessionId: string) => string
  onOpenSession: (sessionId: string) => void
  onToggleGrouping: () => void
}

const TRIAGE_FINISHED_MS = 1_600

function cardIn(root: HTMLElement | null, sessionId: string) {
  return root?.querySelector<HTMLElement>(`[data-board-card="${CSS.escape(sessionId)}"]`) ?? null
}

function nextFrame() {
  return new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
}

export function useBoardController({
  rootRef,
  navLanes,
  attention,
  attentionAll,
  onRevealAttention,
  laneHandles,
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
  const skipFlipRef = useRef(new Set<string>())
  const actionsRef = useRef(actions)
  actionsRef.current = actions
  const navRef = useRef(navLanes)
  navRef.current = navLanes
  const attentionRef = useRef(attention)
  attentionRef.current = attention
  const attentionAllRef = useRef(attentionAll)
  attentionAllRef.current = attentionAll
  const [triageRequested, setTriageRequested] = useState(false)

  const allIds = useMemo(
    () => navLanes.flatMap((lane) => lane.items.map((item) => item.id)),
    [navLanes],
  )
  // 当前卡片消失（换泳道后被筛掉、归档）时退回第一张，保证 Tab 总能进入卡片区。
  const effectiveActiveId = allIds.includes(activeId) ? activeId : (allIds[0] ?? '')

  const focusCard = useCallback(
    async (sessionId: string) => {
      if (!sessionId) return
      setActiveId(sessionId)
      const lane = navRef.current.find((entry) =>
        entry.items.some((item) => item.id === sessionId),
      )?.lane
      const handle = lane ? laneHandles.current?.get(lane) : undefined
      await nextFrame()
      const element = handle ? await handle.reveal(sessionId) : cardIn(rootRef.current, sessionId)
      const target = element ?? cardIn(rootRef.current, sessionId)
      target?.scrollIntoView({ block: 'nearest' })
      target?.focus({ preventScroll: true })
    },
    [laneHandles, rootRef],
  )

  // 下一张需要你卡片：优先排在它后面的那张，已是最后一张时退回前一张。
  const nextAttentionId = useCallback((sessionId: string) => {
    const list = attentionRef.current
    const index = list.findIndex((item) => item.id === sessionId)
    if (index < 0) return list[0]?.id ?? ''
    return list[index + 1]?.id ?? list[index - 1]?.id ?? ''
  }, [])

  const openSession = useCallback(
    (sessionId: string) => {
      const scroll: Record<string, number> = {}
      rootRef.current
        ?.querySelectorAll<HTMLElement>(
          '[data-lane-scroller], [data-board-matrix], [data-board-sections]',
        )
        .forEach((element) => {
          const key =
            element.dataset.laneScroller ??
            (element.hasAttribute('data-board-matrix') ? 'matrix' : 'sections')
          scroll[key] = element.scrollTop
        })
      // 来源卡片的位置交给会话页泳道条：卡片缩进它在泳道条里的芯片（FLIP，只动 transform）。
      const origin = boardReturnRect(cardIn(rootRef.current, sessionId))
      rememberBoardReturn({
        focusId: sessionId,
        scroll,
        ...(origin ? { origin: { ...origin, at: Date.now() } } : {}),
      })
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

  const approveNow = useCallback(
    async (item: BoardItem, approval: BoardApproval, confirmCritical: boolean) => {
      const followUp = nextAttentionId(item.id)
      const leaving = item.approvals.length <= 1
      if (leaving) {
        skipFlipRef.current.add(item.id)
        await collapseCard(cardIn(rootRef.current, item.id))
      }
      const result = await actionsRef.current.approve(item.id, approval.id, { confirmCritical })
      if (!result.ok) {
        skipFlipRef.current.delete(item.id)
        cardIn(rootRef.current, item.id)
          ?.getAnimations()
          .forEach((animation) => animation.cancel())
        return result
      }
      if (leaving && followUp) void focusCard(followUp)
      return result
    },
    [focusCard, nextAttentionId, rootRef],
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
            // 与泳道条一致：截断的预览不能拿来批准，确认层显示“正在读取审批详情”并禁用确认，
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
        // critical（或拿不到详情）只打开确认层；标准审批一步允许。
        if (approval.severity === 'critical') {
          openConfirm(item, approval)
          return
        }
        void approveNow(item, approval, false).then((result) => {
          if (!result.ok && result.reason === 'confirmation-required') openConfirm(item, approval)
          else reportFailure(result)
        })
      },
      onDeny: (item, approval) => {
        const followUp = nextAttentionId(item.id)
        void actionsRef.current.deny(item.id, approval.id).then((result) => {
          reportFailure(result)
          if (result.ok && item.approvals.length <= 1 && followUp) void focusCard(followUp)
        })
      },
      onRetry: (item) => {
        const followUp = nextAttentionId(item.id)
        void actionsRef.current.retry(item.id).then((result) => {
          reportFailure(result)
          if (result.ok && followUp) void focusCard(followUp)
        })
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
      focusCard,
      nextAttentionId,
      notify,
      openConfirm,
      openSession,
      reportFailure,
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
  // 范围是全部需要你事项：有事项被当前筛选藏起来时先请页面清除筛选，等它们回到泳道再开始。
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
  const move = useCallback(
    (delta: number, laneDelta: number) => {
      const lanes = navRef.current.filter((entry) => entry.items.length > 0)
      if (!lanes.length) return
      let laneIndex = lanes.findIndex((entry) =>
        entry.items.some((item) => item.id === effectiveActiveId),
      )
      let index =
        laneIndex < 0
          ? 0
          : lanes[laneIndex].items.findIndex((item) => item.id === effectiveActiveId)
      if (laneIndex < 0) laneIndex = 0
      if (laneDelta) {
        laneIndex = Math.max(0, Math.min(lanes.length - 1, laneIndex + laneDelta))
        index = Math.min(index, lanes[laneIndex].items.length - 1)
      } else {
        index = Math.max(0, Math.min(lanes[laneIndex].items.length - 1, index + delta))
      }
      void focusCard(lanes[laneIndex].items[index].id)
    },
    [effectiveActiveId, focusCard],
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
      // 允许 / 拒绝 / 重试只作用于真正聚焦的卡片（或逐个处理中程序聚焦的那张）：
      // 焦点在空白处时按 A、D、R 不能落到“第一张卡片”上，避免误批准或误重试。
      const acting = onCard || Boolean(triage)
      const item = navRef.current
        .flatMap((lane) => lane.items)
        .find((entry) => entry.id === effectiveActiveId)
      let handled = true
      if (event.key === 'Escape' && triage) exitTriage()
      else if (
        matchesBoardShortcut(event, bindings.boardNext) ||
        (onCard && plain && event.key === 'ArrowDown')
      )
        move(1, 0)
      else if (
        matchesBoardShortcut(event, bindings.boardPrevious) ||
        (onCard && plain && event.key === 'ArrowUp')
      )
        move(-1, 0)
      else if (
        matchesBoardShortcut(event, bindings.boardLaneLeft) ||
        (onCard && plain && event.key === 'ArrowLeft')
      )
        move(0, -1)
      else if (
        matchesBoardShortcut(event, bindings.boardLaneRight) ||
        (onCard && plain && event.key === 'ArrowRight')
      )
        move(0, 1)
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
        // 详情未到时 A 也只打开确认层（层内显示加载中，不可批准）。
        if (!approval || approval.severity === 'critical') openConfirm(item, approval ?? null)
        else handlers.onApprove(item, approval)
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

  // —— 从会话返回：恢复各泳道滚动位置，焦点回到来源卡片 ——
  useLayoutEffect(() => {
    const state = peekBoardReturn()
    if (!state || !ready || !allIds.length) return
    clearBoardReturn()
    rootRef.current
      ?.querySelectorAll<HTMLElement>(
        '[data-lane-scroller], [data-board-matrix], [data-board-sections]',
      )
      .forEach((element) => {
        const key =
          element.dataset.laneScroller ??
          (element.hasAttribute('data-board-matrix') ? 'matrix' : 'sections')
        if (key in state.scroll) element.scrollTop = state.scroll[key]
      })
    if (allIds.includes(state.focusId)) {
      setActiveId(state.focusId)
      // 从泳道条返回：卡片从它在泳道条里的芯片位置展开回去（减少动效时跳过）。
      const from = freshReturnRect(state.from)
      const card = cardIn(rootRef.current, state.focusId)
      if (from && card && !motionReduced()) flipFrom(card, from)
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
    skipFlipRef,
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
