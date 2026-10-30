// 会话页检查器（规格 4.4）：固定标签 改动 N / 终端 / 文件 / 预览 / 计划（有目标、计划或团队时）
// | 组件标签（来自“会话面板”放置，现有数据有就显示）| +。手机外壳里是底部表单（规格 7.3），
// 标签与内容组件相同，改动的 diff 在窄宽度下自动是统一视图、长行横向滚动。本阶段把现有面板组件原样挂进各标签，
// 标签内容的重做在下一阶段。每个会话各标签的阅读位置、终端资源与预览地址仍存在
// session-context-pages-store（与旧上下文面板、网页预览请求共用），开关 / 当前标签 / 宽度是全局偏好。
import { Suspense, useEffect, useRef, type KeyboardEvent } from 'react'
import { useQuery } from '@tanstack/react-query'
import {
  FileDiff,
  Files,
  Globe2,
  ListTodo,
  MessageSquare,
  PanelRightClose,
  Plus,
  TerminalSquare,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { useTerminalService } from '@/features/terminal/public'
import { TaskProcessesPane, TaskTerminalView } from '@/features/terminal/public-views'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { cn } from '@/lib/utils'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import type { EntityRecord, Plan, TeamState } from '@/types/chat'
import { latestComputerUseTarget } from './computer-use-live-support.ts'
import { ComputerUseLiveMirror } from './ComputerUseLiveMirror'
import { InspectorPlan } from './InspectorPlan'
import { getSessionChangeSummary } from './session-change-summary-api'
import { BrowserPane } from './SessionBrowserPane'
import { InspectorChanges } from './InspectorChanges'
import type { ContextPageView, SessionContextTab } from './session-context-pages'
import { useContextPagesStore } from './session-context-pages-store'
import { useSessionInspectorStore, type SessionInspectorTab } from './session-inspector-store'
import { SideChatPane } from './SideChatPane'
import { TaskSourcesPane } from './TaskSourcesPane'
import type { TaskSource } from './task-sources'

type SessionInspectorProps = {
  panelId: string
  sessionId: string
  cwd: string
  streaming: boolean
  plan: Plan | null
  goal: EntityRecord | null
  team: TeamState | null
  // 当前运行的活动流：计划标签的子代理、预览标签的电脑操作镜像都从这里取。
  activity: readonly EntityRecord[]
  // 计划标签只在有目标、计划或团队时出现。
  planTab: boolean
  sources: TaskSource[]
  // sheet：手机外壳的底部表单，标签与关闭键放大到 44px，不显示组件标签入口。
  mode: 'aside' | 'overlay' | 'sheet'
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  onClose: () => void
  onOpenComponents?: () => void
}

const EMPTY_PAGES: never[] = []

// 标签对应的上下文页种类：页记录每个会话的阅读位置与资源，按需创建。
function pageKind(
  tab: SessionInspectorTab,
  terminalView: 'terminal' | 'processes',
  changesView: 'session' | 'workspace',
): SessionContextTab | null {
  if (tab === 'changes') return changesView === 'workspace' ? 'changes' : 'files'
  if (tab === 'terminal') return terminalView
  if (tab === 'files') return 'sources'
  if (tab === 'preview') return 'browser'
  if (tab === 'side-chat') return 'side-chat'
  return null
}

export function SessionInspector({
  panelId,
  sessionId,
  cwd,
  streaming,
  plan,
  goal,
  team,
  activity,
  planTab,
  sources,
  mode,
  requestConfirm,
  onClose,
  onOpenComponents,
}: SessionInspectorProps) {
  const { t } = useI18n()
  const sheet = mode === 'sheet'
  const terminal = useTerminalService()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const terminalAvailable = Boolean(terminal) && runtimeFeatureAvailable(capabilities, 'terminal')
  const vcsAvailable = runtimeFeatureAvailable(capabilities, 'vcs')
  const storedTab = useSessionInspectorStore((state) => state.tab)
  const setTab = useSessionInspectorStore((state) => state.setTab)
  const terminalView = useSessionInspectorStore((state) => state.terminalView)
  const setTerminalView = useSessionInspectorStore((state) => state.setTerminalView)
  const changesView = useSessionInspectorStore((state) => state.changesView)
  const setChangesView = useSessionInspectorStore((state) => state.setChangesView)
  const pages = useContextPagesStore((store) => store.sessions[sessionId]?.pages) ?? EMPTY_PAGES
  const sideChatOpen = pages.some((page) => page.kind === 'side-chat')
  // 当前会话没有侧聊页或计划时，记住的标签退回“改动”，不显示一个不存在的标签。
  const tab: SessionInspectorTab =
    (storedTab === 'side-chat' && !sideChatOpen) || (storedTab === 'plan' && !planTab)
      ? 'changes'
      : storedTab
  const kind = pageKind(tab, terminalView, changesView)
  const page = kind ? pages.find((entry) => entry.kind === kind) : undefined
  const listRef = useRef<HTMLDivElement>(null)
  // 标签多于一行能放下时（窄检查器、手机表单里加了计划与侧聊），选中的标签滚进可见区域。
  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')
      ?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' })
  }, [tab])

  // 当前标签需要的上下文页不存在时创建一页（只为可见标签创建，不预建全部）。
  useEffect(() => {
    if (!kind || page) return
    if (kind === 'terminal' || kind === 'processes') {
      if (!terminalAvailable) return
    }
    const store = useContextPagesStore.getState()
    const contentOpen = store.sessions[sessionId]?.contentOpen ?? false
    store.update(sessionId, kind, { type: 'add', kind })
    // 按需建页只为保存阅读位置与资源；页记录与手机外壳共用，“只显示内容”的开关保持原样，
    // 否则窗口变窄或平板转成竖屏进入手机外壳时，会直接落在资源页而不是对话上。
    if (!contentOpen) store.update(sessionId, kind, { type: 'workspace', contentOpen: false })
  }, [kind, page, sessionId, terminalAvailable])

  const changeSummary = useQuery({
    queryKey: ['session-change-summary', sessionId, streaming],
    queryFn: () => getSessionChangeSummary(sessionId),
    staleTime: 10_000,
  })
  const changedFiles =
    changeSummary.data?.status === 'known' ? (changeSummary.data.changedFiles ?? 0) : 0

  const tabs = [
    {
      id: 'changes' as const,
      icon: FileDiff,
      label:
        changedFiles > 0
          ? t('board:inspector.changesCount', { count: changedFiles })
          : t('board:inspector.changes'),
    },
    { id: 'terminal' as const, icon: TerminalSquare, label: t('board:inspector.terminal') },
    { id: 'files' as const, icon: Files, label: t('board:inspector.files') },
    { id: 'preview' as const, icon: Globe2, label: t('board:inspector.preview') },
    ...(planTab ? [{ id: 'plan' as const, icon: ListTodo, label: t('board:inspector.plan') }] : []),
    ...(sideChatOpen
      ? [{ id: 'side-chat' as const, icon: MessageSquare, label: t('board:inspector.sideChat') }]
      : []),
  ]

  const view = (patch: Partial<ContextPageView>) => {
    if (!page) return
    useContextPagesStore
      .getState()
      .update(sessionId, page.kind, { type: 'view', id: page.id, patch })
  }

  const closeSideChat = () => {
    const sideChat = pages.find((entry) => entry.kind === 'side-chat')
    if (sideChat)
      useContextPagesStore.getState().update(sessionId, 'side-chat', {
        type: 'close',
        id: sideChat.id,
      })
    setTab('changes')
    // 手机底部表单：关掉侧聊就回到对话（侧聊从“⋯”打开，表单是为它弹出的），不停在“改动”上。
    if (sheet) {
      onClose()
      return
    }
    requestAnimationFrame(() => document.getElementById(`${panelId}-changes-tab`)?.focus())
  }

  const tabKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
    event.preventDefault()
    const index = tabs.findIndex((entry) => entry.id === tab)
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? tabs.length - 1
          : (index + (event.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length
    setTab(tabs[next].id)
    requestAnimationFrame(() => document.getElementById(`${panelId}-${tabs[next].id}-tab`)?.focus())
  }

  const loading = (
    <p role="status" className="p-4 text-sm text-content-muted">
      {t('chat:focusSession.gitLoading')}
    </p>
  )

  let body
  if (tab === 'changes') {
    body = (
      <InspectorChanges
        sessionId={sessionId}
        cwd={cwd}
        streaming={streaming}
        scope={changesView}
        onScopeChange={setChangesView}
        vcsAvailable={vcsAvailable}
        view={page?.view}
        onViewChange={page ? view : undefined}
        requestConfirm={requestConfirm}
      />
    )
  } else if (tab === 'terminal') {
    body = !terminalAvailable ? (
      <InlineEmptyState
        icon={TerminalSquare}
        className="m-4"
        title={t('board:inspector.terminalUnavailable')}
      />
    ) : (
      <div className="flex min-h-0 flex-1 flex-col">
        <div className="flex shrink-0 items-center gap-2 border-b border-stroke-soft px-3 py-2">
          <SegmentedControl
            size="sm"
            aria-label={t('board:inspector.terminalScope')}
            value={terminalView}
            onValueChange={setTerminalView}
            options={[
              { value: 'terminal', label: t('board:inspector.terminal') },
              { value: 'processes', label: t('board:inspector.processes') },
            ]}
          />
        </div>
        <Suspense fallback={loading}>
          {!page ? (
            loading
          ) : terminalView === 'processes' ? (
            <TaskProcessesPane
              sessionId={sessionId}
              view={page.view}
              onViewChange={view}
              onOpenTerminal={(id) => {
                const store = useContextPagesStore.getState()
                const terminalPage = store.sessions[sessionId]?.pages.find(
                  (entry) => entry.kind === 'terminal',
                )
                if (terminalPage)
                  store.update(sessionId, 'terminal', {
                    type: 'resource',
                    id: terminalPage.id,
                    resourceId: id,
                  })
                else
                  store.update(sessionId, 'terminal', {
                    type: 'open',
                    kind: 'terminal',
                    resourceId: id,
                  })
                setTerminalView('terminal')
              }}
            />
          ) : (
            <TaskTerminalView
              key={page.id}
              sessionId={sessionId}
              cwd={cwd}
              terminalId={page.resourceId || undefined}
              onCreated={(id) =>
                useContextPagesStore
                  .getState()
                  .update(sessionId, 'terminal', { type: 'resource', id: page.id, resourceId: id })
              }
            />
          )}
        </Suspense>
      </div>
    )
  } else if (tab === 'files') {
    body = page ? (
      <TaskSourcesPane sources={sources} view={page.view} onViewChange={view} />
    ) : (
      loading
    )
  } else if (tab === 'preview') {
    // 预览 = 网页预览 + 电脑操作镜像（规格 4.4）：运行中有目标窗口时镜像排在网页预览上方；
    // 这时转录里不再开第二条帧流（AgentRunActivity 读同一个检查器状态让出镜像）。
    const mirrorTarget = streaming ? latestComputerUseTarget([...activity]) : null
    body = page ? (
      <div className="flex min-h-0 flex-1 flex-col" data-inspector-preview>
        {mirrorTarget && (
          <div className="max-h-[50%] shrink-0 overflow-y-auto border-b border-stroke-soft p-2">
            <ComputerUseLiveMirror
              target={mirrorTarget}
              fallbackImage={mirrorTarget.previewImage}
              streaming={streaming}
            />
          </div>
        )}
        <BrowserPane
          page={page}
          onChange={(patch) =>
            useContextPagesStore
              .getState()
              .update(sessionId, 'browser', { type: 'browser', id: page.id, ...patch })
          }
        />
      </div>
    ) : (
      loading
    )
  } else if (tab === 'plan') {
    body = <InspectorPlan plan={plan} goal={goal} team={team} activity={activity} />
  } else {
    body = <SideChatPane parentId={sessionId} active />
  }

  return (
    <section
      id={panelId}
      aria-label={t('board:inspector.label')}
      data-session-inspector={mode}
      data-inspector-tab={tab}
      className="flex h-full min-h-0 w-full min-w-0 flex-col bg-panel text-content"
    >
      <header
        className={cn(
          'flex shrink-0 items-center gap-1 border-b border-stroke-soft pr-1.5 pl-2',
          // 触屏（手机 App 平板的覆盖层、触屏电脑）与手机表单一样用 52px 标签行：标签行是横向滚动容器，
          // 会裁掉伪元素外扩，标签只能把可见高度抬到 44px。
          sheet ? 'h-[52px]' : 'h-11 pointer-coarse:h-[52px]',
        )}
      >
        <div
          ref={listRef}
          role="tablist"
          aria-label={t('board:inspector.tabs')}
          className="flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto [scrollbar-width:none]"
          onKeyDown={tabKeyDown}
        >
          {tabs.map(({ id, icon: Icon, label }) => {
            const selected = id === tab
            return (
              <button
                key={id}
                id={`${panelId}-${id}-tab`}
                type="button"
                role="tab"
                aria-selected={selected}
                aria-controls={`${panelId}-body`}
                tabIndex={selected ? 0 : -1}
                data-inspector-tab-id={id}
                className={cn(
                  'flex flex-none items-center gap-1.5 rounded-sm px-2 text-[13px] whitespace-nowrap transition-colors duration-(--d-fast) focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus',
                  sheet
                    ? 'h-11 min-w-11 justify-center px-2.5'
                    : 'h-8 pointer-coarse:h-11 pointer-coarse:min-w-11',
                  selected
                    ? 'bg-solid font-medium text-content shadow-edge ring-1 ring-stroke-strong ring-inset'
                    : 'text-content-secondary hover:bg-surface-hover hover:text-content',
                )}
                onClick={() => setTab(id)}
              >
                {/* 390 宽的底部表单放不下图标加文字的五个标签：只留文字。 */}
                {!sheet && <Icon className="size-3.5" aria-hidden="true" />}
                {label}
              </button>
            )
          })}
          {tab === 'side-chat' && (
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              className={cn(sheet && 'size-11')}
              aria-label={t('board:inspector.closeTab', { tab: t('board:inspector.sideChat') })}
              onClick={closeSideChat}
            >
              <X aria-hidden="true" />
            </Button>
          )}
          {/* 组件标签来自“会话面板”放置；现有组件还没有这种放置，没有数据就不显示任何组件标签。
              手机底部表单只放规格 7.3 的固定标签，不提供“+”。 */}
          {!sheet && <span aria-hidden="true" className="mx-1 h-4 w-px flex-none bg-stroke" />}
          {!sheet && (
            <Popover>
              <PopoverTrigger asChild>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  className="flex-none"
                  aria-label={t('board:inspector.add')}
                  title={t('board:inspector.add')}
                >
                  <Plus aria-hidden="true" />
                </Button>
              </PopoverTrigger>
              <PopoverContent align="end" className="w-64 text-xs">
                <p className="text-content-secondary">{t('board:inspector.addHint')}</p>
                {onOpenComponents && (
                  <Button
                    type="button"
                    variant="secondary"
                    size="sm"
                    className="self-start"
                    onClick={onOpenComponents}
                  >
                    {t('board:inspector.openComponents')}
                  </Button>
                )}
              </PopoverContent>
            </Popover>
          )}
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className={cn('flex-none', sheet && 'size-11')}
          aria-label={t('board:inspector.close')}
          title={t('board:inspector.close')}
          aria-controls={panelId}
          onClick={onClose}
        >
          {mode === 'aside' ? <PanelRightClose aria-hidden="true" /> : <X aria-hidden="true" />}
        </Button>
      </header>
      <div
        id={`${panelId}-body`}
        role="tabpanel"
        aria-labelledby={`${panelId}-${tab}-tab`}
        className="flex min-h-0 flex-1 flex-col overflow-hidden"
      >
        {body}
      </div>
    </section>
  )
}
