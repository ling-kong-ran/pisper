import { useSessionContextStore } from './session-context-store'
import { TASK_VIEW_WILL_CHANGE } from './events'
import { useBackLayer } from '@/hooks/use-back-layer'
import { useMobileShell } from '@/components/layout/MobileShellContext'
import {
  lazy,
  Suspense,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { createPortal } from 'react-dom'
import { useI18n } from '@/app/use-i18n'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { ChatMessage, Plan } from '@/types/chat'
import { createContextPages } from './session-context-pages'
import { useContextPagesStore } from './session-context-pages-store'
import { SessionContextLayout } from './SessionContextLayout'
import { sessionContextDisplayWidth } from './session-context-layout'
import { useSessionContextAutoReveal } from './useSessionContextAutoReveal'
import { TASK_CONTENT_REQUESTED, parseTaskContentRequest } from './public-workspace'
import { TaskWorkspaceSlots } from './task-workspace-context'
import { TaskSummary } from './TaskSummary'
import { collectTaskSources } from './task-sources'
import { resolveWorkspacePresentation } from './workspace-presentation'

const ContentPanel = lazy(() =>
  import('./SessionContextPanel').then((module) => ({ default: module.SessionContextPanel })),
)

export function TaskWorkspace({
  sessionId,
  messages,
  taskName,
  cwd,
  panelId,
  active,
  mobile,
  plan,
  streaming,
  completed,
  runStartedAt,
  plansAvailable,
  requestConfirm,
  children,
}: {
  sessionId: string
  messages: readonly ChatMessage[]
  taskName: string
  cwd: string
  panelId: string
  active: boolean
  mobile: boolean
  plan: Plan | null
  streaming: boolean
  completed: boolean
  runStartedAt?: string | null
  plansAvailable: boolean
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  children: ReactNode
}) {
  const { t } = useI18n()
  const shell = useMobileShell()
  const shellWasActive = useRef(false)
  const previousShellPane = useRef(shell.pane)
  const mobileToolsVisible =
    active && shell.active && shell.tab === 'session-tools' && shell.pane === 'context'
  const [mobileToolsMounted, setMobileToolsMounted] = useState(false)
  useEffect(() => {
    if (mobileToolsVisible) setMobileToolsMounted(true)
  }, [mobileToolsVisible])
  const [headerHost, setHeaderHost] = useState<HTMLDivElement | null>(null)
  const [sourceActionHost, setSourceActionHost] = useState<HTMLDivElement | null>(null)
  const slots = useMemo(
    () => ({ headerHost, sourceActionHost, setSourceActionHost }),
    [headerHost, sourceActionHost],
  )
  const sources = useMemo(() => collectTaskSources(messages), [messages])
  const defaultContentWidth = useRef(useSessionContextStore.getState().width).current
  const root = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(0)
  const initial = useMemo(() => createContextPages(), [])
  const workspace = useContextPagesStore((store) => store.sessions[sessionId]) || initial
  const update = useContextPagesStore((store) => store.update)
  useEffect(() => {
    if (!active) return
    const enteringShell = shell.active && !shellWasActive.current
    shellWasActive.current = shell.active
    if (enteringShell && workspace.contentOpen && workspace.pages.length > 0)
      shell.openContext('session-tools')
  }, [active, shell, workspace.contentOpen, workspace.pages.length])
  useEffect(() => {
    const previous = previousShellPane.current
    previousShellPane.current = shell.pane
    if (!active || !shell.active) return
    // 返回对话同时关闭内容标志；标签页保留，后续桌面断点不会再次覆盖聊天。
    if (previous !== 'chat' && shell.pane === 'chat' && workspace.contentOpen)
      update(sessionId, 'files', { type: 'workspace', contentOpen: false })
  }, [active, sessionId, shell.active, shell.pane, update, workspace.contentOpen])
  useLayoutEffect(() => {
    if (workspace.contentWidth == null)
      update(sessionId, 'new', { type: 'workspace', contentWidth: defaultContentWidth })
  }, [workspace.contentWidth, sessionId, defaultContentWidth, update])
  useLayoutEffect(() => {
    const element = root.current
    if (!element) return
    const measure = () => setWidth(element.clientWidth)
    measure()
    let frame: number | null = null
    // 宽度决定分栏形态，放到下一帧更新，避免在观察回调内触发分栏再次 resize。
    const observer = new ResizeObserver(() => {
      if (frame !== null) cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => {
        frame = null
        measure()
      })
    })
    observer.observe(element)
    return () => {
      observer.disconnect()
      if (frame !== null) cancelAnimationFrame(frame)
    }
  }, [])
  const presentation = resolveWorkspacePresentation({
    width,
    mobile,
    contentOpen: workspace.contentOpen && workspace.pages.length > 0,
  })
  useEffect(() => {
    if (!mobileToolsVisible || workspace.contentOpen) return
    update(
      sessionId,
      'files',
      workspace.pages.length
        ? { type: 'workspace', contentOpen: true }
        : { type: 'open', kind: 'files' },
    )
  }, [mobileToolsVisible, sessionId, update, workspace.contentOpen, workspace.pages.length])
  useSessionContextAutoReveal({
    sessionId,
    streaming,
    completed,
    runStartedAt,
    enabled: true,
    open: shell.active ? mobileToolsVisible : workspace.contentOpen,
    onReveal: () => {
      update(sessionId, 'files', { type: 'open', kind: 'files' })
      if (shell.active) shell.openContext('session-tools')
    },
  })
  useEffect(() => {
    if (!active) return
    const open = (event: Event) => {
      const kind = parseTaskContentRequest((event as CustomEvent<unknown>).detail)
      if (!kind) return
      update(sessionId, kind, { type: 'open', kind })
      if (!shell.active) return
      if (kind === 'changes') shell.openContext('changes')
      else if (kind === 'terminal' || kind === 'processes') shell.openContext('terminal')
      else shell.openContext('session-tools')
    }
    window.addEventListener(TASK_CONTENT_REQUESTED, open)
    return () => window.removeEventListener(TASK_CONTENT_REQUESTED, open)
  }, [active, sessionId, update, shell])
  const closeContent = () => {
    window.dispatchEvent(new Event(TASK_VIEW_WILL_CHANGE))
    if (shell.active) {
      shell.setPane('chat')
    }
    update(sessionId, 'files', { type: 'workspace', contentOpen: false })
    if (!shell.active)
      requestAnimationFrame(() =>
        (
          document.getElementById(`task-${sessionId}-new-content-tab`) ||
          document.getElementById(`task-${sessionId}-more`)
        )?.focus(),
      )
  }
  useBackLayer(active && !shell.active && presentation === 'content', 100, closeContent)
  const content = (shell.active ? mobileToolsMounted : workspace.contentOpen) &&
    workspace.pages.length > 0 && (
      <Suspense
        fallback={
          <div role="status" className="p-4 text-sm text-content-muted">
            {t('chat:focusSession.gitLoading')}
          </div>
        }
      >
        <ContentPanel
          panelId={`${panelId}-context`}
          compact={false}
          embedded
          active={active && (!shell.active || mobileToolsVisible)}
          sessionId={sessionId}
          cwd={cwd}
          sources={sources}
          tab={workspace.pages.find((page) => page.id === workspace.activeId)?.kind || 'files'}
          plan={plan}
          streaming={streaming}
          plansAvailable={plansAvailable}
          requestConfirm={requestConfirm}
          onTabChange={() => {}}
          onClose={closeContent}
          onBack={closeContent}
        />
      </Suspense>
    )
  return (
    <TaskWorkspaceSlots.Provider value={slots}>
      {shell.active && shell.sessionToolsHost
        ? createPortal(content, shell.sessionToolsHost)
        : null}
      <div
        ref={root}
        id={`task-${sessionId}-workspace`}
        tabIndex={-1}
        data-task-workspace={sessionId}
        data-active={active}
        className="relative flex h-full min-h-0 min-w-0 flex-col overflow-hidden bg-background"
      >
        <div
          ref={setHeaderHost}
          className={
            presentation === 'split'
              ? '@container absolute left-0 top-0 z-10'
              : '@container shrink-0'
          }
          style={
            presentation === 'split'
              ? {
                  width: `calc(100% - ${sessionContextDisplayWidth(workspace.contentWidth ?? defaultContentWidth, width) + 8}px)`,
                }
              : undefined
          }
          data-task-header
        />
        <SessionContextLayout
          preferredWidth={workspace.contentWidth ?? defaultContentWidth}
          onWidthChange={(contentWidth) => {
            update(sessionId, 'new', { type: 'workspace', contentWidth })
            useSessionContextStore.getState().setWidth(contentWidth)
          }}
          layoutId={panelId}
          availableWidth={width}
          conversationHeaderInset={56}
          presentation={
            shell.active
              ? 'closed'
              : presentation === 'split'
                ? 'aside'
                : presentation === 'content'
                  ? 'content'
                  : 'closed'
          }
          context={shell.active ? null : content}
        >
          {children}
        </SessionContextLayout>
        {active && workspace.summaryOpen && (
          <TaskSummary
            sessionId={sessionId}
            cwd={cwd}
            taskName={taskName}
            sources={sources}
            mobile={mobile}
            streaming={streaming}
          />
        )}
      </div>
    </TaskWorkspaceSlots.Provider>
  )
}
