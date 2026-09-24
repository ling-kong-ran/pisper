import { useEffect, useRef, useState, type DragEvent } from 'react'
import { ChevronRight, Redo2, Undo2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { cn } from '@/lib/utils'
import {
  addCanvasNode,
  CANVAS_KINDS,
  findCanvasNode,
  isCanvasContainerKind,
  moveCanvasNode,
  removeCanvasNode,
  updateCanvasNode,
  type ChatCanvasKind,
  type ChatCanvasNode,
} from './chat-canvas'
import { parseCanvasCss } from './chat-canvas-style'
import {
  CANVAS_KIND_DRAG,
  CANVAS_NODE_DRAG,
  canvasKindLabels,
  canvasNodes,
  canvasParent,
  copyCanvasNode,
} from './chat-canvas-editor-model'
import { ChatCanvasPalette } from './ChatCanvasPalette'
import { ChatCanvasInspector } from './ChatCanvasInspector'
import { ChatLayoutPreview, type CanvasDropHandlers } from './ChatLayoutPreview'

function CanvasTree({
  root,
  selectedId,
  dropHint,
  onSelect,
  onDragOver,
  onDrop,
}: {
  root: ChatCanvasNode
  selectedId: string
  dropHint: string
  onSelect: (id: string) => void
} & CanvasDropHandlers) {
  const { t } = useI18n()
  const labels = canvasKindLabels(t)
  const render = (node: ChatCanvasNode, depth: number) => (
    <li key={node.id} className="min-w-0">
      <button
        type="button"
        aria-pressed={selectedId === node.id}
        onClick={() => onSelect(node.id)}
        draggable={node.id !== root.id}
        onDragStart={(event) => {
          event.stopPropagation()
          event.dataTransfer.setData(CANVAS_NODE_DRAG, node.id)
          event.dataTransfer.effectAllowed = 'move'
        }}
        onDragOver={(event) => onDragOver(event, node)}
        onDrop={(event) => onDrop(event, node)}
        className={cn(
          'flex min-h-8 w-full min-w-0 items-center gap-1.5 rounded-md pr-2 text-left text-xs focus-visible:outline-2 focus-visible:outline-ring',
          selectedId === node.id ? 'bg-muted font-medium' : 'hover:bg-muted/50',
          dropHint.startsWith(`${node.id}:`) && 'ring-2 ring-ring',
        )}
        style={{ paddingLeft: 8 + depth * 12 }}
        title={node.id}
      >
        {isCanvasContainerKind(node.kind) && <ChevronRight className="size-3 shrink-0 rotate-90" />}
        <span className="truncate">
          {labels[node.kind]}
          {node.kind === 'text' && node.text ? ` · ${node.text}` : ''}
        </span>
      </button>
      {node.children?.length ? (
        <ul className="min-w-0">{node.children.map((child) => render(child, depth + 1))}</ul>
      ) : null}
    </li>
  )
  return (
    <ul className="max-h-80 min-w-0 overflow-auto" aria-label={t('chat-layout:canvas.tree')}>
      {render(root, 0)}
    </ul>
  )
}

export function ChatCanvasEditor({
  root,
  mobile,
  onChange,
  onValidityChange,
}: {
  root: ChatCanvasNode
  mobile: boolean
  onChange: (root: ChatCanvasNode) => void
  onValidityChange: (valid: boolean) => void
}) {
  const { t } = useI18n()
  const [selectedId, setSelectedId] = useState(root.id)
  const [past, setPast] = useState<ChatCanvasNode[]>([])
  const [future, setFuture] = useState<ChatCanvasNode[]>([])
  const [cssDrafts, setCssDrafts] = useState<Record<string, string>>({})
  const [error, setError] = useState('')
  const [dropHint, setDropHint] = useState('')
  const [panel, setPanel] = useState('canvas')
  const previousRoot = useRef(root)
  const selected = findCanvasNode(root, selectedId) ?? root
  const entries = canvasNodes(root)
  const invalidIds = entries
    .filter((node) => {
      if (!(node.id in cssDrafts)) return false
      try {
        parseCanvasCss(cssDrafts[node.id])
        return false
      } catch {
        return true
      }
    })
    .map((node) => node.id)
  useEffect(() => {
    onValidityChange(invalidIds.length === 0)
  }, [invalidIds.length, onValidityChange])
  useEffect(() => {
    if (previousRoot.current === root) return
    previousRoot.current = root
    setSelectedId(root.id)
    setPast([])
    setFuture([])
    setCssDrafts({})
    setError('')
  }, [root])
  const publish = (next: ChatCanvasNode) => {
    previousRoot.current = next
    onChange(next)
  }
  const commit = (next: ChatCanvasNode) => {
    if (next === root) return
    setPast((items) => [...items, root].slice(-50))
    setFuture([])
    setError('')
    publish(next)
  }
  const run = (action: () => ChatCanvasNode) => {
    try {
      commit(action())
    } catch {
      setError(t('chat-layout:canvas.operationError'))
    }
  }
  const targetContainer = isCanvasContainerKind(selected.kind)
    ? selected
    : (canvasParent(root, selected.id) ?? root)
  const add = (kind: ChatCanvasKind) => {
    try {
      const next = addCanvasNode(root, targetContainer.id, kind)
      const added = findCanvasNode(next, targetContainer.id)?.children?.at(-1)
      commit(next)
      if (added) setSelectedId(added.id)
    } catch {
      setError(t('chat-layout:canvas.operationError'))
    }
  }
  const changeCss = (css: string) => {
    setCssDrafts((drafts) => ({ ...drafts, [selected.id]: css }))
    try {
      parseCanvasCss(css)
      const next = updateCanvasNode(root, selected.id, { css })
      commit(next)
    } catch {
      /* 草稿错误在样式区显示，保留画布上一次有效样式。 */
    }
  }
  const undo = () => {
    if (invalidIds.length) {
      setCssDrafts({})
      setError('')
      return
    }
    const next = past.at(-1)
    if (!next) return
    setPast((items) => items.slice(0, -1))
    setFuture((items) => [root, ...items])
    setCssDrafts({})
    setError('')
    publish(next)
  }
  const redo = () => {
    const next = future[0]
    if (!next) return
    setPast((items) => [...items, root])
    setFuture((items) => items.slice(1))
    setCssDrafts({})
    setError('')
    publish(next)
  }
  const move = (delta: number) => {
    const parent = canvasParent(root, selected.id)
    const index = parent?.children?.findIndex((node) => node.id === selected.id) ?? -1
    if (parent && index >= 0) run(() => moveCanvasNode(root, selected.id, parent.id, index + delta))
  }
  const dropPosition = (event: DragEvent<HTMLElement>, node: ChatCanvasNode) => {
    const rect = event.currentTarget.getBoundingClientRect()
    const ratio = rect.height ? (event.clientY - rect.top) / rect.height : 0.5
    if (node.id === root.id || (isCanvasContainerKind(node.kind) && ratio >= 0.25 && ratio <= 0.75))
      return 'inside'
    return ratio < 0.5 ? 'before' : 'after'
  }
  const onDragOver: CanvasDropHandlers['onDragOver'] = (event, node) => {
    if (
      !event.dataTransfer.types.includes(CANVAS_NODE_DRAG) &&
      !event.dataTransfer.types.includes(CANVAS_KIND_DRAG)
    )
      return
    event.preventDefault()
    event.stopPropagation()
    event.dataTransfer.dropEffect = event.dataTransfer.types.includes(CANVAS_NODE_DRAG)
      ? 'move'
      : 'copy'
    setDropHint(`${node.id}:${dropPosition(event, node)}`)
  }
  const onDrop: CanvasDropHandlers['onDrop'] = (event, node) => {
    event.preventDefault()
    event.stopPropagation()
    setDropHint('')
    const zone = dropPosition(event, node)
    const parent = zone === 'inside' ? node : canvasParent(root, node.id)
    if (!parent || !isCanvasContainerKind(parent.kind)) return
    const nodeId = event.dataTransfer.getData(CANVAS_NODE_DRAG)
    const kind = CANVAS_KINDS.find(
      (entry) => entry === event.dataTransfer.getData(CANVAS_KIND_DRAG),
    )
    try {
      let next = root
      let movingId = nodeId
      if (!movingId && kind) {
        next = addCanvasNode(root, parent.id, kind)
        movingId = findCanvasNode(next, parent.id)?.children?.at(-1)?.id ?? ''
      }
      if (!movingId || movingId === node.id) return
      // 目标序号按移除拖动项后的兄弟列表计算，避免同容器向下移动偏移一位。
      const siblings = (findCanvasNode(next, parent.id)?.children ?? []).filter(
        (entry) => entry.id !== movingId,
      )
      const anchorIndex = siblings.findIndex((entry) => entry.id === node.id)
      const index = zone === 'inside' ? siblings.length : anchorIndex + (zone === 'after' ? 1 : 0)
      next = moveCanvasNode(next, movingId, parent.id, index)
      commit(next)
      setSelectedId(movingId)
    } catch {
      setError(t('chat-layout:canvas.operationError'))
    }
  }
  const selectedDescendants = new Set(canvasNodes(selected).map((node) => node.id))
  const parents = entries.filter(
    (node) => isCanvasContainerKind(node.kind) && !selectedDescendants.has(node.id),
  )
  return (
    <div className="min-w-0 space-y-3" onDragEnd={() => setDropHint('')}>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-xs leading-5 text-muted-foreground">
          {t('chat-layout:canvas.editHint')}
        </p>
        <div className="flex gap-1">
          <Button
            variant="outline"
            size="sm"
            disabled={!past.length && !invalidIds.length}
            onClick={undo}
          >
            <Undo2 />
            {t('chat-layout:canvas.undo')}
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={!future.length || Boolean(invalidIds.length)}
            onClick={redo}
          >
            <Redo2 />
            {t('chat-layout:canvas.redo')}
          </Button>
        </div>
      </div>
      {(error || invalidIds.length > 0) && (
        <div
          role="alert"
          className="rounded-lg border border-border bg-muted/40 p-3 text-sm leading-6"
        >
          {error || t('chat-layout:canvas.invalidStyles', { count: invalidIds.length })}
          {invalidIds.length > 0 && (
            <Button
              variant="link"
              size="sm"
              onClick={() => {
                setSelectedId(invalidIds[0])
                setPanel('style')
              }}
            >
              {t('chat-layout:canvas.fixStyle')}
            </Button>
          )}
        </div>
      )}
      <Tabs value={panel} onValueChange={setPanel} className="min-w-0">
        <TabsList className="w-full xl:hidden">
          <TabsTrigger value="library">{t('chat-layout:canvas.library')}</TabsTrigger>
          <TabsTrigger value="canvas">{t('chat-layout:canvas.canvas')}</TabsTrigger>
          <TabsTrigger value="style">{t('chat-layout:canvas.style')}</TabsTrigger>
        </TabsList>
        <div className="grid min-w-0 gap-4 xl:grid-cols-[210px_minmax(0,1fr)_280px]">
          <TabsContent
            value="library"
            forceMount
            className="min-w-0 rounded-xl border border-border p-3 data-[state=inactive]:hidden xl:data-[state=inactive]:block"
          >
            <ChatCanvasPalette root={root} onAdd={add} />
            <div className="mt-4 space-y-2 border-t border-border pt-4">
              <h3 className="text-xs font-medium">{t('chat-layout:canvas.tree')}</h3>
              <CanvasTree
                root={root}
                selectedId={selected.id}
                dropHint={dropHint}
                onSelect={setSelectedId}
                onDragOver={onDragOver}
                onDrop={onDrop}
              />
            </div>
          </TabsContent>
          <TabsContent
            value="canvas"
            forceMount
            className="min-w-0 data-[state=inactive]:hidden xl:data-[state=inactive]:block"
          >
            <ChatLayoutPreview
              root={root}
              selectedId={selected.id}
              mobile={mobile}
              dropHint={dropHint}
              onSelect={setSelectedId}
              onDragOver={onDragOver}
              onDrop={onDrop}
            />
          </TabsContent>
          <TabsContent
            value="style"
            forceMount
            className="min-w-0 rounded-xl border border-border p-3 data-[state=inactive]:hidden xl:data-[state=inactive]:block"
          >
            <ChatCanvasInspector
              root={root}
              node={selected}
              css={cssDrafts[selected.id] ?? selected.css}
              cssError={invalidIds.includes(selected.id) ? t('chat-layout:canvas.cssError') : ''}
              parents={parents}
              onCss={changeCss}
              onText={(text) => run(() => updateCanvasNode(root, selected.id, { text }))}
              onMove={move}
              onCopy={() => run(() => copyCanvasNode(root, selected.id))}
              onDelete={() => run(() => removeCanvasNode(root, selected.id))}
              onMoveTo={(parentId) =>
                run(() =>
                  moveCanvasNode(
                    root,
                    selected.id,
                    parentId,
                    findCanvasNode(root, parentId)?.children?.filter(
                      (node) => node.id !== selected.id,
                    ).length ?? 0,
                  ),
                )
              }
            />
          </TabsContent>
        </div>
      </Tabs>
    </div>
  )
}
