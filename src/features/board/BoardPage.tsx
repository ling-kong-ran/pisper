// 全部代理（原看板，规格 v2 第 3 节；路径 /board、页面 id board 不变）：页头（标题、统计、分组方式、新任务）
// + 居中的分组列表（最大 880）。按状态：需要你 → 运行中 → 排队 → 今天完成；按项目：每个项目一个分区，
// 区内仍按状态。每个代理一行（OverviewRow），行内一步处理普通批准与重试；高风险、提问只能打开处理。
// 这一页没有任务输入框：“新任务”与 ⌘N 打开派发对话框（不认领新任务请求，交给应用壳）。
// 右侧 320 的组件栏（overview-card 放置）在 A7 才有组件：没有组件时不渲染、不留空位。
// 数据只读看板 store（useBoardSessions），不新开轮询。手机外壳（规格 v2 第 7.3 节）是同一页：
// 标题与 ✎ 在外壳顶栏里，页内只有 44px 的“按状态 / 按项目”分段；需要你的行变成卡片（44px 按钮），
// 其余仍是列表行。第 4 期的手机看板（泳道跳转条、卡片墙、确认层）随之退役，高风险一律进会话按住确认。
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { defaultRangeExtractor, useVirtualizer, type Range } from '@tanstack/react-virtual'
import { ArrowRight, ChevronDown, ChevronRight, Folder, SquarePen, X } from 'lucide-react'
import type { BoardLane } from '@shared/session-board-status.mjs'
import { APP_NAME } from '@/app/brand'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { Kbd } from '@/components/ui/kbd'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import { formatFileSize, formatTokenCount } from '@/lib/format'
import { formatShortcut } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { useSidebarStore } from '@/stores/sidebar-store'
import { useWorkspaceOrderStore } from '@/features/chat/workspace-order-store'
import type { BoardItem } from './board-model'
import { useBoardViewStore } from './board-view-store'
import { OVERVIEW_FOCUS_EVENT, type OverviewFocusDetail } from './overview-focus'
import { useLaneLabels } from './lane-labels'
import { useLaunchDefaultsStore } from './launch-defaults-store'
import {
  buildOverviewModel,
  overviewShouldVirtualize,
  type OverviewProjectSection,
  type OverviewSection,
} from './overview-model'
import { OverviewRow } from './OverviewRow'
import { ProjectSectionMenu, type ProjectSectionActions } from './ProjectSectionMenu'
import { useBoardAnnouncer } from './useBoardAnnouncer'
import { useBoardController } from './useBoardController'
import { useBoardSessions } from './useBoardSessions'
import { useFleetSystemInfo } from './useFleetSystemInfo'
import { useLabeledProjects, type LabeledProject } from './useLabeledProjects'
import { useMinute, useNow } from './useNow'
import { usePhoneShell } from './usePhoneShell'
import { useProjectCommands } from './useProjectCommands'
import { useSessionRowActions } from './useSessionRowActions'

const NO_ROW_BINDINGS = Object.freeze({ approve: null, deny: null, retry: null })

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
  // 新任务：交给应用壳打开派发对话框（桌面）或底部表单（手机）。
  onNewTask: () => void
  // 组件栏（overview-card 放置，A7）：有组件时才传，页面据此在右侧留 320 栏。
  widgets?: ReactNode
}

export function BoardPage(props: BoardPageProps) {
  const phone = usePhoneShell()
  const { t } = useI18n()
  // 文档标题跟随页面名（规格 2.4：看板改名全部代理，路径不变）；离开时还给应用名。
  useEffect(() => {
    document.title = `${t('board:page.title')} · ${APP_NAME}`
    return () => {
      document.title = APP_NAME
    }
  }, [t])
  // 切换外壳时整页换掉（key），不沿用另一种形态的焦点与滚动状态。
  return <OverviewPage key={phone ? 'phone' : 'desktop'} {...props} phone={phone} />
}

