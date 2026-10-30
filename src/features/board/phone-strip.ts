// 手机会话页代理条的条目推导（纯函数，便于测试）：顺序与桌面泳道条一致（需要你 → 运行中 → 排队），
// 排队只在 Runtime 声明排队能力时出现；当前会话无论在哪条泳道都保留，其他已完成的代理不上条
// （看板、⌘K 与历史会话里仍能找到），让 390 宽的条子只放还需要关注的代理。
import type { BoardLane } from '@shared/session-board-status.mjs'
import { stripOrder } from './strip-layout'

export function phoneStripItems<T extends { id: string; lane: BoardLane }>(
  items: readonly T[],
  currentId: string,
  { queue }: { queue: boolean },
): T[] {
  return stripOrder(items).filter(
    (item) =>
      item.id === currentId ||
      item.lane === 'attention' ||
      item.lane === 'running' ||
      (queue && item.lane === 'queued'),
  )
}
