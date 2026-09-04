// 快捷栏布局编辑器：用户可在输入框与收纳区之间移动工具，并调整各区顺序。
import { Archive, ArrowDown, ArrowUp, Pin, RotateCcw, SlidersHorizontal } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import {
  DEFAULT_COMPOSER_TOOLBAR_LAYOUT,
  type ComposerToolbarLayout,
  type ComposerToolId,
  type ComposerToolLocation,
} from '@/features/chat/composer-toolbar-layout'
import { useComposerToolbarStore } from '@/stores/composer-toolbar-store'

function sameLayout(left: ComposerToolbarLayout, right: ComposerToolbarLayout) {
  return (
    left.inline.join('\0') === right.inline.join('\0') &&
    left.overflow.join('\0') === right.overflow.join('\0')
  )
}

function ToolList({
  ids,
  location,
  labels,
  onMove,
  onReorder,
}: {
  ids: ComposerToolId[]
  location: ComposerToolLocation
  labels: Record<ComposerToolId, string>
  onMove: (id: ComposerToolId, location: ComposerToolLocation) => void
  onReorder: (id: ComposerToolId, direction: -1 | 1) => void
}) {
  const { t } = useI18n()
  const target = location === 'inline' ? 'overflow' : 'inline'
  const MoveIcon = location === 'inline' ? Archive : Pin
  const moveLabel =
    location === 'inline'
      ? t('chat:focusSession.moveShortcutToOverflow')
      : t('chat:focusSession.pinShortcutToComposer')

  return (
    <div className="divide-y divide-[var(--stroke-soft)] border-y border-[var(--stroke-soft)]">
      {ids.map((id, index) => (
        <div className="flex min-h-11 min-w-0 items-center gap-2 py-1.5" key={id}>
          <span className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[13px] font-semibold text-[var(--text)]">
            {labels[id]}
          </span>
          <div className="flex flex-none items-center gap-1">
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              title={t('chat:focusSession.moveShortcutEarlier')}
              aria-label={t('chat:focusSession.moveShortcutEarlierName', { name: labels[id] })}
              disabled={index === 0}
              onClick={() => onReorder(id, -1)}
            >
              <ArrowUp />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              title={t('chat:focusSession.moveShortcutLater')}
              aria-label={t('chat:focusSession.moveShortcutLaterName', { name: labels[id] })}
              disabled={index === ids.length - 1}
              onClick={() => onReorder(id, 1)}
            >
              <ArrowDown />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              title={moveLabel}
              aria-label={t('chat:focusSession.moveShortcutName', {
                name: labels[id],
                location: moveLabel,
              })}
              onClick={() => onMove(id, target)}
            >
              <MoveIcon />
            </Button>
          </div>
        </div>
      ))}
    </div>
  )
}

export function ComposerToolbarSettings({ labels }: { labels: Record<ComposerToolId, string> }) {
  const { t } = useI18n()
  const layout = useComposerToolbarStore((state) => state.layout)
  const setToolLocation = useComposerToolbarStore((state) => state.setToolLocation)
  const moveTool = useComposerToolbarStore((state) => state.moveTool)
  const resetLayout = useComposerToolbarStore((state) => state.resetLayout)

  return (
    <Dialog>
      <DialogTrigger asChild>
        <button
          type="button"
          className="grid size-9 min-w-9 place-items-center rounded-[var(--r-sm)] border border-transparent bg-[var(--surface-muted)] text-[var(--text-muted)] hover:border-[var(--accent-border)] hover:bg-[var(--accent-soft)] hover:text-[var(--star-strong)]"
          title={t('chat:focusSession.customizeShortcuts')}
          aria-label={t('chat:focusSession.customizeShortcuts')}
        >
          <SlidersHorizontal size={16} />
        </button>
      </DialogTrigger>
      <DialogContent
        className="gap-4 rounded-[var(--r-md)] p-4"
        style={{ width: 'min(640px, calc(100vw - 24px))', maxWidth: 640 }}
      >
        <DialogHeader className="pr-9">
          <DialogTitle>{t('chat:focusSession.customizeShortcuts')}</DialogTitle>
          <DialogDescription className="sr-only">
            {t('chat:focusSession.customizeShortcutsDescription')}
          </DialogDescription>
        </DialogHeader>
        <div className="grid min-w-0 grid-cols-2 gap-5 max-[650px]:grid-cols-1">
          <section className="min-w-0">
            <h3 className="mb-2 text-[12px] font-bold text-[var(--text-soft)]">
              {t('chat:focusSession.composerShortcuts')}
            </h3>
            <ToolList
              ids={layout.inline}
              location="inline"
              labels={labels}
              onMove={setToolLocation}
              onReorder={moveTool}
            />
          </section>
          <section className="min-w-0">
            <h3 className="mb-2 text-[12px] font-bold text-[var(--text-soft)]">
              {t('chat:focusSession.overflowShortcuts')}
            </h3>
            {layout.overflow.length ? (
              <ToolList
                ids={layout.overflow}
                location="overflow"
                labels={labels}
                onMove={setToolLocation}
                onReorder={moveTool}
              />
            ) : (
              <p className="border-y border-[var(--stroke-soft)] py-3 text-[12px] text-[var(--text-muted)]">
                {t('chat:focusSession.noOverflowShortcuts')}
              </p>
            )}
          </section>
        </div>
        <div className="flex justify-end border-t border-[var(--stroke-soft)] pt-3">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={sameLayout(layout, DEFAULT_COMPOSER_TOOLBAR_LAYOUT)}
            onClick={resetLayout}
          >
            <RotateCcw />
            {t('chat:focusSession.restoreShortcutDefaults')}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
