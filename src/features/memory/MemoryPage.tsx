// 记忆页（功能 › 会话与资料）。用功能页模板：“‹ 功能”· 标题 · 工具行（搜索、添加记忆、更多；空间选择）·
// 四个视图：演算（默认，懒加载，按时间滑动看记忆如何演变，选中后详情在下方）、时间线（从新到旧，按日期分组，
// 展开看详情）、会话（每次对话的摘要）、整理记录（每次改动和理由，可撤销）。记忆不需要确认，所以这里没有候选箱。
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Ellipsis, Plus, RefreshCw } from 'lucide-react'
import { FEATURE_SEARCH_INPUT_ID } from '@/app/more-pages'
import type { Notify } from '@/app/route-context'
import { STORAGE_KEYS } from '@/app/storage'
import { useI18n } from '@/app/use-i18n'
import { AppSelect } from '@/components/AppSelect'
import {
  FeatureAlert,
  FeatureList,
  FeaturePage,
  FeatureRow,
  FeatureSearch,
  FeatureSection,
  FeatureToolbar,
} from '@/components/app/FeaturePage'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { pageStateStorage } from '@/lib/page-state-storage'
import { useShortcutLabel } from '@/lib/shortcuts'
import { MemoryActivity } from './MemoryActivity'
import { MemoryBadges, MemoryDetailPanel } from './MemoryDetailPanel'
import { MemoryEditDialog, MemorySpaceDialog } from './MemoryDialogs'
import { MemorySessions } from './MemorySessions'
import {
  clearMemoryEpisodes,
  consolidateMemory,
  deleteMemorySpace,
  exportMemoryMarkdown,
  fetchMemoryDashboard,
} from './memory-api'
import {
  formatMemoryDate,
  MEMORY_TYPE_ICONS,
  memoryBadges,
  memorySourceLabel,
  memorySpaceLabel,
  memoryTypeLabel,
} from './memory-labels'
import { groupMemoriesByTime, type MemoryTimelineGroup } from './memory-timeline'
import type { MemoryDashboard, MemoryListStatus, MemoryNode, MemorySpace } from './memory-types'

// 画布和动画不小，只有打开这个视图才加载。
const MemoryEvolution = lazy(() => import('./MemoryEvolution'))

type MemoryView = 'evolution' | 'timeline' | 'sessions' | 'activity'
type TimelineStatus = Exclude<MemoryListStatus, 'all'>
const ALL_SPACES = 'all'

function readStoredView(): MemoryView {
  try {
    const stored = pageStateStorage.getItem(STORAGE_KEYS.memoryView)
    return stored === 'timeline' || stored === 'sessions' || stored === 'activity'
      ? stored
      : 'evolution'
  } catch {
    return 'evolution'
  }
}

