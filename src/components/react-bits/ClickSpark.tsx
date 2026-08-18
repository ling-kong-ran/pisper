// 装饰动画组件：点击处迸发粒子火花。
import {
  useCallback,
  useState,
  type ComponentProps,
  type CSSProperties,
  type PointerEvent,
} from 'react'
import { cn } from '@/lib/utils'
import './react-bits.css'

type SparkBurst = { id: number; x: number; y: number }

export function ClickSpark({
  className,
  onPointerDownCapture,
  children,
  ...props
}: ComponentProps<'span'>) {
  const [burst, setBurst] = useState<SparkBurst | null>(null)

  const createSpark = useCallback(
    (event: PointerEvent<HTMLSpanElement>) => {
      const bounds = event.currentTarget.getBoundingClientRect()
      setBurst({ id: Date.now(), x: event.clientX - bounds.left, y: event.clientY - bounds.top })
      onPointerDownCapture?.(event)
    },
    [onPointerDownCapture],
  )

  return (
    <span className={cn('rb-click-spark', className)} onPointerDownCapture={createSpark} {...props}>
      {children}
      {burst && (
        <span
          aria-hidden="true"
          className="rb-click-spark-burst"
          key={burst.id}
          style={{ left: burst.x, top: burst.y }}
        >
          {Array.from({ length: 8 }, (_, index) => (
            <i key={index} style={{ '--rb-spark-angle': `${index * 45}deg` } as CSSProperties} />
          ))}
        </span>
      )}
    </span>
  )
}
