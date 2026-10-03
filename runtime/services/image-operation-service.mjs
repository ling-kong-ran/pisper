import { constants } from 'node:fs'
import { lstat, mkdir, mkdtemp, open, realpath, rm, writeFile } from 'node:fs/promises'
import { isAbsolute, join, relative, resolve, sep } from 'node:path'
import { assertRasterBounds, readRasterDimensions } from '../../shared/image/raster-image.mjs'
import { parseWorkflowMedia } from '../../shared/workflow/workflow-inputs.mjs'
import {
  normalizeImageSettings,
  parseImageOutput,
  imageOperationError,
} from '../../shared/image/image-operations.mjs'
import {
  buildActionSheetPrompt,
  suggestActionSheetGrid,
} from '../../shared/vendor/framebaker/action-prompts.mjs'

/** @typedef {import('../../shared/workflow/workflow-inputs.mjs').WorkflowMedia} Media */
/** @typedef {import('../../shared/image/image-operations.mjs').ImageFrame} Frame */
/** @typedef {import('../../shared/image/image-operations.mjs').ImageOutput} Output */
/** @typedef {import('../../shared/image/image-operations.mjs').ImageSettings} Settings */
/** @typedef {Omit<Frame, 'media'> & {buffer: Uint8Array, mimeType: string}} Pixels */
/** @typedef {{frames: Pixels[], atlas?: {buffer: Uint8Array, width: number, height: number, frames: NonNullable<Output['atlas']>['frames']}}} Processed */
/** @typedef {{process(input:{operation:'background'|'frames'|'transform'|'export'|'inpaint'|'edit',edits?:unknown,frames:Pixels[],settings:Settings},options:{signal:AbortSignal}):Promise<Processed>,suggestBackground(input:{buffer:Uint8Array,mimeType:string},options:{signal:AbortSignal}):Promise<string>}} Processor */
/** @typedef {{kind:'image',prompt:string,cwd:string,model:string,sourceImages:string[],outputName:string,outputFormat:'png',aspectRatio:string}} ImageRequest */
/** @typedef {{operation:'input'|'background'|'inpaint'|'generate'|'frames'|'transform'|'preview'|'export'|'edit',settings?:unknown,source?:Media,images?:Frame[],prompt?:string,model?:{provider:string,model:string}|null,signal?:AbortSignal,resumeOutput?:unknown,edits?:unknown}} Execution */
const MAX_IMAGE_BYTES = 8 * 1024 * 1024
const MAX_BATCH_BYTES = 128 * 1024 * 1024
const EXTENSIONS = new Map([
  ['image/png', 'png'],
  ['image/jpeg', 'jpg'],
  ['image/webp', 'webp'],
])
const SAFE_ERRORS = new Set([
  'workflow_image_invalid',
  'workflow_image_invalid_edits',
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
])
const DIRECTIONS = {
  S: 'FRONT (face and chest toward viewer)',
  SW: 'FRONT-LEFT three-quarter',
  W: 'LEFT profile',
  NW: 'BACK-LEFT three-quarter',
  N: 'BACK (back toward viewer)',
  NE: 'BACK-RIGHT three-quarter',
  E: 'RIGHT profile',
  SE: 'FRONT-RIGHT three-quarter',
}

/** @param {AbortSignal} signal */
function active(signal) {
  if (signal.aborted) throw imageOperationError('workflow_image_cancelled')
}

/** @param {Uint8Array} bytes @param {string} mimeType */
function dimensions(bytes, mimeType) {
  if (!bytes.byteLength || bytes.byteLength > MAX_IMAGE_BYTES)
    throw imageOperationError('workflow_image_too_large')
  const value = readRasterDimensions(bytes)
  if (!value || value.mimeType !== mimeType) throw imageOperationError('workflow_image_invalid')
  try {
    assertRasterBounds(value)
  } catch {
    throw imageOperationError('workflow_image_too_large')
  }
  return value
}

/** @param {string} path */
async function assertDirectory(path) {
  const info = await lstat(path)
  if (!info.isDirectory() || info.isSymbolicLink() || (await realpath(path)) !== path)
    throw imageOperationError('workflow_image_invalid')
}

