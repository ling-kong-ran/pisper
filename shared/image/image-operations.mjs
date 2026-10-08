<<<<<<<< HEAD:shared/image/image-operations.mjs
// 通用图片运算契约。历史 wire 标记 workflow-images 仅标识数据格式，不引用工作流实体。
import { parseWorkflowMedia } from '../workflow/workflow-inputs.mjs'

/** @type {readonly import('./image-operations.mjs').ImageDirection[]} */
export const IMAGE_DIRECTIONS = Object.freeze(['S', 'SW', 'W', 'NW', 'N', 'NE', 'E', 'SE'])
/** @param {string} code */
export function imageOperationError(code) {
  return Object.assign(new Error(code), { code, statusCode: 400 })
}
/** @param {unknown} value @returns {Record<string, unknown>} */
function record(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw imageOperationError('workflow_image_invalid')
  return /** @type {Record<string, unknown>} */ (value)
}
/** @param {unknown} value @param {number} fallback @param {number} min @param {number} max */
function number(value, fallback, min, max) {
  if (value === undefined) return fallback
  if (typeof value !== 'number' || !Number.isFinite(value) || value < min || value > max)
    throw imageOperationError('workflow_image_invalid')
  return value
}
/** @param {unknown} value @param {string} fallback @param {number} max */
function text(value, fallback, max) {
  if (value === undefined) return fallback
  if (typeof value !== 'string' || value.length > max)
    throw imageOperationError('workflow_image_invalid')
  return value.trim()
}
/** @param {unknown} value @returns {import('./image-operations.mjs').ImageSettings} */
export function normalizeImageSettings(value = {}) {
  const input = record(value)
  const colors = input.colors ?? []
  const directions = input.directions ?? ['S']
  const order = input.frameOrder ?? []
  const transforms = input.transforms ?? []
  const region = record(input.region ?? {})
  if (
    !Array.isArray(colors) ||
    colors.length > 8 ||
    colors.some((color) => typeof color !== 'string' || !/^#[0-9a-f]{6}$/i.test(color)) ||
    !Array.isArray(directions) ||
    directions.length < 1 ||
    directions.length > 8 ||
    directions.some((direction) => !IMAGE_DIRECTIONS.some((id) => id === direction)) ||
    new Set(directions).size !== directions.length ||
    !Array.isArray(order) ||
    order.length > 512 ||
    order.some((index) => !Number.isSafeInteger(index) || index < 0 || index >= 512) ||
    new Set(order).size !== order.length ||
    !Array.isArray(transforms) ||
    transforms.length > 512
  )
    throw imageOperationError('workflow_image_invalid')
  const method = input.method ?? 'color'
  const align = input.align ?? 'bottom-center'
  if (
    (method !== 'color' && method !== 'model') ||
    (align !== 'center' && align !== 'bottom-center' && align !== 'none')
  )
    throw imageOperationError('workflow_image_invalid')
  const inputName = text(input.inputName, 'reference', 80)
  if (
    !/^[a-zA-Z][a-zA-Z0-9_]{0,79}$/.test(inputName) ||
    ['constructor', 'prototype', '__proto__'].includes(inputName)
  )
    throw imageOperationError('workflow_image_invalid')
  /** @type {import('./image-operations.mjs').ImageSettings} */
  const result = {
    inputName,
    method,
    colors: colors.map((color) => String(color)),
    tolerance: number(input.tolerance, 24, 0, 255),
    softness: number(input.softness, 8, 0, 64),
    edgeConnected: input.edgeConnected !== false,
    region: {
      x: number(region.x, 25, 0, 99),
      y: number(region.y, 25, 0, 99),
      width: number(region.width, 20, 1, 100),
      height: number(region.height, 10, 1, 100),
    },
    action: text(input.action, 'idle', 160),
    frameCount: Math.round(number(input.frameCount, 4, 1, 16)),
    directions: /** @type {import('./image-operations.mjs').ImageDirection[]} */ ([...directions]),
    columns: Math.round(number(input.columns, 4, 1, 16)),
    rows: Math.round(number(input.rows, 1, 1, 16)),
    durationMs: Math.round(number(input.durationMs, 125, 16, 10000)),
    trim: input.trim !== false,
    align,
    padding: Math.round(number(input.padding, 4, 0, 64)),
    maxFrameSize: Math.round(number(input.maxFrameSize, 256, 16, 1024)),
    frameOrder: order.map((index) => Number(index)),
    transforms: transforms.map((entry) => {
      const transform = record(entry)
      return {
        index: Math.round(number(transform.index, 0, 0, 511)),
        x: number(transform.x, 0, -4096, 4096),
        y: number(transform.y, 0, -4096, 4096),
        rotation: number(transform.rotation, 0, -360, 360),
        scale: number(transform.scale, 1, 0.05, 8),
        opacity: number(transform.opacity, 1, 0, 1),
        durationMs: Math.round(number(transform.durationMs, 125, 16, 10000)),
        enabled: transform.enabled !== false,
      }
    }),
    filename: text(input.filename, 'animation', 100),
  }
  if (new Set(result.transforms.map((entry) => entry.index)).size !== result.transforms.length)
    throw imageOperationError('workflow_image_invalid')
  return result
}
/** @param {unknown} value @returns {import('./image-operations.mjs').ImageOutput} */
export function parseImageOutput(value) {
  const data = record(value)
  if (
    data.type !== 'workflow-images' ||
    data.version !== 1 ||
    !Array.isArray(data.frames) ||
    data.frames.length > 512
  )
    throw imageOperationError('workflow_image_invalid')
  const frames = data.frames.map((entry) => {
    const frame = record(entry)
    const media = parseWorkflowMedia(frame.media)
    if (!media.mimeType.startsWith('image/')) throw imageOperationError('workflow_image_invalid')
    return {
      media,
      width: number(frame.width, 1, 1, 4096),
      height: number(frame.height, 1, 1, 4096),
      durationMs: number(frame.durationMs, 125, 16, 10000),
      action: text(frame.action, '', 160),
      direction: text(frame.direction, '', 16),
      columns: number(frame.columns, 1, 1, 16),
      rows: number(frame.rows, 1, 1, 16),
      frameCount: number(frame.frameCount, 1, 1, 256),
    }
  })
  /** @type {import('./image-operations.mjs').ImageOutput} */
  const output = { type: 'workflow-images', version: 1, frames }
  if (data.atlas !== undefined) {
    const atlas = record(data.atlas)
    if (!Array.isArray(atlas.frames) || atlas.frames.length > 512)
      throw imageOperationError('workflow_image_invalid')
    output.atlas = {
      media: parseWorkflowMedia(atlas.media),
      width: number(atlas.width, 1, 1, 4096),
      height: number(atlas.height, 1, 1, 4096),
      frames: atlas.frames.map((entry) => {
        const frame = record(entry)
        return {
          x: number(frame.x, 0, 0, 4096),
          y: number(frame.y, 0, 0, 4096),
          width: number(frame.width, 1, 1, 4096),
          height: number(frame.height, 1, 1, 4096),
          durationMs: number(frame.durationMs, 125, 16, 10000),
          action: text(frame.action, '', 160),
          direction: text(frame.direction, '', 16),
        }
      }),
    }
  }
  return output
}
========
// 旧 Rust 分支构建脚本的兼容入口；业务协议以 release 的分域模块为唯一来源。
export * from './image/image-operations.mjs'
>>>>>>>> origin/develop-rust:shared/image-operations.mjs
