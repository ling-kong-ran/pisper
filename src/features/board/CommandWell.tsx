// 审批命令：可换行，最多 3 行；内容超出时出现“展开”。文本始终完整，只是被裁在可视高度里（批准前绝不截断）。
// 先在空白处断行，单个词比一行还长时才拆词（overflow-wrap:anywhere），分支名与路径不会被拆在中间。
// well：手机卡片里的等宽框；plain：全部代理行的第 2 行（无框，规格 3.3）。
import { useLayoutEffect, useRef, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { cn } from '@/lib/utils'

export function CommandWell({
  text,
  prefix,
  phone = false,
  args = false,
  variant = 'well',
  tabIndex,
}: {
  text: string
  prefix: string
  phone?: boolean
  // 参数清单（没有命令和路径的审批）：读屏名与“展开”文案按参数说。
  args?: boolean
  variant?: 'well' | 'plain'
  // 全部代理行用 roving tabindex：行不是当前行时“展开”也退出 Tab 序列。
  tabIndex?: number
}) {
  const { t } = useI18n()
  const ref = useRef<HTMLPreElement>(null)
  const [overflowing, setOverflowing] = useState(false)
  const [expanded, setExpanded] = useState(false)
  useLayoutEffect(() => {
    const node = ref.current
    if (!node || expanded) return
    const measure = () => setOverflowing(node.scrollHeight > node.clientHeight + 1)
    measure()
    if (typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(measure)
    observer.observe(node)
    return () => observer.disconnect()
  }, [expanded, text])
  const plain = variant === 'plain'
  const toggle = (overflowing || expanded) && (
    <button
      type="button"
      data-card-stop
      tabIndex={tabIndex}
      className={cn(
        'text-left text-xs text-content-secondary hover:text-content focus-visible:outline-2 focus-visible:outline-focus pointer-coarse:min-h-11',
        plain
          ? 'mt-0.5 rounded-xs underline underline-offset-2'
          : 'w-full border-t border-well-stroke px-2.5 py-1',
      )}
      aria-expanded={expanded}
      onClick={() => setExpanded((value) => !value)}
    >
      {expanded
        ? t('board:card.collapse')
        : args
          ? t('board:card.expandArgs')
          : t('board:card.expand')}
    </button>
  )
  const pre = (
    <pre
      ref={ref}
      data-command-text
      aria-label={args ? t('board:confirm.fullArgs') : undefined}
      className={cn(
        'min-w-0 font-mono text-xs leading-5 whitespace-pre-wrap wrap-anywhere',
        plain ? 'text-content-secondary' : 'text-well-text',
        !expanded && 'line-clamp-3',
        phone && 'text-sm',
      )}
    >
      {prefix && (
        <span className={cn('select-none', plain ? 'text-content-muted' : 'text-well-muted')}>
          {prefix}{' '}
        </span>
      )}
      {text}
    </pre>
  )
  if (plain)
    return (
      <div className="flex min-w-0 flex-col items-start">
        {pre}
        {toggle}
      </div>
    )
  return (
    <div className="min-w-0 rounded-sm border border-well-stroke bg-well">
      {/* 内边距放在外层：折叠到 3 行时，第 4 行不会从内边距里露出半截。 */}
      <div className="px-2.5 py-1.5">{pre}</div>
      {toggle}
    </div>
  )
}
