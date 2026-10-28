'use client'

import * as React from 'react'
import { Dialog as DialogPrimitive } from 'radix-ui'

import { cn } from '@/lib/utils'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { XIcon } from 'lucide-react'

const DialogBusyContext = React.createContext(false)
const DialogPortalContainerContext = React.createContext<HTMLElement | null>(null)

function Dialog({
  busy = false,
  open,
  defaultOpen = false,
  onOpenChange,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Root> & { busy?: boolean }) {
  const [localOpen, setLocalOpen] = React.useState(defaultOpen)
  return (
    <DialogBusyContext.Provider value={busy}>
      <DialogPrimitive.Root
        data-slot="dialog"
        open={open ?? localOpen}
        onOpenChange={(nextOpen) => {
          if (!nextOpen && busy) return
          if (open === undefined) setLocalOpen(nextOpen)
          onOpenChange?.(nextOpen)
        }}
        {...props}
      />
    </DialogBusyContext.Provider>
  )
}

function DialogTrigger({ ...props }: React.ComponentProps<typeof DialogPrimitive.Trigger>) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />
}

function DialogPortal({ ...props }: React.ComponentProps<typeof DialogPrimitive.Portal>) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />
}

function DialogClose({ ...props }: React.ComponentProps<typeof DialogPrimitive.Close>) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />
}

function DialogOverlay({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Overlay>) {
  return (
    <DialogPrimitive.Overlay
      data-slot="dialog-overlay"
      className={cn(
        'fixed inset-0 isolate z-50 bg-[var(--overlay-scrim,rgba(0,0,0,0.3))] duration-[var(--duration-fast,150ms)] data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0',
        className,
      )}
      {...props}
    />
  )
}

function DialogContent({
  className,
  overlayClassName,
  children,
  showCloseButton = true,
  size = 'default',
  surface = 'default',
  scrollMode = 'content',
  ref,
  onEscapeKeyDown,
  onPointerDownOutside,
  onInteractOutside,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content> & {
  showCloseButton?: boolean
  overlayClassName?: string
  size?: 'default' | 'compact' | 'wide'
  surface?: 'default' | 'confirmation'
  scrollMode?: 'content' | 'body'
}) {
  const { t } = useI18n()
  const busy = React.useContext(DialogBusyContext)
  const [container, setContainer] = React.useState<HTMLDivElement | null>(null)
  const contentRef = React.useCallback(
    (element: HTMLDivElement | null) => {
      setContainer(element)
      if (typeof ref === 'function') return ref(element)
      if (ref) ref.current = element
    },
    [ref],
  )
  return (
    <DialogPortal>
      <DialogOverlay className={overlayClassName} />
      <DialogPrimitive.Content
        {...props}
        ref={contentRef}
        data-slot="dialog-content"
        data-surface={surface}
        data-scroll-mode={scrollMode}
        aria-busy={busy || props['aria-busy'] || undefined}
        onEscapeKeyDown={(event) => {
          onEscapeKeyDown?.(event)
          if (busy) event.preventDefault()
        }}
        onPointerDownOutside={(event) => {
          onPointerDownOutside?.(event)
          if (busy) event.preventDefault()
        }}
        onInteractOutside={(event) => {
          onInteractOutside?.(event)
          if (busy) event.preventDefault()
        }}
        className={cn(
          'fixed top-1/2 left-1/2 z-50 grid max-h-[calc(100vh-2rem)] supports-[height:100dvh]:max-h-[calc(100dvh-2rem)] w-full -translate-x-1/2 -translate-y-1/2 gap-4 overflow-y-auto overscroll-contain rounded-2xl bg-popover p-4 text-sm text-popover-foreground ring-1 ring-[var(--stroke-soft)] duration-[var(--duration-fast,150ms)] outline-none data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95',
          size === 'default' && 'max-w-[min(92vw,520px)]',
          size === 'compact' && 'max-w-[min(92vw,430px)]',
          size === 'wide' && 'max-w-[min(92vw,600px)]',
          surface === 'confirmation' && 'border border-surface-highlight shadow-dialog ring-0',
          scrollMode === 'body' && 'flex flex-col overflow-hidden',
          className,
        )}
      >
        <DialogPortalContainerContext.Provider value={container}>
          {children}
          {showCloseButton && (
            <DialogPrimitive.Close data-slot="dialog-close" asChild>
              <Button
                variant="ghost"
                className="absolute top-2 right-2"
                size="icon-sm"
                disabled={busy}
              >
                <XIcon />
                <span className="sr-only">{t('common:ui.closeDialog')}</span>
              </Button>
            </DialogPrimitive.Close>
          )}
        </DialogPortalContainerContext.Provider>
      </DialogPrimitive.Content>
    </DialogPortal>
  )
}

function DialogBody({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="dialog-body"
      className={cn('min-h-0 flex-1 overflow-y-auto overscroll-contain', className)}
      {...props}
    />
  )
}

function DialogHeader({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="dialog-header"
      className={cn('flex shrink-0 flex-col gap-2', className)}
      {...props}
    />
  )
}

function DialogFooter({
  className,
  showCloseButton = false,
  children,
  ...props
}: React.ComponentProps<'div'> & {
  showCloseButton?: boolean
}) {
  const { t } = useI18n()
  return (
    <div
      data-slot="dialog-footer"
      className={cn(
        '-mx-4 -mb-4 flex shrink-0 flex-col-reverse gap-2 rounded-b-xl border-t bg-[var(--surface-subtle)] p-4 sm:flex-row sm:justify-end',
        className,
      )}
      {...props}
    >
      {children}
      {showCloseButton && (
        <DialogPrimitive.Close asChild>
          <Button variant="outline" size="dialogAction">
            {t('common:ui.closeDialog')}
          </Button>
        </DialogPrimitive.Close>
      )}
    </div>
  )
}

function DialogTitle({ className, ...props }: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn('font-heading text-base leading-none font-medium', className)}
      {...props}
    />
  )
}

function DialogDescription({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn(
        'text-sm text-muted-foreground *:[a]:underline *:[a]:underline-offset-3 *:[a]:hover:text-foreground',
        className,
      )}
      {...props}
    />
  )
}

export {
  Dialog,
  DialogClose,
  DialogBody,
  DialogPortalContainerContext,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
}
