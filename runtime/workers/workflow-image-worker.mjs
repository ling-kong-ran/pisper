import { parentPort, workerData } from 'node:worker_threads'
import { createRequire } from 'node:module'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { PNG } from 'pngjs'
import jpeg from 'jpeg-js'
import { assertRasterBounds, readRasterDimensions } from '../../shared/image/raster-image.mjs'
import { imageOperationError } from '../../shared/image/image-operations.mjs'
import { applyImageAlphaStrokes } from '../../shared/image/image-alpha-strokes.mjs'
import { normalizeImageFrameEdits } from '../../shared/image/image-frame-edits.mjs'
import {
  computeOpaqueBounds,
  extractPalette,
  removeColorPixels,
} from '../../shared/vendor/framebaker/pixels.mjs'
import {
  spriteSheetLayout,
  transformedFrameRectBounds,
} from '../../shared/vendor/framebaker/geometry.mjs'

/** @typedef {import('../services/workflow-image-processing.mjs').ProcessingFrame} ProcessingFrame */
/** @typedef {import('../../shared/image/image-operations.mjs').ImageSettings} Settings */
/** @typedef {ProcessingFrame & { data: Uint8ClampedArray }} Pixels */
/** @typedef {import('../../shared/vendor/framebaker/pixels.mjs').RgbColor} RgbColor */

const require = createRequire(import.meta.url)
const LIMIT = 16_000_000

/** @param {number} width @param {number} height @param {number} [count] */
function bounds(width, height, count = 1) {
  assertRasterBounds({ width, height })
  if (width * height * count > LIMIT) throw imageOperationError('workflow_image_too_large')
}

/** @param {ProcessingFrame} frame @returns {Promise<Pixels>} */
async function decode(frame) {
  const size = readRasterDimensions(frame.buffer)
  assertRasterBounds(size)
  if (
    !size ||
    size.mimeType !== frame.mimeType ||
    size.width !== frame.width ||
    size.height !== frame.height
  )
    throw imageOperationError('workflow_image_invalid')
  let image
  if (frame.mimeType === 'image/png')
    image = PNG.sync.read(Buffer.from(frame.buffer), { checkCRC: true })
  else if (frame.mimeType === 'image/jpeg')
    image = jpeg.decode(frame.buffer, {
      useTArray: true,
      formatAsRGBA: true,
      tolerantDecoding: false,
      maxResolutionInMP: 16,
      maxMemoryUsageInMB: 256,
    })
  else if (frame.mimeType === 'image/webp') {
    const decoder = await import('@jsquash/webp/decode.js')
    // 显式提供包内固定 WASM，不让 headless Node 尝试 fetch(file://)。
    const wasm = await readFile(require.resolve('@jsquash/webp/codec/dec/webp_dec.wasm'))
    await decoder.init({ wasmBinary: new Uint8Array(wasm).buffer })
    image = await decoder.default(new Uint8Array(frame.buffer).buffer)
  } else throw imageOperationError('workflow_image_invalid')
  if (
    image.width !== size.width ||
    image.height !== size.height ||
    image.data.length !== size.width * size.height * 4
  )
    throw imageOperationError('workflow_image_invalid')
  return { ...frame, data: new Uint8ClampedArray(image.data) }
}

/** @param {Pixels} frame */
function encode(frame) {
  const { data, buffer: _buffer, ...metadata } = frame
  return {
    ...metadata,
    mimeType: 'image/png',
    buffer: pngBuffer(frame.width, frame.height, data),
  }
}

/** @param {number} width @param {number} height @param {Uint8ClampedArray} data */
function pngBuffer(width, height, data) {
  const image = new PNG({ width, height })
  image.data = Buffer.from(data)
  return PNG.sync.write(image)
}

/** @param {string} hex @returns {RgbColor} */
function rgb(hex) {
  return [
    parseInt(hex.slice(1, 3), 16),
    parseInt(hex.slice(3, 5), 16),
    parseInt(hex.slice(5, 7), 16),
  ]
}

