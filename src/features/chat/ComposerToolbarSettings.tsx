// 快捷栏布局编辑器：用户可在输入框与收纳区之间移动工具，并调整各区顺序。
import { useLayoutEffect, useRef } from 'react'
import { Archive, ArrowDown, ArrowUp, Pin, RotateCcw, SlidersHorizontal, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog'
import { cn } from '@/lib/utils'
import {
  DEFAULT_COMPOSER_TOOLBAR_LAYOUT,
  type ComposerToolbarLayout,
  type ComposerToolId,
  type ComposerToolLocation,
} from '@/features/chat/composer-toolbar-layout'
import { useComposerToolbarStore } from '@/features/chat/composer-toolbar-store'

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
  onMoveButtonRef,
}: {
  ids: ComposerToolId[]
  location: ComposerToolLocation
  labels: Record<ComposerToolId, string>
  onMove: (id: ComposerToolId, location: ComposerToolLocation) => void
  onReorder: (id: ComposerToolId, direction: -1 | 1) => void
  onMoveButtonRef: (id: ComposerToolId, button: HTMLButtonElement | null) => void
}) {
  const { t } = useI18n()
  const target = location === 'inline' ? 'overflow' : 'inline'
  const MoveIcon = location === 'inline' ? Archive : Pin
  const moveLabel =
    location === 'inline'
      ? t('chat:focusSession.moveShortcutToOverflow')
      : t('chat:focusSession.pinShortcutToComposer')

  return (
    <ul className="divide-y divide-[var(--stroke-soft)]">
      {ids.map((id, index) => (
        <li className="flex min-h-12 min-w-0 items-center gap-1 py-0.5" key={id}>
          <span className="min-w-0 flex-1 px-1 text-xs leading-5 text-[var(--text)]">
            {labels[id]}
          </span>
          <div className="flex flex-none items-center">
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              className="size-11 text-[var(--text-muted)]"
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
              className="size-11 text-[var(--text-muted)]"
              title={t('chat:focusSession.moveShortcutLater')}
              aria-label={t('chat:focusSession.moveShortcutLaterName', { name: labels[id] })}
              disabled={index === ids.length - 1}
              onClick={() => onReorder(id, 1)}
            >
              <ArrowDown />
            </Button>
            <Button
              ref={(button) => onMoveButtonRef(id, button)}
              type="button"
              variant="ghost"
              size="sm"
              className="h-11 min-w-11 gap-1 px-2 text-xs"
              title={moveLabel}
              aria-label={t('chat:focusSession.moveShortcutName', {
                name: labels[id],
                location: moveLabel,
              })}
              onClick={() => onMove(id, target)}
            >
              <MoveIcon />
              {location === 'inline'
                ? t('chat:focusSession.storeShortcut')
                : t('chat:focusSession.showShortcut')}
            </Button>
          </div>
        </li>
      ))}
    </ul>
  )
}

