// 装饰动画组件：跟随鼠标的聚光卡片（卡片边缘高亮）。
import { useCallback, type ComponentProps, type PointerEvent } from 'react'
import { cn } from '@/lib/utils'
import './react-bits.css'

export function SpotlightCard({ className, onPointerMove, ...props }: ComponentProps<'div'>) {
  const moveSpotlight = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      const bounds = event.currentTarget.getBoundingClientRect()
      event.currentTarget.style.setProperty('--rb-spotlight-x', `${event.clientX - bounds.left}px`)
      event.currentTarget.style.setProperty('--rb-spotlight-y', `${event.clientY - bounds.top}px`)
      onPointerMove?.(event)
    },
    [onPointerMove],
  )

  return (
    <div className={cn('rb-spotlight-card', className)} onPointerMove={moveSpotlight} {...props} />
  )
}
