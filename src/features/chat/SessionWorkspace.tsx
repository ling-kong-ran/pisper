// 会话页（/board/:sessionId，桌面布局）的单会话工作区：对话列（顶栏 52 + 运行中计划的 2px 细进度 + 转录与输入框）
// + 检查器（规格 v2 第 4.4 节）。检查器默认关着、按需滑入：改动按钮 / ⌘I / 回合结束且本回合有改动时自动滑入一次
//（运行中不自动弹出）。窗口 ≥1280 时 456px 推开对话（对话列重新居中，可拖宽）；<1280 宽或 ≤760 高（以及手机 App 平板）
// 时是 420px 覆盖层：无遮罩，打开时焦点进面板、Tab 在面板里循环，Esc 关闭后焦点回到改动按钮。
// 设置 › 界面的“固定检查器”打开后，宽屏检查器像第 4 期一样常驻（开关跨页面加载保留）。
// 滑入 220ms cubic-bezier(.2,.8,.2,1)（--ease），对话列同时从原位置移到新的居中位置；减少动效时直接出现。
// 手机外壳（规格 7.3）：页头换成 56px 顶栏，下面是应用壳交来的代理条；检查器改为底部表单
//（改动 / 终端 / 文件 / 预览），由顶栏的“+N 改动 ›”或“⋯”打开，拖拽条、关闭键、Esc 与 Android 返回键都先关表单。
// 表单关闭时播放与滑入对应的退场动画，只为退场多挂载一会儿（useExitPresence：动画结束或兜底超时即卸载，
// 减少动效时立即卸载），焦点回到打开它的入口（通常是“+N 改动”）。
// 外部的“打开资源”请求（网页预览、任务内容请求、运行结束后的自动展示）仍写进 session-context-pages-store，
// 这里订阅它并切到对应标签，所以这些入口不需要知道会话页换了布局。
// 应用级的会话树（⇧⌘B）与侧聊快捷键经 SESSION_COMMAND_EVENT 交到当前可见的工作区处理。
import {
  lazy,
  Suspense,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from 'react'
import { useI18n } from '@/app/use-i18n'
import { SheetDragHandle } from '@/components/app/SheetDragHandle'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { useBackLayer } from '@/hooks/use-back-layer'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { useExitPresence } from '@/hooks/useExitPresence'
import { matchesShortcut, optionTypingInEditable, shortcutEventBlocked } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { useShortcutStore } from '@/stores/shortcut-store'
import type { ChatMessage, EntityRecord, Plan, TeamState } from '@/types/chat'
import { SESSION_COMMAND_EVENT, TASK_VIEW_WILL_CHANGE } from './events'
import { TASK_CONTENT_REQUESTED, parseTaskContentRequest } from './public-workspace'
import type { SessionContextTab } from './session-context-pages'
import { useContextPagesStore } from './session-context-pages-store'
import { openSessionSideChat } from './session-header-menu'
import {
  SESSION_CONVERSATION_MIN_WIDTH,
  SESSION_INSPECTOR_DEFAULT_WIDTH,
  SESSION_INSPECTOR_MIN_WIDTH,
  SESSION_INSPECTOR_OVERLAY_WIDTH,
  inspectorDisplayWidth,
  useSessionInspectorStore,
  type SessionInspectorTab,
} from './session-inspector-store'
import {
  SessionInspectorContext,
  SessionPageChromeContext,
  type SessionInspectorControl,
} from './session-workspace-context'
import { collectTaskSources } from './task-sources'
import { TaskWorkspaceSlots } from './task-workspace-context'
import { useSessionContextAutoReveal } from './useSessionContextAutoReveal'

const SessionInspector = lazy(() =>
  import('./SessionInspector').then((module) => ({ default: module.SessionInspector })),
)

const EMPTY_ACTIVITY: readonly EntityRecord[] = []

// 上下文页种类 → 检查器标签与子视图：外部请求打开某种资源时切到对应位置。
function inspectorTarget(kind: SessionContextTab): {
  tab: SessionInspectorTab
  terminalView?: 'terminal' | 'processes'
  changesView?: 'session' | 'workspace'
} | null {
  switch (kind) {
    case 'changes':
      return { tab: 'changes', changesView: 'workspace' }
    case 'files':
      return { tab: 'changes', changesView: 'session' }
    case 'terminal':
    case 'processes':
      return { tab: 'terminal', terminalView: kind }
    case 'sources':
      return { tab: 'files' }
    case 'browser':
      return { tab: 'preview' }
    case 'plan':
      return { tab: 'plan' }
    case 'side-chat':
      return { tab: 'side-chat' }
    default:
      return null
  }
}

// 底部表单关闭后焦点的去处：打开时的焦点元素。从“⋯”菜单项打开时菜单随即卸载，改记菜单的触发键
//（Radix 菜单内容用 aria-labelledby 指向它）；焦点已在表单里或在 body 上时不记，关闭时退回“+N 改动”。
function sheetFocusReturnTarget(active: Element | null): HTMLElement | null {
  if (!(active instanceof HTMLElement) || active === document.body) return null
  if (active.closest('[data-inspector-sheet]')) return null
  const menu = active.closest('[role="menu"]')
  if (!menu) return active
  const trigger = menu.getAttribute('aria-labelledby')
  return trigger ? document.getElementById(trigger) : null
}

// 覆盖层里可用 Tab 到达的元素：焦点在面板里循环（规格 4.4 / 9.2：1080 宽时覆盖层要测焦点陷阱与 Esc）。
const FOCUSABLE =
  'a[href], button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), iframe, [tabindex]:not([tabindex="-1"])'

function trapOverlayFocus(event: ReactKeyboardEvent<HTMLElement>) {
  if (event.key !== 'Tab' || event.altKey || event.ctrlKey || event.metaKey) return
  const items = [...event.currentTarget.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (item) => item.getClientRects().length > 0,
  )
  if (!items.length) return
  const first = items[0]
  const last = items[items.length - 1]
  const active = document.activeElement
  if (event.shiftKey ? active === first : active === last) {
    event.preventDefault()
    ;(event.shiftKey ? last : first).focus()
  }
}

function planProgress(plan: Plan | null) {
  const items = plan?.items ?? []
  if (!items.length) return null
  return { done: items.filter((item) => item.status === 'completed').length, total: items.length }
}

export function SessionWorkspace({
  sessionId,
  messages,
  cwd,
  active,
  plan,
  goal = null,
  team = null,
  activity = EMPTY_ACTIVITY,
  planTab,
  streaming,
  completed,
  runStartedAt,
  requestConfirm,
  children,
}: {
  sessionId: string
  messages: readonly ChatMessage[]
  cwd: string
  active: boolean
  plan: Plan | null
  goal?: EntityRecord | null
  team?: TeamState | null
  activity?: readonly EntityRecord[]
  planTab: boolean
  streaming: boolean
  completed: boolean
  runStartedAt?: string | null
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  children: ReactNode
}) {
  const { t } = useI18n()
  const chrome = useContext(SessionPageChromeContext)
  const phone = chrome?.phone ?? null
  // 手机外壳：检查器是底部表单。它与紧凑窗口的覆盖层同属“不并排”的临时层：
  // 默认关闭、不写持久化偏好、运行结束不自动弹出，Esc 与原生返回先关它。
  const sheetMode = Boolean(phone)
  const overlayMode = sheetMode || Boolean(chrome?.compact || chrome?.overlayOnly)
  const [headerHost, setHeaderHost] = useState<HTMLDivElement | null>(null)
  const [sourceActionHost, setSourceActionHost] = useState<HTMLDivElement | null>(null)
  const slots = useMemo(
    () => ({ headerHost, sourceActionHost, setSourceActionHost }),
    [headerHost, sourceActionHost],
  )
  const sources = useMemo(() => collectTaskSources(messages), [messages])
  const asideOpen = useSessionInspectorStore((state) => state.open)
  const pinned = useSessionInspectorStore((state) => state.pinned)
  const setAsideOpen = useSessionInspectorStore((state) => state.setOpen)
  const preferredWidth = useSessionInspectorStore((state) => state.width)
  const setWidth = useSessionInspectorStore((state) => state.setWidth)
  // 覆盖层每次进入紧凑形态都从关闭开始（规格：≤1279 时默认关闭），不写进持久化偏好。
  const [overlayOpen, setOverlayOpen] = useState(false)
  useEffect(() => {
    if (!overlayMode) setOverlayOpen(false)
  }, [overlayMode])
  const open = overlayMode ? overlayOpen : asideOpen
  // 手机底部表单关闭后只为退场动画多挂载一会儿，退场结束（或兜底超时、减少动效时立即）就与检查器面板一起卸载。
  // 只跟踪表单：覆盖层与并排检查器仍是关闭即卸载。
  const sheetPresence = useExitPresence(sheetMode && open)
  const sheetLeaving = sheetMode && sheetPresence.leaving
  const sheetOpenerRef = useRef<HTMLElement | null>(null)
  const panelId = `session-${sessionId}-inspector`
  const root = useRef<HTMLDivElement>(null)
  const [width, setRootWidth] = useState(0)
  useLayoutEffect(() => {
    const element = root.current
    if (!element) return
    const measure = () => setRootWidth(element.clientWidth)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  const displayWidth = inspectorDisplayWidth(preferredWidth, width || preferredWidth + 800)
  // 覆盖层从会话顶栏下方开始：标题菜单、⑂ 与改动按钮始终可点。
  const [headerHeight, setHeaderHeight] = useState(0)
  useLayoutEffect(() => {
    if (!headerHost) return
    const measure = () => setHeaderHeight(headerHost.offsetHeight)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(headerHost)
    return () => observer.disconnect()
  }, [headerHost])
  const asideRef = useRef<HTMLElement>(null)
  // 这次打开是否播放滑入（规格 6.6）：只有本次挂载期间被打开才滑入；固定检查器随页面加载就在，直接出现。
  const [entering, setEntering] = useState(false)
  // 覆盖层由用户打开（改动按钮、⌘I、标题菜单、计划进度）时焦点进面板；外部请求（网页预览、自动展示）不抢焦点。
  const [focusOnOpen, setFocusOnOpen] = useState(false)

  const setOpen = useCallback(
    (next: boolean, focus = false) => {
      window.dispatchEvent(new Event(TASK_VIEW_WILL_CHANGE))
      // 表单挂载时焦点会被移进表单：打开入口要在这一刻记下，关闭后把焦点送回去。
      if (sheetMode && next) {
        const opener = sheetFocusReturnTarget(document.activeElement)
        if (opener) sheetOpenerRef.current = opener
      }
      setEntering(next)
      setFocusOnOpen(next && focus)
      if (overlayMode) setOverlayOpen(next)
      else setAsideOpen(next)
    },
    [overlayMode, setAsideOpen, sheetMode],
  )
  const showTab = useCallback(
    (tab: SessionInspectorTab, focus: boolean) => {
      useSessionInspectorStore.getState().setTab(tab)
      setOpen(true, focus)
    },
    [setOpen],
  )
  const show = useCallback((tab: SessionInspectorTab) => showTab(tab, true), [showTab])
  const close = useCallback(() => {
    // 焦点在检查器里（关闭键、标签、终端），或覆盖层关闭时焦点无主（落在 body）时，送回顶栏的改动按钮，
    // 键盘用户不会落到 body（规格 4.4）；在输入框里 ⌘I 打开、再 ⌘I 关闭时焦点留在原处，可以接着输入。
    const active = document.activeElement
    const focusInside =
      Boolean(active && asideRef.current?.contains(active)) ||
      (overlayMode && !sheetMode && (!active || active === document.body))
    setOpen(false)
    if (!focusInside) return
    requestAnimationFrame(() =>
      document
        .querySelector<HTMLElement>(
          `[aria-controls="${CSS.escape(panelId)}"][data-inspector-toggle]`,
        )
        ?.focus(),
    )
  }, [overlayMode, panelId, setOpen, sheetMode])
  // 关闭也走 close：焦点在面板里时按 ⌘I 关掉覆盖层，焦点同样回到改动按钮，不落到 body。
  const toggle = useCallback(() => (open ? close() : setOpen(true, true)), [close, open, setOpen])
  // 表单卸载（退场结束）后焦点回到打开它的入口；入口已不在（例如改动清零后芯片消失）时退回“+N 改动”或“⋯”。
  const returnSheetFocus = useCallback(
    (event: Event) => {
      event.preventDefault()
      const opener = sheetOpenerRef.current
      sheetOpenerRef.current = null
      const target =
        (opener?.isConnected ? opener : null) ??
        document.querySelector<HTMLElement>(
          `[data-session-changes][aria-controls="${CSS.escape(panelId)}"]`,
        ) ??
        document.getElementById(`task-${sessionId}-more`)
      target?.focus({ preventScroll: true })
    },
    [panelId, sessionId],
  )
  const mode = sheetMode ? 'sheet' : overlayMode ? 'overlay' : 'aside'
  const control = useMemo<SessionInspectorControl>(
    () => ({ open, mode, panelId, toggle, show }),
    [open, mode, panelId, show, toggle],
  )

  // 外部请求（网页预览、旧的任务内容事件、自动展示）→ 切到对应标签并打开检查器。
  const reveal = useCallback(
    (kind: SessionContextTab) => {
      const target = inspectorTarget(kind)
      if (!target) return
      const store = useSessionInspectorStore.getState()
      if (target.terminalView) store.setTerminalView(target.terminalView)
      if (target.changesView) store.setChangesView(target.changesView)
      showTab(target.tab, false)
    },
    [showTab],
  )
  useEffect(
    () =>
      useContextPagesStore.subscribe((state, previous) => {
        const next = state.sessions[sessionId]
        const before = previous.sessions[sessionId]
        if (!next || next === before || !next.contentOpen) return
        const page = next.pages.find((entry) => entry.id === next.activeId)
        if (!page) return
        const previousPage = before?.pages.find((entry) => entry.id === before.activeId)
        const changed =
          next.activeId !== before?.activeId ||
          !before?.contentOpen ||
          (page.kind === 'browser' && page.url !== previousPage?.url)
        if (changed) reveal(page.kind)
      }),
    [reveal, sessionId],
  )
  useEffect(() => {
    if (!active) return
    const request = (event: Event) => {
      const kind = parseTaskContentRequest((event as CustomEvent<unknown>).detail)
      if (kind) useContextPagesStore.getState().update(sessionId, kind, { type: 'open', kind })
    }
    window.addEventListener(TASK_CONTENT_REQUESTED, request)
    return () => window.removeEventListener(TASK_CONTENT_REQUESTED, request)
  }, [active, sessionId])
  // ⇧⌘B 会话树与侧聊快捷键（应用壳广播）：会话树与点 ⑂ 走同一条路径（对话框关闭后焦点回到 ⑂）；
  // 还没有对话时没有 ⑂，也就没有会话树可开。侧聊与标题菜单里的“侧聊”相同。
  useEffect(() => {
    if (!active) return
    const command = (event: Event) => {
      const detail = (event as CustomEvent<unknown>).detail
      if (detail === 'side-chat') openSessionSideChat(sessionId, control)
      else if (detail === 'tree')
        root.current?.querySelector<HTMLButtonElement>('[data-session-branches]')?.click()
    }
    window.addEventListener(SESSION_COMMAND_EVENT, command)
    return () => window.removeEventListener(SESSION_COMMAND_EVENT, command)
  }, [active, control, sessionId])
  // 回合结束且本回合留下改动：宽屏时自动滑入一次“改动 · 本会话”（运行中不弹出，免得挤占宽度）；
  // 覆盖层与底部表单不自动弹出，避免盖住刚出来的回复与输入框。
  useSessionContextAutoReveal({
    sessionId,
    streaming,
    completed,
    runStartedAt,
    enabled: !overlayMode,
    open,
    onReveal: () => reveal('files'),
  })

  // ⌘I：开关检查器（应用级组合键，输入框内同样生效）；读 shortcut-store 的当前绑定，改绑后立即生效。
  const inspectorBinding = useShortcutStore((state) => state.bindings.toggleInspector)
  useEffect(() => {
    if (!active) return
    const keydown = (event: KeyboardEvent) => {
      if (!matchesShortcut(event, inspectorBinding) || shortcutEventBlocked(event)) return
      // 改绑成 ⌥ + 字母时，macOS 输入框里的 ⌥ 组合是在打特殊字符，交还输入框。
      if (optionTypingInEditable(event) && event.key.length === 1) return
      event.preventDefault()
      toggle()
    }
    window.addEventListener('keydown', keydown)
    return () => window.removeEventListener('keydown', keydown)
  }, [active, inspectorBinding, toggle])
  // 覆盖层是浮层：Esc / 原生返回先关它（Esc 永远不用于离开会话页）。底部表单本身是对话框，
  // Esc 与原生返回先由它处理；这里的返回层只是兜底，两者不会同时生效。
  useBackLayer(active && overlayMode && overlayOpen, 100, close)

  // —— 宽屏拖宽 ——
  const cleanup = useRef<() => void>(() => {})
  useEffect(() => () => cleanup.current(), [])
  const beginResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return
    cleanup.current()
    event.preventDefault()
    event.currentTarget.focus()
    event.currentTarget.setPointerCapture(event.pointerId)
    const startX = event.clientX
    const startWidth = displayWidth
    const move = (next: PointerEvent) =>
      setWidth(inspectorDisplayWidth(startWidth - (next.clientX - startX), width))
    const finish = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', finish)
      window.removeEventListener('pointercancel', finish)
    }
    cleanup.current = finish
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', finish, { once: true })
    window.addEventListener('pointercancel', finish, { once: true })
  }

  const progress = streaming ? planProgress(plan) : null
  // 宽屏滑入时对话列同步移到新的居中位置：先停在原位（向右偏移检查器宽度的一半），再随检查器一起滑过去。
  const recenter = entering && open && !overlayMode
  // 紧凑窗口打开覆盖层时，宽度允许就给对话内容让出覆盖层的位置（布局变化，不是动画）：
  // 转录与输入框在剩余宽度里重新居中，停止键与发送键不会被盖住。放不下时保持纯覆盖。
  const overlayInset =
    overlayMode &&
    !sheetMode &&
    open &&
    width - SESSION_INSPECTOR_OVERLAY_WIDTH >= SESSION_CONVERSATION_MIN_WIDTH
      ? SESSION_INSPECTOR_OVERLAY_WIDTH
      : 0
  // 检查器收起时整个面板不渲染（预览 iframe 只在“预览”标签可见时存在）；手机表单的退场期间例外，退场结束即卸载。
  const inspector = (open || sheetLeaving) && (
    <Suspense
      fallback={
        <p role="status" className="p-4 text-sm text-content-muted">
          {t('chat:focusSession.gitLoading')}
        </p>
      }
    >
      <SessionInspector
        panelId={panelId}
        sessionId={sessionId}
        cwd={cwd}
        streaming={streaming}
        plan={plan}
        goal={goal}
        team={team}
        activity={activity}
        planTab={planTab}
        sources={sources}
        mode={mode}
        requestConfirm={requestConfirm}
        onClose={close}
        onOpenComponents={chrome?.onOpenComponents}
        autoFocus={mode === 'overlay' && focusOnOpen}
      />
    </Suspense>
  )

  return (
    <TaskWorkspaceSlots.Provider value={slots}>
      <SessionInspectorContext.Provider value={control}>
        <div
          ref={root}
          id={`task-${sessionId}-workspace`}
          tabIndex={-1}
          data-task-workspace={sessionId}
          data-session-workspace={mode}
          data-active={active}
          className={cn(
            'relative flex h-full min-h-0 min-w-0 overflow-hidden bg-panel',
            // 手机会话页没有底部标签栏：输入框自己让出底部安全区，软键盘弹起时贴着键盘。
            sheetMode &&
              "pb-[var(--pisper-safe-area-bottom)] [[data-mobile-keyboard='open']_&]:pb-0",
          )}
        >
          <div className="flex min-h-0 min-w-0 flex-1 flex-col" data-session-conversation>
            <div ref={setHeaderHost} className="@container shrink-0" data-task-header />
            {phone?.strip}
            {progress && (
              // 运行中的计划（规格 4.1）：顶栏下方一条 2px 细进度，右侧“计划 2/4”打开检查器的计划标签看详情。
              // 读数浮在转录上方，不另占一行高度。
              <div className="relative z-10 h-0.5 shrink-0 bg-surface-muted" data-plan-progress>
                <span
                  role="progressbar"
                  aria-label={t('board:session.planLabel', progress)}
                  aria-valuemin={0}
                  aria-valuemax={progress.total}
                  aria-valuenow={progress.done}
                  className="absolute inset-0 origin-left bg-st-run transition-transform duration-(--d-base)"
                  style={{ transform: `scaleX(${progress.done / progress.total})` }}
                />
                {planTab && (
                  <button
                    type="button"
                    className="absolute top-1.5 right-3 inline-flex h-6 items-center rounded-sm bg-panel px-2 font-mono text-xs text-content-secondary tabular-nums shadow-edge hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:outline-focus pointer-coarse:h-11"
                    aria-label={t('board:session.planOpen', {
                      plan: t('board:session.plan', progress),
                    })}
                    aria-controls={panelId}
                    data-plan-progress-open
                    onClick={() => show('plan')}
                  >
                    {t('board:session.plan', progress)}
                  </button>
                )}
              </div>
            )}
            <div
              className={cn(
                'min-h-0 min-w-0 flex-1 overflow-hidden',
                recenter && 'animate-in duration-[220ms] ease-(--ease)',
              )}
              data-overlay-inset={overlayInset || undefined}
              style={
                overlayInset
                  ? { paddingRight: overlayInset }
                  : recenter
                    ? ({ '--tw-enter-translate-x': `${displayWidth / 2}px` } as CSSProperties)
                    : undefined
              }
            >
              {children}
            </div>
          </div>
          {!overlayMode && open && (
            <>
              <div
                role="separator"
                tabIndex={0}
                aria-orientation="vertical"
                aria-label={t('chat:sessionContext.resize')}
                title={t('chat:sessionContext.resizeHint')}
                aria-controls={panelId}
                aria-valuemin={SESSION_INSPECTOR_MIN_WIDTH}
                aria-valuemax={inspectorDisplayWidth(Number.POSITIVE_INFINITY, width)}
                aria-valuenow={displayWidth}
                onPointerDown={beginResize}
                onDoubleClick={() => setWidth(SESSION_INSPECTOR_DEFAULT_WIDTH)}
                onKeyDown={(event) => {
                  if (!['ArrowLeft', 'ArrowRight', 'Enter'].includes(event.key)) return
                  event.preventDefault()
                  if (event.key === 'Enter') {
                    setWidth(SESSION_INSPECTOR_DEFAULT_WIDTH)
                    return
                  }
                  const step = event.shiftKey ? 64 : 16
                  setWidth(
                    inspectorDisplayWidth(
                      displayWidth + (event.key === 'ArrowLeft' ? step : -step),
                      width,
                    ),
                  )
                }}
                className="relative -mr-1 w-2 shrink-0 cursor-col-resize touch-none select-none outline-none after:absolute after:inset-y-0 after:left-1/2 after:w-px after:bg-stroke-soft after:transition-colors after:duration-(--d-fast) hover:after:w-[3px] hover:after:-translate-x-px hover:after:bg-focus focus-visible:after:w-[3px] focus-visible:after:bg-focus"
              />
              <aside
                ref={asideRef}
                className={cn(
                  'h-full min-h-0 shrink-0 overflow-hidden border-l border-stroke-soft',
                  entering && 'animate-in slide-in-from-right duration-[220ms] ease-(--ease)',
                )}
                style={{ width: displayWidth }}
                data-inspector-aside
                data-inspector-pinned={pinned || undefined}
              >
                {inspector}
              </aside>
            </>
          )}
          {overlayMode && (open || sheetLeaving) && (
            // 覆盖层与手机底部表单同属临时层：手机是表单（拖拽条、关闭键、Esc 与 Android 返回键都先关它，
            // 关闭后以 data-state=closed 播完退场动画再卸载，不留空白），其余是右侧覆盖层（关闭即卸载）。
            <>
              {sheetMode ? (
                <Sheet open={open} onOpenChange={(next) => !next && close()}>
                  <SheetContent
                    side="bottom"
                    showCloseButton={false}
                    data-inspector-sheet
                    data-sheet-leaving={sheetLeaving || undefined}
                    // 退场动画结束就卸载整块表单；子元素冒上来的动画事件不算。
                    onAnimationEnd={(event) => {
                      if (event.target === event.currentTarget && !open) sheetPresence.finish()
                    }}
                    onCloseAutoFocus={returnSheetFocus}
                    // 高度固定（覆盖底部表单默认的 h-auto）：切换标签时表单不跳；顶部留出状态栏与一截转录，
                    // 提示这是临时浮层。底色与检查器正文同为 --panel，拖拽条与底部安全区不出现色带（深色明显）。
                    // 不传 aria-describedby：Radix 自动关联下面的 sr-only 说明。
                    className="gap-0 data-[side=bottom]:h-[min(86dvh,calc(100dvh-var(--pisper-safe-area-top)-48px))] overflow-hidden rounded-t-(--r-sheet) bg-panel p-0 pb-[var(--pisper-safe-area-bottom)]"
                  >
                    <SheetTitle className="sr-only">{t('board:inspector.label')}</SheetTitle>
                    <SheetDescription className="sr-only">
                      {t('board:phone.inspectorDescription')}
                    </SheetDescription>
                    <SheetDragHandle onDismiss={close} />
                    {/* 检查器内容里的分段控件（本会话 / 工作区、统一 / 并排、终端 / 进程）在表单里放大到 44px 触控目标。 */}
                    <div className="flex min-h-0 flex-1 flex-col [&_[data-slot=segmented-control]]:h-11 [&_[data-slot=segmented-control-item]]:h-10 [&_[data-slot=segmented-control-item]]:px-3">
                      {inspector}
                    </div>
                  </SheetContent>
                </Sheet>
              ) : (
                // sheetLeaving 只在手机表单时成立：走到这里时覆盖层一定是打开的。
                <aside
                  ref={asideRef}
                  data-inspector-overlay
                  // 无遮罩的覆盖层：焦点进来后 Tab 在面板里循环，Esc（返回层）关闭并把焦点交回改动按钮。
                  onKeyDown={trapOverlayFocus}
                  className={cn(
                    'absolute right-0 bottom-0 z-30 max-w-[calc(100%-24px)] overflow-hidden border-l border-stroke bg-panel shadow-float',
                    'animate-in slide-in-from-right duration-[220ms] ease-(--ease)',
                  )}
                  style={{ width: SESSION_INSPECTOR_OVERLAY_WIDTH, top: headerHeight }}
                >
                  {inspector}
                </aside>
              )}
            </>
          )}
        </div>
      </SessionInspectorContext.Provider>
    </TaskWorkspaceSlots.Provider>
  )
}
