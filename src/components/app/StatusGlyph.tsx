// 状态字形：每种代理状态都用独立形状表达（形状 + 文字 + 颜色三重编码之一），
// 色盲或灰度下也能区分。颜色只用 currentColor，由调用方用 --st-* 状态 token 着色，
// 因此本组件不关心强调色，也不依赖任何 Feature。
import type { ComponentProps, ReactNode } from 'react'
import type { StatusGlyphKind } from '@/components/app/status-glyph-kinds'
import { cn } from '@/lib/utils'

// 24 视图框、1.75 描边，与 lucide 图标同一网格，混排时笔画粗细一致。
const GLYPH_PATHS: Record<StatusGlyphKind, ReactNode> = {
  // 运行中：缺口弧 + 实心中心点；只有弧会因旋转产生动感，中心点保持静止的视觉重心。
  run: (
    <>
      <path d="M12 3a9 9 0 1 0 9 9" />
      <circle cx="12" cy="12" r="3.2" fill="currentColor" stroke="none" />
    </>
  ),
  // 等待批准：盾牌 + 感叹号。
  approval: (
    <>
      <path d="M12 2.75 4.75 5.6v5.65c0 4.55 3.05 8.25 7.25 9.95 4.2-1.7 7.25-5.4 7.25-9.95V5.6Z" />
      <path d="M12 8v4.5" />
      <path d="M12 16.25h.01" />
    </>
  ),
  // 等你回答：带尾巴的对话气泡 + 问号。
  question: (
    <>
      <path d="M12 3a9 9 0 0 0-7.8 13.5L3 21l4.5-1.2A9 9 0 1 0 12 3Z" />
      <path d="M9.6 9.4a2.5 2.5 0 0 1 4.85.85c0 1.65-2.45 2.3-2.45 2.3" />
      <path d="M12 16h.01" />
    </>
  ),
  // 失败：八边形 + ×，与已完成的圆形轮廓明显不同。
  failed: (
    <>
      <path d="M8.2 2.8h7.6l5.4 5.4v7.6l-5.4 5.4H8.2l-5.4-5.4V8.2Z" />
      <path d="m9.25 9.25 5.5 5.5" />
      <path d="m14.75 9.25-5.5 5.5" />
    </>
  ),
  // 排队：虚线圆，8 段虚线均分周长（2π·9 ≈ 56.55）。
  queued: <circle cx="12" cy="12" r="9" strokeDasharray="2.9 4.17" />,
  // 已完成：圆 + 勾。
  done: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="m8.25 12.25 2.5 2.5 5-5.25" />
    </>
  ),
  // 已停止：实心方块。
  stopped: <rect x="5" y="5" width="14" height="14" rx="2.5" fill="currentColor" stroke="none" />,
}

export type StatusGlyphProps = Omit<ComponentProps<'svg'>, 'children'> & {
  status: StatusGlyphKind
  // 运行中默认旋转；静态快照（例如说明图例）可以传 false。
  spin?: boolean
  // 只有字形单独出现、旁边没有状态文字时才传入，供读屏读出；否则保持装饰性。
  label?: string
}

export function StatusGlyph({
  status,
  spin = status === 'run',
  label,
  className,
  ...props
}: StatusGlyphProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      data-status-glyph={status}
      {...(label ? { role: 'img', 'aria-label': label } : { 'aria-hidden': true })}
      focusable="false"
      className={cn(
        'size-4 shrink-0',
        // 只动 transform；用户选「减少动效」或系统要求减少动效（且未强制完整动效）时停转。
        spin &&
          'animate-[spin_1.1s_linear_infinite] [:root[data-motion=reduced]_&]:animate-none motion-reduce:[:root:not([data-motion=full])_&]:animate-none',
        className,
      )}
      {...props}
    >
      {GLYPH_PATHS[status]}
    </svg>
  )
}