/** @param {unknown} result @param {string} jobDirectory */
async function readGeneratedImage(result, jobDirectory) {
  if (
    !result ||
    typeof result !== 'object' ||
    !('path' in result) ||
    typeof result.path !== 'string' ||
    !('mimeType' in result) ||
    typeof result.mimeType !== 'string'
  )
    throw imageOperationError('workflow_image_invalid')
  const generated = join(jobDirectory, 'generated')
  const directory = join(generated, 'visuals')
  const path = resolve(result.path)
  const suffix = relative(directory, path)
  if (
    !suffix ||
    suffix.startsWith(`..${sep}`) ||
    suffix === '..' ||
    isAbsolute(suffix) ||
    suffix.includes(sep)
  )
    throw imageOperationError('workflow_image_invalid')
  await assertDirectory(jobDirectory)
  await assertDirectory(generated)
  await assertDirectory(directory)
  const info = await lstat(path)
  if (!info.isFile() || info.isSymbolicLink() || (await realpath(path)) !== path)
    throw imageOperationError('workflow_image_invalid')
  const handle = await open(path, constants.O_RDONLY | (constants.O_NOFOLLOW || 0))
  try {
    const current = await handle.stat()
    if (!current.isFile() || current.size < 1 || current.size > MAX_IMAGE_BYTES)
      throw imageOperationError('workflow_image_too_large')
    const buffer = Buffer.alloc(current.size)
    let offset = 0
    while (offset < buffer.length) {
      const { bytesRead } = await handle.read(buffer, offset, buffer.length - offset, offset)
      if (!bytesRead) throw imageOperationError('workflow_image_invalid')
      offset += bytesRead
    }
    return { buffer, ...dimensions(buffer, result.mimeType) }
  } finally {
    await handle.close()
  }
}

/** @param {string} action @param {number} index @param {number} count */
function actionPhase(action, index, count) {
  // 各帧使用明确的时间相位，避免模型把同一静止姿势复制到每个格子。
  const phases = /^(idle|待机|呼吸)$/i.test(action)
    ? ['rest', 'inhale rise', 'full breath', 'exhale lower']
    : /^(walk|走路|行走)$/i.test(action)
      ? ['left heel contact', 'left support passing', 'right heel contact', 'right support passing']
      : /^(run|跑步|奔跑)$/i.test(action)
        ? ['left foot contact', 'airborne right lead', 'right foot contact', 'airborne left lead']
        : /^(attack|攻击)$/i.test(action)
          ? [
              'ready',
              'anticipate',
              'wind up',
              'strike',
              'follow through',
              'recover',
              'settle',
              'ready',
            ]
          : []
  return phases[Math.floor((index * phases.length) / count)] || `motion phase ${index + 1}/${count}`
}

/** @param {Settings} settings @param {import('../../shared/image/image-operations.mjs').ImageDirection} direction @param {string} prompt @param {string} background */
function sheetPrompt(settings, direction, prompt, background) {
  const { cols, rows } = suggestActionSheetGrid(settings.frameCount)
  return buildActionSheetPrompt({
    cols,
    rows,
    frames: Array.from({ length: settings.frameCount }, (_, index) => ({
      id: 'action',
      label: settings.action,
      prompt: actionPhase(settings.action, index, settings.frameCount),
    })),
    characterPrompt:
      'Preserve reference art style, palette, identity, outfit, equipment and proportions. Do not convert art style.',
    extra: `Every frame faces ${DIRECTIONS[direction]}; rotate whole body, never just the head. Fixed orthographic camera, scale, ground baseline and lighting. Full body and equipment, 15% safe margin in each equal cell. No grid lines, labels, cast shadows or duplicate poses. Solid ${background} background, including empty cells. ${prompt}`,
  })
}

export class ImageOperationService {
  /** @param {{dataDir:string,media:Pick<import('./workflow-media-service.mjs').WorkflowMediaService,'init'|'load'|'upload'>,processor:Processor,generateImage:(request:ImageRequest,options:{signal:AbortSignal,allowFallback:false})=>Promise<unknown>}} options */
  constructor({ dataDir, media, processor, generateImage }) {
    this.dataDir = resolve(dataDir)
    this.media = media
    this.processor = processor
    this.generateImage = generateImage
    this.closed = false
    /** @type {Promise<string>|null} */
    this.initializing = null
    /** @type {Map<Promise<unknown>,AbortController>} */
    this.active = new Map()
  }

  init() {
    this.initializing ??= (async () => {
      await mkdir(this.dataDir, { recursive: true, mode: 0o700 })
      const root = join(await realpath(this.dataDir), 'image-operation-jobs')
      await mkdir(root, { mode: 0o700 }).catch((error) => {
        if (!error || typeof error !== 'object' || !('code' in error) || error.code !== 'EEXIST')
          throw error
      })
      await assertDirectory(root)
      await this.media.init()
      return root
    })()
    return this.initializing
  }

  /** @param {Execution} execution */
  execute(execution) {
    if (this.closed) return Promise.reject(imageOperationError('workflow_image_closed'))
    const controller = new AbortController()
    const signal = execution.signal
      ? AbortSignal.any([execution.signal, controller.signal])
      : controller.signal
    const promise = this.run({ ...execution, signal }).finally(() => this.active.delete(promise))
    this.active.set(promise, controller)
    return promise
  }