/** 只抽取外边缘的实际像素主色，不把占面积最大的角色颜色误认成背景。 @param {Pixels} frame */
function borderColor(frame) {
  const count = frame.width * 2 + frame.height * 2
  const pixels = new Uint8ClampedArray(count * 4)
  let offset = 0
  /** @param {number} x @param {number} y */
  const sample = (x, y) => {
    pixels.set(
      frame.data.subarray((y * frame.width + x) * 4, (y * frame.width + x) * 4 + 4),
      offset,
    )
    offset += 4
  }
  for (let x = 0; x < frame.width; x++) {
    sample(x, 0)
    sample(x, frame.height - 1)
  }
  for (let y = 0; y < frame.height; y++) {
    sample(0, y)
    sample(frame.width - 1, y)
  }
  return extractPalette(pixels, count, 1, 1)
}

/** @param {Pixels} frame @param {Settings} settings */
function colorBackground(frame, settings) {
  const targets = settings.colors.length ? settings.colors.map(rgb) : borderColor(frame)
  const changed = removeColorPixels(frame.data, frame.width, frame.height, {
    targets,
    tolerance: settings.tolerance,
    softness: settings.softness,
  })
  if (!settings.edgeConnected) return { ...frame, data: changed }
  const { width, height } = frame
  const seen = new Uint8Array(width * height)
  const queue = new Int32Array(width * height)
  let head = 0,
    tail = 0
  /** @param {number} x @param {number} y */
  const visit = (x, y) => {
    if (x < 0 || x >= width || y < 0 || y >= height) return
    const index = y * width + x
    if (seen[index]) return
    seen[index] = 1
    if (frame.data[index * 4 + 3] !== 0 && changed[index * 4 + 3] === frame.data[index * 4 + 3])
      return
    queue[tail++] = index
  }
  for (let x = 0; x < width; x++) {
    visit(x, 0)
    visit(x, height - 1)
  }
  for (let y = 0; y < height; y++) {
    visit(0, y)
    visit(width - 1, y)
  }
  const output = new Uint8ClampedArray(frame.data)
  while (head < tail) {
    const index = queue[head++]
    output[index * 4 + 3] = changed[index * 4 + 3]
    const x = index % width,
      y = Math.floor(index / width)
    visit(x - 1, y)
    visit(x + 1, y)
    visit(x, y - 1)
    visit(x, y + 1)
  }
  return { ...frame, data: output }
}

/** @param {Uint8ClampedArray} data @param {number} width @param {number} height @param {number} x @param {number} y @param {number} channel */
function sampleBilinear(data, width, height, x, y, channel) {
  const sx = Math.max(0, Math.min(width - 1, x)),
    sy = Math.max(0, Math.min(height - 1, y))
  const x0 = Math.floor(sx),
    y0 = Math.floor(sy),
    x1 = Math.min(width - 1, x0 + 1),
    y1 = Math.min(height - 1, y0 + 1)
  const dx = sx - x0,
    dy = sy - y0
  return (
    (data[(y0 * width + x0) * 4 + channel] * (1 - dx) +
      data[(y0 * width + x1) * 4 + channel] * dx) *
      (1 - dy) +
    (data[(y1 * width + x0) * 4 + channel] * (1 - dx) +
      data[(y1 * width + x1) * 4 + channel] * dx) *
      dy
  )
}

