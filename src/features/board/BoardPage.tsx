// 看板页：页头（注意力胶囊、筛选、分组）、仪表区（第 1 期只有启动台）与占满剩余高度的泳道区。
// 数据全部来自看板数据层（useBoardSessions），这里只做布局与交互编排；手机外壳换成 PhoneBoard。
// 本页挂载启动台后认领“新任务”请求（⌘N / 标题栏按钮）并聚焦启动台；手机上交给派发底部表单。
// 项目入口（新建、管理、横带“⋯”）经 useProjectCommands 复用聊天领域的项目操作。
import { lazy, Suspense, useCallback, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { BOARD_LANES, type BoardLane } from '@shared/session-board-status.mjs'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { Kbd } from '@/components/ui/kbd'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import { QuickPromptIdeas, updateComposerDraft } from '@/features/chat/public-launch'
import { useWorkspaceOrderStore } from '@/features/chat/workspace-order-store'
import { formatShortcut } from '@/lib/shortcuts'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { ApprovalConfirm } from './ApprovalConfirm'
import { BoardHeader, type BoardHeaderProject } from './BoardHeader'
import { projectLabel } from './card-presentation'
import { useLaneLabels } from './lane-labels'
import { boardLaneLayout, laneGridTemplate, visibleLaneItems } from './lane-layout'
import { LaneColumn, type LaneHandle } from './LaneColumn'
import { LAUNCH_DRAFT_KEY, LaunchConsole, type LaunchConsoleHandle } from './LaunchConsole'
import { useBoardNewTaskHandler } from './new-task'
import { PhoneBoard } from './PhoneBoard'
import { ProjectMatrix, type ProjectRow } from './ProjectMatrix'
import { useBoardViewStore } from './board-view-store'
import { sortProjectsByOrder } from './board-model'
import { useLaunchDefaultsStore } from './launch-defaults-store'
import { useBoardController, type BoardNavLane } from './useBoardController'
import { useBoardSessions } from './useBoardSessions'
import { useLaneMotion } from './useLaneMotion'
import { useNow } from './useNow'
import { usePhoneShell } from './usePhoneShell'
import { useProjectCommands } from './useProjectCommands'

// 管理项目对话框与手动填写目录的对话框（系统目录对话框不可用时）只在打开时需要：按需加载。
const WorkspacePicker = lazy(() =>
  import('@/components/WorkspacePicker').then((module) => ({ default: module.WorkspacePicker })),
)
const ManageProjectsDialog = lazy(() =>
  import('./ManageProjectsDialog').then((module) => ({ default: module.ManageProjectsDialog })),
)

export type BoardPageProps = {
  onOpenSession: (sessionId: string) => void
  onOpenHistory: () => void
  onConfigureModels: () => void
  sessionHref: (sessionId: string) => string
  notify: Notify
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  // 交给应用壳的新任务入口：桌面看板会自己认领并聚焦启动台，手机由壳打开派发底部表单。
  onNewTask: () => void
}

function useElementWidth<T extends HTMLElement>() {
  const ref = useRef<T>(null)
  const [width, setWidth] = useState(0)
  useLayoutEffect(() => {
    const node = ref.current
    if (!node) return
    setWidth(node.getBoundingClientRect().width)
    if (typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width))
    observer.observe(node)
    return () => observer.disconnect()
  }, [])
  return [ref, width] as const
}

export function BoardPage(props: BoardPageProps) {
  const phone = usePhoneShell()
  return <BoardView key={phone ? 'phone' : 'desktop'} {...props} phone={phone} />
}

