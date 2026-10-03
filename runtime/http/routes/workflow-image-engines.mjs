// 工作流图像节点的可选引擎资源。项目和运行状态由普通工作流持有。
import { SpriteEngineError } from '../../../shared/game/sprite-engine-catalog.mjs'
/** @typedef {{runtime:{spriteEngines:import('../../services/sprite-engine-service.mjs').SpriteEngineService},params:Record<string,string>,json:(status:number,value:unknown)=>void,res:import('node:http').ServerResponse}} EngineRouteContext */
/** @param {EngineRouteContext} context @param {unknown} error */
function respondError(context, error) {
  if (error instanceof SpriteEngineError)
    context.json(error.statusCode, { error: error.message, code: error.code })
  else context.json(500, { error: '本地图像算法操作失败。', code: 'sprite_engine_download_failed' })
}
/** @param {string} method @param {string} suffix @param {number} status @param {(context:EngineRouteContext)=>Promise<unknown>} action @param {string} base */
function route(method, suffix, status, action, base) {
  return {
    method,
    path: `${base}${suffix}`,
    /** @param {EngineRouteContext} context */ async handler(context) {
      try {
        context.json(status, await action(context))
      } catch (error) {
        respondError(context, error)
      }
    },
  }
}
export const spriteEngineRoutes = [
  route('GET', '', 200, ({ runtime }) => runtime.spriteEngines.catalog(), '/api/sprite-engines'),
  route(
    'POST',
    '/:engineId/download',
    202,
    ({ runtime, params }) => runtime.spriteEngines.download(params.engineId),
    '/api/sprite-engines',
  ),
  route(
    'POST',
    '/:engineId/cancel',
    200,
    ({ runtime, params }) => runtime.spriteEngines.cancel(params.engineId),
    '/api/sprite-engines',
  ),
  route(
    'DELETE',
    '/:engineId',
    200,
    ({ runtime, params }) => runtime.spriteEngines.remove(params.engineId),
    '/api/sprite-engines',
  ),
  {
    method: 'GET',
    path: '/api/sprite-engines/:engineId/files/:fileName',
    /** @param {EngineRouteContext} context */
    async handler(context) {
      try {
        const file = await context.runtime.spriteEngines.file(
          context.params.engineId,
          context.params.fileName,
        )
        context.res.writeHead(200, {
          'Content-Type': file.mimeType,
          'Content-Length': file.buffer.length,
          'Cache-Control': 'private, max-age=60',
          'X-Content-Type-Options': 'nosniff',
        })
        context.res.end(file.buffer)
      } catch (error) {
        respondError(context, error)
      }
    },
  },
]
