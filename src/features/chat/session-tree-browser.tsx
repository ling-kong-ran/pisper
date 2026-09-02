// 会话树左侧浏览区：页签切换、搜索框、「全部标记」列表与桌面/移动端树画布。
// 从 SessionTreeDialog 拆出，仅负责展示与事件上抛，不持有数据请求状态。
import type { RefObject } from 'react'
import { LoaderCircle, Search, Tag } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
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
        className="chat-resource-tabs w-full [padding:12px_12px_0] session-tree-tabs [&_[data-slot='tabs-list']]:grid [&_[data-slot='tabs-list']]:h-[36px] [&_[data-slot='tabs-list']]:w-full [&_[data-slot='tabs-list']]:grid-cols-[repeat(4,minmax(0,1fr))] [&_[data-slot='tabs-list']]:[border:1px_solid_var(--stroke-soft)] [&_[data-slot='tabs-list']]:rounded-[var(--r-sm)] [&_[data-slot='tabs-list']]:bg-[var(--surface-muted)] [&_[data-slot='tabs-trigger']]:min-w-0 [&_[data-slot='tabs-trigger']]:gap-[5px] [&_[data-slot='tabs-trigger']]:rounded-[var(--r-xs)] [&_[data-slot='tabs-trigger']]:[padding-inline:6px] [&_[data-slot='tabs-trigger']]:text-[11px] [&_[data-slot='tabs-trigger'][data-state='active']]:bg-[var(--solid)] [&_[data-slot='tabs-trigger'][data-state='active']]:text-[var(--text)]"
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
      <label className="chat-resource-search [&:focus-within]:border-[var(--focus)] [&:focus-within]:shadow-[0_0_0_2px_var(--focus-ring)] [&_input]:w-full [&_input]:h-[36px] [&_input]:border-0 [&_input]:[outline:0]! [&_input]:bg-transparent [&_input]:text-[var(--text)] [&_input]:text-[13px] flex items-center gap-[7px] [margin-top:8px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] [padding:0_10px] text-[var(--text-muted)] flex-none [margin:8px_12px_0] bg-[var(--solid)]">
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
          className="flex min-w-0 min-h-0 [flex:1_1_0] flex-col gap-[8px] overflow-auto [overscroll-behavior:contain] [padding:12px_14px]"
          data-testid="session-tree-marks-list"
        >
          {error && <p className="danger-text [margin:0_2px]">{error}</p>}
          {marksLoading ? (
            <p className="grid min-h-[180px] place-items-center text-[var(--text-muted)] text-[12px]">
              {t('chat:sessionTree.loadingMarks')}
            </p>
          ) : visibleMarks.length ? (
            visibleMarks.map((mark) => (
              <button
                type="button"
                className={`session-tree-mark hover:border-[var(--stroke-hover)] hover:bg-[var(--solid)] [&.active]:border-[var(--brand-blue-border)] [&_>_em]:rounded-[var(--r-xs)] [&_>_em]:bg-[var(--brand-blue-soft)] [&_>_em]:text-[var(--brand-blue-strong)] [&_>_em]:p-[2px_6px] [&_>_em]:text-[10px] [&_>_em]:[font-style:normal] grid grid-cols-[30px_minmax(0,1fr)_auto] items-center gap-[8px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[color-mix(in_srgb,var(--solid)_94%,transparent)] text-[var(--text)] [padding:8px_10px_8px_4px] text-left cursor-pointer ${mark.active ? ' active' : ''}`}
                key={`${mark.sessionId}:${mark.entryId}`}
                disabled={Boolean(openingMark)}
                aria-busy={openingMark === mark}
                onClick={() => onOpenMark(mark)}
              >
                <span className="session-tree-marker [.session-tree-node[data-kind='user']_&]:bg-[var(--star-soft)] [.session-tree-node[data-kind='user']_&]:text-[var(--star-strong)] [.session-tree-node[data-kind='assistant']_&]:bg-[var(--brand-blue-soft)] [.session-tree-node[data-kind='assistant']_&]:text-[var(--brand-blue-strong)] [.session-tree-node[data-kind='tool']_&]:bg-[var(--warning-soft)] [.session-tree-node[data-kind='tool']_&]:text-[var(--warning-strong)] [.session-tree-node[data-kind='summary']_&]:bg-[var(--violet-soft)] [.session-tree-node[data-kind='summary']_&]:text-[var(--violet-strong)] [.session-tree-node[data-kind='compaction']_&]:bg-[var(--violet-soft)] [.session-tree-node[data-kind='compaction']_&]:text-[var(--violet-strong)] [.session-tree-node.active_&]:border-[var(--brand-blue-border)] [.session-tree-node.active_&]:shadow-[0_0_0_2px_var(--brand-blue-soft)] [.session-tree-node.leaf_&]:bg-[var(--star)] [.session-tree-node.leaf_&]:text-[var(--on-accent)] [.session-tree-mark.active_&]:border-[var(--brand-blue-border)] [.session-tree-mark.active_&]:shadow-[0_0_0_2px_var(--brand-blue-soft)] relative z-[2] grid w-[26px] h-[26px] place-items-center [justify-self:center] [border:2px_solid_var(--surface-subtle)] rounded-[50%] bg-[var(--surface-muted)] text-[var(--text-muted)]">
                  {openingMark === mark ? (
                    <LoaderCircle className="size-3.5 animate-spin" aria-hidden="true" />
                  ) : (
                    <Tag size={14} />
                  )}
                </span>
                <span className="session-tree-node-copy [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_strong]:text-[12px] [&_strong]:font-[650] [&_small]:text-[var(--text-muted)] [&_small]:text-[10px] flex min-w-0 flex-col gap-[2px]">
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
            <p className="grid min-h-[180px] place-items-center text-[var(--text-muted)] text-[12px]">
              {t('chat:sessionTree.noMarksYet')}
            </p>
          )}
        </div>
      ) : (
        <div
          className="min-w-0 min-h-0 [flex:1_1_0] overflow-auto [overscroll-behavior:contain] [scrollbar-gutter:stable]"
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
              <div className="session-tree-canvas flex w-[max-content] min-w-[100%] items-start justify-center gap-[36px] [padding:26px_28px_48px]">
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
            <p className="grid min-h-[180px] place-items-center text-[var(--text-muted)] text-[12px]">
              {loading ? t('chat:sessionTree.loading') : error || t('chat:sessionTree.empty')}
            </p>
          )}
        </div>
      )}
    </div>
  )
}
