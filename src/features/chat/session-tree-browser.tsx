// 会话树左侧浏览区：页签切换、搜索框、「全部标记」列表与桌面/移动端树画布。
// 从 SessionTreeDialog 拆出，仅负责展示与事件上抛，不持有数据请求状态。
import type { RefObject } from 'react'
import { Search, Tag } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { OrbitSpinner } from '@/components/ui/spinner'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import type { SessionTreeNode, SessionTreeLabelMatch } from '@/features/chat/chat-api'
import type { DisplayNode, TreeSegment, TreeView } from '@/features/chat/session-tree-model'
import { MobileSessionTreeList, SessionTreeSegment } from '@/features/chat/session-tree-nodes'

type NodeLabelFn = (node: SessionTreeNode) => string

export function SessionTreeBrowser({
  view,
  onViewChange,
  query,
  onQueryChange,
  loading,
  error,
  mobileLayout,
  viewportRef,
  segments,
  visibleNodes,
  selectedId,
  typeLabel,
  stateLabel,
  onSelect,
  visibleMarks,
  marksLoading,
  openingMark,
  onOpenMark,
}: {
  view: TreeView
  onViewChange: (view: TreeView) => void
  query: string
  onQueryChange: (query: string) => void
  loading: boolean
  error: string
  mobileLayout: boolean
  viewportRef: RefObject<HTMLDivElement | null>
  segments: TreeSegment[]
  visibleNodes: DisplayNode[]
  selectedId: string
  typeLabel: NodeLabelFn
  stateLabel: NodeLabelFn
  onSelect: (id: string) => void
  visibleMarks: SessionTreeLabelMatch[]
  marksLoading: boolean
  openingMark: SessionTreeLabelMatch | null
  onOpenMark: (mark: SessionTreeLabelMatch) => void
}) {
  const { t, language } = useI18n()

  // 标记条目的会话/节点时间，格式无效时回退到「未知时间」。
  const markTime = (value: string) => {
    const timestamp = Date.parse(value)
    if (!Number.isFinite(timestamp)) return t('navigation:appOverlays.unknownTime')
    return new Intl.DateTimeFormat(language, {
      dateStyle: 'medium',
      timeStyle: 'short',
    }).format(timestamp)
  }

  return (
    <div className="chat-resource-browser flex min-h-0 min-w-0 flex-col bg-transparent session-tree-browser">
      <Tabs
        className="chat-resource-tabs session-tree-tabs w-full px-3 pt-3 [&_[data-slot='tabs-list']]:grid [&_[data-slot='tabs-list']]:w-full [&_[data-slot='tabs-list']]:grid-cols-4 [&_[data-slot='tabs-trigger']]:min-w-0"
        value={view}
        onValueChange={(value) => onViewChange(value as TreeView)}
      >
        <TabsList>
          <TabsTrigger value="conversation">{t('chat:sessionTree.conversation')}</TabsTrigger>
          <TabsTrigger value="labeled">{t('chat:sessionTree.labeled')}</TabsTrigger>
          <TabsTrigger value="all">{t('chat:sessionTree.all')}</TabsTrigger>
          <TabsTrigger value="marks">{t('chat:sessionTree.allMarks')}</TabsTrigger>
        </TabsList>
      </Tabs>
      <label className="chat-resource-search mx-3 mt-2 flex h-8.5 flex-none items-center gap-2 rounded-sm border border-control bg-panel px-2.5 text-content-muted focus-within:border-focus focus-within:ring-3 focus-within:ring-focus-ring [&_input]:h-full [&_input]:w-full [&_input]:border-0 [&_input]:[outline:0]! [&_input]:bg-transparent [&_input]:text-sm [&_input]:text-content">
        <Search size={15} />
        <input
          value={query}
          disabled={Boolean(openingMark)}
          onChange={(event) => onQueryChange(event.target.value)}
          placeholder={t('chat:sessionTree.searchPlaceholder')}
        />
      </label>
      {view === 'marks' ? (
        <div
          className="flex min-h-0 min-w-0 flex-1 basis-0 flex-col gap-2 overflow-auto overscroll-contain px-3.5 py-3"
          data-testid="session-tree-marks-list"
        >
          {error && (
            <p role="alert" className="mx-0.5 text-xs text-danger">
              {error}
            </p>
          )}
          {marksLoading ? (
            <p className="grid min-h-[180px] place-items-center text-xs text-content-muted">
              {t('chat:sessionTree.loadingMarks')}
            </p>
          ) : visibleMarks.length ? (
            visibleMarks.map((mark) => (
              <button
                type="button"
                className={`session-tree-mark grid cursor-pointer grid-cols-[30px_minmax(0,1fr)_auto] items-center gap-2 rounded-sm border border-stroke bg-panel py-2 pr-2.5 pl-1 text-left text-content transition-colors duration-(--d-fast) hover:bg-surface-subtle [&.active]:border-star-border [&_>_em]:rounded-xs [&_>_em]:bg-star-soft [&_>_em]:px-1.5 [&_>_em]:py-0.5 [&_>_em]:text-xs [&_>_em]:text-star-strong [&_>_em]:not-italic ${mark.active ? ' active' : ''}`}
                key={`${mark.sessionId}:${mark.entryId}`}
                disabled={Boolean(openingMark)}
                aria-busy={openingMark === mark}
                onClick={() => onOpenMark(mark)}
              >
                <span className="session-tree-marker relative z-[2] grid size-6 place-items-center justify-self-center rounded-sm bg-surface-muted text-content-secondary [.session-tree-node[data-kind='tool']_&]:bg-warning-soft [.session-tree-node[data-kind='tool']_&]:text-warning-strong [.session-tree-node.leaf_&]:bg-star [.session-tree-node.leaf_&]:text-on-accent [.session-tree-mark.active_&]:bg-star-soft [.session-tree-mark.active_&]:text-star-strong">
                  {openingMark === mark ? <OrbitSpinner className="size-3.5" /> : <Tag size={14} />}
                </span>
                <span className="session-tree-node-copy flex min-w-0 flex-col gap-0.5 [&_small]:truncate [&_small]:text-xs [&_small]:text-content-muted [&_strong]:truncate [&_strong]:text-sm [&_strong]:font-medium">
                  <strong>{mark.label}</strong>
                  <small>{mark.sessionName || t('navigation:appOverlays.untitledChat')}</small>
                  <small>
                    {t('chat:sessionTree.markTimes', {
                      sessionTime: markTime(mark.sessionModified),
                      nodeTime: markTime(mark.nodeTimestamp),
                    })}
                  </small>
                </span>
                {openingMark === mark ? (
                  <em>{t('chat:sessionTree.navigating')}</em>
                ) : (
                  mark.active && <em>{t('chat:sessionTree.markActive')}</em>
                )}
              </button>
            ))
          ) : (
            <p className="grid min-h-[180px] place-items-center text-xs text-content-muted">
              {t('chat:sessionTree.noMarksYet')}
            </p>
          )}
        </div>
      ) : (
        <div
          className="min-h-0 min-w-0 flex-1 basis-0 overflow-auto overscroll-contain [scrollbar-gutter:stable]"
          data-testid="session-tree-list"
          ref={viewportRef}
        >
          {segments.length > 0 ? (
            mobileLayout ? (
              <MobileSessionTreeList
                nodes={visibleNodes}
                viewportRef={viewportRef}
                selectedId={selectedId}
                typeLabel={typeLabel}
                stateLabel={stateLabel}
                onSelect={onSelect}
              />
            ) : (
              <div className="session-tree-canvas flex w-max min-w-full items-start justify-center gap-9 px-7 pt-6.5 pb-12">
                {segments.map((segment) => (
                  <SessionTreeSegment
                    segment={segment}
                    viewportRef={viewportRef}
                    selectedId={selectedId}
                    typeLabel={typeLabel}
                    stateLabel={stateLabel}
                    onSelect={onSelect}
                    key={segment.id}
                  />
                ))}
              </div>
            )
          ) : (
            <p className="grid min-h-[180px] place-items-center text-xs text-content-muted">
              {loading ? t('chat:sessionTree.loading') : error || t('chat:sessionTree.empty')}
            </p>
          )}
        </div>
      )}
    </div>
  )
}
