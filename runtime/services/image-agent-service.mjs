import { importAgentImage } from './image-agent-media.mjs'
import { exportAgentImages } from './image-agent-export.mjs'

/** @typedef {import('./image-operation-service.mjs').Execution} ImageExecution */
/** @typedef {Pick<import('./workflow-media-service.mjs').WorkflowMediaService,'upload'|'read'>} AgentMedia */

/**
 * Agent 图像调用的生命周期所有者。导入/导出也必须被等待，
 * 不能仅关闭像素 Worker 后就关闭这些调用仍在使用的媒体存储。
 */
export class ImageAgentService {
  /** @param {{operations:Pick<import('./image-operation-service.mjs').ImageOperationService,'execute'>,media:AgentMedia,plugin:{assertAllowed:(consumer:'agent')=>Promise<void>}}} dependencies */
  constructor({ operations, media, plugin }) {
    this.operations = operations
    this.media = media
    this.plugin = plugin
    this.closed = false
    /** @type {Map<Promise<unknown>,AbortController>} */
    this.active = new Map()
    /** @type {Promise<void>|null} */
    this.disposing = null
  }

  /** @template T @param {(signal:AbortSignal)=>Promise<T>} operation @param {AbortSignal} [signal] @returns {Promise<T>} */
  run(operation, signal) {
    if (this.closed)
      return Promise.reject(
        Object.assign(new Error('image_tools_closed'), {
          code: 'image_tools_closed',
          statusCode: 503,
        }),
      )
    const controller = new AbortController()
    const combined = signal ? AbortSignal.any([signal, controller.signal]) : controller.signal
    // 先登记再启动，关闭与调用发生在同一轮事件循环时仍能取消尚未开始的任务。
    const pending = Promise.resolve()
      .then(() => {
        combined.throwIfAborted()
        return operation(combined)
      })
      .finally(() => this.active.delete(pending))
    this.active.set(pending, controller)
    return pending
  }

  /** @param {ImageExecution} request @param {{signal?:AbortSignal}} [options] */
  execute(request, { signal } = {}) {
    const callerSignal =
      signal && request.signal
        ? AbortSignal.any([signal, request.signal])
        : (signal ?? request.signal)
    return this.run(async (combined) => {
      await this.plugin.assertAllowed('agent')
      combined.throwIfAborted()
      return this.operations.execute({ ...request, signal: combined })
    }, callerSignal)
  }

  /** @param {string} cwd @param {string} sourceImage @param {{signal?:AbortSignal}} [options] */
  importImage(cwd, sourceImage, { signal } = {}) {
    return this.run(
      (combined) =>
        importAgentImage({
          cwd,
          sourceImage,
          media: this.media,
          plugin: this.plugin,
          signal: combined,
        }),
      signal,
    )
  }

  /** @param {string} cwd @param {unknown} output @param {{signal?:AbortSignal}} [options] */
  exportImages(cwd, output, { signal } = {}) {
    return this.run(
      (combined) =>
        exportAgentImages({
          cwd,
          output,
          media: this.media,
          plugin: this.plugin,
          signal: combined,
        }),
      signal,
    )
  }

  dispose() {
    if (this.disposing) return this.disposing
    this.closed = true
    for (const controller of this.active.values()) controller.abort()
    // 媒体提交可能不能中途撤销；等全部承诺完成后才允许外层关闭存储。
    this.disposing = Promise.allSettled(this.active.keys()).then(() => {})
    return this.disposing
  }
}
