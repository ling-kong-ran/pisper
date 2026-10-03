export const MAX_RASTER_SIDE: number
export const MAX_RASTER_PIXELS: number
export type RasterDimensions = { width: number; height: number; mimeType: string }
export function readRasterDimensions(bytes: Uint8Array): RasterDimensions | null
export function assertRasterBounds(dimensions: { width: number; height: number } | null): void
