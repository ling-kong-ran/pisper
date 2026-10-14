export type ImageEraseStroke = {
  points: Array<{ x: number; y: number }>
  /** 相对于原帧较短边的笔刷半径。 */
  radius: number
  restore: boolean
}
export type ImageFrameEdit = {
  sourceIndex: number
  x: number
  y: number
  rotation: number
  scale: number
  opacity: number
  durationMs: number
  eraseStrokes: ImageEraseStroke[]
}
export type ImageFrameEdits = { frames: ImageFrameEdit[] }
export function normalizeImageFrameEdits(value: unknown): ImageFrameEdits
