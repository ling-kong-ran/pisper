// 泳道字形：“需要你”泳道用三角警示（与标题栏舰队标签、页头胶囊的“▲ 需要你”一致），
// 盾牌只表示“等待批准”这一种卡片状态——失败卡也排在需要你泳道里，泳道本身不能用审批的盾牌。
// 其余泳道沿用对应状态的字形。颜色由调用方用 text-st-* 着色。
import { TriangleAlert } from 'lucide-react'
import type { BoardLane } from '@shared/session-board-status.mjs'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { cn } from '@/lib/utils'
import { BOARD_LANE_GLYPHS } from './card-presentation'

export function LaneGlyph({
  lane,
  spin,
  className,
}: {
  lane: BoardLane
  spin?: boolean
  className?: string
}) {
  if (lane === 'attention')
    return (
      <TriangleAlert
        aria-hidden="true"
        strokeWidth={1.75}
        className={cn('size-4', className)}
        data-lane-glyph="attention"
      />
    )
  return <StatusGlyph status={BOARD_LANE_GLYPHS[lane]} spin={spin} className={className} />
}
