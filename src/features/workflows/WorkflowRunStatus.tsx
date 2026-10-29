// 工作流运行状态的视觉原语：状态方块、分段步进表与状态徽标。
// 与对话里的工具活动日志同一语言——形状与颜色共同表达结果，运行中统一用品牌轨道指示。
import { Check, Clock3, Square, TriangleAlert, X } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'

const SQUARE_TONES: Record<string, string> = {
  completed: 'bg-success-soft text-success',
  failed: 'bg-danger-soft text-danger',
  waiting_approval: 'bg-warning-soft text-warning-strong',
  running: 'text-accent-fg',
}

// 18px 状态方块：勾 / 叉 / 时钟 / 轨道 / 停止块 / 警示，未开始的节点只留空方块。
export function RunStatusSquare({ status, className }: { status?: string; className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        'grid size-[18px] flex-none place-items-center rounded-xs',
        SQUARE_TONES[status || ''] ?? 'bg-surface-muted text-content-muted',
        className,
      )}
    >
      {status === 'running' ? (
        <OrbitSpinner className="size-3.5" />
      ) : status === 'completed' ? (
        <Check size={12} strokeWidth={2.25} />
      ) : status === 'failed' ? (
        <X size={12} strokeWidth={2.25} />
      ) : status === 'waiting_approval' ? (
        <Clock3 size={12} />
      ) : status === 'cancelled' ? (
        <Square size={8} fill="currentColor" strokeWidth={0} />
      ) : status === 'interrupted' ? (
        <TriangleAlert size={11} />
      ) : null}
    </span>
  )
}

function meterTone(status: string) {
  if (status === 'completed') return 'bg-success'
  if (status === 'failed') return 'bg-danger'
  if (status === 'running') return 'bg-star'
  if (status === 'waiting_approval') return 'bg-warning-strong'
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

// 状态徽标：运行中 = 强调色软底 + 轨道；等待审批 = 警示；完成 = 成功；失败 = 危险；其余中性。
export function RunStatusBadge({ status, label }: { status: string; label: string }) {
  if (status === 'running')
    return (
      <Badge variant="run" className="gap-1.5">
        <OrbitSpinner />
        {label}
      </Badge>
    )
  return (
    <Badge
      variant={
        status === 'completed'
          ? 'ok'
          : status === 'failed'
            ? 'bad'
            : status === 'waiting_approval'
              ? 'warn'
              : 'default'
      }
    >
      {label}
    </Badge>
  )
}
