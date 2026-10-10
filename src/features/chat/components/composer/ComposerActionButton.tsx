// 工具栏与收纳菜单共用同一个真实按钮，菜单标签也属于点击区域。
import type { ComponentProps } from 'react'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'

type ComposerActionButtonProps = Omit<ComponentProps<typeof Button>, 'variant' | 'size'> & {
  label: string
  menu?: boolean
}

export function ComposerActionButton({
  label,
  menu = false,
  children,
  className,
  ...props
}: ComposerActionButtonProps) {
  return (
    <Button
      type="button"
      variant="ghost"
      size={menu ? 'default' : 'icon'}
      aria-label={label}
      title={label}
      {...props}
      className={cn(
        'relative shrink-0 rounded-lg font-normal text-foreground [&>svg]:size-4',
        menu ? 'h-10 w-full justify-start gap-3 px-3 text-sm max-sm:h-11' : 'size-9 bg-transparent',
        className,
      )}
    >
      {children}
      {menu && <span className="min-w-0 flex-1 truncate text-left">{label}</span>}
    </Button>
  )
}
