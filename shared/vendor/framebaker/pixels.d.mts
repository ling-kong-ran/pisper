export type RgbColor = [number, number, number]
export type CropRect = { x: number; y: number; w: number; h: number }
export type RemoveColorOptions = { targets: RgbColor[]; tolerance: number; softness?: number }
export type DetectComponentsOptions = {
  alphaThreshold?: number
  minAreaRatio?: number
  minAreaPixels?: number
  maxComponents?: number
}
export function removeColorPixels(
  data: Uint8ClampedArray,
  width: number,
  height: number,
  options: RemoveColorOptions,
): Uint8ClampedArray
export function extractPalette(
  data: Uint8ClampedArray,
  width: number,
  height: number,
  maxColors: number,
): RgbColor[]
export function computeOpaqueBounds(
  data: Uint8ClampedArray,
  width: number,
  height: number,
  alphaThreshold?: number,
): CropRect | null
export function detectOpaqueComponents(
  data: Uint8ClampedArray,
  width: number,
  height: number,
  options?: DetectComponentsOptions,
): CropRect[]