/** @param {Pixels[]} frames @param {string} directory */
async function modelBackground(frames, directory) {
  if (!directory) throw imageOperationError('workflow_image_engine_missing')
  const ort = require(join(directory, 'ort.wasm.min.js'))
  ort.env.wasm.numThreads = 1
  ort.env.wasm.proxy = false
  ort.env.wasm.wasmPaths = directory + '/'
  const session = await ort.InferenceSession.create(
    new Uint8Array(await readFile(join(directory, 'u2netp.onnx'))),
    { executionProviders: ['wasm'] },
  )
  try {
    const results = []
    for (const frame of frames) {
      const plane = 320 * 320
      const input = new Float32Array(plane * 3)
      let maximum = 0
      for (let y = 0; y < 320; y++)
        for (let x = 0; x < 320; x++)
          for (let c = 0; c < 3; c++) {
            const value = sampleBilinear(
              frame.data,
              frame.width,
              frame.height,
              ((x + 0.5) * frame.width) / 320 - 0.5,
              ((y + 0.5) * frame.height) / 320 - 0.5,
              c,
            )
            input[c * plane + y * 320 + x] = value
            maximum = Math.max(maximum, value)
          }
      const means = [0.485, 0.456, 0.406],
        deviations = [0.229, 0.224, 0.225]
      for (let c = 0; c < 3; c++)
        for (let i = 0; i < plane; i++)
          input[c * plane + i] =
            (input[c * plane + i] / (maximum || 255) - means[c]) / deviations[c]
      const outputs = await session.run({
        [session.inputNames[0]]: new ort.Tensor('float32', input, [1, 3, 320, 320]),
      })
      const prediction = outputs[session.outputNames[0]]?.data
      if (!(prediction instanceof Float32Array) || prediction.length !== plane)
        throw imageOperationError('workflow_image_processing_failed')
      let min = Infinity,
        max = -Infinity
      for (const value of prediction) {
        if (!Number.isFinite(value)) throw imageOperationError('workflow_image_processing_failed')
        min = Math.min(min, value)
        max = Math.max(max, value)
      }
      const mask = new Uint8ClampedArray(plane * 4)
      for (let i = 0; i < plane; i++)
        mask[i * 4 + 3] = max > min ? ((prediction[i] - min) / (max - min)) * 255 : 0
      const data = new Uint8ClampedArray(frame.data)
      for (let y = 0; y < frame.height; y++)
        for (let x = 0; x < frame.width; x++) {
          const i = (y * frame.width + x) * 4 + 3
          data[i] = Math.round(
            (data[i] *
              sampleBilinear(
                mask,
                320,
                320,
                ((x + 0.5) * 320) / frame.width - 0.5,
                ((y + 0.5) * 320) / frame.height - 0.5,
                3,
              )) /
              255,
          )
        }
      results.push({ ...frame, data })
    }
    return results
  } finally {
    await session.release()
  }
}

/** @param {Pixels[]} frames @param {Settings} settings @param {string} directory */
async function inpaint(frames, settings, directory) {
  if (!directory) throw imageOperationError('workflow_image_engine_missing')
  const cv = await require(join(directory, 'opencv.js'))
  return frames.map((frame) => {
    const { width, height } = frame
    const x = Math.min(width - 1, Math.floor((settings.region.x * width) / 100))
    const y = Math.min(height - 1, Math.floor((settings.region.y * height) / 100))
    const right = Math.min(
      width,
      x + Math.max(1, Math.round((settings.region.width * width) / 100)),
    )
    const bottom = Math.min(
      height,
      y + Math.max(1, Math.round((settings.region.height * height) / 100)),
    )
    const source = new cv.Mat(height, width, cv.CV_8UC3)
    const mask = cv.Mat.zeros(height, width, cv.CV_8UC1)
    const result = new cv.Mat()
    try {
      for (let i = 0; i < width * height; i++)
        for (let c = 0; c < 3; c++) source.data[i * 3 + c] = frame.data[i * 4 + c]
      for (let row = y; row < bottom; row++)
        for (let col = x; col < right; col++) mask.data[row * width + col] = 255
      cv.inpaint(source, mask, result, 3, cv.INPAINT_TELEA)
      const data = new Uint8ClampedArray(frame.data)
      for (let row = y; row < bottom; row++)
        for (let col = x; col < right; col++)
          for (let c = 0; c < 3; c++)
            data[(row * width + col) * 4 + c] = result.data[(row * width + col) * 3 + c]
      return { ...frame, data }
    } finally {
      source.delete()
      mask.delete()
      result.delete()
    }
  })
}

