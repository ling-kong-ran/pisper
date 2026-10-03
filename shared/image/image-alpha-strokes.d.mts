import type { ImageEraseStroke } from './image-frame-edits.mjs'

export function applyImageAlphaStrokes(
  source: { width: number; height: number; data: Uint8ClampedArray },
  strokes: ImageEraseStroke[],
  work?: { remaining: number },
): Uint8ClampedArray
