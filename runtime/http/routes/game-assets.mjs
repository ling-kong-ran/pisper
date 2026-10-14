// 独立工作台 HTTP 边界；不读取工作流能力、实体或 Agent 插件开关。
import { GameAssetError, parseGameAssetProjectInput } from '../../../shared/game-assets.mjs'
import { normalizeImageSettings, parseImageOutput } from '../../../shared/image-operations.mjs'
import { normalizeImageFrameEdits } from '../../../shared/image-frame-edits.mjs'
import { parseWorkflowMedia, WorkflowInputError } from '../../../shared/workflow-inputs.mjs'
import {
  parseSpriteEngineCatalog,
  SpriteEngineError,
} from '../../../shared/sprite-engine-catalog.mjs'

/** @typedef {{id:string,name:string,providerId:string,providerName?:string}} ImageModel */
/** @typedef {{runtime:{gameAssets:Pick<import('../../services/game-assets-service.mjs').GameAssetsService,'catalog'|'save'|'remove'|'run'|'getJob'|'stop'|'edit'>,gameAssetMedia:Pick<import('../../services/workflow-media-service.mjs').WorkflowMediaService,'upload'|'read'>,gameAssetOperations:Pick<import('../../services/image-operation-service.mjs').ImageOperationService,'execute'>,spriteEngines:Pick<import('../../services/sprite-engine-service.mjs').SpriteEngineService,'catalog'|'download'|'cancel'>,visualGeneration:{getModelStatus(kind:'image'):Promise<{models:ImageModel[]}>}},req:import('node:http').IncomingMessage,res:import('node:http').ServerResponse,url:URL,params:Record<string,string>,bodyBuffer:(max:number)=>Promise<Buffer>,json:(status:number,value:unknown)=>void}} Context */
const SAFE_CODES = new Set([
  'workflow_image_invalid',
  'workflow_image_source_required',
  'workflow_image_too_large',
  'workflow_image_cancelled',
  'workflow_image_closed',
  'workflow_image_generation_failed',
  'workflow_image_processing_failed',
  'workflow_image_timeout',
  'workflow_image_invalid_edits',
  'workflow_media_invalid',
  'workflow_media_missing',
  'workflow_media_too_large',
  'sprite_engine_missing',
  'sprite_engine_invalid',
  'sprite_engine_download_failed',
])
/** @param {Context} context @param {unknown} failure */
function fail(context, failure) {
  if (context.res.destroyed || context.res.writableEnded || context.res.headersSent) return
  if (
    failure instanceof GameAssetError ||
    failure instanceof WorkflowInputError ||
    failure instanceof SpriteEngineError
  ) {
    context.json(failure.statusCode, { error: failure.code, code: failure.code })
    return
  }
  const code = failure && typeof failure === 'object' && 'code' in failure ? failure.code : null
  if (typeof code === 'string' && SAFE_CODES.has(code)) {
    const status =
      failure && typeof failure === 'object' && 'statusCode' in failure ? failure.statusCode : 400
    context.json(
      typeof status === 'number' && Number.isInteger(status) && status >= 400 && status <= 599
        ? status
        : 400,
      { error: code, code },
    )
  } else context.json(500, { error: 'game_assets_failed', code: 'game_assets_failed' })
}
/** @param {unknown} value @param {string[]} [keys] @returns {Record<string,unknown>} */
function record(value, keys) {
  if (
    !value ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    (keys && Object.keys(value).some((key) => !keys.includes(key)))
  )
    throw new GameAssetError('game_assets_invalid')
  return /** @type {Record<string,unknown>} */ (value)
}
/** @param {Context} context */
async function body(context) {
  try {
    const bytes = await context.bodyBuffer(2 * 1024 * 1024)
    return record(JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)))
  } catch {
    throw new GameAssetError('game_assets_invalid')
  }
}
/** @param {string} value */
function uuid(value) {
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value))
    throw new GameAssetError('game_assets_invalid')
  return value
}
/** @param {string} value */
function engineId(value) {
  if (value !== 'background' && value !== 'inpaint') throw new GameAssetError('game_assets_invalid')
  return value
}
/** @param {string} method @param {string} path @param {number} status @param {(context:Context)=>Promise<unknown>} action */
function route(method, path, status, action) {
  return {
    method,
    path: `/api/game-assets${path}`,
    /** @param {Context} context */
    async handler(context) {
      try {
        const value = await action(context)
        if (!context.res.destroyed && !context.res.writableEnded) context.json(status, value)
      } catch (failure) {
        fail(context, failure)
      }
    },
  }
}

