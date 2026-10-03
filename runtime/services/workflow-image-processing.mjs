import { Worker } from 'node:worker_threads'
import { normalizeImageSettings, imageOperationError } from '../../shared/image/image-operations.mjs'
import { assertRasterBounds, readRasterDimensions } from '../../shared/image/raster-image.mjs'
import { normalizeImageFrameEdits } from '../../shared/image/image-frame-edits.mjs'

/** @typedef {import('../../shared/image/image-operations.mjs').ImageSettings} ImageSettings */
/** @typedef {{ buffer: Uint8Array, mimeType: string, width: number, height: number, durationMs: number, action: string, direction: string, columns: number, rows: number, frameCount: number }} ProcessingFrame */
/** @typedef {Omit<ProcessingFrame, 'buffer' | 'mimeType'> & { buffer: Buffer, mimeType: 'image/png' }} ProcessedFrame */
/** @typedef {{ frames: ProcessedFrame[], recommendedBackground?: string, atlas?: { buffer: Buffer, width: number, height: number, frames: Array<{ x: number, y: number, width: number, height: number, durationMs: number, action: string, direction: string }> } }} ProcessingResult */
/** @typedef {{ operation: 'background' | 'inpaint' | 'frames' | 'transform' | 'edit' | 'export' | 'palette', frames: ProcessingFrame[], settings: ImageSettings, edits?: import('../../shared/image/image-frame-edits.mjs').ImageFrameEdits }} ProcessingRequest */

/** 先检查压缩头与总量，避免将不可信的超大图片送入解码器。 @param {ProcessingFrame[]} frames */
function validateFrames(frames) {
  if (!Array.isArray(frames) || frames.length < 1 || frames.length > 512)
    throw imageOperationError('workflow_image_invalid')
  let pixels = 0
  let bytes = 0
  for (const frame of frames) {
    if (
      !frame ||
      !(frame.buffer instanceof Uint8Array) ||
      frame.buffer.byteLength > 8 * 1024 * 1024
    )
      throw imageOperationError('workflow_image_invalid')
    const dimensions = readRasterDimensions(frame.buffer)
    assertRasterBounds(dimensions)
    if (
      !dimensions ||
      dimensions.mimeType !== frame.mimeType ||
      dimensions.width !== frame.width ||
      dimensions.height !== frame.height
    )
      throw imageOperationError('workflow_image_invalid')
    if (
      !Number.isSafeInteger(frame.durationMs) ||
      frame.durationMs < 16 ||
      frame.durationMs > 10000 ||
      typeof frame.action !== 'string' ||
      frame.action.length > 160 ||
      typeof frame.direction !== 'string' ||
      frame.direction.length > 16 ||
      !Number.isSafeInteger(frame.columns) ||
      frame.columns < 1 ||
      frame.columns > 16 ||
      !Number.isSafeInteger(frame.rows) ||
      frame.rows < 1 ||
      frame.rows > 16 ||
      !Number.isSafeInteger(frame.frameCount) ||
      frame.frameCount < 1 ||
      frame.frameCount > 256
    )
      throw imageOperationError('workflow_image_invalid')
    pixels += frame.width * frame.height
    bytes += frame.buffer.byteLength
    if (pixels > 16_000_000 || bytes > 64 * 1024 * 1024)
      throw imageOperationError('workflow_image_too_large')
  }
}

/** 图片计算只在短命 Worker 中运行；队列所有者独立于任何页面观察者。 */
export class WorkflowImageProcessor {
  /** @param {{ engines: { getExecutionDirectory: (id: string) => Promise<string> } }} dependencies */
  constructor({ engines }) {
    this.engines = engines
    this.queue = Promise.resolve()
    /** @type {Set<AbortController>} */
    this.jobs = new Set()
    this.closed = false
  }