// 列表的扁平条目：分区标题、行、分区的空状态。虚拟列表与直接渲染共用同一份顺序。
type Entry =
  | { kind: 'header'; key: string; section: OverviewSection }
  | { kind: 'row'; key: string; item: BoardItem; phase: number }
  | { kind: 'empty'; key: string; lane: BoardLane }

function OverviewPage({
  onOpenSession,
  onOpenHistory,
  sessionHref,
  notify,
  requestConfirm,
  requestText,
  onNewTask,
  widgets,
  phone,
}: BoardPageProps & { phone: boolean }) {
  const { t } = useI18n()
  const board = useBoardSessions({ doneWindow: 'today' })
  const view = useBoardViewStore()
  const sidebarCollapsed = useSidebarStore((state) => state.collapsed)
  const laneLabels = useLaneLabels()
  const rootRef = useRef<HTMLDivElement>(null)
  const scrollerRef = useRef<HTMLDivElement>(null)
  const minute = useMinute()
  const projects = useLabeledProjects(board.projects)
  const doneCollapsed = view.collapsedLanes.includes('done')
  const model = useMemo(
    () =>
      buildOverviewModel(board.all, {
        now: minute,
        groupBy: view.groupBy,
        capabilities: board.capabilities,
        lanes: view.lanes,
        projects: view.projects,
        projectOrder: projects,
        doneCollapsed,
      }),
    [
      board.all,
      board.capabilities,
      doneCollapsed,
      minute,
      projects,
      view.groupBy,
      view.lanes,
      view.projects,
    ],
  )
  const projectByKey = useMemo(
    () => new Map(projects.map((project) => [project.key, project])),
    [projects],
  )
  const ticking = model.counts.attention + model.counts.running > 0
  const now = useNow(ticking)
  const ready = !board.isLoading && !(board.error && board.total === 0)

  // 扁平条目：折叠的今天完成只留标题；按状态分组时需要你 / 运行中为空也留标题与一句安静的空状态。
  const entries = useMemo<Entry[]>(() => {
    const list: Entry[] = []
    for (const section of model.sections) {
      const key = section.kind === 'status' ? `status:${section.id}` : `project:${section.key}`
      list.push({ kind: 'header', key, section })
      const collapsed = section.kind === 'status' && section.id === 'done' && doneCollapsed
      if (collapsed) continue
      if (!section.items.length && section.kind === 'status')
        list.push({ kind: 'empty', key: `${key}:empty`, lane: section.id })
      let running = 0
      for (const item of section.items)
        list.push({
          kind: 'row',
          key: item.id,
          item,
          // 运行中弧的相位：黄金分割步进，同一分区里的多行不会落在同一角度附近。
          phase: item.lane === 'running' ? (running++ * 0.381966) % 1 : 0,
        })
    }
    return list
  }, [doneCollapsed, model.sections])
  const rowCount = model.order.length
  const virtual = overviewShouldVirtualize(rowCount)
  // 虚拟列表始终挂着当前行（roving tabindex 唯一 tabIndex=0 的那行）：滚轮把它滚出可视窗口时，
  // Tab 仍能进入列表，聚焦中的行也不会被卸载、把焦点丢到 body 上。值在控制器算出 activeId 后同一次渲染里更新。
  const activeIndexRef = useRef(-1)
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLElement>({
    count: virtual ? entries.length : 0,
    getScrollElement: () => scrollerRef.current,
    estimateSize: (index) => (entries[index]?.kind === 'row' ? 64 : 40),
    overscan: 8,
    getItemKey: (index) => entries[index]?.key ?? index,
    rangeExtractor: (range: Range) => {
      const indexes = defaultRangeExtractor(range)
      const active = activeIndexRef.current
      if (active < 0 || active >= range.count || indexes.includes(active)) return indexes
      return [...indexes, active].sort((left, right) => left - right)
    },
  })
  const reveal = useCallback(
    async (sessionId: string) => {
      const find = () =>
        rootRef.current?.querySelector<HTMLElement>(
          `[data-board-card="${CSS.escape(sessionId)}"]`,
        ) ?? null
      if (!virtual || find()) return find()
      const index = entries.findIndex((entry) => entry.kind === 'row' && entry.key === sessionId)
      if (index < 0) return null
      virtualizer.scrollToIndex(index, { align: 'auto' })
      for (let frame = 0; frame < 3 && !find(); frame += 1)
        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
      return find()
    },
    [entries, virtual, virtualizer],
  )

  const controller = useBoardController({
    rootRef,
    order: model.order,
    attention: model.attention,
    attentionAll: model.attentionAll,
    onRevealAttention: view.clearFilters,
    reveal,
    ready: !board.isLoading,
    notify,
    sessionHref,
    onOpenSession,
    onToggleGrouping: () => view.setGroupBy(view.groupBy === 'lane' ? 'project' : 'lane'),
  })
  activeIndexRef.current = virtual
    ? entries.findIndex((entry) => entry.kind === 'row' && entry.key === controller.activeId)
    : -1
  // 行 ⋯ 的“打开会话”走控制器：与点行、↵ 一样记下滚动位置与来源行，回来时恢复。
  const menu = useSessionRowActions({
    notify,
    requestText,
    onOpenSession: controller.handlers.onOpen,
  })

  // ⌥A（侧栏命令）在这一页把焦点交给等待最久的那一行：经控制器 focusCard（虚拟列表里先滚到它），
  // 只有它确实被当前筛选藏起来时才清筛选，等它回到列表再聚焦；不按 DOM 里有没有来判断。
  const [pendingFocus, setPendingFocus] = useState('')
  const focusCard = controller.focusCard
  const orderRef = useRef(model.order)
  orderRef.current = model.order
  const attentionAllRef = useRef(model.attentionAll)
  attentionAllRef.current = model.attentionAll
  useEffect(() => {
    const listener = (event: Event) => {
      const sessionId = (event as CustomEvent<OverviewFocusDetail>).detail?.sessionId
      if (!sessionId) return
      if (orderRef.current.some((item) => item.id === sessionId)) {
        void focusCard(sessionId)
        return
      }
      if (!attentionAllRef.current.some((item) => item.id === sessionId)) return
      setPendingFocus(sessionId)
      useBoardViewStore.getState().clearFilters()
    }
    window.addEventListener(OVERVIEW_FOCUS_EVENT, listener)
    return () => window.removeEventListener(OVERVIEW_FOCUS_EVENT, listener)
  }, [focusCard])
  useEffect(() => {
    if (!pendingFocus || !model.order.some((item) => item.id === pendingFocus)) return
    setPendingFocus('')
    void focusCard(pendingFocus)
  }, [focusCard, model.order, pendingFocus])
  const announcement = useBoardAnnouncer({
    items: board.all,
    laneName: laneLabels.name,
    template: useCallback(
      (title: string, lane: string) => t('board:announce.moved', { title, lane }),
      [t],
    ),
    separator: t('board:sidebar.announceSeparator'),
    // 新进入“需要你”由常驻侧栏的 SidebarCommands 播报，这里不重复。
    skipAttention: true,
  })

  // 项目入口：分区 ⋯ 里的“在此项目新建任务”先把派发目标记成该项目，再打开派发对话框。
  const commands = useProjectCommands({
    notify,
    requestConfirm,
    requestText,
    onProjectChosen: onNewTask,
    announceChoice: true,
  })
  const [manageMounted, setManageMounted] = useState(false)
  const [manageOpen, setManageOpen] = useState(false)
  const moveWorkspace = useWorkspaceOrderStore((state) => state.moveWorkspace)
  // 上移 / 下移只在主区画出来的项目分区之间换位：登记了但今天没有代理、或被当前筛选清空的项目
  // 不出现在列表里，和它换位看不出任何变化；第一个 / 最后一个也按看得见的分区算。
  const projectKeys = useMemo(
    () =>
      model.sections.flatMap((section) =>
        section.kind === 'project' && projectByKey.has(section.key) ? [section.key] : [],
      ),
    [model.sections, projectByKey],
  )
  const projectActions = useMemo<ProjectSectionActions>(
    () => ({
      onNewTask: (project: LabeledProject) => {
        useLaunchDefaultsStore.getState().rememberLaunch({ cwd: project.cwd })
        onNewTask()
      },
      onRename: (project: LabeledProject) => void commands.rename(project),
      onResetName: (project: LabeledProject) => commands.resetName(project),
      onMove: (project: LabeledProject, offset: -1 | 1) =>
        moveWorkspace(project.key, offset, projectKeys),
      onManage: () => {
        setManageMounted(true)
        setManageOpen(true)
      },
      onDelete: (project: LabeledProject) => void commands.remove(project),
    }),
    [commands, moveWorkspace, onNewTask, projectKeys],
  )
  const { mounted: pickerMounted, ...pickerProps } = commands.picker

  const bindings = controller.bindings
  const rowBindings = useMemo(
    () => ({
      approve: bindings.boardApprove,
      deny: bindings.boardDeny,
      retry: bindings.boardRetry,
    }),
    [bindings.boardApprove, bindings.boardDeny, bindings.boardRetry],
  )
  const triage = controller.triage
  const grouped = view.groupBy === 'lane'

  const renderEntry = (entry: Entry) => {
    // 第一个分区标题贴着页头，其余与上一分区隔开。
    const spaced = entries[0] !== entry
    if (entry.kind === 'header')
      return entry.section.kind === 'status' ? (
        <StatusHeading
          spaced={spaced}
          lane={entry.section.id}
          count={entry.section.items.length}
          collapsed={entry.section.id === 'done' && doneCollapsed}
          dimmed={Boolean(triage) && entry.section.id !== 'attention'}
          onToggle={
            entry.section.id === 'done'
              ? () => view.setLaneCollapsed('done', !doneCollapsed)
              : undefined
          }
        />
      ) : (
        <ProjectHeading
          spaced={spaced}
          section={entry.section}
          project={projectByKey.get(entry.section.key)}
          first={projectKeys[0] === entry.section.key}
          last={projectKeys.at(-1) === entry.section.key}
          actions={projectActions}
          deleting={Boolean(commands.busyKey)}
        />
      )
    if (entry.kind === 'empty')
      return (
        <p
          className={cn(
            'py-2 pr-3 pl-[40px] text-xs leading-5 text-content-muted',
            triage && entry.lane !== 'attention' && 'opacity-50',
          )}
        >
          {entry.lane === 'attention'
            ? t('board:sidebar.emptyAttention')
            : t('board:sidebar.emptyRunning')}
        </p>
      )
    const item = entry.item
    const card = phone && item.lane === 'attention'
    const row = (
      <OverviewRow
        item={item}
        now={now}
        project={grouped ? (projectByKey.get(item.workspaceKey)?.label ?? '') : ''}
        active={item.id === controller.activeId}
        dimmed={Boolean(triage) && item.lane !== 'attention'}
        phase={entry.phase}
        // 手机没有键盘单字母键：按钮上不写 A / D / R 键帽。
        bindings={phone ? NO_ROW_BINDINGS : rowBindings}
        handlers={controller.handlers}
        menu={menu}
        card={card}
      />
    )
    return card ? <div className="py-1">{row}</div> : row
  }

  let content: ReactNode
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
  else if (model.total === 0)
    content = (
      <div data-overview-empty className="flex flex-col items-center gap-3 py-24 text-center">
        <p className="text-base text-content-secondary">{t('board:overview.empty')}</p>
        <Button variant="outline" onClick={onNewTask}>
          <SquarePen aria-hidden="true" />
          {t('board:overview.newTask')}
        </Button>
        {!board.catalogEmpty && <HistoryLink onClick={onOpenHistory} />}
      </div>
    )
  else if (model.filteredEmpty)
    content = (
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
    )
  else if (virtual)
    content = (
      <div
        data-overview-list
        data-virtual
        className="relative w-full"
        style={{ height: virtualizer.getTotalSize() }}
      >
        {virtualizer.getVirtualItems().map((virtualItem) => {
          const entry = entries[virtualItem.index]
          if (!entry) return null
          return (
            <div
              key={virtualItem.key}
              ref={virtualizer.measureElement}
              data-index={virtualItem.index}
              className="absolute inset-x-0 top-0"
              style={{ transform: `translateY(${virtualItem.start}px)` }}
            >
              {renderEntry(entry)}
            </div>
          )
        })}
      </div>
    )
  else
    content = (
      <div data-overview-list className="flex flex-col">
        {model.sections.map((section) => {
          const key = section.kind === 'status' ? `status:${section.id}` : `project:${section.key}`
          const sectionEntries = entries.filter(
            (entry) =>
              entry.key === key ||
              entry.key === `${key}:empty` ||
              (entry.kind === 'row' && section.items.includes(entry.item)),
          )
          const [heading, ...rest] = sectionEntries
          const rows = rest.filter((entry) => entry.kind === 'row')
          const empty = rest.find((entry) => entry.kind === 'empty')
          return (
            <section
              key={key}
              aria-labelledby={`overview-${key}`}
              data-overview-section={section.kind === 'status' ? section.id : section.key}
            >
              {heading && renderEntry(heading)}
              {empty && renderEntry(empty)}
              {rows.length > 0 && (
                <ul className="m-0 flex list-none flex-col p-0">
                  {rows.map((entry) => (
                    // 行与行之间一条 1px 细线，从标题文字处开始（左侧让出状态图形）。
                    // 手机卡片之间不画细线，卡片自带边框与间距。
                    <li
                      key={entry.key}
                      data-card={(phone && entry.item.lane === 'attention') || undefined}
                      className="relative [&+&]:before:absolute [&+&]:before:top-0 [&+&]:before:right-3 [&+&]:before:left-10 [&+&]:before:h-px [&+&]:before:bg-stroke data-[card]:before:hidden [[data-card]+&]:before:hidden"
                    >
                      {renderEntry(entry)}
                    </li>
                  ))}
                </ul>
              )}
            </section>
          )
        })}
      </div>
    )

  const filtered = view.lanes.length > 0 || view.projects.length > 0
  return (
    <div
      ref={rootRef}
      data-board-root
      data-overview
      className="relative flex h-full min-h-0 w-full min-w-0 flex-col bg-background"
    >
      {/* 主区顶栏 52：没有控件，只作 Windows 自绘标题栏的拖拽区（右侧 144px 留给窗口按钮）；手机由外壳顶栏代替。 */}
      {!phone && <div data-window-drag-region aria-hidden="true" className="h-[52px] flex-none" />}
      <div
        ref={scrollerRef}
        data-overview-scroller
        className="min-h-0 flex-1 overflow-y-auto overscroll-contain pb-20"
      >
        <div
          className={cn(
            'mx-auto flex w-full gap-10 px-6 max-[650px]:px-4',
            widgets ? 'max-w-[1280px]' : 'max-w-[928px]',
          )}
        >
          <div className="min-w-0 flex-1">
            {phone && (
              // 手机：页名在外壳顶栏里，这里的 h1 只给读屏。
              <h1 className="sr-only">{t('board:page.title')}</h1>
            )}
            {phone ? (
              // 手机：标题、搜索与 ✎ 在外壳顶栏，页内只留 44px 高的分组方式分段（规格 7.3）。
              ready &&
              model.total > 0 && (
                <div className="sticky top-0 z-[1] -mx-4 mb-2 bg-background px-4 pt-1 pb-2">
                  <SegmentedControl
                    size="default"
                    aria-label={t('board:group.label')}
                    value={view.groupBy}
                    onValueChange={view.setGroupBy}
                    className="grid h-11 w-full grid-cols-2 [&>button]:h-full"
                    options={[
                      { value: 'lane', label: t('board:group.lane') },
                      { value: 'project', label: t('board:group.project') },
                    ]}
                  />
                </div>
              )
            ) : (
              <header
                data-focus-region="header"
                className="mb-5 flex min-w-0 flex-wrap items-end gap-x-4 gap-y-3"
              >
                <div className="min-w-0 flex-1">
                  <h1 className="text-2xl leading-8 font-semibold tracking-[-.01em] text-content">
                    {t('board:page.title')}
                  </h1>
                  {ready && model.total > 0 && (
                    <FleetSummary
                      agents={model.total}
                      projects={model.projectCount}
                      running={model.running}
                    />
                  )}
                </div>
                {ready && model.total > 0 && (
                  <SegmentedControl
                    size="default"
                    aria-label={t('board:group.label')}
                    aria-keyshortcuts={formatShortcut(bindings.boardToggleGrouping) || undefined}
                    value={view.groupBy}
                    onValueChange={view.setGroupBy}
                    options={[
                      { value: 'lane', label: t('board:group.lane') },
                      { value: 'project', label: t('board:group.project') },
                    ]}
                  />
                )}
                {ready && model.total > 0 && (
                  <Button variant="outline" onClick={onNewTask}>
                    <SquarePen aria-hidden="true" />
                    {t('board:overview.newTask')}
                  </Button>
                )}
              </header>
            )}
            {/* 筛选在侧栏里看得见（规格 2.3）；侧栏收成窄轨时筛选不在界面上，页内补一行说明与“清除筛选”，
                免得窄轨计数点进来后看不出被筛过。 */}
            {ready && filtered && (sidebarCollapsed || phone) && model.total > 0 && (
              <FilterLine
                lanes={view.lanes}
                projects={view.projects
                  .map((key) => projectByKey.get(key)?.label ?? '')
                  .filter(Boolean)}
                laneName={laneLabels.name}
                onClear={view.clearFilters}
              />
            )}
            {content}
            {ready && model.total > 0 && !model.filteredEmpty && (
              <div className={cn('mt-3 pl-1', triage && 'opacity-50')}>
                <HistoryLink onClick={onOpenHistory} />
              </div>
            )}
          </div>
          {/* 组件栏（A7）：只有放了组件时才出现，不留空位、不放占位（规格 3.1）。 */}
          {widgets && (
            <aside
              data-overview-widgets
              aria-label={t('board:overview.widgets')}
              className="w-80 flex-none max-[1279px]:hidden"
            >
              {widgets}
            </aside>
          )}
        </div>
      </div>
      {triage && (
        <TriageBar
          index={triage.index}
          total={triage.total}
          finished={triage.finished}
          answers={board.capabilities.askUser}
          keys={{
            approve: formatShortcut(bindings.boardApprove),
            deny: formatShortcut(bindings.boardDeny),
            retry: formatShortcut(bindings.boardRetry),
          }}
          onExit={controller.exitTriage}
        />
      )}
      <p aria-live="polite" role="status" className="sr-only">
        {announcement}
      </p>
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
            commands={commands}
          />
        </Suspense>
      )}
    </div>
  )
}

