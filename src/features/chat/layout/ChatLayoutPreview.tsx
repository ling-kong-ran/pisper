import { ArrowUp, Check, FileText, MoreHorizontal, Plus } from 'lucide-react'
import type { DragEvent, KeyboardEvent, ReactNode } from 'react'
import { useI18n } from '@/app/use-i18n'
import { cn } from '@/lib/utils'
import type { ChatCanvasNode } from './chat-canvas'
import { isCanvasContainerKind } from './chat-canvas'
import { chatCanvasNodeStyle } from './chat-canvas-render'
import { CANVAS_NODE_DRAG, canvasKindLabels } from './chat-canvas-editor-model'

export type CanvasDropHandlers = {
  onDragOver: (event: DragEvent<HTMLElement>, node: ChatCanvasNode) => void
  onDrop: (event: DragEvent<HTMLElement>, node: ChatCanvasNode) => void
}

type PreviewProps = CanvasDropHandlers & {
  root: ChatCanvasNode
  selectedId: string
  mobile: boolean
  dropHint: string
  onSelect: (id: string) => void
}

function PreviewNode({
  node,
  rootId,
  selectedId,
  dropHint,
  labels,
  content,
  onSelect,
  onDragOver,
  onDrop,
}: {
  node: ChatCanvasNode
  rootId: string
  selectedId: string
  dropHint: string
  labels: Record<ChatCanvasNode['kind'], string>
  content: Partial<Record<ChatCanvasNode['kind'], ReactNode>>
  onSelect: (id: string) => void
} & Pick<PreviewProps, 'selectedId' | 'dropHint'> &
  CanvasDropHandlers) {
  const container = isCanvasContainerKind(node.kind)
  const selectWithKey = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault()
      event.stopPropagation()
      onSelect(node.id)
    }
  }
  return (
    <div
      role="button"
      tabIndex={0}
      aria-label={`${labels[node.kind]} · ${node.id}`}
      aria-pressed={node.id === selectedId}
      draggable={node.id !== rootId}
      onClick={(event) => {
        event.stopPropagation()
        onSelect(node.id)
      }}
      onKeyDown={selectWithKey}
      onDragStart={(event) => {
        event.stopPropagation()
        event.dataTransfer.setData(CANVAS_NODE_DRAG, node.id)
        event.dataTransfer.effectAllowed = 'move'
      }}
      onDragOver={(event) => onDragOver(event, node)}
      onDrop={(event) => onDrop(event, node)}
      data-canvas-node={node.id}
      className={cn(
        'relative min-h-5 min-w-0 cursor-pointer rounded-sm transition-shadow focus-visible:outline-2 focus-visible:outline-ring',
        container && 'border border-dashed border-foreground/15',
        dropHint.startsWith(`${node.id}:`) && 'ring-2 ring-primary',
      )}
      style={{
        ...chatCanvasNodeStyle(node, node.id === rootId),
        ...(node.id === selectedId
          ? { outline: '2px solid var(--ring)', outlineOffset: '-2px' }
          : {}),
      }}
    >
      {container ? (
        node.children?.length ? (
          node.children.map((child) => (
            <PreviewNode
              key={child.id}
              node={child}
              rootId={rootId}
              selectedId={selectedId}
              dropHint={dropHint}
              labels={labels}
              content={content}
              onSelect={onSelect}
              onDragOver={onDragOver}
              onDrop={onDrop}
            />
          ))
        ) : (
          <span className="p-3 text-xs text-muted-foreground">{labels[node.kind]} +</span>
        )
      ) : node.kind === 'text' ? (
        <span className="whitespace-pre-wrap [overflow-wrap:anywhere]">
          {node.text || labels.text}
        </span>
      ) : node.kind === 'divider' ? (
        <hr className="w-full border-current opacity-25" />
      ) : node.kind === 'spacer' ? (
        <div className="min-h-5" />
      ) : (
        content[node.kind]
      )}
    </div>
  )
}

export function ChatLayoutPreview({
  root,
  selectedId,
  mobile,
  dropHint,
  onSelect,
  onDragOver,
  onDrop,
}: PreviewProps) {
  const { t } = useI18n()
  const labels = canvasKindLabels(t)
  const content: Partial<Record<ChatCanvasNode['kind'], ReactNode>> = {
    header: (
      <div className="flex min-h-11 items-center justify-between gap-3 border-b border-border px-3 text-sm font-medium">
        <span>{t('chat-layout:layout.previewSession')}</span>
        <MoreHorizontal className="size-4 shrink-0" />
      </div>
    ),
    messages: (
      <div className="space-y-5 p-4 text-sm leading-7">
        <div className="ml-auto w-fit max-w-[90%] rounded-2xl bg-muted px-4 py-2">
          {t('chat-layout:layout.previewUser')}
        </div>
        <div>
          <p>{t('chat-layout:layout.previewAssistant')}</p>
          <div className="mt-3 flex items-center gap-2 text-xs text-muted-foreground">
            <Check className="size-3" />
            {t('chat-layout:layout.previewDone')}
          </div>
        </div>
      </div>
    ),
    composer: (
      <div className="rounded-xl border border-border bg-background p-3 text-sm">
        <p className="mb-7 text-muted-foreground">{t('chat-layout:layout.previewComposer')}</p>
        <div className="flex items-center justify-between">
          <Plus className="size-4" />
          <span className="grid size-7 place-items-center rounded-lg bg-primary text-primary-foreground">
            <ArrowUp className="size-4" />
          </span>
        </div>
      </div>
    ),
    model: (
      <span className="inline-flex rounded-lg border border-border px-3 py-2 text-xs">
        {t('chat-layout:canvas.model')}
      </span>
    ),
    tools: (
      <div className="flex gap-3 p-2">
        <Plus className="size-4" />
        <FileText className="size-4" />
        <MoreHorizontal className="size-4" />
      </div>
    ),
    usage: (
      <p className="px-2 py-1 text-xs text-muted-foreground">
        {t('chat-layout:layout.previewUsage')}
      </p>
    ),
    workspace: <p className="px-2 py-1 text-xs">{t('chat-layout:layout.previewProject')}</p>,
    context: (
      <div className="min-h-24 space-y-4 p-3 text-xs">
        <h4 className="font-medium">{t('chat-layout:layout.previewContext')}</h4>
        <div className="flex items-center gap-2">
          <FileText className="size-4" />
          {t('chat-layout:layout.previewFiles')}
        </div>
        <div className="h-1.5 w-4/5 rounded bg-foreground/10" />
      </div>
    ),
  }
  return (
    <figure className="m-0 min-w-0 space-y-3">
      <div className="max-h-[65dvh] min-h-80 min-w-0 overflow-auto rounded-xl border border-border bg-muted/30 p-3">
        <div
          className={cn(
            'h-[500px] min-h-[420px] min-w-0 bg-background p-2 text-foreground shadow-sm',
            mobile ? 'mx-auto max-w-[360px]' : 'w-full',
          )}
        >
          <PreviewNode
            node={root}
            rootId={root.id}
            selectedId={selectedId}
            dropHint={dropHint}
            labels={labels}
            content={content}
            onSelect={onSelect}
            onDragOver={onDragOver}
            onDrop={onDrop}
          />
        </div>
      </div>
      <figcaption className="text-xs leading-5 text-muted-foreground">
        {t('chat-layout:canvas.previewHint')}
      </figcaption>
    </figure>
  )
}