function BoardView({
  onOpenSession,
  onOpenHistory,
  onConfigureModels,
  sessionHref,
  notify,
  requestConfirm,
  requestText,
  onNewTask,
  phone,
}: BoardPageProps & { phone: boolean }) {
  const { t } = useI18n()
  const board = useBoardSessions()
  const laneLabels = useLaneLabels()
  const view = useBoardViewStore()
  const workspaceNames = useWorkspaceOrderStore((state) => state.names)
  const workspaceOrder = useWorkspaceOrderStore((state) => state.order)
  const [rootRef, rootWidth] = useElementWidth<HTMLDivElement>()
  const lanesRef = useRef<HTMLDivElement>(null)
  const consoleRef = useRef<LaunchConsoleHandle>(null)
  const laneHandles = useRef(new Map<BoardLane, LaneHandle>())
  const [peeked, setPeeked] = useState<BoardLane[]>([])
  const [manageMounted, setManageMounted] = useState(false)
  const [manageOpen, setManageOpen] = useState(false)
  const plansAvailable = useRuntimeCapabilitiesStore(
    (state) => state.capabilities.features.goals !== false,
  )
  const narrow = rootWidth > 0 && rootWidth < 1280
  const groupBy = view.groupBy
  const ready = !board.isLoading && !(board.error && board.total === 0)
  // 今天没有卡片（首次运行或只是今天还没有活动）：照常渲染看板骨架——需要你、运行中收成竖条，
  // 已完成泳道展开并保留时间窗切换与“全部历史会话 →”，启动台放大放在泳道区上方。
  const quiet = ready && board.total === 0

  useLayoutEffect(() => {
    if (quiet) setPeeked((current) => (current.includes('done') ? current : [...current, 'done']))
  }, [quiet])

  // 排队泳道依赖 sessionQueue 能力：没有能力时整条隐藏，而不是一条永远为空的列。
  const availableLanes = useMemo(
    () => BOARD_LANES.filter((lane) => lane !== 'queued' || board.capabilities.sessionQueue),
    [board.capabilities.sessionQueue],
  )
  const visibleLanes = useMemo(
    () => availableLanes.filter((lane) => !view.lanes.length || view.lanes.includes(lane)),
    [availableLanes, view.lanes],
  )
  const counts = useMemo(
    () =>
      Object.fromEntries(BOARD_LANES.map((lane) => [lane, board.lanes[lane].length])) as Record<
        BoardLane,
        number
      >,
    [board.lanes],
  )
  const layout = useMemo(
    () =>
      boardLaneLayout({
        counts,
        lanes: visibleLanes,
        collapsed: view.collapsedLanes.filter((lane) => lane === 'done'),
        peeked,
      }),
    [counts, peeked, view.collapsedLanes, visibleLanes],
  )
  const template = laneGridTemplate(layout, rootWidth - 32)

  const headerProjects = useMemo<BoardHeaderProject[]>(
    () =>
      sortProjectsByOrder(board.projects, workspaceOrder).map((project) => ({
        ...project,
        label:
          workspaceNames[project.key] ||
          (project.cwd ? projectLabel(project.cwd) : t('board:filter.noProject')),
      })),
    [board.projects, t, workspaceNames, workspaceOrder],
  )

  const projectRows = useMemo<ProjectRow[]>(
    () =>
      headerProjects
        .filter((project) => !view.projects.length || view.projects.includes(project.key))
        .map((project) => ({
          key: project.key,
          cwd: project.cwd,
          label: project.label,
          custom: Boolean(project.cwd && workspaceNames[project.key]),
          items: board.items.filter(
            (item) => item.workspaceKey === project.key && visibleLanes.includes(item.lane),
          ),
        }))
        .filter((row) => row.items.length > 0),
    [board.items, headerProjects, view.projects, visibleLanes, workspaceNames],
  )

  const navLanes = useMemo<BoardNavLane[]>(() => {
    if (groupBy === 'project')
      return visibleLanes.map((lane) => ({
        lane,
        items: projectRows.flatMap((row) => row.items.filter((item) => item.lane === lane)),
      }))
    return layout
      .filter((entry) => entry.mode === 'expanded')
      .map((entry) => ({
        lane: entry.lane,
        items: visibleLaneItems(entry.lane, board.lanes[entry.lane]),
      }))
  }, [board.lanes, groupBy, layout, projectRows, visibleLanes])

  const attentionItems = board.lanes.attention
  const controller = useBoardController({
    rootRef,
    navLanes,
    attention: attentionItems,
    attentionAll: board.attentionAll,
    onRevealAttention: view.clearFilters,
    laneHandles,
    ready: !board.isLoading,
    notify,
    sessionHref,
    onOpenSession,
    onToggleGrouping: () => view.setGroupBy(groupBy === 'lane' ? 'project' : 'lane'),
  })
  const announcement = useLaneMotion({
    rootRef,
    items: board.items,
    laneName: laneLabels.name,
    announceTemplate: (title, lane) => t('board:announce.moved', { title, lane }),
    skipFlipRef: controller.skipFlipRef,
  })
  const ticking = board.attentionAll.length > 0 || board.lanes.running.length > 0
  const now = useNow(ticking)

  const consoleAvailable = ready
  // ⌘N / 标题栏“新任务”：看板上聚焦启动台；手机上不认领，由应用壳打开派发底部表单。
  useBoardNewTaskHandler(() => {
    if (phone || !consoleAvailable || !consoleRef.current) return false
    consoleRef.current.focus()
    return true
  })

  // 新项目或“在此项目新建任务”之后：桌面直接聚焦启动台；手机或启动台暂不可用时交给应用壳。
  const focusLaunch = useCallback(() => {
    if (!phone && consoleRef.current) consoleRef.current.focus()
    else onNewTask()
  }, [onNewTask, phone])
  const projects = useProjectCommands({
    notify,
    requestConfirm,
    requestText,
    onProjectChosen: focusLaunch,
    announceChoice: !phone,
  })
  const openManageProjects = useCallback(() => {
    setManageMounted(true)
    setManageOpen(true)
  }, [])
  const { mounted: pickerMounted, ...pickerProps } = projects.picker
  const projectLayers = (
    <>
      {pickerMounted && (
        <Suspense fallback={null}>
          <WorkspacePicker {...pickerProps} />
        </Suspense>
      )}
      {manageMounted && (
        <Suspense fallback={null}>
          <ManageProjectsDialog
            open={manageOpen}
            onOpenChange={setManageOpen}
            commands={projects}
          />
        </Suspense>
      )}
    </>
  )

  const onDispatched = useCallback(
    (sessionId: string, { open }: { open: boolean }) => {
      if (open) controller.handlers.onOpen(sessionId)
    },
    [controller.handlers],
  )

  // 页头胶囊按全部需要你事项计数：筛选只影响泳道里显示什么，不能让胶囊报“全部清空”。
  const attentionSummary = {
    total: board.attentionAll.length,
    approvals: board.attentionAll.filter((item) => item.status === 'approval').length,
    questions: board.attentionAll.filter((item) => item.status === 'question').length,
    failures: board.attentionAll.filter((item) => item.status === 'failed').length,
    hidden: Math.max(0, board.attentionAll.length - attentionItems.length),
  }
  const triageKey = formatShortcut(controller.bindings.boardTriage)
  const dimLane = (lane: BoardLane) => Boolean(controller.triage) && lane !== 'attention'
  const filtered = view.projects.length > 0 || view.lanes.length > 0

  if (phone)
    return (
      <>
        <PhoneBoard
          rootRef={rootRef}
          board={board}
          availableLanes={availableLanes}
          projects={headerProjects}
          now={now}
          controller={controller}
          announcement={announcement}
          onConfigureModels={onConfigureModels}
          onOpenHistory={onOpenHistory}
          projectPicking={projects.picking}
          onNewProject={() => void projects.newProject()}
          onManageProjects={openManageProjects}
        />
        {projectLayers}
      </>
    )

  const confirmLayer = (
    <ApprovalConfirm
      target={controller.confirm?.target ?? null}
      phone={false}
      pending={controller.confirm?.pending ?? false}
      error={controller.confirm?.error ?? ''}
      onConfirm={() => void controller.confirmApproval()}
      onOpenChange={controller.setConfirmOpen}
    />
  )

  let content
  if (board.isLoading) content = <PageState kind="loading" title={t('board:page.loading')} />
  else if (board.error && board.total === 0)
    content = (
      <PageState
        kind="error"
        title={t('board:page.error')}
        description={t('board:page.errorHint')}
        action={
          <Button variant="secondary" onClick={() => void board.refresh()}>
            {t('board:page.retry')}
          </Button>
        }
      />
    )
  else
    content = (
      <>
        <div className="flex-none px-4 pb-3">
          {view.deckCollapsed && !quiet ? (
            <LaunchConsole
              ref={consoleRef}
              variant="collapsed"
              compactChips
              onDispatched={onDispatched}
              onConfigureModels={onConfigureModels}
            />
          ) : (
            // 仪表区：8 单位网格。第 1 期还没有组件放置数据，启动台占满一行，网格留给第 5 期的组件卡片，
            // 不放占位假卡。空看板与有卡片时是同一个启动台实例（只换外形），派发后焦点与状态都不丢。
            <div className="grid grid-cols-8 gap-3" data-board-deck>
              {quiet && (
                <div className="col-span-8 flex min-w-0 flex-col gap-1 pt-2">
                  <h2 className="text-lg font-semibold text-content">
                    {board.catalogEmpty ? t('board:page.empty') : t('board:page.quiet')}
                  </h2>
                  <p className="text-sm text-content-muted">
                    {board.catalogEmpty ? t('board:page.emptyHint') : t('board:page.quietHint')}
                  </p>
                </div>
              )}
              <LaunchConsole
                ref={consoleRef}
                variant={quiet ? 'hero' : 'deck'}
                autoFocus={quiet}
                compactChips={narrow}
                className="col-span-8"
                onDispatched={onDispatched}
                onConfigureModels={onConfigureModels}
              />
              {quiet && board.catalogEmpty && (
                // 建议卡片按容器宽度分页（每页 3 张）或横滑，依赖外层 @container。
                <div className="@container col-span-8 min-w-0">
                  <QuickPromptIdeas
                    plansAvailable={plansAvailable}
                    onPromptSelect={(prompt) => {
                      // 只填入草稿并聚焦，不自动派发。
                      updateComposerDraft(LAUNCH_DRAFT_KEY, { text: prompt })
                      consoleRef.current?.fill(prompt)
                    }}
                  />
                </div>
              )}
            </div>
          )}
        </div>
        {board.total > 0 && board.items.length === 0 && filtered ? (
          <PageState
            kind="empty"
            title={t('board:filter.emptyTitle')}
            description={t('board:filter.emptyHint')}
            action={
              <Button variant="secondary" onClick={view.clearFilters}>
                {t('board:filter.clear')}
              </Button>
            }
          />
        ) : groupBy === 'project' ? (
          <ProjectMatrix
            rows={projectRows}
            lanes={visibleLanes}
            template={laneGridTemplate(
              visibleLanes.map((lane) => ({ lane, mode: 'expanded', count: 1, railReason: null })),
              rootWidth - 32,
            )}
            now={now}
            activeId={controller.activeId}
            dimLane={dimLane}
            handlers={controller.handlers}
            actions={{
              onNewTask: (row) => {
                useLaunchDefaultsStore.getState().rememberLaunch({ cwd: row.cwd })
                focusLaunch()
              },
              onRename: (row) => void projects.rename(row),
              onResetName: (row) => projects.resetName(row),
              onDelete: (row) => void projects.remove(row),
              deletingKey: projects.busyKey,
            }}
          />
        ) : (
          <div
            ref={lanesRef}
            role="region"
            aria-label={t('board:page.lanes')}
            className="grid min-h-0 flex-1 gap-3 px-4 pb-4"
            style={{ gridTemplateColumns: template }}
          >
            {layout.map((entry) => (
              <LaneColumn
                key={entry.lane}
                laneRef={(handle) => {
                  if (handle) laneHandles.current.set(entry.lane, handle)
                  else laneHandles.current.delete(entry.lane)
                }}
                entry={entry}
                items={board.lanes[entry.lane]}
                now={now}
                activeId={controller.activeId}
                dimmed={dimLane(entry.lane)}
                handlers={controller.handlers}
                doneWindow={view.doneWindow}
                onDoneWindowChange={view.setDoneWindow}
                onPeek={(lane, open) =>
                  setPeeked((current) =>
                    open
                      ? [...new Set([...current, lane])]
                      : current.filter((item) => item !== lane),
                  )
                }
                onCollapse={(lane, collapsed) => {
                  view.setLaneCollapsed(lane, collapsed)
                  if (!collapsed) setPeeked((current) => current.filter((item) => item !== lane))
                }}
                onOpenHistory={onOpenHistory}
                offerWeek={!board.catalogEmpty}
                onDispatch={focusLaunch}
              />
            ))}
          </div>
        )}
      </>
    )

  return (
    <div
      ref={rootRef}
      data-board-root
      className="relative flex h-full min-h-0 w-full min-w-0 flex-col bg-background"
    >
      <BoardHeader
        total={board.total}
        projectCount={board.projects.length}
        running={board.lanes.running.length}
        attention={attentionSummary}
        ready={ready}
        projects={headerProjects}
        projectFilter={view.projects}
        laneFilter={view.lanes}
        availableLanes={availableLanes}
        groupBy={groupBy}
        narrow={narrow}
        deckCollapsed={view.deckCollapsed}
        triageKey={triageKey}
        onTriage={controller.triage ? controller.exitTriage : controller.startTriage}
        onProjectFilter={view.setProjectFilter}
        onToggleProject={view.toggleProjectFilter}
        onToggleLane={view.toggleLaneFilter}
        onClearLanes={() => view.setLaneFilter([])}
        onGroupBy={view.setGroupBy}
        onDeckCollapsed={view.setDeckCollapsed}
        projectPicking={projects.picking}
        onNewProject={() => void projects.newProject()}
        onManageProjects={openManageProjects}
      />
      {content}
      {controller.triage && (
        <TriageBar
          index={controller.triage.index}
          total={controller.triage.total}
          finished={controller.triage.finished}
          keys={{
            approve: formatShortcut(controller.bindings.boardApprove),
            deny: formatShortcut(controller.bindings.boardDeny),
            retry: formatShortcut(controller.bindings.boardRetry),
          }}
          onExit={controller.exitTriage}
        />
      )}
      <p aria-live="polite" role="status" className="sr-only">
        {announcement}
      </p>
      {confirmLayer}
      {projectLayers}
    </div>
  )
}

