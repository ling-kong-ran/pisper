import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { Slot } from 'radix-ui'

import { cn } from '@/lib/utils'
import { OrbitSpinner } from '@/components/ui/spinner'

// Quiet Precision 按钮：主操作是唯一使用强调色的实心键（32px、r-sm、顶部 1px 内高光）；
// 次级为键帽（键帽面 + 强描边 + 键帽投影），幽灵按钮平铺。焦点由 base 层统一的
// 2px --focus 外描边提供，因此这里不再叠加 ring，避免与选中态混淆。
// 触屏（pointer: coarse）下常规尺寸统一抬到 44px：该媒体变体在生成的 CSS 中排在
// max-[650px]:* 之后，若这里低于 44px 会覆盖调用方的手机尺寸。xs/icon-xs/toolbar 是
// 紧凑内联尺寸，触屏热区由调用方按所在布局决定。
const buttonVariants = cva(
  "group/button inline-flex shrink-0 items-center justify-center rounded-sm border border-transparent bg-clip-padding text-sm font-medium whitespace-nowrap transition-colors duration-[var(--duration-fast,150ms)] select-none disabled:pointer-events-none disabled:opacity-50 aria-disabled:opacity-50 aria-invalid:border-destructive [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: 'bg-star text-on-accent shadow-primary hover:bg-star-hover',
        outline:
          'border-stroke-strong bg-key hover:bg-surface-hover hover:text-foreground aria-expanded:bg-surface-hover aria-expanded:text-foreground',
        secondary:
          'border-stroke-strong bg-key text-foreground shadow-key hover:bg-surface-subtle aria-expanded:bg-surface-hover',
        ghost:
          'text-content-secondary hover:bg-surface-hover hover:text-foreground aria-expanded:bg-surface-hover aria-expanded:text-foreground',
        destructive: 'bg-danger-soft text-danger hover:bg-danger-border',
        link: 'text-brand underline-offset-4 hover:underline',
      },
      size: {
        default: 'h-8 gap-1.5 px-3 pointer-coarse:min-h-11',
        toolbar: 'h-6 gap-1 px-2 text-xs',
        dialogAction: 'min-h-9 gap-2 px-4 py-2 leading-5 pointer-coarse:min-h-11',
        xs: "h-6 gap-1 px-2 text-xs has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3",
        sm: "h-7 pointer-coarse:h-11 gap-1 px-2.5 has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3.5",
        lg: 'h-9 pointer-coarse:h-11 gap-1.5 px-3 has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2',
        icon: 'size-7 pointer-coarse:size-11',
        'icon-xs': "size-6 [&_svg:not([class*='size-'])]:size-3",
        'icon-sm': 'size-7 pointer-coarse:size-11',
        'icon-lg': 'size-9 pointer-coarse:size-11',
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
        {loading && <OrbitSpinner className="absolute size-4 text-current" />}
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
