// 会话树对话框：会话从属关系（父子）的树形浏览，虚拟化渲染大列表。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import {
  Bot,
  Bookmark,
  FileText,
  GitBranch,
  LoaderCircle,
  MessageSquare,
  MessageSquarePlus,
  Search,
  Settings,
  Tag,
  Trash2,
  TreePine,
  User,
  Wrench,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import type { SessionSummary } from '@/types/chat'
import { chatErrorMessage } from './chat-errors'
import {
  chatApi,
  type SessionTreeLabelMatch,
  type SessionTreeNode,
  type SessionTreeResponse,
} from './chat-api'
import { requestSessionSelection } from './events'

type TreeView = 'conversation' | 'labeled' | 'all' | 'marks'
type DisplayNode = Omit<SessionTreeNode, 'children'> & {
  children: DisplayNode[]
}
type TreeSegment = {
  id: string
  nodes: DisplayNode[]
  children: TreeSegment[]
  active: boolean
}

const conversationKinds = new Set(['user', 'assistant', 'summary', 'compaction', 'position'])
const nonDerivableKinds = new Set(['tool', 'tool-call'])
const TREE_ROW_HEIGHT = 56
const TREE_OVERSCAN = 12
const TREE_VIRTUALIZE_THRESHOLD = 120
const TREE_TRACK_SCROLL_MARGIN = 26

function buildDisplayTree(nodes: SessionTreeNode[]): DisplayNode[] {
  const roots: DisplayNode[] = []
  const byId = new Map<string, DisplayNode>()
  for (const node of nodes) {
    const parent = node.parentId ? byId.get(node.parentId) : undefined
    const displayNode: DisplayNode = {
      ...node,
      children: [],
    }
    byId.set(node.id, displayNode)
    if (parent) parent.children.push(displayNode)
    else roots.push(displayNode)
  }
  return roots
}

function flattenTree(nodes: DisplayNode[]): DisplayNode[] {
  const flattened: DisplayNode[] = []
  const stack = [...nodes].reverse()
  while (stack.length > 0) {
    const node = stack.pop()
    if (!node) continue
    flattened.push(node)
    for (let index = node.children.length - 1; index >= 0; index -= 1) {
      stack.push(node.children[index])
    }
  }
  return flattened
}

function filterTree(
  nodes: DisplayNode[],
  predicate: (node: DisplayNode) => boolean,
): DisplayNode[] {
  const roots: DisplayNode[] = []
  const visibleAncestor = new Map<string, DisplayNode | null>()
  for (const node of flattenTree(nodes)) {
    const parent = node.parentId ? visibleAncestor.get(node.parentId) || null : null
    if (!predicate(node)) {
      visibleAncestor.set(node.id, parent)
      continue
    }
    const visibleNode = { ...node, children: [] }
    visibleAncestor.set(node.id, visibleNode)
    if (parent) parent.children.push(visibleNode)
    else roots.push(visibleNode)
  }
  return roots
}

function createSegment(node: DisplayNode): TreeSegment {
  const nodes = [node]
  let cursor = node
  while (cursor.children.length === 1) {
    cursor = cursor.children[0]
    nodes.push(cursor)
  }
  const segment: TreeSegment = {
    id: node.id,
    nodes,
    children: [],
    active: nodes.some((entry) => entry.active),
  }
  const stack = [{ segment, children: cursor.children }]
  while (stack.length > 0) {
    const current = stack.pop()
    if (!current) continue
    for (const child of current.children) {
      const childNodes = [child]
      let childCursor = child
      while (childCursor.children.length === 1) {
        childCursor = childCursor.children[0]
        childNodes.push(childCursor)
      }
      const childSegment: TreeSegment = {
        id: child.id,
        nodes: childNodes,
        children: [],
        active: childNodes.some((entry) => entry.active),
      }
      current.segment.children.push(childSegment)
      stack.push({ segment: childSegment, children: childCursor.children })
    }
  }
  return segment
}

