// 会话树节点行渲染：移动端列表与桌面端分段轨道（含虚拟化）。
// 从 SessionTreeDialog 拆出。节点是细线卡片，选中项抬成键帽并带 3px 强调色刻度；
// 标记为单色方块，只有当前位置（leaf）使用强调色实心。
import { useEffect, type RefObject } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { Bookmark } from 'lucide-react'
import type { SessionTreeNode } from '@/features/chat/chat-api'
import {
  nodeIcon,
  TREE_OVERSCAN,
  TREE_ROW_HEIGHT,
  TREE_TRACK_SCROLL_MARGIN,
  TREE_VIRTUALIZE_THRESHOLD,
  type DisplayNode,
  type TreeSegment,
} from '@/features/chat/session-tree-model'

type NodeLabelFn = (node: SessionTreeNode) => string

export function MobileSessionTreeList({
  nodes,
  viewportRef,
  selectedId,
  typeLabel,
  stateLabel,
  onSelect,
}: {
  nodes: DisplayNode[]
  viewportRef: RefObject<HTMLDivElement | null>
  selectedId: string
  typeLabel: NodeLabelFn
  stateLabel: NodeLabelFn
  onSelect: (id: string) => void
}) {
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: nodes.length,
    getScrollElement: () => viewportRef.current,
    estimateSize: () => 64,
    getItemKey: (index) => nodes[index]?.id ?? `mobile-tree-node-${index}`,
    overscan: TREE_OVERSCAN,
    useAnimationFrameWithResizeObserver: true,
  })

  useEffect(() => {
    if (!selectedId) return
    const index = nodes.findIndex((node) => node.id === selectedId)
    if (index >= 0) virtualizer.scrollToIndex(index, { align: 'center' })
  }, [nodes, selectedId, virtualizer])

  return (
    <div className="relative w-full" style={{ height: `${virtualizer.getTotalSize()}px` }}>
      {virtualizer.getVirtualItems().map((virtualItem) => {
        const node = nodes[virtualItem.index]
        const Icon = nodeIcon(node)
        return (
          <div
            className="absolute top-0 left-0 w-full px-3 py-1"
            key={virtualItem.key}
            style={{ transform: `translateY(${virtualItem.start}px)` }}
          >
            <button
              type="button"
              className={`session-tree-node relative grid items-center gap-2 rounded-sm border border-stroke bg-panel text-left text-content transition-colors duration-(--d-fast) hover:bg-surface-subtle [&.selected]:border-key-border [&.selected]:bg-key [&.selected]:shadow-key [&.selected]:before:absolute [&.selected]:before:inset-y-2 [&.selected]:before:-left-px [&.selected]:before:w-[3px] [&.selected]:before:rounded-r-[3px] [&.selected]:before:bg-star min-h-14 w-full grid-cols-[34px_minmax(0,1fr)_auto] py-2 pr-2.5 pl-1 ${node.id === selectedId ? ' selected' : ''}${node.active ? ' active' : ''}${node.leaf ? ' leaf' : ''}`}
              data-kind={node.kind}
              data-pisper-tree-entry={node.id}
              onClick={() => onSelect(node.id)}
            >
              <span className="session-tree-marker relative z-[2] grid size-6 place-items-center justify-self-center rounded-sm bg-surface-muted text-content-secondary [.session-tree-node[data-kind='tool']_&]:bg-warning-soft [.session-tree-node[data-kind='tool']_&]:text-warning-strong [.session-tree-node.leaf_&]:bg-star [.session-tree-node.leaf_&]:text-on-accent [.session-tree-mark.active_&]:bg-star-soft [.session-tree-mark.active_&]:text-star-strong">
                <Icon size={14} />
              </span>
              <span className="session-tree-node-copy flex min-w-0 flex-col gap-0.5 [&_small]:truncate [&_small]:text-xs [&_small]:text-content-muted [&_strong]:truncate [&_strong]:text-sm [&_strong]:font-medium">
                <strong>{node.label || node.text || typeLabel(node)}</strong>
                <small>{node.label && node.text ? node.text : typeLabel(node)}</small>
              </span>
              <span className="session-tree-node-state inline-flex min-w-0 shrink-0 items-center gap-1 text-xs text-content-muted [.session-tree-node.leaf_&]:text-accent-fg">
                {node.label && <Bookmark size={12} />}
                {stateLabel(node)}
              </span>
            </button>
          </div>
        )
      })}
    </div>
  )
}