  /** @param {Media} reference @param {AbortSignal} signal */
  async load(reference, signal) {
    active(signal)
    const stored = await this.media.load(reference.id)
    const actual = stored.metadata.media
    if (
      actual.id !== reference.id ||
      actual.name !== reference.name ||
      actual.size !== reference.size ||
      actual.mimeType !== reference.mimeType
    )
      throw imageOperationError('workflow_media_invalid')
    const image = dimensions(stored.buffer, actual.mimeType)
    active(signal)
    return { ...image, buffer: stored.buffer }
  }

  /** @param {Frame[]} frames @param {AbortSignal} signal */
  async pixels(frames, signal) {
    /** @type {Pixels[]} */
    const result = []
    let total = 0
    for (const frame of frames) {
      total += frame.media.size
      if (total > MAX_BATCH_BYTES) throw imageOperationError('workflow_image_too_large')
      const image = await this.load(frame.media, signal)
      if (image.width !== frame.width || image.height !== frame.height)
        throw imageOperationError('workflow_image_invalid')
      const { media: _media, ...metadata } = frame
      result.push({ ...metadata, ...image })
    }
    return result
  }

  /** @param {Pixels} frame @param {string} name @param {AbortSignal} signal @returns {Promise<Frame>} */
  async save(frame, name, signal) {
    active(signal)
    const actual = dimensions(frame.buffer, frame.mimeType)
    if (actual.width !== frame.width || actual.height !== frame.height)
      throw imageOperationError('workflow_image_invalid')
    const media = await this.media.upload({ name, mimeType: frame.mimeType, buffer: frame.buffer })
    return parseImageOutput({
      type: 'workflow-images',
      version: 1,
      frames: [
        {
          media,
          width: actual.width,
          height: actual.height,
          durationMs: frame.durationMs,
          action: frame.action,
          direction: frame.direction,
          columns: frame.columns,
          rows: frame.rows,
          frameCount: frame.frameCount,
        },
      ],
    }).frames[0]
  }

  /** @param {Execution & {signal:AbortSignal}} execution */
  async run({
    operation,
    settings: rawSettings,
    source,
    images,
    prompt,
    model,
    signal,
    resumeOutput,
    edits,
  }) {
    /** @type {Output} */
    const output = { type: 'workflow-images', version: 1, frames: [] }
    let failureCode = 'workflow_image_processing_failed'
    try {
      active(signal)
      if (
        ![
          'input',
          'background',
          'inpaint',
          'generate',
          'frames',
          'transform',
          'preview',
          'export',
          'edit',
        ].includes(operation)
      )
        throw imageOperationError('workflow_image_invalid')
      if (resumeOutput !== undefined && operation !== 'generate')
        throw imageOperationError('workflow_image_invalid')
      const settings = normalizeImageSettings(rawSettings)
      await this.init()
      if (operation === 'input') {
        const reference = parseWorkflowMedia(source)
        const image = await this.load(reference, signal)
        output.frames.push({
          media: reference,
          width: image.width,
          height: image.height,
          durationMs: settings.durationMs,
          action: '',
          direction: '',
          columns: 1,
          rows: 1,
          frameCount: 1,
        })
      } else {
        const frames = parseImageOutput({
          type: 'workflow-images',
          version: 1,
          frames: images ?? [],
        }).frames
        if (!frames.length) throw imageOperationError('workflow_image_source_required')
        if (frames.length > 512) throw imageOperationError('workflow_image_too_large')
        const loaded = await this.pixels(frames, signal)
        if (operation === 'preview') {
          output.frames = frames
        } else if (operation === 'generate') {
          failureCode = 'workflow_image_generation_failed'
          await this.generate({ prompt, model }, settings, loaded, output, signal, resumeOutput)
        } else {
          const processed = await this.processor.process(
            { operation, frames: loaded, settings, ...(edits === undefined ? {} : { edits }) },
            { signal },
          )
          active(signal)
          if (!Array.isArray(processed.frames) || processed.frames.length > 512)
            throw imageOperationError('workflow_image_invalid')
          if (operation === 'export') output.frames = frames
          else
            for (const [index, frame] of processed.frames.entries()) {
              output.frames.push(await this.save(frame, `frame-${index + 1}.png`, signal))
            }
          if (processed.atlas) {
            const atlas = processed.atlas
            const size = dimensions(atlas.buffer, 'image/png')
            if (atlas.width !== size.width || atlas.height !== size.height)
              throw imageOperationError('workflow_image_invalid')
            const media = await this.media.upload({
              name: `${settings.filename || 'animation'}.png`,
              mimeType: 'image/png',
              buffer: atlas.buffer,
            })
            output.atlas = { media, width: size.width, height: size.height, frames: atlas.frames }
          }
        }
      }
      active(signal)
      return { output: parseImageOutput(output), summary: `${output.frames.length} frames` }
    } catch (failure) {
      const candidate =
        failure && typeof failure === 'object' && 'code' in failure ? failure.code : ''
      const code = signal.aborted
        ? 'workflow_image_cancelled'
        : typeof candidate === 'string' && SAFE_ERRORS.has(candidate)
          ? candidate
          : failureCode
      const error = imageOperationError(code)
      // 已完成方向的付费产物仍交给运行记录保存，失败方向不自动重试。
      if (output.frames.length) Object.assign(error, { partialOutput: parseImageOutput(output) })
      throw error
    }
  }

