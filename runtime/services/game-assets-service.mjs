import { randomUUID } from 'node:crypto'
import { lstat, mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import {
  GameAssetError,
  parseGameAssetProjectInput,
  parseGameAssetProject,
  parseGameAssetJob,
  parseGameAssetsCatalog,
} from '../../shared/game-assets.mjs'
import { parseImageOutput } from '../../shared/image-operations.mjs'
import { normalizeImageFrameEdits } from '../../shared/image-frame-edits.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

/** @typedef {import('../../shared/game-assets.mjs').GameAssetProject} Project */
/** @typedef {import('../../shared/game-assets.mjs').GameAssetJob} Job */
/** @typedef {import('../../shared/game-assets.mjs').GameAssetsCatalog} Catalog */
/** @typedef {import('../../shared/workflow-inputs.mjs').WorkflowMedia} Media */
/** @typedef {import('../../shared/image-operations.mjs').ImageOutput} Output */
/** @typedef {import('../../shared/image-operations.mjs').ImageSettings} Settings */
/** @typedef {{operation:'input'|'background'|'inpaint'|'generate'|'frames'|'transform'|'preview'|'export'|'edit',settings?:Partial<Settings>,source?:Media,images?:Output['frames'],prompt?:string,model?:Project['model'],resumeOutput?:Output,edits?:import('../../shared/image-frame-edits.mjs').ImageFrameEdits,signal?:AbortSignal}} ImageOperation */
/** @typedef {{execute(input:ImageOperation):Promise<{output:Output,summary:string}>}} Operations */
const SAFE_ERRORS = new Set([
  'game_assets_cancelled',
  'game_assets_storage_failed',
  'game_assets_media_invalid',
  'workflow_image_invalid',
  'workflow_image_source_required',
  'workflow_image_too_large',
  'workflow_image_cancelled',
  'workflow_image_closed',
  'workflow_image_generation_failed',
  'workflow_image_processing_failed',
  'workflow_image_timeout',
  'workflow_media_invalid',
  'workflow_media_missing',
  'workflow_media_too_large',
  'sprite_engine_missing',
  'sprite_engine_invalid',
  'workflow_image_invalid_edits',
])
/** @returns {Output} */
function emptyOutput() {
  return { type: 'workflow-images', version: 1, frames: [] }
}
/** @param {AbortSignal} signal */
function active(signal) {
  if (signal.aborted) throw new GameAssetError('game_assets_cancelled')
}
/** @param {unknown} failure */
function errorCode(failure) {
  const code = failure && typeof failure === 'object' && 'code' in failure ? failure.code : null
  return typeof code === 'string' && SAFE_ERRORS.has(code) ? code : 'game_assets_processing_failed'
}
/** @param {Media} left @param {Media} right */
function sameMedia(left, right) {
  return (
    left.id === right.id &&
    left.name === right.name &&
    left.size === right.size &&
    left.mimeType === right.mimeType
  )
}

export class GameAssetsService {
  /** @param {{dataDir:string,media:Pick<import('./workflow-media-service.mjs').WorkflowMediaService,'init'|'load'>,operations:Operations}} options */
  constructor({ dataDir, media, operations }) {
    this.path = join(dataDir, 'game-assets.json')
    this.dataDir = dataDir
    this.media = media
    this.operations = operations
    /** @type {Catalog} */
    this.state = { projects: [], jobs: [] }
    /** @type {Promise<void>|null} */
    this.initializing = null
    this.queue = Promise.resolve()
    /** @type {Map<string,{projectId:string,controller:AbortController,promise:Promise<void>}>} */
    this.running = new Map()
    this.closed = false
    /** @type {Promise<void>|null} */
    this.disposing = null
  }

  init() {
    this.initializing ??= (async () => {
      await mkdir(this.dataDir, { recursive: true, mode: 0o700 })
      await this.media.init()
      const info = await lstat(this.path).catch((failure) => {
        if (
          failure &&
          typeof failure === 'object' &&
          'code' in failure &&
          failure.code === 'ENOENT'
        )
          return null
        throw failure
      })
      if (info && (!info.isFile() || info.isSymbolicLink() || info.size > 64 * 1024 * 1024))
        throw new GameAssetError('game_assets_storage_invalid', 500)
      if (!info) return
      /** @type {unknown} */
      let stored
      try {
        stored = await readJson(this.path, null)
      } catch {
        throw new GameAssetError('game_assets_storage_invalid', 500)
      }
      if (
        !stored ||
        typeof stored !== 'object' ||
        !('version' in stored) ||
        stored.version !== 1 ||
        !('projects' in stored) ||
        !('jobs' in stored) ||
        Object.keys(stored).some((key) => !['version', 'projects', 'jobs'].includes(key))
      )
        throw new GameAssetError('game_assets_storage_invalid', 500)
      let parsed
      try {
        parsed = parseGameAssetsCatalog({ projects: stored.projects, jobs: stored.jobs })
      } catch {
        throw new GameAssetError('game_assets_storage_invalid', 500)
      }
      const interrupted = parsed.jobs.some((job) => job.status === 'running')
      const recovered = {
        projects: parsed.projects,
        jobs: parsed.jobs.map((job) =>
          job.status === 'running'
            ? parseGameAssetJob({
                ...job,
                status: 'interrupted',
                finishedAt: new Date().toISOString(),
                error: 'game_assets_interrupted',
              })
            : job,
        ),
      }
      // 重启只标记中断，不重新产生付费请求；先持久化，后公开恢复结果。
      if (interrupted) await this.write(recovered)
      this.state = recovered
    })()
    return this.initializing
  }

  /** @param {Catalog} state */
  async write(state) {
    try {
      await writeJsonAtomic(this.path, { version: 1, ...state }, { mode: 0o600 })
    } catch {
      throw new GameAssetError('game_assets_storage_failed', 500)
    }
  }

  /** @param {Catalog} state */
  async commit(state) {
    const parsed = parseGameAssetsCatalog(state)
    await this.write(parsed)
    this.state = parsed
  }

  /** @template T @param {()=>Promise<T>} operation @param {boolean} [internal] @returns {Promise<T>} */
  async exclusive(operation, internal = false) {
    await this.init()
    const pending = this.queue.then(() => {
      if (this.closed && !internal) throw new GameAssetError('game_assets_closed', 503)
      return operation()
    })
    this.queue = pending.then(
      () => {},
      () => {},
    )
    return pending
  }

  async catalog() {
    await this.init()
    return structuredClone(this.state)
  }

  /** @param {Media|null} reference */
  async validateReference(reference) {
    if (!reference) return
    try {
      const { metadata } = await this.media.load(reference.id)
      if (!sameMedia(reference, metadata.media) || !reference.mimeType.startsWith('image/'))
        throw new GameAssetError('game_assets_media_invalid')
    } catch {
      throw new GameAssetError('game_assets_media_invalid')
    }
  }

  /** @param {unknown} value */
  async validateOutput(value) {
    const output = parseImageOutput(value)
    /** @type {Map<string,Media>} */
    const checked = new Map()
    for (const reference of [
      ...output.frames.map((frame) => frame.media),
      ...(output.atlas ? [output.atlas.media] : []),
    ]) {
      const previous = checked.get(reference.id)
      if (previous && !sameMedia(previous, reference))
        throw new GameAssetError('game_assets_media_invalid')
      if (!previous) await this.validateReference(reference)
      checked.set(reference.id, reference)
    }
    return output
  }

  /** @param {string} projectId */
  assertAvailable(projectId) {
    if ([...this.running.values()].some((task) => task.projectId === projectId))
      throw new GameAssetError('game_assets_busy', 409)
  }

  /** @param {unknown} value */
  save(value) {
    const input = parseGameAssetProjectInput(value)
    return this.exclusive(async () => {
      const previous = this.state.projects.find((project) => project.id === input.id)
      if (input.id && !previous) throw new GameAssetError('game_assets_not_found', 404)
      if (!previous && this.state.projects.length >= 100)
        throw new GameAssetError('game_assets_limit', 409)
      if (previous) this.assertAvailable(previous.id)
      await this.validateReference(input.reference)
      await this.validateReference(input.originalReference)
      const now = new Date().toISOString()
      const project = parseGameAssetProject({
        ...input,
        id: previous?.id ?? randomUUID(),
        createdAt: previous?.createdAt ?? now,
        updatedAt: now,
      })
      await this.commit({
        ...this.state,
        projects: [project, ...this.state.projects.filter((entry) => entry.id !== project.id)],
      })
      return structuredClone(project)
    })
  }

  /** @param {string} id */
  remove(id) {
    return this.exclusive(async () => {
      if (!this.state.projects.some((project) => project.id === id))
        throw new GameAssetError('game_assets_not_found', 404)
      this.assertAvailable(id)
      await this.commit({
        projects: this.state.projects.filter((project) => project.id !== id),
        jobs: this.state.jobs.filter((job) => job.projectId !== id),
      })
    })
  }

  /** @param {string} id */
  async getJob(id) {
    await this.init()
    const job = this.state.jobs.find((entry) => entry.id === id)
    return job ? structuredClone(job) : null
  }

  /** @param {string} projectId */
  run(projectId) {
    return this.exclusive(async () => {
      const project = this.state.projects.find((entry) => entry.id === projectId)
      if (!project) throw new GameAssetError('game_assets_not_found', 404)
      this.assertAvailable(projectId)
      if (this.running.size >= 2) throw new GameAssetError('game_assets_busy', 409)
      if (!project.reference) throw new GameAssetError('game_assets_source_required')
      const enabled = project.actions.filter((action) => action.enabled)
      if (!enabled.length) throw new GameAssetError('game_assets_actions_required')
      await this.validateReference(project.reference)
      const job = parseGameAssetJob({
        id: randomUUID(),
        projectId,
        status: 'running',
        startedAt: new Date().toISOString(),
        finishedAt: null,
        completed: 0,
        total: enabled.length,
        error: null,
        output: emptyOutput(),
        originalOutput: emptyOutput(),
        revision: 0,
      })
      // 同一项目保留最近一次任务；跨项目最多 100 份结果，不挤掉仍运行的项目。
      await this.commit({
        ...this.state,
        jobs: [job, ...this.state.jobs.filter((entry) => entry.projectId !== projectId)].slice(
          0,
          100,
        ),
      })
      const controller = new AbortController()
      const promise = Promise.resolve()
        .then(() => this.executeProject(project, job, controller.signal))
        .finally(() => this.running.delete(job.id))
      this.running.set(job.id, { projectId, controller, promise })
      // 后台存储错误留给 getJob 和 dispose 返回，避免未观察的 Promise 拒绝。
      void promise.catch(() => {})
      return structuredClone(job)
    })
  }

  /** @param {Job} job */
  persistJob(job) {
    return this.exclusive(async () => {
      await this.commit({
        ...this.state,
        jobs: this.state.jobs.map((entry) => (entry.id === job.id ? job : entry)),
      })
    }, true)
  }

  /** @param {Project} project @param {Job} originalJob @param {AbortSignal} signal */
  async executeProject(project, originalJob, signal) {
    let job = structuredClone(originalJob)
    let pending = emptyOutput()
    try {
      active(signal)
      const reference = project.reference
      if (!reference) throw new GameAssetError('game_assets_source_required')
      const input = await this.operations.execute({ operation: 'input', source: reference, signal })
      const source = await this.validateOutput(input.output)
      for (const action of project.actions.filter((entry) => entry.enabled)) {
        active(signal)
        const settings = {
          action: action.name,
          frameCount: project.frameCount,
          directions: project.directions,
          method: /** @type {const} */ ('color'),
          maxFrameSize: 256,
        }
        for (const operation of /** @type {const} */ ([
          'generate',
          'background',
          'frames',
          'transform',
        ])) {
          active(signal)
          const result = await this.operations.execute({
            operation,
            images: operation === 'generate' ? source.frames : pending.frames,
            settings,
            prompt: [project.prompt, action.prompt].filter(Boolean).join('\n\n'),
            model: project.model,
            signal,
          })
          pending = await this.validateOutput(result.output)
        }
        active(signal)
        const output = { ...emptyOutput(), frames: [...job.output.frames, ...pending.frames] }
        job = parseGameAssetJob({
          ...job,
          completed: job.completed + 1,
          output,
          originalOutput: output,
        })
        pending = emptyOutput()
        await this.persistJob(job)
      }
      active(signal)
      const exported = await this.operations.execute({
        operation: 'export',
        images: job.output.frames,
        settings: { filename: project.name, maxFrameSize: 256 },
        signal,
      })
      const output = await this.validateOutput(exported.output)
      active(signal)
      job = parseGameAssetJob({
        ...job,
        status: 'completed',
        finishedAt: new Date().toISOString(),
        output,
        originalOutput: output,
      })
      await this.persistJob(job)
    } catch (failure) {
      if (failure && typeof failure === 'object' && 'partialOutput' in failure) {
        try {
          pending = await this.validateOutput(failure.partialOutput)
        } catch {
          /* 保留此前已验证的阶段产物。 */
        }
      }
      let output = job.output
      if (pending.frames.length) {
        try {
          output = parseImageOutput({
            ...emptyOutput(),
            frames: [...job.output.frames, ...pending.frames],
          })
        } catch {
          /* 不接受越界或损坏的部分结果。 */
        }
      }
      job = parseGameAssetJob({
        ...job,
        status: signal.aborted ? 'cancelled' : 'failed',
        finishedAt: new Date().toISOString(),
        error: signal.aborted ? 'game_assets_cancelled' : errorCode(failure),
        output,
        originalOutput: output,
      })
      try {
        await this.persistJob(job)
      } catch {
        // 磁盘不可写时不把后台任务永远显示为运行；磁盘中的旧任务在重启时变为 interrupted。
        this.state = {
          ...this.state,
          jobs: this.state.jobs.map((entry) =>
            entry.id === job.id ? { ...job, error: 'game_assets_storage_failed' } : entry,
          ),
        }
        throw new GameAssetError('game_assets_storage_failed', 500)
      }
    }
  }

  /** @param {string} jobId @param {unknown} value */
  async edit(jobId, value) {
    const edits = normalizeImageFrameEdits(value)
    const task = await this.exclusive(async () => {
      const job = this.state.jobs.find((entry) => entry.id === jobId)
      if (!job) throw new GameAssetError('game_assets_not_found', 404)
      this.assertAvailable(job.projectId)
      if (job.status === 'running' || this.running.size >= 2)
        throw new GameAssetError('game_assets_busy', 409)
      if (
        !job.originalOutput.frames.length ||
        edits.frames.some((frame) => frame.sourceIndex >= job.originalOutput.frames.length)
      )
        throw new GameAssetError('game_assets_invalid')
      const source = await this.validateOutput(job.originalOutput)
      const controller = new AbortController()
      const { signal } = controller
      const promise = Promise.resolve()
        .then(async () => {
          try {
            active(signal)
            // 每次从未修改的帧重放编辑，复制、删除、排序都由 sourceIndex 序列表示。
            const edited = await this.operations.execute({
              operation: 'edit',
              images: source.frames,
              edits,
              settings: { maxFrameSize: 256 },
              signal,
            })
            const frames = await this.validateOutput(edited.output)
            active(signal)
            const project = this.state.projects.find((entry) => entry.id === job.projectId)
            const exported = await this.operations.execute({
              operation: 'export',
              images: frames.frames,
              settings: { filename: project?.name ?? 'animation', maxFrameSize: 256 },
              signal,
            })
            const output = await this.validateOutput(exported.output)
            active(signal)
            await this.persistJob(
              parseGameAssetJob({ ...job, output, edits, revision: job.revision + 1 }),
            )
          } catch (failure) {
            throw new GameAssetError(signal.aborted ? 'game_assets_cancelled' : errorCode(failure))
          }
        })
        .finally(() => this.running.delete(jobId))
      // 编辑取消不改变原任务结果；等待关闭和停止的调用方无需把取消当存储失败。
      const tracked = promise.catch((failure) => {
        if (!signal.aborted) throw failure
      })
      this.running.set(jobId, { projectId: job.projectId, controller, promise: tracked })
      void tracked.catch(() => {})
      return { promise }
    })
    await task.promise
    return this.getJob(jobId)
  }

  /** @param {string} jobId */
  async stop(jobId) {
    await this.init()
    await this.queue
    const job = this.state.jobs.find((entry) => entry.id === jobId)
    if (!job) throw new GameAssetError('game_assets_not_found', 404)
    const running = this.running.get(jobId)
    if (running) {
      running.controller.abort()
      await running.promise
    }
    return this.getJob(jobId)
  }

  dispose() {
    this.disposing ??= (async () => {
      this.closed = true
      // 初始化错误已由入口返回；关闭时仍需推进其他组件的清理，且不覆盖损坏存储。
      await this.initializing?.catch(() => {})
      await this.queue
      const running = [...this.running.values()]
      for (const task of running) task.controller.abort()
      const results = await Promise.allSettled(running.map((task) => task.promise))
      await this.queue
      if (
        results.some(
          (result) =>
            result.status === 'rejected' &&
            errorCode(result.reason) === 'game_assets_storage_failed',
        )
      )
        throw new GameAssetError('game_assets_storage_failed', 500)
    })()
    return this.disposing
  }
}
