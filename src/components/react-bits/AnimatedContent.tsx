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
  collapse?: boolean
  reveal?: boolean
  spring?: boolean
  allowOverflow?: boolean
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
  collapse = false,
  reveal = false,
  spring = false,
  allowOverflow = false,
  className,
}: AnimatedContentProps) {
  const reduceMotion = useReducedMotion()
  const [settled, setSettled] = useState(false)
  const axis = direction === 'horizontal' ? 'x' : 'y'
  const offset = reverse ? -distance : distance
  const collapsedSize =
    direction === 'horizontal' ? { width: 0 } : collapse ? { height: 0 } : { height: 'auto' }
  const expandedSize =
    direction === 'horizontal'
      ? { width: 'auto' }
      : collapse
        ? { height: 'auto' }
        : { height: 'auto' }
  const revealHidden = reveal
    ? {
        ...(allowOverflow ? {} : { clipPath: 'inset(100% 0 0 0 round 10px)' }),
        filter: 'blur(9px)',
      }
    : {}
  const revealVisible = reveal
    ? { ...(allowOverflow ? {} : { clipPath: 'inset(0% 0 0 0 round 10px)' }), filter: 'blur(0px)' }
    : {}
  const hidden = reduceMotion
    ? { opacity: 1, ...collapsedSize }
    : {
        [axis]: offset,
        opacity: animateOpacity ? initialOpacity : 1,
        scale,
        ...collapsedSize,
        ...revealHidden,
      }
  const visible = reduceMotion
    ? { opacity: 1, ...expandedSize }
    : { [axis]: 0, opacity: 1, scale: 1, ...expandedSize, ...revealVisible }

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
          style={{ overflow: allowOverflow || settled ? 'visible' : 'hidden' }}
          transition={
            reduceMotion
              ? { duration: 0 }
              : spring
                ? { type: 'spring', stiffness: 430, damping: 26, mass: 0.72 }
                : { duration, ease: [0.22, 1, 0.36, 1] }
          }
        >
          {children}
        </motion.div>
      ) : null}
    </AnimatePresence>
  )
}
