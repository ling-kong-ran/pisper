// 状态字形（规格 v2 第 6.5 节，16px）：每种状态都用独立形状表达，1x 静态截图、色盲或灰度下也能区分。
// 颜色只用 currentColor：默认按状态取 --st-* 工具类，调用方可以用 className 覆盖；
// 本组件不关心强调色，也不依赖任何 Feature。
import type { CSSProperties, ComponentProps, ReactNode } from 'react'
import type { StatusGlyphKind } from '@/components/app/status-glyph-kinds'
import { cn } from '@/lib/utils'

// 16 视图框，按规格逐项给出几何尺寸；描边粗细各不相同，是形状区分的一部分。
const GLYPH_PATHS: Record<StatusGlyphKind, ReactNode> = {
  // 需要你：8px 实心圆点（半径 4）。琥珀是唯一的状态强调，形状越简单越不吵。
  attention: <circle cx="8" cy="8" r="4" fill="currentColor" stroke="none" />,
  // 运行中：r5.6、描边 2.2 的粗弧，底圈 16% 不透明，让弧在转动时仍有完整的圆作参照。
  run: (
    <>
      <circle cx="8" cy="8" r="5.6" strokeWidth={2.2} strokeOpacity={0.16} />
      <path d="M8 2.4a5.6 5.6 0 0 1 5.4 7.1" strokeWidth={2.2} />
    </>
  ),
  // 排队：1.4 描边的虚线圈。
  queued: <circle cx="8" cy="8" r="5.4" strokeWidth={1.4} strokeDasharray="1.9 2.3" />,
  // 已完成：1.7 描边的勾，不带圈（带圈会和排队的虚线圈在 1x 下混在一起）。
  done: <path d="m3.6 8.4 2.9 2.9 5.9-6.4" strokeWidth={1.7} />,
  // 失败（已处理）：细 ✕；还在等人处理的失败属于需要你，用琥珀圆点。
  failed: <path d="m5 5 6 6m0-6-6 6" strokeWidth={1.4} />,
  // 已停止：实心小方块。
  stopped: <rect x="5" y="5" width="6" height="6" rx="1" fill="currentColor" stroke="none" />,
}

// 默认颜色：类名写成完整字面量，保证 Tailwind 能静态扫描到。
const GLYPH_COLORS: Record<StatusGlyphKind, string> = {
  attention: 'text-st-attn-fill',
  run: 'text-st-run',
  queued: 'text-st-queue',
  done: 'text-st-done',
  failed: 'text-st-fail',
  stopped: 'text-st-queue',
}

export type StatusGlyphProps = Omit<ComponentProps<'svg'>, 'children'> & {
  status: StatusGlyphKind
  // 运行中默认旋转；静态快照（例如说明图例）可以传 false。
  spin?: boolean
  // 相位（0–1）：同屏多个运行中的弧错开角度，看起来是各自在转，而不是同一个图标重复。
  // 用独立的 rotate 属性叠加在旋转动画的 transform 之外，减少动效时停转也保留错开的角度。
  phase?: number
  // 只有字形单独出现、旁边没有状态文字时才传入，供读屏读出并作为悬停提示；否则保持装饰性。
  label?: string
}

export function StatusGlyph({
  status,
  spin = status === 'run',
  phase,
  label,
  className,
  style,
  ...props
}: StatusGlyphProps) {
  const turn = Number.isFinite(phase) ? ((((phase as number) % 1) + 1) % 1) * 360 : 0
  const glyphStyle: CSSProperties | undefined =
    status === 'run' && turn ? { rotate: `${Math.round(turn)}deg`, ...style } : style
  return (
    <svg
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      data-status-glyph={status}
      {...(label ? { role: 'img', 'aria-label': label } : { 'aria-hidden': true })}
      focusable="false"
      className={cn(
        'size-4 shrink-0',
        GLYPH_COLORS[status],
        // 只动 transform；用户选「减少动效」或系统要求减少动效（且未强制完整动效）时停转。
        spin &&
          status === 'run' &&
          'animate-[spin_1.2s_linear_infinite] [:root[data-motion=reduced]_&]:animate-none motion-reduce:[:root:not([data-motion=full])_&]:animate-none',
        className,
      )}
      style={glyphStyle}
      {...props}
    >
      {label ? <title>{label}</title> : null}
      {GLYPH_PATHS[status]}
    </svg>
  )
}
