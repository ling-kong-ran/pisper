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
