import { WorkflowInputError } from '../../../shared/workflow-inputs.mjs'

/** @typedef {{runtime:{workflowMedia: import('../../services/workflow-media-service.mjs').WorkflowMediaService},bodyBuffer:(max:number)=>Promise<Buffer>,req:import('node:http').IncomingMessage,res:import('node:http').ServerResponse,url:URL,params:Record<string,string>,json:(status:number,value:unknown)=>void}} MediaContext */
/** @param {MediaContext} context @param {unknown} failure */
function respondError(context, failure) {
  context.json(failure instanceof WorkflowInputError ? failure.statusCode : 400, {
    error: '工作流素材上传或读取失败。',
    code: failure instanceof WorkflowInputError ? failure.code : 'workflow_media_invalid',
  })
}

export const workflowMediaRoutes = [
  {
    method: 'POST',
    path: '/api/workflow-media',
    /** @param {MediaContext} context */
    async handler(context) {
      try {
        const mimeType = String(context.req.headers['content-type'] || '')
          .split(';')[0]
          .trim()
          .toLowerCase()
        const buffer = await context.bodyBuffer(
          (mimeType.startsWith('image/') ? 8 : 64) * 1024 * 1024,
        )
        context.json(
          201,
          await context.runtime.workflowMedia.upload({
            name: context.url.searchParams.get('name') || 'media',
            mimeType,
            buffer,
          }),
        )
      } catch (failure) {
        respondError(context, failure)
      }
    },
  },
  {
    method: 'GET',
    path: '/api/workflow-media/:mediaId/content',
    /** @param {MediaContext} context */
    async handler(context) {
      try {
        const { media, buffer } = await context.runtime.workflowMedia.read(context.params.mediaId)
        context.res.writeHead(200, {
          'Content-Type': media.mimeType,
          'Content-Length': buffer.length,
          'Cache-Control': 'private, max-age=60',
          'X-Content-Type-Options': 'nosniff',
        })
        context.res.end(buffer)
      } catch (failure) {
        respondError(context, failure)
      }
    },
  },
]