function TriageBar({
  index,
  total,
  finished,
  keys,
  onExit,
}: {
  index: number
  total: number
  finished: boolean
  keys: { approve: string; deny: string; retry: string }
  onExit: () => void
}) {
  const { t } = useI18n()
  return (
    <div
      role="status"
      aria-live="polite"
      data-triage-bar
      className="absolute bottom-4 left-1/2 z-20 flex h-10 max-w-[calc(100%-32px)] -translate-x-1/2 items-center gap-3 overflow-hidden rounded-lg border border-stroke bg-solid px-3 text-xs whitespace-nowrap text-content-secondary shadow-float"
    >
      {finished ? (
        <b className="font-semibold text-st-done">{t('board:triage.finished')}</b>
      ) : (
        <>
          <b className="font-semibold text-content">
            {t('board:triage.progress', { index, total })}
          </b>
          <span className="flex items-center gap-1">
            <Kbd>{keys.approve}</Kbd>
            {t('board:triage.approve')}
          </span>
          <span className="flex items-center gap-1">
            <Kbd>{keys.deny}</Kbd>
            {t('board:triage.deny')}
          </span>
          <span className="flex items-center gap-1">
            <Kbd>{keys.retry}</Kbd>
            {t('board:triage.retry')}
          </span>
          <span className="flex items-center gap-1">
            <Kbd>↵</Kbd>
            {t('board:triage.open')}
          </span>
          <Button variant="ghost" size="xs" onClick={onExit}>
            <Kbd>Esc</Kbd>
            {t('board:triage.exit')}
          </Button>
        </>
      )}
    </div>
  )
}
