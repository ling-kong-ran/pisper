// 会话页（/board/:sessionId，桌面布局）的单会话工作区：对话列（页头 44 + 计划细进度 + 转录与输入框）
// + 检查器。宽屏时检查器 456px 并排、可拖宽、可收起；≤1279 宽或 ≤760 高（以及手机 App 平板）时默认关闭，
// 改为从右侧滑出的 420px 覆盖层（无遮罩，--sh-float）。⌘I 开关检查器。
// 外部的“打开资源”请求（网页预览、任务内容请求、运行结束后的自动展示）仍写进 session-context-pages-store，
// 这里订阅它并切到对应标签，所以这些入口不需要知道会话页换了布局。
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
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from 'react'
import { useI18n } from '@/app/use-i18n'
import { useBackLayer } from '@/hooks/use-back-layer'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { matchesShortcut, shortcutEventBlocked } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import type { ChatMessage, EntityRecord, Plan, TeamState } from '@/types/chat'
import { TASK_VIEW_WILL_CHANGE } from './events'
import { TASK_CONTENT_REQUESTED, parseTaskContentRequest } from './public-workspace'
import type { SessionContextTab } from './session-context-pages'
import { useContextPagesStore } from './session-context-pages-store'
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

export const INSPECTOR_SHORTCUT = 'Mod+KeyI'
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
  const overlayMode = Boolean(chrome?.compact || chrome?.overlayOnly)
  const [headerHost, setHeaderHost] = useState<HTMLDivElement | null>(null)
  const [sourceActionHost, setSourceActionHost] = useState<HTMLDivElement | null>(null)
  const slots = useMemo(
    () => ({ headerHost, sourceActionHost, setSourceActionHost }),
    [headerHost, sourceActionHost],
  )
  const sources = useMemo(() => collectTaskSources(messages), [messages])
  const asideOpen = useSessionInspectorStore((state) => state.open)
  const setAsideOpen = useSessionInspectorStore((state) => state.setOpen)
  const preferredWidth = useSessionInspectorStore((state) => state.width)
  const setWidth = useSessionInspectorStore((state) => state.setWidth)
  // 覆盖层每次进入紧凑形态都从关闭开始（规格：≤1279 时默认关闭），不写进持久化偏好。
  const [overlayOpen, setOverlayOpen] = useState(false)
  useEffect(() => {
    if (!overlayMode) setOverlayOpen(false)
  }, [overlayMode])
  const open = overlayMode ? overlayOpen : asideOpen
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
  // 覆盖层从会话页头下方开始：页头的“⋯”、侧聊与检查器开关始终可点。
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

  const setOpen = useCallback(
    (next: boolean) => {
      window.dispatchEvent(new Event(TASK_VIEW_WILL_CHANGE))
      if (overlayMode) setOverlayOpen(next)
      else setAsideOpen(next)
    },
    [overlayMode, setAsideOpen],
  )
  const show = useCallback(
    (tab: SessionInspectorTab) => {
      useSessionInspectorStore.getState().setTab(tab)
      setOpen(true)
    },
    [setOpen],
  )
  const toggle = useCallback(() => setOpen(!open), [open, setOpen])
  const close = useCallback(() => {
    // 焦点原本在检查器里（关闭键、标签、终端）时才送回页头开关，键盘用户不会落到 body；
    // 在输入框里 ⌘I 打开、Esc 关闭时焦点留在原处，可以接着输入。
    const active = document.activeElement
    const focusInside = Boolean(active && asideRef.current?.contains(active))
    setOpen(false)
    if (!focusInside) return
    requestAnimationFrame(() =>
      document
        .querySelector<HTMLElement>(
          `[aria-controls="${CSS.escape(panelId)}"][data-inspector-toggle]`,
        )
        ?.focus(),
    )
  }, [panelId, setOpen])
  const control = useMemo<SessionInspectorControl>(
    () => ({ open, mode: overlayMode ? 'overlay' : 'aside', panelId, toggle, show }),
    [open, overlayMode, panelId, show, toggle],
  )

  // 外部请求（网页预览、旧的任务内容事件、自动展示）→ 切到对应标签并打开检查器。
  const reveal = useCallback(
    (kind: SessionContextTab) => {
      const target = inspectorTarget(kind)
      if (!target) return
      const store = useSessionInspectorStore.getState()
      if (target.terminalView) store.setTerminalView(target.terminalView)
      if (target.changesView) store.setChangesView(target.changesView)
      show(target.tab)
    },
    [show],
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
  // 运行结束且本轮留下改动：宽屏时自动展开“改动 · 本会话”；覆盖层不自动弹出，避免盖住刚出来的回复。
  useSessionContextAutoReveal({
    sessionId,
    streaming,
    completed,
    runStartedAt,
    enabled: !overlayMode,
    open,
    onReveal: () => reveal('files'),
  })

  // ⌘I：开关检查器（应用级组合键，输入框内同样生效）。
  useEffect(() => {
    if (!active) return
    const keydown = (event: KeyboardEvent) => {
      if (!matchesShortcut(event, INSPECTOR_SHORTCUT) || shortcutEventBlocked(event)) return
      event.preventDefault()
      toggle()
    }
    window.addEventListener('keydown', keydown)
    return () => window.removeEventListener('keydown', keydown)
  }, [active, toggle])
  // 覆盖层是浮层：Esc / 原生返回先关它（Esc 永远不用于离开会话页）。
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
  // 紧凑窗口打开覆盖层时，宽度允许就给对话内容让出覆盖层的位置（布局变化，不是动画）：
  // 转录与输入框在剩余宽度里重新居中，停止键与发送键不会被盖住。放不下时保持纯覆盖。
  const overlayInset =
    overlayMode && open && width - SESSION_INSPECTOR_OVERLAY_WIDTH >= SESSION_CONVERSATION_MIN_WIDTH
      ? SESSION_INSPECTOR_OVERLAY_WIDTH
      : 0
  const inspector = open && (
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
        mode={overlayMode ? 'overlay' : 'aside'}
        requestConfirm={requestConfirm}
        onClose={close}
        onOpenComponents={chrome?.onOpenComponents}
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
          data-session-workspace={overlayMode ? 'overlay' : 'aside'}
          data-active={active}
          className="relative flex h-full min-h-0 min-w-0 overflow-hidden bg-panel"
        >
          <div className="flex min-h-0 min-w-0 flex-1 flex-col" data-session-conversation>
            <div ref={setHeaderHost} className="@container shrink-0" data-task-header />
            {progress && (
              <div
                role="progressbar"
                aria-label={t('board:session.planLabel', progress)}
                aria-valuemin={0}
                aria-valuemax={progress.total}
                aria-valuenow={progress.done}
                data-plan-progress
                className="flex h-6 shrink-0 items-center gap-2 border-b border-stroke-soft px-4 text-xs text-content-muted"
              >
                <span className="flex-none font-medium text-content-secondary">
                  {t('board:session.plan', progress)}
                </span>
                <span className="relative h-1 min-w-0 flex-1 overflow-hidden rounded-full bg-surface-muted">
                  <span
                    className="absolute inset-y-0 left-0 w-full origin-left rounded-full bg-st-run transition-transform duration-(--d-base)"
                    style={{ transform: `scaleX(${progress.done / progress.total})` }}
                  />
                </span>
              </div>
            )}
            <div
              className="min-h-0 min-w-0 flex-1 overflow-hidden"
              data-overlay-inset={overlayInset || undefined}
              style={overlayInset ? { paddingRight: overlayInset } : undefined}
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
                className="h-full min-h-0 shrink-0 overflow-hidden border-l border-stroke-soft"
                style={{ width: displayWidth }}
                data-inspector-aside
              >
                {inspector}
              </aside>
            </>
          )}
          {overlayMode && open && (
            <aside
              ref={asideRef}
              data-inspector-overlay
              className={cn(
                'absolute right-0 bottom-0 z-30 max-w-[calc(100%-24px)] overflow-hidden border-l border-stroke bg-panel shadow-float',
                'animate-in fade-in-0 slide-in-from-right-4 duration-(--d-base) ease-(--ease-enter)',
              )}
              style={{ width: SESSION_INSPECTOR_OVERLAY_WIDTH, top: headerHeight }}
            >
              {inspector}
            </aside>
          )}
        </div>
      </SessionInspectorContext.Provider>
    </TaskWorkspaceSlots.Provider>
  )
}
