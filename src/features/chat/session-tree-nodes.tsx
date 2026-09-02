// 会话树节点行渲染：移动端列表与桌面端分段轨道（含虚拟化）。
// 从 SessionTreeDialog 拆出，样式类与原实现逐字一致。
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
            className="absolute top-0 left-0 w-full px-[12px] py-[4px]"
            key={virtualItem.key}
            style={{ transform: `translateY(${virtualItem.start}px)` }}
          >
            <button
              type="button"
              className={`session-tree-node hover:border-[var(--stroke-hover)] hover:bg-[var(--solid)] [&.selected]:border-[var(--focus)] [&.selected]:bg-[var(--solid)] [&.selected]:shadow-[0_0_0_2px_var(--focus-ring)] grid min-h-[56px] w-full grid-cols-[34px_minmax(0,1fr)_auto] items-center gap-[8px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[var(--solid)] p-[7px_9px_7px_4px] text-left ${node.id === selectedId ? ' selected' : ''}${node.active ? ' active' : ''}${node.leaf ? ' leaf' : ''}`}
              data-kind={node.kind}
              data-pisper-tree-entry={node.id}
              onClick={() => onSelect(node.id)}
            >
              <span className="session-tree-marker [.session-tree-node[data-kind='user']_&]:bg-[var(--star-soft)] [.session-tree-node[data-kind='user']_&]:text-[var(--star-strong)] [.session-tree-node[data-kind='assistant']_&]:bg-[var(--brand-blue-soft)] [.session-tree-node[data-kind='assistant']_&]:text-[var(--brand-blue-strong)] [.session-tree-node[data-kind='tool']_&]:bg-[var(--warning-soft)] [.session-tree-node[data-kind='tool']_&]:text-[var(--warning-strong)] [.session-tree-node.active_&]:border-[var(--brand-blue-border)] [.session-tree-node.leaf_&]:bg-[var(--star)] [.session-tree-node.leaf_&]:text-[var(--on-accent)] grid size-[28px] place-items-center justify-self-center [border:2px_solid_var(--surface-subtle)] rounded-full bg-[var(--surface-muted)] text-[var(--text-muted)]">
                <Icon size={14} />
              </span>
              <span className="session-tree-node-copy flex min-w-0 flex-col gap-[2px] [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_small]:text-[10px] [&_small]:text-[var(--text-muted)] [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_strong]:text-[12px] [&_strong]:font-[650]">
                <strong>{node.label || node.text || typeLabel(node)}</strong>
                <small>{node.label && node.text ? node.text : typeLabel(node)}</small>
              </span>
              <span className="session-tree-node-state [.session-tree-node.active_&]:text-[var(--brand-blue-strong)] [.session-tree-node.leaf_&]:text-[var(--brand-blue-strong)] inline-flex items-center gap-[4px] text-[9px] text-[var(--text-muted)] [text-transform:uppercase]">
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
        className={`session-tree-node hover:border-[var(--stroke-hover)] hover:bg-[var(--solid)] [&.selected]:border-[var(--focus)] [&.selected]:bg-[var(--solid)] [&.selected]:shadow-[0_0_0_2px_var(--focus-ring),0_8px_20px_-18px_var(--shadow)] relative z-[1] grid w-[220px] min-h-[48px] grid-cols-[30px_minmax(0,1fr)_auto] items-center gap-[8px] [border:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] bg-[color-mix(in_srgb,var(--solid)_94%,transparent)] text-[var(--text)] [padding:6px_8px_6px_2px] text-left shadow-[0_4px_14px_-13px_var(--shadow)] cursor-pointer ${node.id === selectedId ? ' selected' : ''}${node.active ? ' active' : ''}${node.leaf ? ' leaf' : ''}`}
        data-kind={node.kind}
        data-pisper-tree-entry={node.id}
        key={node.id}
        onClick={() => onSelect(node.id)}
      >
        <span className="session-tree-marker [.session-tree-node[data-kind='user']_&]:bg-[var(--star-soft)] [.session-tree-node[data-kind='user']_&]:text-[var(--star-strong)] [.session-tree-node[data-kind='assistant']_&]:bg-[var(--brand-blue-soft)] [.session-tree-node[data-kind='assistant']_&]:text-[var(--brand-blue-strong)] [.session-tree-node[data-kind='tool']_&]:bg-[var(--warning-soft)] [.session-tree-node[data-kind='tool']_&]:text-[var(--warning-strong)] [.session-tree-node[data-kind='summary']_&]:bg-[var(--violet-soft)] [.session-tree-node[data-kind='summary']_&]:text-[var(--violet-strong)] [.session-tree-node[data-kind='compaction']_&]:bg-[var(--violet-soft)] [.session-tree-node[data-kind='compaction']_&]:text-[var(--violet-strong)] [.session-tree-node.active_&]:border-[var(--brand-blue-border)] [.session-tree-node.active_&]:shadow-[0_0_0_2px_var(--brand-blue-soft)] [.session-tree-node.leaf_&]:bg-[var(--star)] [.session-tree-node.leaf_&]:text-[var(--on-accent)] [.session-tree-mark.active_&]:border-[var(--brand-blue-border)] [.session-tree-mark.active_&]:shadow-[0_0_0_2px_var(--brand-blue-soft)] relative z-[2] grid w-[26px] h-[26px] place-items-center [justify-self:center] [border:2px_solid_var(--surface-subtle)] rounded-[50%] bg-[var(--surface-muted)] text-[var(--text-muted)]">
          <Icon size={14} />
        </span>
        <span className="session-tree-node-copy [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_strong]:text-[12px] [&_strong]:font-[650] [&_small]:text-[var(--text-muted)] [&_small]:text-[10px] flex min-w-0 flex-col gap-[2px]">
          <strong>{node.label || node.text || typeLabel(node)}</strong>
          <small>{node.label && node.text ? node.text : typeLabel(node)}</small>
        </span>
        <span className="session-tree-node-state [.session-tree-node.active_&]:text-[var(--brand-blue-strong)] [.session-tree-node.leaf_&]:text-[var(--brand-blue-strong)] inline-flex min-w-0 items-center gap-[4px] text-[var(--text-muted)] text-[9px] [text-transform:uppercase]">
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
      <div className="session-tree-track before:absolute before:z-[0] before:top-[22px] before:bottom-[22px] before:left-[50%] before:w-[2px] before:rounded-[var(--r-pill)] before:bg-[var(--stroke)] before:[content:''] before:[transform:translateX(-1px)] [.session-tree-segment.active_>_&::before]:bg-[color-mix(in_srgb,var(--brand-blue)_64%,var(--stroke))] relative z-[1] grid w-[220px] gap-[8px]">
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
        <div className="session-tree-children before:absolute before:top-0 before:left-[50%] before:w-[2px] before:h-[14px] before:rounded-[var(--r-pill)] before:bg-[var(--stroke)] before:[content:''] before:[transform:translateX(-1px)] [.session-tree-segment.active_>_&::before]:bg-[color-mix(in_srgb,var(--brand-blue)_64%,var(--stroke))] relative flex items-start justify-center [margin-top:0] [padding-top:28px]">
          {children.map((child) => (
            <div
              className={`session-tree-child before:absolute before:top-0 before:w-[50%] before:h-[14px] before:[border-top:2px_solid_var(--stroke)] before:[content:''] after:absolute after:top-0 after:w-[50%] after:h-[14px] after:[border-top:2px_solid_var(--stroke)] after:[content:''] before:right-[50%] after:left-[50%] after:[border-left:2px_solid_var(--stroke)] [&:first-child::before]:[border-top-color:transparent] [&:last-child::after]:[border-top-color:transparent] [&.active::before]:border-[color-mix(in_srgb,var(--brand-blue)_64%,var(--stroke))] [&.active::after]:border-[color-mix(in_srgb,var(--brand-blue)_64%,var(--stroke))] [&.active:first-child::before]:[border-top-color:transparent] [&.active:last-child::after]:[border-top-color:transparent] relative flex [padding:14px_12px_0]${child.active ? ' active' : ''}`}
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
