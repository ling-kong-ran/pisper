'use client'

import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { Toast as ToastPrimitive } from 'radix-ui'
import { AlertTriangle, CheckCircle2, Info, X } from 'lucide-react'
import { cn } from '@/lib/utils'

const toastVariants = cva(
  'group pointer-events-auto relative flex min-h-11 w-full items-center gap-2 overflow-hidden rounded-surface border px-3.5 py-2.5 pr-10 text-[13px] leading-5 font-semibold tracking-[0] shadow-floating transition data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:slide-in-from-bottom-2 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:slide-out-to-right-full data-[swipe=cancel]:translate-x-0 data-[swipe=end]:translate-x-[var(--radix-toast-swipe-end-x)] data-[swipe=move]:translate-x-[var(--radix-toast-swipe-move-x)] data-[swipe=move]:transition-none',
  {
    variants: {
      tone: {
        success: 'border-success-border bg-success-soft text-success-strong',
        error: 'border-danger-border bg-danger-soft text-danger',
        info: 'border-border bg-surface-muted text-content-soft',
      },
    },
    defaultVariants: {
      tone: 'success',
    },
  },
)

export type ToastTone = NonNullable<VariantProps<typeof toastVariants>['tone']>

type ToastProps = React.ComponentProps<typeof ToastPrimitive.Root> &
  VariantProps<typeof toastVariants>

function Toast({ className, tone, ...props }: ToastProps) {
  return (
    <ToastPrimitive.Root
      data-slot="toast"
      className={cn(toastVariants({ tone }), className)}
      {...props}
    />
  )
}

function ToastProvider({ ...props }: React.ComponentProps<typeof ToastPrimitive.Provider>) {
  return <ToastPrimitive.Provider data-slot="toast-provider" {...props} />
}

function ToastViewport({
  className,
  ...props
}: React.ComponentProps<typeof ToastPrimitive.Viewport>) {
  return (
    <ToastPrimitive.Viewport
      data-slot="toast-viewport"
      className={cn(
        'fixed right-5 bottom-5 z-[80] flex w-[min(420px,calc(100vw-40px))] max-w-full flex-col gap-2 outline-none max-sm:right-3 max-sm:bottom-3 max-sm:left-3 max-sm:w-auto',
        className,
      )}
      {...props}
    />
  )
}

function ToastDescription({
  className,
  ...props
}: React.ComponentProps<typeof ToastPrimitive.Description>) {
  return (
    <ToastPrimitive.Description
      data-slot="toast-description"
      className={cn('min-w-0 flex-1 [overflow-wrap:anywhere]', className)}
      {...props}
    />
  )
}

function ToastClose({ className, ...props }: React.ComponentProps<typeof ToastPrimitive.Close>) {
  return (
    <ToastPrimitive.Close
      data-slot="toast-close"
      className={cn(
        'absolute top-1/2 right-1.5 grid size-7 -translate-y-1/2 place-items-center rounded-md text-current opacity-70 outline-none transition hover:bg-black/5 hover:opacity-100 focus-visible:ring-3 focus-visible:ring-ring/50 dark:hover:bg-white/10',
        className,
      )}
      {...props}
    />
  )
}

const TOAST_ICONS = {
  success: CheckCircle2,
  error: AlertTriangle,
  info: Info,
} as const

export function AppToast({
  message,
  tone,
  closeLabel,
  ...props
}: Omit<ToastProps, 'children' | 'tone'> & {
  message: React.ReactNode
  tone: ToastTone
  closeLabel: string
}) {
  const Icon = TOAST_ICONS[tone]
  return (
    <Toast tone={tone} type={tone === 'error' ? 'foreground' : 'background'} {...props}>
      <Icon className="size-[18px] shrink-0" />
      <ToastDescription>{message}</ToastDescription>
      <ToastClose aria-label={closeLabel} title={closeLabel}>
        <X className="size-4" />
      </ToastClose>
    </Toast>
  )
}

export { Toast, ToastClose, ToastDescription, ToastProvider, ToastViewport, toastVariants }
