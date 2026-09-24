import { GripVertical, Plus } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  canvasHasKind,
  isCanvasContainerKind,
  type ChatCanvasKind,
  type ChatCanvasNode,
} from './chat-canvas'
import { CANVAS_KIND_DRAG, canvasKindLabels } from './chat-canvas-editor-model'

const groups: ChatCanvasKind[][] = [
  ['row', 'column', 'grid'],
  ['header', 'messages', 'composer', 'model', 'tools', 'usage', 'workspace', 'context'],
  ['text', 'divider', 'spacer'],
]

export function ChatCanvasPalette({
  root,
  onAdd,
}: {
  root: ChatCanvasNode
  onAdd: (kind: ChatCanvasKind) => void
}) {
  const { t } = useI18n()
  const labels = canvasKindLabels(t)
  const titles = [
    t('chat-layout:canvas.containers'),
    t('chat-layout:canvas.functional'),
    t('chat-layout:canvas.decoration'),
  ]
  return (
    <div className="space-y-4">
      {groups.map((group, index) => (
        <section key={index} className="space-y-2">
          <h4 className="text-xs font-medium text-muted-foreground">{titles[index]}</h4>
          <div className="grid grid-cols-2 gap-1.5">
            {group.map((kind) => {
              const singleton =
                !isCanvasContainerKind(kind) && !['text', 'divider', 'spacer'].includes(kind)
              const disabled = singleton && canvasHasKind(root, kind)
              return (
                <Button
                  key={kind}
                  variant="outline"
                  className="min-w-0 justify-start px-2 text-xs"
                  disabled={disabled}
                  draggable={!disabled}
                  onDragStart={(event) => {
                    event.dataTransfer.setData(CANVAS_KIND_DRAG, kind)
                    event.dataTransfer.effectAllowed = 'copy'
                  }}
                  onClick={() => onAdd(kind)}
                  title={
                    disabled
                      ? t('chat-layout:canvas.alreadyAdded')
                      : t('chat-layout:canvas.addComponent', { name: labels[kind] })
                  }
                >
                  {isCanvasContainerKind(kind) ? (
                    <GripVertical className="size-3" />
                  ) : (
                    <Plus className="size-3" />
                  )}
                  {labels[kind]}
                </Button>
              )
            })}
          </div>
        </section>
      ))}
      <p className="text-xs leading-5 text-muted-foreground">{t('chat-layout:canvas.addHint')}</p>
    </div>
  )
}
