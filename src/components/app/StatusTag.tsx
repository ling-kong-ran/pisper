// 状态标签：22px 浅底标签 = 状态字形 + 调用方传入的文字，颜色只取 --st-* 状态族，
// 与强调色无关。文案由调用方经 i18n 提供，本组件不内置任何业务词。
import type { ComponentProps } from 'react'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import {
  STATUS_GLYPH_TONES,
  type StatusGlyphKind,
  type StatusTone,
} from '@/components/app/status-glyph-kinds'
import { cn } from '@/lib/utils'

// 类名写成完整字面量，保证 Tailwind 能静态扫描到。
const STATUS_TAG_TONES: Record<StatusTone, string> = {
  run: 'border-st-run-border bg-st-run-soft text-st-run',
  attn: 'border-st-attn-border bg-st-attn-soft text-st-attn',
  fail: 'border-st-fail-border bg-st-fail-soft text-st-fail',
  done: 'border-st-done-border bg-st-done-soft text-st-done',
  queue: 'border-st-queue-border bg-st-queue-soft text-st-queue',
}

export type StatusTagProps = ComponentProps<'span'> & {
  status: StatusGlyphKind
  // 少数状态需要换色而不换形状（例如已处理的失败仍是八边形），默认按状态推导。
  tone?: StatusTone
  spin?: boolean
}

export function StatusTag({ status, tone, spin, className, children, ...props }: StatusTagProps) {
  const resolvedTone = tone ?? STATUS_GLYPH_TONES[status]
  return (
    <span
      data-status={status}
      data-tone={resolvedTone}
      className={cn(
        'inline-flex h-[22px] w-fit max-w-full shrink-0 items-center gap-[5px] rounded-xs border pr-2 pl-1.5 text-xs leading-none font-medium whitespace-nowrap',
        STATUS_TAG_TONES[resolvedTone],
        className,
      )}
      {...props}
    >
      <StatusGlyph status={status} spin={spin} className="size-3.5" />
      <span className="min-w-0 truncate">{children}</span>
    </span>
  )
}
