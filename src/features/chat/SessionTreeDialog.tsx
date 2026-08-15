import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import {
  Bot,
  Bookmark,
  FileText,
  GitBranch,
  MessageSquare,
  Search,
  Settings,
  Tag,
  TreePine,
  User,
  Wrench,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { chatErrorMessage } from './chat-errors'
import { chatApi, type SessionTreeNode, type SessionTreeResponse } from './chat-api'

type TreeView = 'conversation' | 'branches' | 'labeled' | 'all'
type DisplayNode = Omit<SessionTreeNode, 'children'> & {
  children: DisplayNode[]
  branchRelated: boolean
}
type TreeSegment = {
  id: string
  nodes: DisplayNode[]
  children: TreeSegment[]
  active: boolean
}

const conversationKinds = new Set(['user', 'assistant', 'summary', 'compaction', 'position'])
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
      branchRelated: Boolean(parent?.branchRelated) || node.branchPoint,
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
  if (node.kind === 'tool') return Wrench
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
        className={`session-tree-node${node.id === selectedId ? ' selected' : ''}${node.active ? ' active' : ''}${node.leaf ? ' leaf' : ''}`}
        data-kind={node.kind}
        data-pisper-tree-entry={node.id}
        key={node.id}
        onClick={() => onSelect(node.id)}
      >
        <span className="session-tree-marker">
          <Icon size={14} />
        </span>
        <span className="session-tree-node-copy">
          <strong>{node.label || node.text || typeLabel(node)}</strong>
          <small>{node.label && node.text ? node.text : typeLabel(node)}</small>
        </span>
        <span className="session-tree-node-state">
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
    <div className={`session-tree-segment${segment.active ? ' active' : ''}`}>
      <div className="session-tree-track">
        {shouldVirtualize ? (
          <div
            className="session-tree-track-virtual"
            style={{ height: `${virtualizer.getTotalSize()}px` }}
          >
            {virtualizer.getVirtualItems().map((virtualItem) => (
              <div
                className="session-tree-virtual-item"
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
        <div className="session-tree-children">
          {children.map((child) => (
            <div className={`session-tree-child${child.active ? ' active' : ''}`} key={child.id}>
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
}: {
  open: boolean
  sessionId: string
  streaming: boolean
  onClose: () => void
  onNavigated: (editorText: string | null) => Promise<void> | void
}) {
  const { t, language } = useI18n()
  const [data, setData] = useState<SessionTreeResponse | null>(null)
  const [view, setView] = useState<TreeView>('conversation')
  const [query, setQuery] = useState('')
  const [selectedId, setSelectedId] = useState('')
  const [label, setLabel] = useState('')
  const [summarize, setSummarize] = useState(false)
  const [loading, setLoading] = useState(false)
  const [savingLabel, setSavingLabel] = useState(false)
  const [navigating, setNavigating] = useState(false)
  const [error, setError] = useState('')
  const viewportRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (!open || !sessionId) return
    let cancelled = false
    setView('conversation')
    setQuery('')
    setLoading(true)
    setError('')
    void chatApi
      .getSessionTree(sessionId)
      .then((next) => {
        if (cancelled) return
        setData(next)
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
    setSelectedId('')
    setLabel('')
    setSummarize(false)
    setError('')
  }, [open])

  const annotatedRoots = useMemo(() => buildDisplayTree(data?.nodes || []), [data])
  const typeLabel = useCallback(
    (node: SessionTreeNode) => {
      const known: Record<string, string> = {
        user: t('chat:sessionTree.userMessage'),
        assistant: t('chat:sessionTree.agentMessage'),
        tool: t('chat:sessionTree.toolResult'),
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
          : view === 'branches'
            ? node.branchRelated
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
  const canLabelSelected = selected?.kind === 'assistant' && selected.status === 'completed'

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
      <DialogContent className="chat-resource-dialog session-tree-dialog" showCloseButton>
        <div className="chat-resource-head session-tree-head">
          <div>
            <div className="session-tree-title">
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
        <div className="chat-resource-body session-tree-layout">
          <div className="chat-resource-browser session-tree-browser">
            <Tabs
              className="chat-resource-tabs session-tree-tabs"
              value={view}
              onValueChange={(value) => setView(value as TreeView)}
            >
              <TabsList>
                <TabsTrigger value="conversation">{t('chat:sessionTree.conversation')}</TabsTrigger>
                <TabsTrigger value="branches">{t('chat:sessionTree.branches')}</TabsTrigger>
                <TabsTrigger value="labeled">{t('chat:sessionTree.labeled')}</TabsTrigger>
                <TabsTrigger value="all">{t('chat:sessionTree.all')}</TabsTrigger>
              </TabsList>
            </Tabs>
            <label className="chat-resource-search session-tree-search">
              <Search size={15} />
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={t('chat:sessionTree.searchPlaceholder')}
              />
            </label>
            <div className="session-tree-viewport" data-testid="session-tree-list" ref={viewportRef}>
              {segments.length > 0 ? (
                <div className="session-tree-canvas">
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
                <p className="session-tree-empty">
                  {loading ? t('chat:sessionTree.loading') : error || t('chat:sessionTree.empty')}
                </p>
              )}
            </div>
          </div>
          <div className="chat-resource-config session-tree-inspector">
            {selected ? (
              <>
                <div>
                  <span className="session-tree-selection-kind">
                    {typeLabel(selected)}
                    {stateLabel(selected) && <em>{stateLabel(selected)}</em>}
                  </span>
                  <strong>{selected.label || typeLabel(selected)}</strong>
                  <p>{selected.text || t('chat:sessionTree.noPreview')}</p>
                </div>
                <div className="chat-resource-inputs">
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
                    </>
                  )}
                  <label className="session-tree-summary-option">
                    <span>{t('chat:sessionTree.abandonedBranchSummary')}</span>
                    <input
                      type="checkbox"
                      checked={summarize}
                      disabled={selected.leaf || streaming || Boolean(data?.streaming)}
                      onChange={(event) => setSummarize(event.target.checked)}
                    />
                    <small>{t('chat:sessionTree.abandonedBranchSummaryHint')}</small>
                  </label>
                </div>
                <p>{t('chat:sessionTree.sideEffectsRemain')}</p>
                {selected.timestamp && (
                  <p>
                    {new Intl.DateTimeFormat(language, {
                      dateStyle: 'medium',
                      timeStyle: 'short',
                    }).format(new Date(selected.timestamp))}
                  </p>
                )}
                {error && <p className="danger-text">{error}</p>}
                <Button
                  className="chat-resource-confirm"
                  disabled={navigating || selected.leaf || streaming || Boolean(data?.streaming)}
                  onClick={() => void navigate()}
                >
                  <GitBranch size={15} />
                  {navigating
                    ? t('chat:sessionTree.navigating')
                    : selected.leaf
                      ? t('chat:sessionTree.currentPosition')
                      : t('chat:sessionTree.continueFromNode')}
                </Button>
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
        </div>
      </DialogContent>
    </Dialog>
  )
}