/** @param {Pixels} frame @param {{x:number,y:number,w:number,h:number}} rect */
function crop(frame, rect) {
  const data = new Uint8ClampedArray(rect.w * rect.h * 4)
  for (let y = 0; y < rect.h; y++)
    data.set(
      frame.data.subarray(
        ((rect.y + y) * frame.width + rect.x) * 4,
        ((rect.y + y) * frame.width + rect.x + rect.w) * 4,
      ),
      y * rect.w * 4,
    )
  return { ...frame, data, width: rect.w, height: rect.h, columns: 1, rows: 1, frameCount: 1 }
}

/** @param {Pixels[]} frames @param {Settings} settings */
function splitFrames(frames, settings) {
  const result = []
  for (const frame of frames) {
    const generatedGrid = frame.columns > 1 || frame.rows > 1
    const columns = generatedGrid ? frame.columns : settings.columns
    const rows = generatedGrid ? frame.rows : settings.rows
    const count = generatedGrid ? frame.frameCount : Math.min(settings.frameCount, columns * rows)
    if (
      frame.width < columns ||
      frame.height < rows ||
      count > columns * rows ||
      result.length + count > 512
    )
      throw imageOperationError('workflow_image_invalid_grid')
    for (let index = 0; index < count; index++) {
      const col = index % columns,
        row = Math.floor(index / columns)
      const x = Math.floor((col * frame.width) / columns),
        y = Math.floor((row * frame.height) / rows)
      const item = crop(frame, {
        x,
        y,
        w: Math.floor(((col + 1) * frame.width) / columns) - x,
        h: Math.floor(((row + 1) * frame.height) / rows) - y,
      })
      result.push({ ...item, durationMs: generatedGrid ? frame.durationMs : settings.durationMs })
    }
  }
  return result
}