export const gameAssetRoutes = [
  route('GET', '', 200, async ({ runtime }) => {
    const [catalog, models, engines] = await Promise.all([
      runtime.gameAssets.catalog(),
      runtime.visualGeneration.getModelStatus('image'),
      runtime.spriteEngines.catalog(),
    ])
    return {
      ...catalog,
      models: models.models.map((model) => ({
        id: `${model.providerId}/${model.id}`,
        name: model.name,
        providerId: model.providerId,
        providerName: model.providerName ?? model.providerId,
      })),
      engines: parseSpriteEngineCatalog(engines).engines,
    }
  }),
  route('POST', '/projects', 201, async (context) => {
    const input = parseGameAssetProjectInput(await body(context))
    if (input.id !== undefined) throw new GameAssetError('game_assets_invalid')
    return context.runtime.gameAssets.save(input)
  }),
  route('PATCH', '/projects/:projectId', 200, async (context) => {
    const projectId = uuid(context.params.projectId)
    const input = await body(context)
    if (input.id !== undefined && input.id !== projectId)
      throw new GameAssetError('game_assets_invalid')
    return context.runtime.gameAssets.save(parseGameAssetProjectInput({ ...input, id: projectId }))
  }),
  route('DELETE', '/projects/:projectId', 200, async ({ runtime, params }) => {
    await runtime.gameAssets.remove(uuid(params.projectId))
    return { deleted: true }
  }),
  route('POST', '/projects/:projectId/run', 202, async ({ runtime, params }) => ({
    job: await runtime.gameAssets.run(uuid(params.projectId)),
  })),
  route('GET', '/jobs/:jobId', 200, async ({ runtime, params }) => {
    const job = await runtime.gameAssets.getJob(uuid(params.jobId))
    if (!job) throw new GameAssetError('game_assets_not_found', 404)
    return job
  }),
  route('POST', '/jobs/:jobId/stop', 200, async ({ runtime, params }) => ({
    job: await runtime.gameAssets.stop(uuid(params.jobId)),
  })),
  route('POST', '/jobs/:jobId/frames', 200, async (context) => {
    const edits = normalizeImageFrameEdits(record(await body(context), ['frames']))
    return context.runtime.gameAssets.edit(uuid(context.params.jobId), edits)
  }),
  route('POST', '/media', 201, async (context) => {
    const mimeType = String(context.req.headers['content-type'] || '')
      .split(';')[0]
      .trim()
      .toLowerCase()
    if (!['image/png', 'image/jpeg', 'image/webp'].includes(mimeType))
      throw new GameAssetError('game_assets_media_invalid', 415)
    const length = Number(context.req.headers['content-length'])
    if (Number.isFinite(length) && length > 8 * 1024 * 1024)
      throw new GameAssetError('workflow_media_too_large', 413)
    let buffer
    try {
      buffer = await context.bodyBuffer(8 * 1024 * 1024)
    } catch {
      throw new GameAssetError('game_assets_media_invalid')
    }
    return context.runtime.gameAssetMedia.upload({
      name: context.url.searchParams.get('name') || 'reference.png',
      mimeType,
      buffer,
    })
  }),
  {
    method: 'GET',
    path: '/api/game-assets/media/:mediaId/content',
    /** @param {Context} context */
    async handler(context) {
      try {
        if (!/^[a-zA-Z0-9][a-zA-Z0-9_-]{0,79}$/.test(context.params.mediaId))
          throw new GameAssetError('game_assets_media_invalid')
        const { media, buffer } = await context.runtime.gameAssetMedia.read(context.params.mediaId)
        if (!media.mimeType.startsWith('image/'))
          throw new GameAssetError('game_assets_media_invalid')
        context.res.writeHead(200, {
          'Content-Type': media.mimeType,
          'Content-Length': buffer.length,
          'Cache-Control': 'private, max-age=60',
          'X-Content-Type-Options': 'nosniff',
        })
        context.res.end(buffer)
      } catch (failure) {
        fail(context, failure)
      }
    },
  },
  route('POST', '/process', 200, async (context) => {
    const input = record(await body(context), ['reference', 'operation', 'image'])
    if (input.operation !== 'background' && input.operation !== 'inpaint')
      throw new GameAssetError('game_assets_invalid')
    const reference = parseWorkflowMedia(input.reference)
    if (!reference.mimeType.startsWith('image/'))
      throw new GameAssetError('game_assets_media_invalid')
    const settings = normalizeImageSettings(input.image)
    const controller = new AbortController()
    const abort = () => {
      if (!context.res.writableEnded) controller.abort()
    }
    context.res.once('close', abort)
    if (context.res.destroyed || context.req.aborted) controller.abort()
    try {
      const source = await context.runtime.gameAssetOperations.execute({
        operation: 'input',
        source: reference,
        signal: controller.signal,
      })
      controller.signal.throwIfAborted()
      const result = await context.runtime.gameAssetOperations.execute({
        operation: input.operation,
        images: parseImageOutput(source.output).frames,
        settings,
        signal: controller.signal,
      })
      const output = parseImageOutput(result.output)
      if (output.frames.length !== 1) throw new GameAssetError('game_assets_invalid')
      return output.frames[0].media
    } finally {
      context.res.removeListener('close', abort)
    }
  }),
  route('POST', '/engines/:engineId/download', 202, ({ runtime, params }) =>
    runtime.spriteEngines.download(engineId(params.engineId)),
  ),
  route('POST', '/engines/:engineId/cancel', 200, ({ runtime, params }) =>
    runtime.spriteEngines.cancel(engineId(params.engineId)),
  ),
]