  /** @param {unknown} previous @param {Settings} settings @param {AbortSignal} signal */
  async resume(previous, settings, signal) {
    const output = parseImageOutput(previous)
    const { cols, rows } = suggestActionSheetGrid(settings.frameCount)
    if (
      output.atlas ||
      new Set(output.frames.map((frame) => frame.direction)).size !== output.frames.length ||
      output.frames.some(
        (frame) =>
          frame.action !== settings.action ||
          !settings.directions.some((direction) => direction === frame.direction) ||
          frame.columns !== cols ||
          frame.rows !== rows ||
          frame.frameCount !== settings.frameCount ||
          frame.durationMs !== settings.durationMs,
      )
    )
      throw imageOperationError('workflow_image_invalid')
    // 参数快照由调用方的领域服务确认；这里仍核验实际素材，不能相信旧输出中的引用元数据。
    await this.pixels(output.frames, signal)
    return output.frames.sort(
      (left, right) =>
        settings.directions.findIndex((direction) => direction === left.direction) -
        settings.directions.findIndex((direction) => direction === right.direction),
    )
  }

  /** @param {{prompt?:string,model?:{provider:string,model:string}|null}} request @param {Settings} settings @param {Pixels[]} sources @param {Output} output @param {AbortSignal} signal @param {unknown} [resumeOutput] */
  async generate(request, settings, sources, output, signal, resumeOutput) {
    if (sources.length > 8) throw imageOperationError('workflow_image_invalid')
    if (resumeOutput !== undefined)
      output.frames = await this.resume(resumeOutput, settings, signal)
    const remainingDirections = settings.directions.filter(
      (direction) => !output.frames.some((frame) => frame.direction === direction),
    )
    if (!remainingDirections.length) return
    const root = await this.init()
    await assertDirectory(root)
    const directory = await mkdtemp(join(root, 'job-'))
    try {
      const sourceImages = []
      for (const [index, image] of sources.entries()) {
        const extension = EXTENSIONS.get(image.mimeType)
        if (!extension) throw imageOperationError('workflow_image_invalid')
        const path = join(directory, `reference-${index}.${extension}`)
        await writeFile(path, image.buffer, { flag: 'wx', mode: 0o600 })
        sourceImages.push(path)
      }
      const background =
        settings.colors[0] || (await this.processor.suggestBackground(sources[0], { signal }))
      if (!/^#[0-9a-f]{6}$/i.test(background)) throw imageOperationError('workflow_image_invalid')
      const { cols, rows } = suggestActionSheetGrid(settings.frameCount)
      for (const direction of remainingDirections) {
        active(signal)
        const generated = await this.generateImage(
          {
            kind: 'image',
            cwd: directory,
            model: request.model ? `${request.model.provider}/${request.model.model}` : '',
            sourceImages,
            prompt: sheetPrompt(settings, direction, request.prompt || '', background),
            outputName: `action-${direction}`,
            outputFormat: 'png',
            aspectRatio: rows === 1 && cols > 1 ? '16:9' : '1:1',
          },
          { signal, allowFallback: false },
        )
        active(signal)
        const image = await readGeneratedImage(generated, directory)
        const frame = await this.save(
          {
            ...image,
            durationMs: settings.durationMs,
            action: settings.action,
            direction,
            columns: cols,
            rows,
            frameCount: settings.frameCount,
          },
          `action-${direction}.${EXTENSIONS.get(image.mimeType)}`,
          signal,
        )
        output.frames.push(frame)
        output.frames.sort(
          (left, right) =>
            settings.directions.findIndex((direction) => direction === left.direction) -
            settings.directions.findIndex((direction) => direction === right.direction),
        )
      }
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  }

  async dispose() {
    this.closed = true
    for (const controller of this.active.values()) controller.abort()
    await Promise.allSettled(this.active.keys())
  }
}
