// 长消息列表虚拟化参数：估算行高与上下超额渲染，
// 以及顶部追加（加载更早消息）时保持滚动位置的快照结构。
export const TRANSCRIPT_ESTIMATED_ROW_HEIGHT = 164
export const TRANSCRIPT_OVERSCAN = 6

export type TranscriptPrependSnapshot = {
  scrollHeight: number
  scrollTop: number
}

// 顶部追加更早消息后保持滚动位置：快照记录追加前的 scrollTop/高度，
// 新位置 = 旧 scrollTop + 高度增量，防止视觉跳变。
export function anchoredScrollTopAfterPrepend(
  snapshot: TranscriptPrependSnapshot,
  nextScrollHeight: number,
) {
  return snapshot.scrollTop + nextScrollHeight - snapshot.scrollHeight
}