  /** @param {ProcessingRequest} request @param {{ signal?: AbortSignal }} [options] @returns {Promise<ProcessingResult>} */
  async process(request, { signal } = {}) {
    if (this.closed) throw imageOperationError('workflow_image_closed')
    signal?.throwIfAborted()
    if (
      !['background', 'inpaint', 'frames', 'transform', 'edit', 'export', 'palette'].includes(
        request.operation,
      )
    )
      throw imageOperationError('workflow_image_invalid')
    validateFrames(request.frames)
    const settings = normalizeImageSettings(request.settings)
    const edits = request.operation === 'edit' ? normalizeImageFrameEdits(request.edits) : undefined
    if (edits?.frames.some((frame) => frame.sourceIndex >= request.frames.length))
      throw imageOperationError('workflow_image_invalid_edits')
    const controller = new AbortController()
    this.jobs.add(controller)
    const abort = () => controller.abort(signal?.reason)
    signal?.addEventListener('abort', abort, { once: true })
    const task = this.queue.then(async () => {
      controller.signal.throwIfAborted()
      const engineDirectory =
        request.operation === 'inpaint'
          ? await this.engines.getExecutionDirectory('inpaint')
          : request.operation === 'background' && settings.method === 'model'
            ? await this.engines.getExecutionDirectory('background')
            : ''
      controller.signal.throwIfAborted()
      return this.execute({ ...request, settings, edits }, engineDirectory, controller.signal)
    })
    this.queue = task.then(
      () => {},
      () => {},
    )
    // 已取消的排队调用立即返回；队列中的占位仍等待前项结束并检查取消。
    let cancel = () => {}
    /** @type {Promise<never>} */
    const cancelled = new Promise((_, reject) => {
      cancel = () => reject(controller.signal.reason)
      controller.signal.addEventListener('abort', cancel, { once: true })
      if (controller.signal.aborted) cancel()
    })
    try {
      return await Promise.race([task, cancelled])
    } finally {
      signal?.removeEventListener('abort', abort)
      controller.signal.removeEventListener('abort', cancel)
      this.jobs.delete(controller)
    }
  }

  /** @param {{buffer: Uint8Array, mimeType: string}} image @param {{signal?:AbortSignal}} [options] */
  async suggestBackground(image, options) {
    const dimensions = readRasterDimensions(image.buffer)
    assertRasterBounds(dimensions)
    if (!dimensions) throw imageOperationError('workflow_image_invalid')
    const result = await this.process(
      {
        operation: 'palette',
        frames: [
          {
            ...image,
            width: dimensions.width,
            height: dimensions.height,
            durationMs: 125,
            action: '',
            direction: '',
            columns: 1,
            rows: 1,
            frameCount: 1,
          },
        ],
        settings: normalizeImageSettings(),
      },
      options,
    )
    if (!result.recommendedBackground || !/^#[0-9a-f]{6}$/i.test(result.recommendedBackground))
      throw imageOperationError('workflow_image_processing_failed')
    return result.recommendedBackground
  }

  /** @param {ProcessingRequest} request @param {string} engineDirectory @param {AbortSignal} signal @returns {Promise<ProcessingResult>} */
  async execute(request, engineDirectory, signal) {
    signal.throwIfAborted()
    const worker = new Worker(new URL('../workers/workflow-image-worker.mjs', import.meta.url), {
      workerData: { ...request, engineDirectory },
      // 独立 ESM 文件不继承启动器的 --input-type、SEA 或调试参数。
      execArgv: [],
      resourceLimits: { maxOldGenerationSizeMb: 256 },
    })
    let abort = () => {}
    const timer = setTimeout(() => abort(), 120_000)
    try {
      /** @type {ProcessingResult} */
      const result = await new Promise((resolve, reject) => {
        abort = () =>
          reject(signal.aborted ? signal.reason : imageOperationError('workflow_image_timeout'))
        signal.addEventListener('abort', abort, { once: true })
        worker.once('message', (message) => {
          if (message?.error) {
            reject(
              imageOperationError(
                typeof message.error === 'string' && /^workflow_image_[a-z_]+$/.test(message.error)
                  ? message.error
                  : 'workflow_image_processing_failed',
              ),
            )
          } else if (message && Array.isArray(message.frames)) resolve(message)
          else reject(imageOperationError('workflow_image_processing_failed'))
        })
        worker.once('error', () => reject(imageOperationError('workflow_image_processing_failed')))
        worker.once('exit', () => reject(imageOperationError('workflow_image_processing_failed')))
        if (signal.aborted) abort()
      })
      return {
        ...result,
        frames: result.frames.map((frame) => ({ ...frame, buffer: Buffer.from(frame.buffer) })),
        ...(result.atlas
          ? { atlas: { ...result.atlas, buffer: Buffer.from(result.atlas.buffer) } }
          : {}),
      }
    } finally {
      clearTimeout(timer)
      signal.removeEventListener('abort', abort)
      await worker.terminate()
    }
  }

  async dispose() {
    this.closed = true
    for (const controller of this.jobs) controller.abort()
    await this.queue
  }
}