function nodeIcon(node: SessionTreeNode) {
  if (node.kind === 'user') return User
  if (node.kind === 'assistant') return Bot
  if (node.kind === 'tool' || node.kind === 'tool-call') return Wrench
  if (node.kind === 'settings') return Settings
  if (node.kind === 'label') return Tag
  if (node.kind === 'summary' || node.kind === 'compaction') return FileText
  if (node.kind === 'position') return GitBranch
  if (node.label) return Bookmark
  return MessageSquare
}

function SessionTreeSegment({
  segment,
  viewportRef,
  selectedId,
  typeLabel,
  stateLabel,
  onSelect,
}: {
  segment: TreeSegment
  viewportRef: React.RefObject<HTMLDivElement | null>
  selectedId: string
  typeLabel: (node: SessionTreeNode) => string
  stateLabel: (node: SessionTreeNode) => string
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

export function SessionTreeDialog({
  open,
  sessionId,
  streaming,
  onClose,
  onNavigated,
  onCreateChildSession,
}: {
  open: boolean
  sessionId: string
  streaming: boolean
  onClose: () => void
  onNavigated: (editorText: string | null) => Promise<void> | void
  onCreateChildSession: (boundaryEntryId: string) => Promise<void> | void
}) {
  const { t, language } = useI18n()
  const [data, setData] = useState<SessionTreeResponse | null>(null)
  const [sessions, setSessions] = useState<SessionSummary[]>([])
  const [view, setView] = useState<TreeView>('conversation')
  const [query, setQuery] = useState('')
  const [selectedId, setSelectedId] = useState('')
  const [label, setLabel] = useState('')
  const [allMarks, setAllMarks] = useState<SessionTreeLabelMatch[]>([])
  const [marksLoaded, setMarksLoaded] = useState(false)
  const [marksLoading, setMarksLoading] = useState(false)
  const [openingMark, setOpeningMark] = useState<SessionTreeLabelMatch | null>(null)
  const [summarize, setSummarize] = useState(false)
  const [loading, setLoading] = useState(false)
  const [savingLabel, setSavingLabel] = useState(false)
  const [navigating, setNavigating] = useState(false)
  const [creatingChild, setCreatingChild] = useState(false)
  const [openingRelatedId, setOpeningRelatedId] = useState('')
  const [error, setError] = useState('')
  const viewportRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (!open || !sessionId) return
    let cancelled = false
    setView('conversation')
    setQuery('')
    setLoading(true)
    setError('')
    void Promise.all([chatApi.getSessionTree(sessionId), chatApi.listSessions()])
      .then(([next, sessionData]) => {
        if (cancelled) return
        setData(next)
        setSessions(sessionData.sessions || [])
        setSelectedId(next.leafId || next.nodes[0]?.id || '')
      })
      .catch((reason) => {
        if (!cancelled) setError(chatErrorMessage(reason))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [open, sessionId])

  // Release the (potentially 10k+ node) tree payload as soon as the dialog closes.
  useEffect(() => {
    if (open) return
    setData(null)
    setSessions([])
    setSelectedId('')
    setLabel('')
    setOpeningMark(null)
    setSummarize(false)
    setError('')
  }, [open])

  // 首次切到「全部标记」页签时拉取跨会话标记列表（搜索索引已持久化，代价很低）。
  // 注意：marksLoading 只作防重入守卫，不能进依赖数组——effect 内同步 setState
  // 会触发重跑并把在途请求的 active 置 false，导致永远停在加载态。
  useEffect(() => {
    if (!open || view !== 'marks' || marksLoaded) return
    let active = true
    setMarksLoading(true)
    setError('')
    void chatApi
      .listSessionTreeLabels(500)
      .then((result) => {
        if (!active) return
        setAllMarks(result.labels || [])
        setMarksLoaded(true)
      })
      .catch((reason) => active && setError(chatErrorMessage(reason)))
      .finally(() => active && setMarksLoading(false))
    return () => {
      active = false
    }
  }, [open, view, marksLoaded])

  const visibleMarks = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase(language)
    if (!needle) return allMarks
    return allMarks.filter((mark) =>
      `${mark.label} ${mark.sessionName}`.toLocaleLowerCase(language).includes(needle),
    )
  }, [allMarks, language, query])

  // 打开标签定位：非活动条目先导航到目标条目（激活分支），
  // 再请求打开会话；防重入避免重复导航。
  const openMark = async (mark: SessionTreeLabelMatch) => {
    if (openingMark) return
    setOpeningMark(mark)
    setError('')
    try {
      if (!mark.active) {
        await chatApi.navigateSessionTreeTarget(mark.sessionId, mark.entryId)
      }
      requestSessionSelection(mark.sessionId, 'open', mark.entryId)
      onClose()
    } catch (reason) {
      setError(chatErrorMessage(reason))
      setOpeningMark(null)
    }
  }

  const markTime = (value: string) => {
    const timestamp = Date.parse(value)
    if (!Number.isFinite(timestamp)) return t('navigation:appOverlays.unknownTime')
    return new Intl.DateTimeFormat(language, {
      dateStyle: 'medium',
      timeStyle: 'short',
    }).format(timestamp)
  }

  const annotatedRoots = useMemo(() => buildDisplayTree(data?.nodes || []), [data])
  // 节点类型 → 本地化标签（未知类型回退原始 type 字段）。
  const typeLabel = useCallback(
    (node: SessionTreeNode) => {
      const known: Record<string, string> = {
        user: t('chat:sessionTree.userMessage'),
        assistant: t('chat:sessionTree.agentMessage'),
        tool: t('chat:sessionTree.toolResult'),
        'tool-call': t('chat:sessionTree.toolCall'),
        summary: t('chat:sessionTree.branchSummary'),
        compaction: t('chat:sessionTree.compactionSummary'),
        settings: t('chat:sessionTree.settingsChange'),
        label: t('chat:sessionTree.labelChange'),
        position: t('chat:sessionTree.branchPosition'),
        extension: t('chat:sessionTree.extensionEntry'),
        metadata: t('chat:sessionTree.metadataEntry'),
        message: t('chat:sessionTree.message'),
      }
      return known[node.kind] || node.type
    },
    [t],
  )
  const visibleRoots = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase(language)
    return filterTree(annotatedRoots, (node) => {
      const inView =
        view === 'conversation'
          ? conversationKinds.has(node.kind)
          : view === 'labeled'
            ? Boolean(node.label)
            : true
      if (!inView) return false
      if (!needle) return true
      return `${node.label} ${node.text} ${typeLabel(node)}`
        .toLocaleLowerCase(language)
        .includes(needle)
    })
  }, [annotatedRoots, language, query, typeLabel, view])
  const segments = useMemo(() => visibleRoots.map(createSegment), [visibleRoots])
  const visibleNodes = useMemo(() => flattenTree(visibleRoots), [visibleRoots])
  const allNodes = useMemo(() => flattenTree(annotatedRoots), [annotatedRoots])
  const selected = allNodes.find((node) => node.id === selectedId) || null
  const parentSession = data?.lineage?.parentSessionId
    ? sessions.find((session) => session.id === data.lineage?.parentSessionId)
    : null
  const childSessions = (data?.lineage?.childSessionIds || [])
    .map((childId) => sessions.find((session) => session.id === childId))
    .filter((session): session is SessionSummary => Boolean(session))
  const canLabelSelected = selected?.kind === 'assistant' && selected.status === 'completed'
  const canCreateChildSelected = canLabelSelected
  const canDeriveSelected = selected ? !nonDerivableKinds.has(selected.kind) : false

  useEffect(() => {
    setLabel(selected?.label || '')
    setSummarize(false)
  }, [selected?.id, selected?.label])

  useEffect(() => {
    if (selectedId && visibleNodes.some((node) => node.id === selectedId)) return
    const activeNode = [...visibleNodes].reverse().find((node) => node.active)
    setSelectedId(activeNode?.id || visibleNodes[0]?.id || '')
  }, [selectedId, visibleNodes])

  useEffect(() => {
    if (!open || !selectedId) return
    const frame = requestAnimationFrame(() => {
      document
        .querySelector<HTMLElement>('[data-testid="session-tree-list"] .session-tree-node.selected')
        ?.scrollIntoView({ block: 'center' })
    })
    return () => cancelAnimationFrame(frame)
  }, [open, query, selectedId, view, visibleNodes.length])

  // 刷新会话树：拉取树数据并把选中项重置到叶子节点（或第一个节点）。
  const refresh = async () => {
    setLoading(true)
    setError('')
    try {
      const next = await chatApi.getSessionTree(sessionId)
      setData(next)
      setSelectedId(next.leafId || next.nodes[0]?.id || '')
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setLoading(false)
    }
  }

  // 保存选中节点的标签；防重入（savingLabel）。
  const saveLabel = async () => {
    if (!selected || savingLabel) return
    setSavingLabel(true)
    setError('')
    try {
      setData(await chatApi.setSessionTreeLabel(sessionId, selected.id, label))
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setSavingLabel(false)
    }
  }

  // 移除选中节点的标签（置空）；防重入。
  const removeLabel = async () => {
    if (!selected || savingLabel) return
    setSavingLabel(true)
    setError('')
    try {
      setData(await chatApi.setSessionTreeLabel(sessionId, selected.id, ''))
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setSavingLabel(false)
    }
  }

  // 从选中的已完成回复创建独立对话；创建成功后由外层打开新 Dock。
  const createChild = async () => {
    if (!selected || !canCreateChildSelected || creatingChild) return
    setCreatingChild(true)
    setError('')
    try {
      await onCreateChildSession(selected.id)
      onClose()
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setCreatingChild(false)
    }
  }

  // 在谱系中打开父会话或其它独立对话；父会话需要先定位到创建边界。
  const openRelatedSession = async (target: SessionSummary, targetEntryId = '') => {
    if (!target.id || openingRelatedId) return
    setOpeningRelatedId(target.id)
    setError('')
    try {
      if (targetEntryId) await chatApi.navigateSessionTreeTarget(target.id, targetEntryId)
      requestSessionSelection(target.id, 'open', targetEntryId)
      onClose()
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setOpeningRelatedId('')
    }
  }

  // 导航到选中节点（非叶子且非流式中）：切换到该历史位置继续会话。
  const navigate = async () => {
    if (!selected || navigating || selected.leaf || streaming || data?.streaming) return
    setNavigating(true)
    setError('')
    try {
      const next = await chatApi.navigateSessionTree(sessionId, selected.id, summarize)
      setData(next)
      if (!next.cancelled) {
        await onNavigated(next.editorText)
        onClose()
      }
    } catch (reason) {
      setError(chatErrorMessage(reason))
    } finally {
      setNavigating(false)
    }
  }

  const stateLabel = (node: SessionTreeNode) =>
    node.leaf
      ? t('chat:sessionTree.current')
      : node.branchPoint
        ? t('chat:sessionTree.fork')
        : node.active
          ? t('chat:sessionTree.active')
          : ''

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !nextOpen && onClose()}>
      <DialogContent
        className="chat-resource-dialog w-[min(820px,calc(100vw_-_32px))] h-[min(600px,calc(100dvh_-_32px))] max-w-[820px] grid-rows-[auto_minmax(0,1fr)] overflow-hidden max-[650px]:w-[calc(100vw_-_16px)] max-[650px]:h-[calc(100dvh_-_16px)] session-tree-dialog !w-[min(1120px,calc(100vw_-_32px))] !h-[min(760px,calc(100dvh_-_32px))] !max-w-[1120px]"
        showCloseButton
      >
        <div className="chat-resource-head [&_[data-slot='dialog-title']]:text-[16px] [&_[data-slot='dialog-description']]:mt-[3px] [&_[data-slot='dialog-description']]:text-[12px] max-[650px]:p-[14px] flex items-start justify-between gap-[16px] [border-bottom:1px_solid_var(--stroke-soft)] [padding:17px_18px_14px] session-tree-head max-[650px]:items-start">
          <div>
            <div className="session-tree-title [&_>_span]:grid [&_>_span]:w-[30px] [&_>_span]:h-[30px] [&_>_span]:place-items-center [&_>_span]:rounded-[var(--r-sm)] [&_>_span]:bg-[var(--success-soft)] [&_>_span]:text-[var(--success)] flex items-center gap-[9px]">
              <span>
                <TreePine size={17} />
              </span>
              <DialogTitle>{t('chat:sessionTree.title')}</DialogTitle>
            </div>
            <DialogDescription>
              {t('chat:sessionTree.description', {
                nodes: data?.nodeCount || 0,
                branches: data?.branchCount || 0,
              })}
            </DialogDescription>
          </div>
        </div>
        <div className="chat-resource-body grid min-h-0 grid-cols-[minmax(0,1.1fr)_minmax(260px,.9fr)] overflow-hidden max-[650px]:grid-cols-[1fr] max-[650px]:grid-rows-[minmax(0,1fr)_minmax(190px,auto)] max-[650px]:overflow-hidden session-tree-layout !grid-cols-[minmax(0,1fr)_292px] bg-[var(--surface-subtle)] max-[650px]:grid-rows-[minmax(0,1fr)_minmax(210px,auto)]">
          <div className="chat-resource-browser flex min-w-0 min-h-0 flex-col [border-right:1px_solid_var(--stroke-soft)] p-[12px] max-[650px]:min-h-0 max-[650px]:[border-right:0] max-[650px]:[border-bottom:1px_solid_var(--stroke-soft)] session-tree-browser ![border-right:0] !p-0 bg-transparent">
            <Tabs
              className="chat-resource-tabs [&_[data-slot='tabs-list']]:grid [&_[data-slot='tabs-list']]:w-full [&_[data-slot='tabs-list']]:h-[36px] [&_[data-slot='tabs-list']]:grid-cols-[repeat(5,minmax(0,1fr))] [&_[data-slot='tabs-list']]:[border:1px_solid_var(--stroke-soft)] [&_[data-slot='tabs-list']]:rounded-[var(--r-sm)] [&_[data-slot='tabs-list']]:bg-[var(--surface-muted)] [&_[data-slot='tabs-trigger']]:min-w-0 [&_[data-slot='tabs-trigger']]:gap-[5px] [&_[data-slot='tabs-trigger']]:rounded-[var(--r-xs)] [&_[data-slot='tabs-trigger']]:[padding-inline:6px] [&_[data-slot='tabs-trigger']]:text-[11px] [&_[data-slot='tabs-trigger'][data-state='active']]:bg-[var(--solid)] [&_[data-slot='tabs-trigger'][data-state='active']]:text-[var(--text)] [&_[data-slot='tabs-trigger']_small]:text-[var(--text-muted)] [&_[data-slot='tabs-trigger']_small]:text-[10px] [&_[data-slot='tabs-trigger']_small]:font-[500] w-full session-tree-tabs [padding:12px_12px_0]"
              value={view}
              onValueChange={(value) => setView(value as TreeView)}
            >
              <TabsList>
                <TabsTrigger value="conversation">{t('chat:sessionTree.conversation')}</TabsTrigger>
                <TabsTrigger value="labeled">{t('chat:sessionTree.labeled')}</TabsTrigger>
                <TabsTrigger value="all">{t('chat:sessionTree.all')}</TabsTrigger>
                <TabsTrigger value="marks">{t('chat:sessionTree.allMarks')}</TabsTrigger>
              </TabsList>
            </Tabs>
            <label className="chat-resource-search [&:focus-within]:border-[var(--focus)] [&:focus-within]:shadow-[0_0_0_2px_var(--focus-ring)] [&_input]:w-full [&_input]:h-[36px] [&_input]:border-0 [&_input]:[outline:0] [&_input]:bg-transparent [&_input]:text-[var(--text)] [&_input]:text-[13px] flex items-center gap-[7px] [margin-top:8px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] [padding:0_10px] text-[var(--text-muted)] flex-none [margin:8px_12px_0] bg-[var(--solid)]">
              <Search size={15} />
              <input
                value={query}
                disabled={Boolean(openingMark)}
                onChange={(event) => setQuery(event.target.value)}
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
                      onClick={() => void openMark(mark)}
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
                        <small>
                          {mark.sessionName || t('navigation:appOverlays.untitledChat')}
                        </small>
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
                  <div className="session-tree-canvas max-[650px]:min-w-[460px] max-[650px]:[padding-inline:12px] flex w-[max-content] min-w-[100%] items-start justify-center gap-[36px] [padding:26px_28px_48px]">
                    {segments.map((segment) => (
                      <SessionTreeSegment
                        segment={segment}
                        viewportRef={viewportRef}
                        selectedId={selectedId}
                        typeLabel={typeLabel}
                        stateLabel={stateLabel}
                        onSelect={setSelectedId}
                        key={segment.id}
                      />
                    ))}
                  </div>
                ) : (
                  <p className="grid min-h-[180px] place-items-center text-[var(--text-muted)] text-[12px]">
                    {loading ? t('chat:sessionTree.loading') : error || t('chat:sessionTree.empty')}
                  </p>
                )}
              </div>
            )}
          </div>
          {view !== 'marks' && (
            <div className="chat-resource-config [&_>_p]:m-[auto] [&_>_p]:text-[var(--text-muted)] [&_>_p]:text-[12px] [&_>_div:first-child]:flex [&_>_div:first-child]:flex-col [&_>_div:first-child]:gap-[4px] [&_strong]:text-[14px] [&_p]:text-[var(--text-muted)] [&_p]:text-[12px] max-[650px]:min-h-0 max-[650px]:max-h-[230px] flex min-w-0 min-h-0 flex-col gap-[16px] overflow-y-auto [overscroll-behavior:contain] [padding:18px] session-tree-inspector max-[650px]:max-h-[250px] [margin:12px_12px_12px_0] [border-left:0] rounded-[var(--r-md)] bg-[color-mix(in_srgb,var(--solid)_88%,var(--surface-subtle))] shadow-[0_10px_28px_-26px_var(--shadow)]">
              {selected ? (
                <>
                  {(parentSession || childSessions.length > 0) && (
                    <div className="session-tree-lineage [&_>_strong]:text-[12px] [&_>_strong]:font-[650] [&_button]:justify-start [&_button]:text-left [&_button]:text-[11px] flex flex-col gap-[6px] [border-bottom:1px_solid_var(--stroke-soft)] [padding-bottom:12px]">
                      <strong>{t('chat:sessionTree.conversationLineage')}</strong>
                      {parentSession && (
                        <Button
                          variant="outline"
                          disabled={Boolean(openingRelatedId)}
                          onClick={() =>
                            void openRelatedSession(
                              parentSession,
                              data?.lineage?.sourceEntryId || '',
                            )
                          }
                        >
                          <MessageSquare size={13} />
                          {t('chat:sessionTree.returnToOriginalChat', {
                            name: parentSession.name || t('chat:chatPage.newChat'),
                          })}
                        </Button>
                      )}
                      {childSessions.map((child) => (
                        <Button
                          variant="outline"
                          disabled={Boolean(openingRelatedId)}
                          key={child.id}
                          onClick={() => void openRelatedSession(child)}
                        >
                          <MessageSquare size={13} />
                          {t('chat:sessionTree.openSeparateChat', {
                            name: child.name || t('chat:chatPage.newChat'),
                          })}
                        </Button>
                      ))}
                    </div>
                  )}
                  <div>
                    <span className="session-tree-selection-kind [&_em]:rounded-[var(--r-xs)] [&_em]:bg-[var(--brand-blue-soft)] [&_em]:text-[var(--brand-blue-strong)] [&_em]:p-[2px_5px] [&_em]:[font-style:normal] flex items-center justify-between gap-[8px] text-[var(--text-muted)] text-[10px] [text-transform:uppercase]">
                      {typeLabel(selected)}
                      {stateLabel(selected) && <em>{stateLabel(selected)}</em>}
                    </span>
                    <strong>{selected.label || typeLabel(selected)}</strong>
                    <p>{selected.text || t('chat:sessionTree.noPreview')}</p>
                  </div>
                  <div className="chat-resource-inputs [&_label]:grid [&_label]:gap-[5px] [&_label]:text-[12px] [&_label]:font-[600] [&_input[type='checkbox']]:w-[16px] [&_input[type='checkbox']]:h-[16px] [&_input[type='checkbox']]:[accent-color:var(--blue)] flex flex-col gap-[11px]">
                    {canLabelSelected && (
                      <>
                        <label>
                          <span>{t('chat:sessionTree.nodeLabel')}</span>
                          <Input
                            value={label}
                            maxLength={80}
                            placeholder={t('chat:sessionTree.labelPlaceholder')}
                            onChange={(event) => setLabel(event.target.value)}
                          />
                        </label>
                        <Button
                          variant="outline"
                          disabled={savingLabel || streaming || Boolean(data?.streaming)}
                          onClick={() => void saveLabel()}
                        >
                          <Tag size={14} />
                          {savingLabel
                            ? t('chat:sessionTree.savingLabel')
                            : t('chat:sessionTree.saveLabel')}
                        </Button>
                        {Boolean(selected.label) && (
                          <Button
                            variant="outline"
                            disabled={savingLabel || streaming || Boolean(data?.streaming)}
                            onClick={() => void removeLabel()}
                          >
                            <Trash2 size={14} />
                            {t('chat:sessionTree.removeLabel')}
                          </Button>
                        )}
                      </>
                    )}
                    {canCreateChildSelected && (
                      <Button
                        variant="outline"
                        disabled={creatingChild || streaming || Boolean(data?.streaming)}
                        onClick={() => void createChild()}
                      >
                        <MessageSquarePlus size={14} />
                        {creatingChild
                          ? t('chat:sessionTree.creatingChildChat')
                          : t('chat:sessionTree.createChildChat')}
                      </Button>
                    )}
                    {canDeriveSelected && (
                      <label className="session-tree-summary-option [&_small]:[grid-column:1/-1] [&_small]:text-[var(--text-muted)] [&_small]:font-[400] [&_small]:leading-[1.45] grid-cols-[minmax(0,1fr)_auto] items-center">
                        <span>{t('chat:sessionTree.abandonedBranchSummary')}</span>
                        <input
                          type="checkbox"
                          checked={summarize}
                          disabled={selected.leaf || streaming || Boolean(data?.streaming)}
                          onChange={(event) => setSummarize(event.target.checked)}
                        />
                        <small>{t('chat:sessionTree.abandonedBranchSummaryHint')}</small>
                      </label>
                    )}
                  </div>
                  {canDeriveSelected && <p>{t('chat:sessionTree.sideEffectsRemain')}</p>}
                  {selected.timestamp && (
                    <p>
                      {new Intl.DateTimeFormat(language, {
                        dateStyle: 'medium',
                        timeStyle: 'short',
                      }).format(new Date(selected.timestamp))}
                    </p>
                  )}
                  {error && <p className="danger-text">{error}</p>}
                  {canDeriveSelected && (
                    <Button
                      className="chat-resource-confirm hover:bg-[var(--star-hover)] hover:text-[var(--on-accent)] min-w-[112px] min-h-[36px] self-start bg-[var(--star)] text-[var(--on-accent)] font-[650]"
                      disabled={
                        navigating || selected.leaf || streaming || Boolean(data?.streaming)
                      }
                      onClick={() => void navigate()}
                    >
                      <GitBranch size={15} />
                      {navigating
                        ? t('chat:sessionTree.navigating')
                        : selected.leaf
                          ? t('chat:sessionTree.currentPosition')
                          : t('chat:sessionTree.continueFromNode')}
                    </Button>
                  )}
                </>
              ) : (
                <p>
                  {loading
                    ? t('chat:sessionTree.loading')
                    : error || t('chat:sessionTree.chooseNode')}
                </p>
              )}
              {error && !selected && (
                <Button variant="outline" onClick={() => void refresh()}>
                  {t('chat:sessionTree.retry')}
                </Button>
              )}
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