function storeView(view: MemoryView) {
  try {
    pageStateStorage.setItem(STORAGE_KEYS.memoryView, view)
  } catch {
    // 存不下只影响下次默认打开哪个视图。
  }
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

const EMPTY_DASHBOARD: MemoryDashboard = { spaces: [], selectedSpaceId: '', nodes: [] }

export function MemoryPage({
  notify,
  onBack,
  requestConfirm,
}: {
  notify: Notify
  // 从“功能”进入时由应用壳提供，渲染标题上方的“‹ 功能”。
  onBack?: () => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}) {
  const { t, language } = useI18n()
  const searchShortcut = useShortcutLabel('focusSearch')
  // 搜索词归页面自己（换页即清空）；外壳的“/”按固定 id 聚焦这里的搜索框。
  const [query, setQuery] = useState('')
  const [view, setView] = useState<MemoryView>(readStoredView)
  const [spaceId, setSpaceId] = useState(ALL_SPACES)
  const [status, setStatus] = useState<TimelineStatus>('current')
  const [data, setData] = useState<MemoryDashboard>(EMPTY_DASHBOARD)
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  // 至少成功读到过一次：之前的失败是“读不出来”（整页错误 + 重试），之后的失败只是一条提示、数据保留。
  const [loaded, setLoaded] = useState(false)
  const [error, setError] = useState('')
  const [expandedId, setExpandedId] = useState('')
  const [selectedId, setSelectedId] = useState<string | null>(null)
  // 记忆有变化（新增、编辑、删除、撤销）时加一：演算视图、详情和整理记录据此重新取数。
  const [refreshKey, setRefreshKey] = useState(0)
  const [editing, setEditing] = useState<MemoryNode | { spaceId: string } | null>(null)
  const [spaceDialog, setSpaceDialog] = useState<MemorySpace | 'new' | null>(null)
  const [consolidating, setConsolidating] = useState(false)
  // 晚到的响应不能盖掉更新的搜索结果。
  const loadRevision = useRef(0)

  const load = useCallback(async () => {
    const revision = ++loadRevision.current
    setLoading(true)
    setError('')
    try {
      const result = await fetchMemoryDashboard({ spaceId, query, status })
      if (revision !== loadRevision.current) return
      setData(result)
      setLoaded(true)
      // 选中的空间被删了（或连到不认识“全部”的旧 Runtime）时，跟随 Runtime 实际返回的空间。
      if (result.selectedSpaceId && result.selectedSpaceId !== spaceId)
        setSpaceId(result.selectedSpaceId)
    } catch (loadError) {
      if (revision === loadRevision.current) setError(errorText(loadError))
    } finally {
      if (revision === loadRevision.current) setLoading(false)
    }
  }, [query, spaceId, status])

  useEffect(() => {
    void load()
    return () => {
      loadRevision.current += 1
    }
  }, [load, refreshKey])

  const loadMore = async () => {
    const last = data.nodes.at(-1)
    const before = last?.validFrom || last?.createdAt
    if (!before || loadingMore) return
    const revision = loadRevision.current
    setLoadingMore(true)
    try {
      const result = await fetchMemoryDashboard({ spaceId, status, before })
      if (revision !== loadRevision.current) return
      setData((previous) => {
        const known = new Set(previous.nodes.map((node) => node.id))
        return {
          ...result,
          nodes: [...previous.nodes, ...result.nodes.filter((node) => !known.has(node.id))],
        }
      })
    } catch (loadError) {
      if (revision === loadRevision.current) setError(errorText(loadError))
    } finally {
      setLoadingMore(false)
    }
  }

  const refresh = useCallback(() => setRefreshKey((value) => value + 1), [])
  const onChanged = useCallback(
    (message: string) => {
      notify(message)
      refresh()
    },
    [notify, refresh],
  )
  const onDeleted = useCallback(
    (message: string) => {
      setExpandedId('')
      setSelectedId(null)
      onChanged(message)
    },
    [onChanged],
  )

  const spaces = useMemo(
    () =>
      [...data.spaces].sort(
        (left, right) => Number(right.kind === 'global') - Number(left.kind === 'global'),
      ),
    [data.spaces],
  )
  const activeSpace = data.spaces.find((space) => space.id === spaceId) || null
  const searching = Boolean(query.trim())
  const groups = useMemo(
    () => groupMemoriesByTime(data.nodes, (node) => node.validFrom || node.createdAt),
    [data.nodes],
  )

  const changeView = (next: string) => {
    const value = next as MemoryView
    setView(value)
    storeView(value)
  }
  // 搜索结果是一份列表，只有时间线放得下；从别的视图开始输入时切过去，不改下次默认打开的视图。
  const changeQuery = (value: string) => {
    setQuery(value)
    setExpandedId('')
    if (value.trim() && view !== 'timeline') setView('timeline')
  }
  const changeSpace = (next: string) => {
    setSpaceId(next)
    setExpandedId('')
    setSelectedId(null)
  }

  const removeSpace = async () => {
    if (!activeSpace) return
    const approved = await requestConfirm({
      title: t('memory:memoryPage.deleteMemorySpace'),
      message: t('memory:memoryPage.deleteMemorySpaceNameAndAllMemoriesInIt', {
        name: memorySpaceLabel(activeSpace, t),
      }),
      confirmLabel: t('memory:memoryPage.delete'),
    })
    if (!approved) return
    try {
      await deleteMemorySpace(activeSpace.id)
      changeSpace(ALL_SPACES)
      onChanged(t('memory:memoryPage.memorySpaceDeleted'))
    } catch (deleteError) {
      setError(errorText(deleteError))
    }
  }

  const exportMarkdown = async () => {
    try {
      const { markdown } = await exportMemoryMarkdown(spaceId === ALL_SPACES ? '' : spaceId)
      const url = URL.createObjectURL(new Blob([markdown], { type: 'text/markdown;charset=utf-8' }))
      const anchor = document.createElement('a')
      anchor.href = url
      anchor.download = `pisper-memory-${new Date().toISOString().slice(0, 10)}.md`
      anchor.click()
      window.setTimeout(() => URL.revokeObjectURL(url), 1000)
      notify(t('memory:memoryPage.exported'))
    } catch (exportError) {
      setError(errorText(exportError))
    }
  }

  const consolidate = async () => {
    if (consolidating) return
    setConsolidating(true)
    setError('')
    try {
      const result = await consolidateMemory()
      onChanged(
        result.ran
          ? t('memory:memoryPage.consolidated')
          : t('memory:memoryPage.nothingToConsolidate'),
      )
    } catch (consolidateError) {
      setError(errorText(consolidateError))
    } finally {
      setConsolidating(false)
    }
  }

  // 对话原文只用来追溯和批量抽取，用户随时可以清掉；已经记下的事实不受影响。
  const clearEpisodes = async () => {
    const approved = await requestConfirm({
      title: t('memory:memoryPage.clearEpisodes'),
      message: t('memory:memoryPage.clearEpisodesConfirm'),
      confirmLabel: t('memory:memoryPage.clearEpisodesAction'),
    })
    if (!approved) return
    try {
      const { cleared } = await clearMemoryEpisodes(spaceId === ALL_SPACES ? '' : spaceId)
      onChanged(t('memory:memoryPage.episodesCleared', { count: cleared }))
    } catch (clearError) {
      setError(errorText(clearError))
    }
  }

  const spaceEditable = Boolean(activeSpace) && activeSpace?.kind !== 'global'
  const renderDetail = (node: MemoryNode) => (
    <MemoryDetailPanel
      id={node.id}
      fallback={node}
      spaces={data.spaces}
      refreshKey={refreshKey}
      onEdit={setEditing}
      onChanged={onChanged}
      onDeleted={onDeleted}
      requestConfirm={requestConfirm}
    />
  )
  const renderRow = (node: MemoryNode) => {
    const Icon = MEMORY_TYPE_ICONS[node.type] || MEMORY_TYPE_ICONS.concept
    const badges = memoryBadges(node, t)
    const space = data.spaces.find((item) => item.id === node.spaceId)
    return (
      <FeatureRow
        key={node.id}
        data-memory-node={node.id}
        icon={<Icon />}
        name={node.title}
        nameMeta={memoryTypeLabel(node.type, t)}
        // 展开后详情里有完整内容，行上不再重复。
        description={expandedId === node.id ? undefined : node.content}
        meta={
          <span className="truncate tabular-nums">
            {formatMemoryDate(node.validFrom || node.createdAt, language)}
            {spaceId === ALL_SPACES && space ? ` · ${memorySpaceLabel(space, t)}` : ''}
            {` · ${memorySourceLabel(node.sourceType, t)}`}
          </span>
        }
        status={badges.length ? <MemoryBadges labels={badges} /> : undefined}
        expanded={expandedId === node.id}
        onExpandedChange={(open) => setExpandedId(open ? node.id : '')}
        toggleLabel={node.title}
        detail={renderDetail(node)}
      />
    )
  }
  const groupTitle = (group: MemoryTimelineGroup<MemoryNode>) => {
    if (group.kind === 'today') return t('memory:memoryPage.groupToday')
    if (group.kind === 'yesterday') return t('memory:memoryPage.groupYesterday')
    if (group.kind === 'week') return t('memory:memoryPage.groupThisWeek')
    if (group.kind === 'unknown' || !group.month) return t('memory:memoryPage.groupUnknown')
    return new Intl.DateTimeFormat(language, { year: 'numeric', month: 'long' }).format(group.month)
  }
  const emptyTitle = searching
    ? t('memory:memoryPage.noMatchingMemories')
    : status === 'archived'
      ? t('memory:memoryPage.noArchivedMemories')
      : status === 'history'
        ? t('memory:memoryPage.noHistoryVersions')
        : t('memory:memoryPage.noMemoriesInSpace')

  return (
    // 切换键在工具行里，所以 Tabs 要包住整个模板。asChild 把它合并到 FeaturePage 上：
    // 页面模板约定 FeaturePage 是 .page-content 的直接子元素，不能多一层 DOM。
    <Tabs asChild value={view} onValueChange={changeView} className="gap-0">
      <FeaturePage
        title={t('navigation:navigation.memory')}
        description={t('memory:memoryPage.lede')}
        back={
          onBack && {
            label: t('navigation:sidebar.features'),
            ariaLabel: t('navigation:shell.backToFeaturesLabel'),
            onBack,
          }
        }
        toolbar={
          <FeatureToolbar
            search={
              <FeatureSearch
                id={FEATURE_SEARCH_INPUT_ID}
                value={query}
                onValueChange={changeQuery}
                placeholder={t('navigation:pageHeader.searchMemoriesOrFiles')}
                shortcut={searchShortcut}
                clearLabel={t('navigation:more.clearSearch')}
              />
            }
            actions={
              <>
                <Button
                  type="button"
                  className="h-9"
                  disabled={!loaded}
                  onClick={() =>
                    setEditing({ spaceId: spaceId === ALL_SPACES ? 'global' : spaceId })
                  }
                >
                  <Plus />
                  {t('memory:memoryPage.addMemory')}
                </Button>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button
                      type="button"
                      variant="secondary"
                      size="icon-lg"
                      className="max-[650px]:max-w-11"
                      disabled={!loaded}
                      aria-label={t('memory:memoryPage.moreActions')}
                      title={t('memory:memoryPage.moreActions')}
                    >
                      <Ellipsis />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem onSelect={() => setSpaceDialog('new')}>
                      {t('memory:memoryPage.newMemorySpace')}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      disabled={!spaceEditable}
                      onSelect={() => activeSpace && setSpaceDialog(activeSpace)}
                    >
                      {t('memory:memoryPage.renameMemorySpace')}
                    </DropdownMenuItem>
                    <DropdownMenuItem
                      variant="destructive"
                      disabled={!spaceEditable}
                      onSelect={() => void removeSpace()}
                    >
                      {t('memory:memoryPage.deleteMemorySpace')}
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem onSelect={() => void exportMarkdown()}>
                      {t('memory:memoryPage.exportMarkdown')}
                    </DropdownMenuItem>
                    <DropdownMenuItem disabled={consolidating} onSelect={() => void consolidate()}>
                      {t('memory:memoryPage.consolidateNow')}
                    </DropdownMenuItem>
                    <DropdownMenuItem onSelect={() => void clearEpisodes()}>
                      {t('memory:memoryPage.clearEpisodes')}
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </>
            }
            filters={
              <div className="flex min-w-0 flex-col gap-2.5">
                <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
                  <TabsList aria-label={t('memory:memoryPage.viewsLabel')}>
                    <TabsTrigger value="evolution">
                      {t('memory:memoryPage.evolutionView')}
                    </TabsTrigger>
                    <TabsTrigger value="timeline">
                      {t('memory:memoryPage.timelineView')}
                    </TabsTrigger>
                    <TabsTrigger value="sessions">
                      {t('memory:memoryPage.sessionsView')}
                    </TabsTrigger>
                    <TabsTrigger value="activity">
                      {t('memory:memoryPage.activityView')}
                    </TabsTrigger>
                  </TabsList>
                  <AppSelect
                    className="w-[200px] max-w-full max-[650px]:w-full"
                    aria-label={t('memory:memoryPage.memorySpace')}
                    value={spaceId}
                    disabled={!loaded}
                    onChange={(event) => changeSpace(event.target.value)}
                  >
                    <option value={ALL_SPACES}>{t('memory:memoryPage.allSpaces')}</option>
                    {spaces.map((space) => (
                      <option value={space.id} key={space.id}>
                        {`${memorySpaceLabel(space, t)} · ${space.nodeCount}`}
                      </option>
                    ))}
                  </AppSelect>
                </div>
                {view === 'timeline' && loaded && (
                  <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
                    {searching ? (
                      <p className="m-0 text-[13px] font-medium text-content-muted tabular-nums">
                        {t('memory:memoryPage.searchResults')} · {data.nodes.length}
                      </p>
                    ) : (
                      <SegmentedControl<TimelineStatus>
                        aria-label={t('memory:memoryPage.statusFilterLabel')}
                        value={status}
                        onValueChange={(next) => {
                          setStatus(next)
                          setExpandedId('')
                        }}
                        options={[
                          {
                            value: 'current',
                            label: t('memory:memoryPage.statusCurrent'),
                            count: data.counts?.current,
                          },
                          {
                            value: 'archived',
                            label: t('memory:memoryPage.statusArchived'),
                            count: data.counts?.archived,
                          },
                          {
                            value: 'history',
                            label: t('memory:memoryPage.statusHistory'),
                            count: data.counts?.history,
                          },
                        ]}
                      />
                    )}
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="text-content-muted max-[650px]:size-11"
                      loading={loading}
                      disabled={loading}
                      aria-label={t('memory:memoryPage.refresh')}
                      title={t('memory:memoryPage.refresh')}
                      onClick={refresh}
                    >
                      <RefreshCw />
                    </Button>
                  </div>
                )}
              </div>
            }
          />
        }
      >
        {!loaded && loading ? (
          <PageState kind="loading" title={t('common:pageTemplate.loading')} />
        ) : !loaded ? (
          // 一次都没读到：写明错误并给重试，不显示成没有记忆。
          <PageState
            kind="error"
            title={t('memory:memoryPage.loadFailed')}
            description={error}
            action={
              <Button variant="secondary" className="max-[650px]:h-11" onClick={() => void load()}>
                {t('common:pageTemplate.retry')}
              </Button>
            }
          />
        ) : (
          <>
            {/* 读到之后的失败（刷新、整理、删除空间）：写在最上面，数据保留。 */}
            {error && <FeatureAlert>{error}</FeatureAlert>}
            <div className="mt-5 min-w-0" aria-busy={loading}>
              <TabsContent value="evolution" className="flex min-w-0 flex-col gap-3">
                <Suspense
                  fallback={<PageState kind="loading" title={t('common:pageTemplate.loading')} />}
                >
                  <MemoryEvolution
                    spaceId={spaceId}
                    refreshKey={refreshKey}
                    selectedId={selectedId}
                    onSelect={setSelectedId}
                  />
                </Suspense>
                <div
                  role="region"
                  data-memory-detail
                  aria-label={t('memory:memoryPage.selectedMemory')}
                  className="min-w-0 rounded-[var(--r-md)] border border-stroke px-4 py-3"
                >
                  {selectedId ? (
                    <MemoryDetailPanel
                      id={selectedId}
                      spaces={data.spaces}
                      refreshKey={refreshKey}
                      withHeading
                      onClose={() => setSelectedId(null)}
                      onEdit={setEditing}
                      onChanged={onChanged}
                      onDeleted={onDeleted}
                      requestConfirm={requestConfirm}
                    />
                  ) : (
                    <p className="m-0 text-xs leading-normal text-content-muted">
                      {t('memory:memoryPage.selectAMemoryToViewDetails')}
                    </p>
                  )}
                </div>
              </TabsContent>
              <TabsContent value="timeline" className="min-w-0" data-memory-timeline>
                {!data.nodes.length ? (
                  <InlineEmptyState compact title={emptyTitle} />
                ) : searching ? (
                  <FeatureList>{data.nodes.map(renderRow)}</FeatureList>
                ) : (
                  groups.map((group) => (
                    <FeatureSection
                      key={group.key}
                      data-memory-group={group.key}
                      className="mt-5 first:mt-0"
                      title={groupTitle(group)}
                      count={group.items.length}
                    >
                      <FeatureList>{group.items.map(renderRow)}</FeatureList>
                    </FeatureSection>
                  ))
                )}
                {!searching && data.hasMore && (
                  <div className="mt-4 flex justify-center">
                    <Button
                      variant="secondary"
                      size="sm"
                      className="max-[650px]:h-11"
                      loading={loadingMore}
                      disabled={loadingMore}
                      onClick={() => void loadMore()}
                    >
                      {t('memory:memoryPage.loadMore')}
                    </Button>
                  </div>
                )}
              </TabsContent>
              <TabsContent value="sessions" className="min-w-0">
                <MemorySessions spaceId={spaceId} spaces={data.spaces} refreshKey={refreshKey} />
              </TabsContent>
              <TabsContent value="activity" className="min-w-0">
                <MemoryActivity spaceId={spaceId} refreshKey={refreshKey} onChanged={onChanged} />
              </TabsContent>
            </div>
          </>
        )}
        {editing && (
          <MemoryEditDialog
            spaces={spaces}
            node={'id' in editing ? editing : null}
            initialSpaceId={editing.spaceId}
            onClose={() => setEditing(null)}
            onSaved={(message) => {
              setEditing(null)
              onChanged(message)
            }}
          />
        )}
        {spaceDialog && (
          <MemorySpaceDialog
            space={spaceDialog === 'new' ? null : spaceDialog}
            onClose={() => setSpaceDialog(null)}
            onSaved={(space, message) => {
              setSpaceDialog(null)
              changeSpace(space.id)
              onChanged(message)
            }}
          />
        )}
      </FeaturePage>
    </Tabs>
  )
}