export function ComposerToolbarSettings({
  labels,
  labeled = false,
}: {
  labels: Record<ComposerToolId, string>
  labeled?: boolean
}) {
  const { t } = useI18n()
  const layout = useComposerToolbarStore((state) => state.layout)
  const setToolLocation = useComposerToolbarStore((state) => state.setToolLocation)
  const setAllToolsLocation = useComposerToolbarStore((state) => state.setAllToolsLocation)
  const moveTool = useComposerToolbarStore((state) => state.moveTool)
  const resetLayout = useComposerToolbarStore((state) => state.resetLayout)
  const moveButtons = useRef(new Map<ComposerToolId, HTMLButtonElement>())
  const movedTool = useRef<ComposerToolId | null>(null)

  // 跨区移动会重挂载行，把焦点留给刚移动的按钮，键盘用户可继续调整。
  useLayoutEffect(() => {
    if (!movedTool.current) return
    moveButtons.current.get(movedTool.current)?.focus()
    movedTool.current = null
  }, [layout])

  const handleMove = (id: ComposerToolId, location: ComposerToolLocation) => {
    movedTool.current = id
    setToolLocation(id, location)
  }
  const setMoveButtonRef = (id: ComposerToolId, button: HTMLButtonElement | null) => {
    if (button) moveButtons.current.set(id, button)
    else moveButtons.current.delete(id)
  }

  return (
    <Dialog>
      <DialogTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          className={cn(
            'h-11 gap-2 text-[var(--text-muted)]',
            labeled ? 'w-full justify-start px-2 text-xs' : 'w-11 px-0',
          )}
          title={t('chat:focusSession.customizeShortcuts')}
          aria-label={t('chat:focusSession.customizeShortcuts')}
        >
          <SlidersHorizontal />
          {labeled && <span>{t('chat:focusSession.customizeShortcuts')}</span>}
        </Button>
      </DialogTrigger>
      <DialogContent
        showCloseButton={false}
        overlayClassName="z-[70]"
        className="z-[71] flex max-h-[calc(100dvh-24px)] w-[calc(100vw-24px)] max-w-[720px] flex-col gap-0 overflow-hidden rounded-xl p-0 sm:max-w-[720px]"
      >
        <div className="flex flex-none items-start gap-3 border-b border-[var(--stroke-soft)] p-4">
          <DialogHeader className="min-w-0 flex-1 gap-2 pt-1">
            <DialogTitle>{t('chat:focusSession.customizeShortcuts')}</DialogTitle>
            <DialogDescription className="text-xs leading-5">
              {t('chat:focusSession.customizeShortcutsDescription')}
            </DialogDescription>
          </DialogHeader>
          <DialogClose asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              className="size-11"
              aria-label={t('common:ui.closeDialog')}
            >
              <X />
            </Button>
          </DialogClose>
        </div>
        <div className="grid min-h-0 min-w-0 grid-cols-1 gap-5 overflow-y-auto overscroll-contain p-4 sm:grid-cols-2">
          <section className="min-w-0" aria-label={t('chat:focusSession.composerShortcuts')}>
            <h3 className="mb-1 flex h-8 items-center gap-2 text-xs font-medium text-[var(--text-secondary)]">
              <Pin size={14} />
              {t('chat:focusSession.composerShortcuts')}
              <span className="text-[var(--text-muted)]">{layout.inline.length}</span>
            </h3>
            {layout.inline.length ? (
              <ToolList
                ids={layout.inline}
                location="inline"
                labels={labels}
                onMove={handleMove}
                onReorder={moveTool}
                onMoveButtonRef={setMoveButtonRef}
              />
            ) : (
              <p className="py-4 text-xs leading-5 text-[var(--text-muted)]">
                {t('chat:focusSession.noInlineShortcuts')}
              </p>
            )}
          </section>
          <section className="min-w-0" aria-label={t('chat:focusSession.overflowShortcuts')}>
            <h3 className="mb-1 flex h-8 items-center gap-2 text-xs font-medium text-[var(--text-secondary)]">
              <Archive size={14} />
              {t('chat:focusSession.overflowShortcuts')}
              <span className="text-[var(--text-muted)]">{layout.overflow.length}</span>
            </h3>
            {layout.overflow.length ? (
              <ToolList
                ids={layout.overflow}
                location="overflow"
                labels={labels}
                onMove={handleMove}
                onReorder={moveTool}
                onMoveButtonRef={setMoveButtonRef}
              />
            ) : (
              <p className="py-4 text-xs leading-5 text-[var(--text-muted)]">
                {t('chat:focusSession.noOverflowShortcuts')}
              </p>
            )}
          </section>
        </div>
        <div className="flex flex-none flex-wrap items-center gap-x-1 border-t border-[var(--stroke-soft)] px-3 py-2">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-11 text-xs"
            disabled={layout.overflow.length === 0}
            onClick={() => setAllToolsLocation('inline')}
          >
            <Pin />
            {t('chat:focusSession.showAllShortcuts')}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-11 text-xs"
            disabled={layout.inline.length === 0}
            onClick={() => setAllToolsLocation('overflow')}
          >
            <Archive />
            {t('chat:focusSession.storeAllShortcuts')}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="ml-auto h-11 text-xs text-[var(--text-muted)]"
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
