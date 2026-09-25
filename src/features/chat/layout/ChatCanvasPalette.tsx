import { GripVertical, Plus, RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { customUiComponentLabel, useCustomUiComponents } from '@/features/custom-ui/catalog'
import {
  canvasHasKind,
  isCanvasContainerKind,
  type ChatCanvasKind,
  type ChatCanvasNode,
} from './chat-canvas'
import {
  CANVAS_CUSTOM_UI_DRAG,
  CANVAS_KIND_DRAG,
  canvasKindLabels,
} from './chat-canvas-editor-model'

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
  onAdd: (kind: ChatCanvasKind, componentId?: string) => void
}) {
  const { t } = useI18n()
  const catalog = useCustomUiComponents()
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
      <section className="space-y-2">
        <div className="flex items-center justify-between gap-2">
          <h4 className="text-xs font-medium text-muted-foreground">
            {t('chat-layout:canvas.customUi')}
          </h4>
          <Button
            variant="ghost"
            size="icon-sm"
            disabled={catalog.isFetching}
            onClick={() => void catalog.refetch()}
            aria-label={t('chat-layout:canvas.refreshWidgets')}
            title={t('chat-layout:canvas.refreshWidgets')}
          >
            <RefreshCw className="size-3.5" />
          </Button>
        </div>
        {catalog.isPending ? (
          <p role="status" className="text-xs text-muted-foreground">
            {t('custom-ui:customUiPage.loading')}
          </p>
        ) : catalog.error ? (
          <p role="alert" className="text-xs leading-5">
            {t('chat-layout:canvas.widgetsFailed')}
          </p>
        ) : !catalog.data?.components.length ? (
          <p className="text-xs leading-5 text-muted-foreground">
            {t('chat-layout:canvas.widgetsEmpty')}
          </p>
        ) : (
          <div className="space-y-1.5">
            {catalog.data.components.map((component) => (
              <Button
                key={component.id}
                variant="outline"
                className="w-full min-w-0 justify-start px-2 text-xs"
                draggable
                onDragStart={(event) => {
                  event.dataTransfer.setData(CANVAS_CUSTOM_UI_DRAG, component.id)
                  event.dataTransfer.effectAllowed = 'copy'
                }}
                onClick={() => onAdd('custom-ui', component.id)}
                title={t('chat-layout:canvas.addComponent', {
                  name: customUiComponentLabel(component, t),
                })}
              >
                <Plus className="size-3 shrink-0" />
                <span className="truncate">{customUiComponentLabel(component, t)}</span>
              </Button>
            ))}
          </div>
        )}
      </section>
      <p className="text-xs leading-5 text-muted-foreground">{t('chat-layout:canvas.addHint')}</p>
    </div>
  )
}
