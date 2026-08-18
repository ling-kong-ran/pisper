// 长消息列表虚拟化参数：估算行高与上下超额渲染，
// 以及顶部追加（加载更早消息）时保持滚动位置的快照结构。
export const TRANSCRIPT_ESTIMATED_ROW_HEIGHT = 164
export const TRANSCRIPT_OVERSCAN = 6

export type TranscriptPrependSnapshot = {
  scrollHeight: number
  scrollTop: number
}

export function anchoredScrollTopAfterPrepend(
  snapshot: TranscriptPrependSnapshot,
  nextScrollHeight: number,
) {
  return snapshot.scrollTop + nextScrollHeight - snapshot.scrollHeight
}
