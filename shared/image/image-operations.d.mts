import type { WorkflowMedia } from '../workflow/workflow-inputs.mjs'

export type ImageDirection = 'S' | 'SW' | 'W' | 'NW' | 'N' | 'NE' | 'E' | 'SE'
export type ImageFrameTransform = {
  index: number
  x: number
  y: number
  rotation: number
  scale: number
  opacity: number
  durationMs: number
  enabled: boolean
}
export type ImageSettings = {
  inputName: string
  method: 'color' | 'model'
  colors: string[]
  tolerance: number
  softness: number
  edgeConnected: boolean
  region: { x: number; y: number; width: number; height: number }
  action: string
  frameCount: number
  directions: ImageDirection[]
  columns: number
  rows: number
  durationMs: number
  trim: boolean
  align: 'center' | 'bottom-center' | 'none'
  padding: number
  maxFrameSize: number
  frameOrder: number[]
  transforms: ImageFrameTransform[]
  filename: string
}
export type ImageFrame = {
  media: WorkflowMedia
  width: number
  height: number
  durationMs: number
  action: string
  direction: string
  columns: number
  rows: number
  frameCount: number
}
export type ImageOutput = {
  type: 'workflow-images'
  version: 1
  frames: ImageFrame[]
  atlas?: {
    media: WorkflowMedia
    width: number
    height: number
    frames: Array<{
      x: number
      y: number
      width: number
      height: number
      durationMs: number
      action: string
      direction: string
    }>
  }
}
export const IMAGE_DIRECTIONS: readonly ImageDirection[]
export function normalizeImageSettings(value?: unknown): ImageSettings
export function parseImageOutput(value: unknown): ImageOutput
export function imageOperationError(code: string): Error & { code: string; statusCode: number }
