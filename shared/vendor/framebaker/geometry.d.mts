export type FrameTransform = { offset_x: number; offset_y: number; rotation: number; scale: number }
export type FrameBounds = { left: number; right: number; top: number; bottom: number }
export type ImageRect = { x: number; y: number; w: number; h: number }
export function transformedFrameBounds(
  width: number,
  height: number,
  frame: FrameTransform,
): FrameBounds
export function transformedFrameRectBounds(
  width: number,
  height: number,
  rect: ImageRect,
  frame: FrameTransform,
): FrameBounds
export function fitScaleForBounds(
  bounds: FrameBounds,
  viewportWidth: number,
  viewportHeight: number,
  margin?: number,
): number
export function normalizeFrameRotation(value: number): number
export const MAX_SPRITE_SHEET_DIMENSION: number
export function spriteSheetLayout(
  cellWidth: number,
  cellHeight: number,
  count: number,
  maxDimension?: number,
): { columns: number; rows: number; width: number; height: number }