/** @param {Pixels[]} frames @param {Settings} settings */
function transformFrames(frames, settings) {
  if (
    settings.frameOrder.some((index) => index >= frames.length) ||
    settings.transforms.some((entry) => entry.index >= frames.length)
  )
    throw imageOperationError('workflow_image_invalid')
  const order = [
    ...settings.frameOrder,
    ...frames.map((_, index) => index).filter((index) => !settings.frameOrder.includes(index)),
  ]
  const items = order.flatMap((index) => {
    const frame = frames[index]
    const edit = settings.transforms.find((entry) => entry.index === index)
    if (edit?.enabled === false) return []
    const rect = settings.trim
      ? (computeOpaqueBounds(frame.data, frame.width, frame.height) ?? { x: 0, y: 0, w: 1, h: 1 })
      : { x: 0, y: 0, w: frame.width, h: frame.height }
    const source = settings.align === 'none' ? frame : crop(frame, rect)
    const area =
      settings.align === 'none' ? rect : { x: 0, y: 0, w: source.width, h: source.height }
    const scale = edit?.scale ?? 1
    const transform = {
      offset_x: edit?.x ?? 0,
      offset_y:
        (edit?.y ?? 0) - (settings.align === 'bottom-center' ? (source.height * scale) / 2 : 0),
      rotation: ((edit?.rotation ?? 0) * Math.PI) / 180,
      scale,
    }
    return [
      {
        source,
        area,
        transform,
        opacity: edit?.opacity ?? 1,
        durationMs: edit?.durationMs ?? frame.durationMs,
        bound: transformedFrameRectBounds(source.width, source.height, area, transform),
      },
    ]
  })
  if (!items.length) throw imageOperationError('workflow_image_empty')
  // 直角旋转的浮点尾差不应凭空增加一圈透明像素，也不应改变统一缩放比例。
  const epsilon = 1e-9
  const left =
    Math.floor(Math.min(...items.map((item) => item.bound.left)) + epsilon) - settings.padding
  const top =
    Math.floor(Math.min(...items.map((item) => item.bound.top)) + epsilon) - settings.padding
  const naturalWidth =
    Math.ceil(Math.max(...items.map((item) => item.bound.right)) - epsilon) +
    settings.padding -
    left
  const naturalHeight =
    Math.ceil(Math.max(...items.map((item) => item.bound.bottom)) - epsilon) +
    settings.padding -
    top
  // 先缩放共同坐标系再分配输出，避免模型的大尺寸图集在整理阶段生成巨型中间帧。
  // 全部帧共用比例和原点，因此脚底对齐与用户的相对位移不会被逐帧缩放破坏。
  const rasterScale = Math.min(1, settings.maxFrameSize / Math.max(naturalWidth, naturalHeight))
  const width = Math.max(1, Math.min(settings.maxFrameSize, Math.ceil(naturalWidth * rasterScale)))
  const height = Math.max(
    1,
    Math.min(settings.maxFrameSize, Math.ceil(naturalHeight * rasterScale)),
  )
  bounds(width, height, items.length)
  return items.map(({ source, area, transform, opacity, durationMs }) => {
    const data = new Uint8ClampedArray(width * height * 4)
    const cos = Math.cos(transform.rotation),
      sin = Math.sin(transform.rotation)
    for (let y = 0; y < height; y++)
      for (let x = 0; x < width; x++) {
        const tx = left + (x + 0.5) / rasterScale - transform.offset_x,
          ty = top + (y + 0.5) / rasterScale - transform.offset_y
        const sx = Math.floor((tx * cos + ty * sin) / transform.scale + source.width / 2)
        const sy = Math.floor((-tx * sin + ty * cos) / transform.scale + source.height / 2)
        if (sx < area.x || sy < area.y || sx >= area.x + area.w || sy >= area.y + area.h) continue
        const from = (sy * source.width + sx) * 4,
          to = (y * width + x) * 4
        data.set(source.data.subarray(from, from + 4), to)
        data[to + 3] = Math.round(data[to + 3] * opacity)
      }
    return { ...source, data, width, height, durationMs, columns: 1, rows: 1, frameCount: 1 }
  })
}

/** @param {Pixels[]} frames @param {unknown} value */
function editFrames(frames, value) {
  const edits = normalizeImageFrameEdits(value)
  if (edits.frames.some((edit) => edit.sourceIndex >= frames.length))
    throw imageOperationError('workflow_image_invalid_edits')
  // 复制帧也计入总量，先限制分配，再处理笔划；避免一个大原图复制数百份占满内存。
  const pixels = edits.frames.reduce((sum, edit) => {
    const source = frames[edit.sourceIndex]
    return sum + source.width * source.height
  }, 0)
  if (pixels > LIMIT) throw imageOperationError('workflow_image_too_large')
  const work = { remaining: 128_000_000 }
  return edits.frames.map((edit) => {
    const original = frames[edit.sourceIndex]
    const source = { ...original, data: applyImageAlphaStrokes(original, edit.eraseStrokes, work) }
    const { width, height } = source
    const data = new Uint8ClampedArray(width * height * 4)
    const radians = (edit.rotation * Math.PI) / 180
    const cos = Math.cos(radians),
      sin = Math.sin(radians)
    // 人工编辑与预览使用原帧固定画布。不能复用自动对齐的包围盒缩放，
    // 否则平移会被重新居中抵消，放大结果也会被缩回原尺寸。
    for (let y = 0; y < height; y++)
      for (let x = 0; x < width; x++) {
        const tx = x + 0.5 - width / 2 - edit.x
        const ty = y + 0.5 - height / 2 - edit.y
        const sx = Math.floor((tx * cos + ty * sin) / edit.scale + width / 2)
        const sy = Math.floor((-tx * sin + ty * cos) / edit.scale + height / 2)
        if (sx < 0 || sy < 0 || sx >= width || sy >= height) continue
        const from = (sy * width + sx) * 4,
          to = (y * width + x) * 4
        data.set(source.data.subarray(from, from + 4), to)
        data[to + 3] = Math.round(data[to + 3] * edit.opacity)
      }
    return { ...source, data, durationMs: edit.durationMs, columns: 1, rows: 1, frameCount: 1 }
  })
}

