// 会话转录的阅读位置：离开会话时记什么、回来时往哪儿校正。只做判断，不碰 DOM，便于单测。
import type { ConversationAnchor } from './session-context-pages'

// 离开时离最新消息不超过这么多就算停在最新消息（与 useAutoScroll 的贴底阈值一致）。
export const LATEST_SLACK_PX = 64

// null：停在最新消息，回来时照样落到最新消息（哪怕离开期间回复变长或来了新消息）。
// 往上翻着读：像素位置只作后备，按 anchor（视口顶上那条消息）还原。
export type ReadingPosition = { top: number; anchor: ConversationAnchor | null } | null

export type TranscriptViewport = {
  scrollTop: number
  scrollHeight: number
  clientHeight: number
  // 滚动容器顶边在视口里的位置（与下面各行的 top / bottom 同一坐标系）。
  viewportTop: number
}
export type TranscriptRowBox = { messageId: string; top: number; bottom: number }

/**
 * 离开会话（或滚动停下）时要记下的阅读位置。
 * 返回 undefined 表示此刻的布局不可信、不要覆盖上一次的值：容器已收起（高度为 0），
 * 或转录正在拆（消息行已经卸载，scrollHeight 只剩外壳，算出来总像“在底部”）。
 */
export function captureReadingPosition(
  view: TranscriptViewport,
  rows: readonly TranscriptRowBox[],
): ReadingPosition | undefined {
  if (!(view.clientHeight > 0) || !rows.length) return undefined
  if (view.scrollHeight - view.clientHeight - view.scrollTop <= LATEST_SLACK_PX) return null
  const first = [...rows]
    .sort((a, b) => a.top - b.top)
    .find((row) => row.bottom > view.viewportTop + 0.5)
  return {
    top: Math.max(0, Math.round(view.scrollTop)),
    anchor: first?.messageId
      ? { messageId: first.messageId, offset: Math.round(view.viewportTop - first.top) }
      : null,
  }
}

/** 还原时这条消息现在的位置与离开时差多少：scrollTop 加上它就回到同一条消息的同一处。 */
export function anchorScrollDelta(rowTop: number, viewportTop: number, anchor: ConversationAnchor) {
  return rowTop - viewportTop + anchor.offset
}
