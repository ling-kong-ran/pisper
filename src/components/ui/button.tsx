import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { Slot } from 'radix-ui'
import { LoaderCircle } from 'lucide-react'

import { cn } from '@/lib/utils'

const buttonVariants = cva(
  "group/button inline-flex shrink-0 items-center justify-center rounded-lg border border-transparent bg-clip-padding text-sm font-medium whitespace-nowrap transition-colors duration-[var(--duration-fast,150ms)] outline-none select-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 active:brightness-95 disabled:pointer-events-none disabled:opacity-50 aria-disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 dark:aria-invalid:border-destructive/50 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: 'bg-primary text-primary-foreground hover:bg-primary-hover',
        outline:
          'border-border bg-background hover:bg-muted hover:text-foreground aria-expanded:bg-muted aria-expanded:text-foreground dark:border-input dark:bg-input/30 dark:hover:bg-input/50',
        secondary:
          'bg-secondary text-secondary-foreground hover:bg-[color-mix(in_oklch,var(--secondary),var(--foreground)_5%)] aria-expanded:bg-secondary aria-expanded:text-secondary-foreground',
        ghost:
          'hover:bg-muted hover:text-foreground aria-expanded:bg-muted aria-expanded:text-foreground dark:hover:bg-muted/50',
        destructive:
          'bg-danger-soft text-danger hover:bg-danger-border focus-visible:border-danger-border focus-visible:ring-danger-border',
        link: 'text-brand underline-offset-4 hover:underline',
      },
      size: {
        default: 'gap-1.5 px-2 py-0.5 leading-[18px] pointer-coarse:min-h-10',
        toolbar: 'h-6 gap-1 px-2 text-xs',
        dialogAction: 'min-h-9 gap-2 px-4 py-2 leading-5',
        xs: "h-6 gap-1 rounded-[min(var(--radius-md),10px)] px-2 text-xs in-data-[slot=button-group]:rounded-lg has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3",
        sm: "h-7 pointer-coarse:h-9 gap-1 rounded-[min(var(--radius-md),12px)] px-2.5 text-[0.8rem] in-data-[slot=button-group]:rounded-lg has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3.5",
        lg: 'h-9 pointer-coarse:h-10 gap-1.5 px-2.5 has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2',
        icon: 'size-8 pointer-coarse:size-10',
        'icon-xs':
          "size-6 rounded-[min(var(--radius-md),10px)] in-data-[slot=button-group]:rounded-lg [&_svg:not([class*='size-'])]:size-3",
        'icon-sm':
          'size-7 pointer-coarse:size-9 rounded-[min(var(--radius-md),12px)] in-data-[slot=button-group]:rounded-lg',
        'icon-lg': 'size-9',
      },
    },
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  },
)

type ButtonProps = React.ComponentProps<'button'> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
    loading?: boolean
    sizingLabels?: readonly React.ReactNode[]
    disabledMode?: 'native' | 'focusable'
  }

function Button({
  className,
  variant = 'default',
  size = 'default',
  asChild = false,
  loading = false,
  sizingLabels,
  disabledMode = 'native',
  disabled = false,
  type = 'button',
  children,
  onClick,
  onAuxClick,
  onKeyDown,
  ...props
}: ButtonProps) {
  const child =
    asChild &&
    React.isValidElement<
      React.HTMLAttributes<HTMLElement> & { href?: string; disabled?: boolean; type?: string }
    >(children)
      ? children
      : null
  const nativeChild = child?.type === 'button'
  const blocked = Boolean(
    disabled ||
    loading ||
    props['aria-disabled'] === true ||
    props['aria-disabled'] === 'true' ||
    child?.props['aria-disabled'] === true ||
    child?.props['aria-disabled'] === 'true' ||
    child?.props.disabled,
  )
  const preventActivation = (event: React.SyntheticEvent) => {
    event.preventDefault()
    event.stopPropagation()
  }
  const label = (content: React.ReactNode) =>
    loading || sizingLabels?.length ? (
      <span className="relative inline-grid place-items-center gap-[inherit]">
        <span
          className={cn(
            'col-start-1 row-start-1 inline-flex items-center justify-center gap-[inherit]',
            loading && 'opacity-0',
          )}
        >
          {content}
        </span>
        {sizingLabels?.map((text, index) => (
          <span key={index} aria-hidden="true" className="invisible col-start-1 row-start-1">
            {text}
          </span>
        ))}
        {loading && <LoaderCircle aria-hidden="true" className="absolute size-4 animate-spin" />}
      </span>
    ) : (
      content
    )
  const Comp = asChild ? Slot.Root : 'button'

  return (
    <Comp
      {...props}
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...(!asChild ? { type, disabled: blocked && disabledMode === 'native' } : {})}
      aria-busy={loading || props['aria-busy'] || undefined}
      aria-disabled={blocked || undefined}
      tabIndex={asChild && !nativeChild && blocked ? -1 : props.tabIndex}
      onClick={(event) => {
        if (blocked) return preventActivation(event)
        onClick?.(event)
      }}
      onAuxClick={(event) => {
        if (blocked) return preventActivation(event)
        onAuxClick?.(event)
      }}
      onKeyDown={(event) => {
        if (blocked && (event.key === 'Enter' || event.key === ' ')) return preventActivation(event)
        onKeyDown?.(event)
      }}
    >
      {child
        ? React.cloneElement(child, {
            // Slot 会先调用子元素事件；在这里阻止链接自身及组合层重复激活。
            ...(nativeChild
              ? { type: child.props.type ?? type, disabled: blocked && disabledMode === 'native' }
              : blocked
                ? { href: undefined, disabled: undefined, type: undefined, tabIndex: -1 }
                : {}),
            children: label(child.props.children),
            onClick: (event) => {
              if (blocked) return preventActivation(event)
              child.props.onClick?.(event)
            },
            onAuxClick: (event) => {
              if (blocked) return preventActivation(event)
              child.props.onAuxClick?.(event)
            },
            onKeyDown: (event) => {
              if (blocked && (event.key === 'Enter' || event.key === ' '))
                return preventActivation(event)
              child.props.onKeyDown?.(event)
            },
          })
        : label(children)}
    </Comp>
  )
}

export { Button, buttonVariants }
export type { ButtonProps }
