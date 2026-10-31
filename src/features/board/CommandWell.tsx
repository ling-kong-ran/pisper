// 审批命令：可换行，最多 3 行；内容超出时出现“展开”。文本始终完整，只是被裁在可视高度里（批准前绝不截断）。
// 只在空白处断行：浏览器默认会在连字符后断行，把 `--coverage.include` 拆成“--”与“coverage.include”、
// `-n1` 拆成“-”与“n1”，读起来像两个参数，审批人会误读。所以每个非空白片段包成一个 inline-block
// （原子行内框，内部不参与外层断行）；只有单个片段比一行还宽时，它在自己的框里任意位置断开（break-all）。
// 不插入软连字符或 U+2011，复制出的文本与原命令一致。
// well：手机卡片里的等宽框；plain：全部代理行的第 2 行（无框，规格 3.3）。
import { useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useI18n } from '@/app/use-i18n'
import { cn } from '@/lib/utils'

export function CommandWell({
  text,
  prefix,
  phone = false,
  args = false,
  variant = 'well',
  tabIndex,
  lead,
}: {
  text: string
  prefix: string
  // 命令前的说明（全部代理高风险行的“高风险 · 改写远端分支”），不随命令一起被选中复制。
  lead?: ReactNode
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
  const parts = useMemo(() => text.split(/(\s+)/).filter(Boolean), [text])
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
        'min-w-0 font-mono text-xs leading-5 whitespace-pre-wrap',
        plain ? 'text-content-secondary' : 'text-well-text',
        // max-h 兜底：单个超长片段是一个多行 inline-block，line-clamp 只按行框数，管不住它。
        !expanded && 'line-clamp-3 max-h-15',
        phone && 'text-sm',
      )}
    >
      {lead && <span className="font-sans select-none">{lead}</span>}
      {prefix && (
        <span className={cn('select-none', plain ? 'text-content-muted' : 'text-well-muted')}>
          {prefix}{' '}
        </span>
      )}
      {parts.map((part, index) =>
        /^\s+$/.test(part) ? (
          part
        ) : (
          <span
            key={index}
            data-command-token
            className="inline-block max-w-full align-top break-all"
          >
            {part}
          </span>
        ),
      )}
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