function HistoryLink({ onClick }: { onClick: () => void }) {
  const { t } = useI18n()
  return (
    <button
      type="button"
      data-overview-history
      className="inline-flex h-8 items-center gap-1 rounded-[var(--r-sm)] px-2 text-[13px] text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content-secondary focus-visible:outline-2 focus-visible:outline-focus pointer-coarse:h-11"
      onClick={onClick}
    >
      {t('board:lane.history')}
      <ArrowRight className="size-3.5" aria-hidden="true" />
    </button>
  )
}

// 副标题“9 个代理 · 3 个项目 · 3 个在运行”；悬停或聚焦时提示子代理并发、今日 token 与内存（规格 9.1）。
// 系统信息只在提示打开时请求；拿不到的项不显示，一项都没有时如实说明。
function FleetSummary({
  agents,
  projects,
  running,
}: {
  agents: number
  projects: number
  running: number
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const info = useFleetSystemInfo(open)
  const parts = info.data
    ? [
        info.data.subAgents
          ? t('board:overview.subAgentSlots', {
              running: info.data.subAgents.running,
              max: info.data.subAgents.max,
            })
          : '',
        info.data.todayTokens !== null
          ? t('board:overview.todayTokens', { tokens: formatTokenCount(info.data.todayTokens) })
          : '',
        info.data.memoryBytes !== null
          ? t('board:overview.memory', { memory: formatFileSize(info.data.memoryBytes) })
          : '',
      ].filter(Boolean)
    : []
  const tip = info.isPending
    ? t('board:overview.systemLoading')
    : parts.length
      ? parts.join(' · ')
      : t('board:overview.systemUnavailable')
  return (
    <TooltipProvider>
      <Tooltip open={open} onOpenChange={setOpen}>
        <TooltipTrigger asChild>
          <p
            // 可聚焦：键盘用户也能读到悬停提示。
            tabIndex={0}
            data-overview-summary
            className="mt-1 w-fit max-w-full truncate rounded-xs text-sm text-content-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus"
          >
            {t('board:page.summary', { agents, projects, running })}
          </p>
        </TooltipTrigger>
        <TooltipContent side="bottom" align="start" sideOffset={6} data-overview-system>
          {tip}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}

// 筛选还在生效时的一行说明与“清除筛选”：筛选本身在侧栏（规格 2.3）与窄轨计数里设置。
function FilterLine({
  lanes,
  projects,
  laneName,
  onClear,
}: {
  lanes: readonly BoardLane[]
  projects: readonly string[]
  laneName: Record<BoardLane, string>
  onClear: () => void
}) {
  const { t } = useI18n()
  // 列表分隔符随语言（中文“、”，英文“, ”）：手机看板存下的旧多选筛选在这里仍可能是多项。
  const separator = t('board:filter.listSeparator')
  const parts = [
    lanes.length
      ? t('board:filter.statusTitle', {
          statuses: lanes.map((lane) => laneName[lane]).join(separator),
        })
      : '',
    projects.length ? t('board:filter.projectsTitle', { projects: projects.join(separator) }) : '',
  ].filter(Boolean)
  return (
    <div
      data-overview-filter
      className="mb-3 flex min-w-0 items-center gap-2 rounded-[var(--r-md)] bg-surface-subtle py-1 pr-1 pl-3 text-xs text-content-secondary"
    >
      <span className="min-w-0 flex-1 truncate">{parts.join(' · ')}</span>
      <Button variant="ghost" size="xs" onClick={onClear}>
        <X aria-hidden="true" />
        {t('board:filter.clear')}
      </Button>
    </div>
  )
}

function StatusHeading({
  spaced,
  lane,
  count,
  collapsed,
  dimmed,
  onToggle,
}: {
  spaced: boolean
  lane: BoardLane
  count: number
  collapsed: boolean
  // 逐个处理时需要你以外的分组整体退后到 50%（规格 3.3），标题与行一起淡化。
  dimmed: boolean
  onToggle?: () => void
}) {
  const { t } = useI18n()
  const title =
    lane === 'attention'
      ? t('board:sidebar.group.attention')
      : lane === 'running'
        ? t('board:sidebar.group.running')
        : lane === 'queued'
          ? t('board:sidebar.group.queued')
          : t('board:sidebar.group.done')
  const amber = lane === 'attention' && count > 0
  const id = `overview-status:${lane}`
  const inner = (
    <>
      {title}
      <span
        data-overview-count
        className={cn(
          'tabular-nums',
          amber
            ? 'inline-grid h-[18px] min-w-[18px] place-items-center rounded-full bg-warning-soft px-1.5 text-[11.5px] font-semibold ring-1 ring-st-attn-border ring-inset'
            : 'text-content-muted',
        )}
      >
        {count}
      </span>
      {onToggle &&
        (collapsed ? (
          <ChevronRight className="size-3.5" aria-hidden="true" />
        ) : (
          <ChevronDown className="size-3.5" aria-hidden="true" />
        ))}
    </>
  )
  const className = cn(
    'flex h-8 items-center gap-2 px-3 text-[13px] font-medium',
    amber ? 'text-warning-strong' : 'text-content-secondary',
  )
  return (
    <h2
      id={id}
      data-overview-heading={lane}
      className={cn(
        'm-0 transition-opacity duration-(--d-fast)',
        spaced && 'mt-5',
        dimmed && 'opacity-50',
      )}
    >
      {onToggle ? (
        <button
          type="button"
          aria-expanded={!collapsed}
          className={cn(
            className,
            'rounded-[var(--r-md)] hover:text-content focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus pointer-coarse:h-11',
          )}
          onClick={onToggle}
        >
          {inner}
        </button>
      ) : (
        <span className={className}>{inner}</span>
      )}
    </h2>
  )
}

function ProjectHeading({
  spaced,
  section,
  project,
  first,
  last,
  actions,
  deleting,
}: {
  spaced: boolean
  section: OverviewProjectSection
  project: LabeledProject | undefined
  first: boolean
  last: boolean
  actions: ProjectSectionActions
  deleting: boolean
}) {
  const { t } = useI18n()
  const label = project?.label || t('board:filter.noProject')
  const counts = section.counts
  const parts = [
    counts.running ? t('board:overview.projectRunning', { count: counts.running }) : '',
    counts.queued ? t('board:overview.projectQueued', { count: counts.queued }) : '',
    counts.done ? t('board:overview.projectDone', { count: counts.done }) : '',
  ].filter(Boolean)
  return (
    <div
      data-overview-heading={section.key}
      className={cn('flex h-9 min-w-0 items-center gap-2.5 pr-1 pl-3', spaced && 'mt-6')}
    >
      <Folder
        className="size-3.5 flex-none text-content-muted"
        strokeWidth={1.75}
        aria-hidden="true"
      />
      <h2
        id={`overview-project:${section.key}`}
        className="m-0 min-w-0 truncate font-mono text-[13.5px] font-semibold text-content"
        title={section.cwd || undefined}
      >
        {label}
      </h2>
      <p className="min-w-0 flex-1 truncate text-xs text-content-muted">
        {counts.attention > 0 && (
          <b className="font-medium text-warning-strong">
            {t('board:overview.projectAttention', { count: counts.attention })}
          </b>
        )}
        {counts.attention > 0 && parts.length > 0 && <span aria-hidden="true"> · </span>}
        {parts.join(' · ')}
      </p>
      {project && (
        <ProjectSectionMenu
          project={project}
          first={first}
          last={last}
          deleting={deleting}
          actions={actions}
        />
      )}
    </div>
  )
}

// 逐个处理的底部提示条（规格 3.3）：“逐个处理 1/3 · A 允许 · D 拒绝 · R 重试 · 1–4 回答 · ↵ 打开 · Esc 退出”。
// “1–4 回答”只在 askUser 能力（A5）下出现。
function TriageBar({
  index,
  total,
  finished,
  answers,
  keys,
  onExit,
}: {
  index: number
  total: number
  finished: boolean
  answers: boolean
  keys: { approve: string; deny: string; retry: string }
  onExit: () => void
}) {
  const { t } = useI18n()
  const hint = (key: string, label: string) =>
    key ? (
      <span className="flex items-center gap-1">
        <Kbd>{key}</Kbd>
        {label}
      </span>
    ) : null
  return (
    <div
      role="status"
      aria-live="polite"
      data-triage-bar
      className="absolute bottom-5 left-1/2 z-20 flex h-10 max-w-[calc(100%-32px)] -translate-x-1/2 items-center gap-3 overflow-hidden rounded-[var(--r-lg)] border border-stroke bg-solid px-3.5 text-[12.5px] whitespace-nowrap text-content-secondary shadow-float"
    >
      {finished ? (
        <b className="font-medium text-content">{t('board:triage.finished')}</b>
      ) : (
        <>
          <b className="font-medium text-content tabular-nums">
            {t('board:triage.progress', { index, total })}
          </b>
          {hint(keys.approve, t('board:triage.approve'))}
          {hint(keys.deny, t('board:triage.deny'))}
          {hint(keys.retry, t('board:triage.retry'))}
          {answers && hint('1–4', t('board:triage.answer'))}
          {hint('↵', t('board:triage.open'))}
          <Button variant="ghost" size="xs" className="-mr-1.5" onClick={onExit}>
            <Kbd>Esc</Kbd>
            {t('board:triage.exit')}
          </Button>
        </>
      )}
    </div>
  )
}
