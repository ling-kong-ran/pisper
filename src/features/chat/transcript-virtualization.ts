// 长消息列表虚拟化参数：估算行高与上下超额渲染，
// 以及顶部追加（加载更早消息）时保持滚动位置的快照结构。
export const TRANSCRIPT_ESTIMATED_ROW_HEIGHT = 164
export const TRANSCRIPT_OVERSCAN = 6

// 窄屏（移动端断点 650px）下消息换行更多、代码块横向受限，实际行高
// 显著大于桌面估算值；按容器宽度分档估算，减少进入长会话时的滚动条跳动。
export const TRANSCRIPT_NARROW_ESTIMATED_ROW_HEIGHT = 260
export const TRANSCRIPT_NARROW_WIDTH = 650

export function estimateTranscriptRowHeight(containerWidth?: number | null) {
  return (containerWidth || 0) > 0 && Number(containerWidth) < TRANSCRIPT_NARROW_WIDTH
    ? TRANSCRIPT_NARROW_ESTIMATED_ROW_HEIGHT
    : TRANSCRIPT_ESTIMATED_ROW_HEIGHT
}

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