export function SessionTreeSegment({
  segment,
  viewportRef,
  selectedId,
  typeLabel,
  stateLabel,
  onSelect,
}: {
  segment: TreeSegment
  viewportRef: RefObject<HTMLDivElement | null>
  selectedId: string
  typeLabel: NodeLabelFn
  stateLabel: NodeLabelFn
  onSelect: (id: string) => void
}) {
  const { nodes, children } = segment
  const shouldVirtualize = nodes.length > TREE_VIRTUALIZE_THRESHOLD

  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: nodes.length,
    getScrollElement: () => viewportRef.current,
    estimateSize: () => TREE_ROW_HEIGHT,
    getItemKey: (index) => nodes[index]?.id ?? `tree-node-${index}`,
    overscan: TREE_OVERSCAN,
    scrollMargin: TREE_TRACK_SCROLL_MARGIN,
    enabled: shouldVirtualize,
    useAnimationFrameWithResizeObserver: true,
  })

  const renderNode = (node: DisplayNode) => {
    const Icon = nodeIcon(node)
    return (
      <button
        type="button"
        className={`session-tree-node relative grid items-center gap-2 rounded-sm border border-stroke bg-panel text-left text-content transition-colors duration-(--d-fast) hover:bg-surface-subtle [&.selected]:border-key-border [&.selected]:bg-key [&.selected]:shadow-key [&.selected]:before:absolute [&.selected]:before:inset-y-2 [&.selected]:before:-left-px [&.selected]:before:w-[3px] [&.selected]:before:rounded-r-[3px] [&.selected]:before:bg-star z-[1] min-h-12 w-[220px] cursor-pointer grid-cols-[30px_minmax(0,1fr)_auto] py-1.5 pr-2 pl-0.5 ${node.id === selectedId ? ' selected' : ''}${node.active ? ' active' : ''}${node.leaf ? ' leaf' : ''}`}
        data-kind={node.kind}
        data-pisper-tree-entry={node.id}
        key={node.id}
        onClick={() => onSelect(node.id)}
      >
        <span className="session-tree-marker relative z-[2] grid size-6 place-items-center justify-self-center rounded-sm bg-surface-muted text-content-secondary [.session-tree-node[data-kind='tool']_&]:bg-warning-soft [.session-tree-node[data-kind='tool']_&]:text-warning-strong [.session-tree-node.leaf_&]:bg-star [.session-tree-node.leaf_&]:text-on-accent [.session-tree-mark.active_&]:bg-star-soft [.session-tree-mark.active_&]:text-star-strong">
          <Icon size={14} />
        </span>
        <span className="session-tree-node-copy flex min-w-0 flex-col gap-0.5 [&_small]:truncate [&_small]:text-xs [&_small]:text-content-muted [&_strong]:truncate [&_strong]:text-sm [&_strong]:font-medium">
          <strong>{node.label || node.text || typeLabel(node)}</strong>
          <small>{node.label && node.text ? node.text : typeLabel(node)}</small>
        </span>
        <span className="session-tree-node-state inline-flex min-w-0 shrink-0 items-center gap-1 text-xs text-content-muted [.session-tree-node.leaf_&]:text-accent-fg">
          {node.label && <Bookmark size={12} />}
          {stateLabel(node)}
        </span>
      </button>
    )
  }

  useEffect(() => {
    if (!shouldVirtualize || !selectedId) return
    const index = nodes.findIndex((node) => node.id === selectedId)
    if (index < 0) return
    virtualizer.scrollToIndex(index, { align: 'center' })
  }, [selectedId, nodes, shouldVirtualize, virtualizer])

  return (
    <div
      className={`session-tree-segment relative flex min-w-[220px] flex-col items-center ${segment.active ? ' active' : ''}`}
    >
      <div className="session-tree-track relative z-[1] grid w-[220px] gap-2 before:absolute before:top-[22px] before:bottom-[22px] before:left-1/2 before:z-0 before:w-px before:bg-stroke-strong [.session-tree-segment.active_>_&::before]:bg-control">
        {shouldVirtualize ? (
          <div className="relative w-[220px]" style={{ height: `${virtualizer.getTotalSize()}px` }}>
            {virtualizer.getVirtualItems().map((virtualItem) => (
              <div
                className="absolute top-0 left-0 w-[220px]"
                key={virtualItem.key}
                style={{ top: `${virtualItem.start - TREE_TRACK_SCROLL_MARGIN}px` }}
              >
                {renderNode(nodes[virtualItem.index])}
              </div>
            ))}
          </div>
        ) : (
          nodes.map(renderNode)
        )}
      </div>
      {children.length > 0 && (
        <div className="session-tree-children relative flex items-start justify-center pt-7 before:absolute before:top-0 before:left-1/2 before:h-3.5 before:w-px before:bg-stroke-strong [.session-tree-segment.active_>_&::before]:bg-control">
          {children.map((child) => (
            <div
              className={`session-tree-child relative flex px-3 pt-3.5 before:absolute before:top-0 before:right-1/2 before:h-3.5 before:w-1/2 before:border-t before:border-stroke-strong after:absolute after:top-0 after:left-1/2 after:h-3.5 after:w-1/2 after:border-t after:border-l after:border-stroke-strong [&:first-child::before]:border-t-transparent [&:last-child::after]:border-t-transparent [&.active]:before:border-control [&.active]:after:border-control [&.active:first-child::before]:border-t-transparent [&.active:last-child::after]:border-t-transparent${child.active ? ' active' : ''}`}
              key={child.id}
            >
              <SessionTreeSegment
                segment={child}
                viewportRef={viewportRef}
                selectedId={selectedId}
                typeLabel={typeLabel}
                stateLabel={stateLabel}
                onSelect={onSelect}
              />
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
