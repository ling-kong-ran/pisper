// 装饰动画组件：列表项依次滑入的动画列表。
import { AnimatePresence, motion } from 'motion/react'
import { Children, type ReactNode } from 'react'
import { cn } from '@/lib/utils'
import './react-bits.css'

export function AnimatedList({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <AnimatePresence initial={false} mode="popLayout">
      {Children.toArray(children).map((child, index) => (
        <motion.div
          animate={{ opacity: 1, y: 0 }}
          className={cn('rb-animated-list-item', className)}
          exit={{ opacity: 0, y: -4 }}
          initial={{ opacity: 0, y: 6 }}
          key={(child as { key?: string | number | null }).key ?? index}
          layout="position"
          transition={{ duration: 0.18, ease: [0.22, 1, 0.36, 1] }}
        >
          {child}
        </motion.div>
      ))}
    </AnimatePresence>
  )
}