/** @param {Pixels[]} frames @param {Settings} settings */
function makeAtlas(frames, settings) {
  const padding = settings.padding
  const cellWidth = Math.max(...frames.map((frame) => frame.width)) + padding * 2
  const cellHeight = Math.max(...frames.map((frame) => frame.height)) + padding * 2
  const layout = spriteSheetLayout(cellWidth, cellHeight, frames.length, 4096)
  bounds(layout.width, layout.height)
  const data = new Uint8ClampedArray(layout.width * layout.height * 4)
  const metadata = frames.map((frame, index) => {
    const x =
      (index % layout.columns) * cellWidth +
      padding +
      Math.floor((cellWidth - padding * 2 - frame.width) / 2)
    const y =
      Math.floor(index / layout.columns) * cellHeight +
      padding +
      Math.floor((cellHeight - padding * 2 - frame.height) / 2)
    for (let row = 0; row < frame.height; row++)
      data.set(
        frame.data.subarray(row * frame.width * 4, (row + 1) * frame.width * 4),
        ((y + row) * layout.width + x) * 4,
      )
    return {
      x,
      y,
      width: frame.width,
      height: frame.height,
      durationMs: frame.durationMs,
      action: frame.action,
      direction: frame.direction,
    }
  })
  return {
    buffer: pngBuffer(layout.width, layout.height, data),
    width: layout.width,
    height: layout.height,
    frames: metadata,
  }
}

/** @param {Pixels} frame */
function recommendBackground(frame) {
  const colors = extractPalette(frame.data, frame.width, frame.height, 8)
  const presets = ['#FF00FF', '#00FF00', '#00FFFF', '#FFFFFF', '#000000']
  let best = presets[0],
    distance = -1
  for (const preset of presets) {
    const candidate = rgb(preset)
    const score = colors.length
      ? Math.min(
          ...colors.map((color) =>
            Math.max(...color.map((value, index) => Math.abs(value - candidate[index]))),
          ),
        )
      : 0
    if (score > distance) {
      distance = score
      best = preset
    }
  }
  return best
}

async function main() {
  const { operation, settings, engineDirectory } = workerData
  /** @type {Pixels[]} */
  let frames = []
  for (const frame of workerData.frames) frames.push(await decode(frame))
  if (operation === 'palette')
    return { frames: [], recommendedBackground: recommendBackground(frames[0]) }
  if (operation === 'background')
    frames =
      settings.method === 'model'
        ? await modelBackground(frames, engineDirectory)
        : frames.map((frame) => colorBackground(frame, settings))
  else if (operation === 'inpaint') frames = await inpaint(frames, settings, engineDirectory)
  else if (operation === 'frames') frames = splitFrames(frames, settings)
  else if (operation === 'transform') frames = transformFrames(frames, settings)
  else if (operation === 'edit') frames = editFrames(frames, workerData.edits)
  const pixels = frames.reduce((sum, frame) => sum + frame.width * frame.height, 0)
  if (frames.length > 512 || pixels > LIMIT) throw imageOperationError('workflow_image_too_large')
  return {
    frames: frames.map(encode),
    ...(operation === 'export' ? { atlas: makeAtlas(frames, settings) } : {}),
  }
}

void main()
  .then((result) => parentPort?.postMessage(result))
  .catch((error) => {
    // 不跨线程透出底层文件路径、解析器栈和输入内容。
    parentPort?.postMessage({
      error:
        typeof error?.code === 'string' && error.code.startsWith('workflow_image_')
          ? error.code
          : 'workflow_image_processing_failed',
    })
  })
