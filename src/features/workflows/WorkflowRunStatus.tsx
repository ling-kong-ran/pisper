// 工作流运行状态的视觉原语：状态方块、分段步进表与状态文字（规格 v2 第 6.1、6.5 节）。
// 只有“等待审批”是琥珀（需要你去批准）；运行中、完成、失败、已停止、中断都是中性灰阶，靠状态字形的形状区分。
import { StatusGlyph } from '@/components/app/StatusGlyph'
import type { StatusGlyphKind } from '@/components/app/status-glyph-kinds'
import { cn } from '@/lib/utils'

// 运行 / 节点状态 → 状态字形；未开始的节点没有字形（只留空位）。
function runGlyph(status?: string): StatusGlyphKind | null {
  if (status === 'running') return 'run'
  if (status === 'waiting_approval') return 'attention'
  if (status === 'completed') return 'done'
  if (status === 'failed' || status === 'interrupted') return 'failed'
  if (status === 'cancelled') return 'stopped'
  return null
}

// 18px 状态位：字形居中；未开始的节点是一枚空的浅底方块，占位对齐。
export function RunStatusSquare({ status, className }: { status?: string; className?: string }) {
  const glyph = runGlyph(status)
  return (
    <span
      aria-hidden="true"
      className={cn(
        'grid size-[18px] flex-none place-items-center rounded-xs',
        !glyph && 'bg-surface-muted',
        className,
      )}
    >
      {glyph && <StatusGlyph status={glyph} className="size-3.5" />}
    </span>
  )
}

// 步进表的胶囊色：运行中是墨色，等待审批琥珀，其余按完成度深浅区分（不再用红绿）。
function meterTone(status: string) {
  if (status === 'running') return 'bg-st-run'
  if (status === 'waiting_approval') return 'bg-st-attn-fill'
  if (status === 'completed') return 'bg-content-muted'
  if (status === 'failed' || status === 'interrupted') return 'bg-content-secondary'
  return 'bg-stroke-strong'
}

// 分段步进表：每个节点一枚 18×4 胶囊，按真实节点状态着色；节点多时胶囊等比收窄。
export function RunStepMeter({
  nodes,
  className,
}: {
  nodes: ReadonlyArray<{ id: string; status: string }>
  className?: string
}) {
  if (!nodes.length) return null
  return (
    <span aria-hidden="true" className={cn('flex h-1 min-w-0 gap-0.5', className)}>
      {nodes.map((node) => (
        <i
          key={node.id}
          className={`h-full w-[18px] min-w-0.5 shrink rounded-full ${meterTone(node.status)}`}
        />
      ))}
    </span>
  )
}

// 状态文字 = 字形 + 文字（规格 6.5 不只靠颜色）：等待审批是琥珀圆点 + 琥珀文字，其余中性。
export function RunStatusText({ status, label }: { status: string; label: string }) {
  const glyph = runGlyph(status)
  const attention = status === 'waiting_approval'
  return (
    <span
      data-run-status={status}
      className={cn(
        'inline-flex min-w-0 items-center gap-1 whitespace-nowrap',
        attention ? 'font-medium text-st-attn' : 'text-content-secondary',
      )}
    >
      {glyph && <StatusGlyph status={glyph} className="size-3.5" />}
      <span className="min-w-0 truncate">{label}</span>
    </span>
  )
}
