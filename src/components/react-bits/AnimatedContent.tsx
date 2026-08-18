// 装饰动画组件：内容切换时的过渡动画（framer-motion），
// 支持按方向淡入淡出/缩放，且尊重系统“减少动态效果”偏好。
import { AnimatePresence, motion, useReducedMotion } from 'motion/react'
import { useState, type ReactNode } from 'react'
import { cn } from '@/lib/utils'

export type AnimatedContentProps = {
  children: ReactNode
  className?: string
  show: boolean
  distance?: number
  direction?: 'vertical' | 'horizontal'
  reverse?: boolean
  duration?: number
  initialOpacity?: number
  animateOpacity?: boolean
  scale?: number
}

export function AnimatedContent({
  children,
  show,
  distance = 100,
  direction = 'vertical',
  reverse = false,
  duration = 0.8,
  initialOpacity = 0,
  animateOpacity = true,
  scale = 1,
  className,
}: AnimatedContentProps) {
  const reduceMotion = useReducedMotion()
  const [settled, setSettled] = useState(false)
  const axis = direction === 'horizontal' ? 'x' : 'y'
  const offset = reverse ? -distance : distance
  const hidden = reduceMotion
    ? { opacity: 1, width: direction === 'horizontal' ? 0 : 'auto' }
    : {
        [axis]: offset,
        opacity: animateOpacity ? initialOpacity : 1,
        scale,
        width: direction === 'horizontal' ? 0 : 'auto',
      }
  const visible = reduceMotion
    ? { opacity: 1, width: 'auto' }
    : { [axis]: 0, opacity: 1, scale: 1, width: 'auto' }

  return (
    <AnimatePresence>
      {show ? (
        <motion.div
          animate={visible}
          className={cn('rb-animated-content', className)}
          exit={hidden}
          initial={hidden}
          onAnimationComplete={() => setSettled(true)}
          onAnimationStart={() => setSettled(false)}
          style={{ overflow: settled ? 'visible' : 'hidden' }}
          transition={reduceMotion ? { duration: 0 } : { duration, ease: [0.22, 1, 0.36, 1] }}
        >
          {children}
        </motion.div>
      ) : null}
    </AnimatePresence>
  )
}
