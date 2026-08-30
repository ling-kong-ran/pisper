// 装饰动画组件：列表项依次滑入；burst 变体用于工具项从锚点连续弹出。
import { AnimatePresence, motion, useReducedMotion } from 'motion/react'
import { Children, Fragment, isValidElement, type ReactNode } from 'react'
import { cn } from '@/lib/utils'
import './react-bits.css'

type AnimatedListProps = {
  children: ReactNode
  className?: string
  animateInitial?: boolean
  variant?: 'list' | 'burst'
  delegateClicks?: boolean
}

type AnimatedListItem = {
  child: ReactNode
  key: string
}

// Fragment 不产生 DOM；递归展开并保留父路径，避免不同层级的自动 key 相互冲突。
function flattenedChildren(children: ReactNode, parentKey = 'root'): AnimatedListItem[] {
  return Children.toArray(children).flatMap((child, index) => {
    const childKey = `${parentKey}/${index}:${
      isValidElement(child) && child.key != null ? String(child.key) : 'item'
    }`
    return isValidElement<{ children?: ReactNode }>(child) && child.type === Fragment
      ? flattenedChildren(child.props.children, childKey)
      : [{ child, key: childKey }]
  })
}

export function AnimatedList({
  children,
  className,
  animateInitial = false,
  variant = 'list',
  delegateClicks = false,
}: AnimatedListProps) {
  const reduceMotion = useReducedMotion()
  const burst = variant === 'burst'

  return (
    <AnimatePresence initial={animateInitial && !reduceMotion} mode="popLayout">
      {flattenedChildren(children).map(({ child, key }, index) => (
        <motion.div
          animate={{ opacity: 1, x: 0, y: 0, scale: 1, rotate: 0 }}
          className={cn('rb-animated-list-item', className)}
          exit={
            reduceMotion
              ? { opacity: 1 }
              : burst
                ? { opacity: 0, x: -8, y: 12, scale: 0.62, rotate: -7 }
                : { opacity: 0, y: -4 }
          }
          initial={
            reduceMotion
              ? false
              : burst
                ? { opacity: 0, x: -18, y: 18, scale: 0.42, rotate: -12 }
                : { opacity: 0, y: 6 }
          }
          key={key}
          layout="position"
          onClick={
            delegateClicks
              ? (event) => {
                  const target = event.target
                  const itemRoot = event.currentTarget.firstElementChild
                  if (
                    !(target instanceof Element) ||
                    (target !== event.currentTarget && target !== itemRoot)
                  ) {
                    return
                  }
                  // 视觉瓦片大于部分旧控件的按钮本体，空白区域仍应触发主操作。
                  event.currentTarget
                    .querySelector<HTMLElement>(
                      'button:not(:disabled), input:not(:disabled), select:not(:disabled), [role="button"]',
                    )
                    ?.click()
                }
              : undefined
          }
          transition={
            burst
              ? {
                  type: 'spring',
                  stiffness: 520,
                  damping: 19,
                  mass: 0.58,
                  delay: reduceMotion ? 0 : 0.045 + index * 0.038,
                }
              : { duration: reduceMotion ? 0 : 0.18, ease: [0.22, 1, 0.36, 1] }
          }
        >
          {child}
        </motion.div>
      ))}
    </AnimatePresence>
  )
}
