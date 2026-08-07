import { redactSecretText } from '../security/secret-redaction.mjs'
import { bodyJson, json as sendJson, sseSend } from './response.mjs'
import { createRouteRegistry } from './route-registry.mjs'
import { configSettingsRoutes } from './routes/config-settings.mjs'
import { desktopRoutes } from './routes/desktop.mjs'
import { integrationRoutes } from './routes/integrations.mjs'
import { memoryAssetRoutes } from './routes/memory-assets.mjs'
import { sessionRuntimeRoutes } from './routes/sessions-runtime.mjs'
import { workflowScheduleRoutes } from './routes/workflows-schedules.mjs'

const registry = createRouteRegistry([
  ...sessionRuntimeRoutes,
  ...configSettingsRoutes,
  ...workflowScheduleRoutes,
  ...memoryAssetRoutes,
  ...integrationRoutes,
  ...desktopRoutes,
])

function publicError(error) {
  return redactSecretText(error instanceof Error ? error.message : String(error))
}

function createHandlerContext({ runtime, services, req, res, url, params }) {
  let sseStarted = false
  return {
    context: {
      runtime,
      services,
      req,
      res,
      url,
      params,
      body: () => bodyJson(req),
      json: (status, value) => sendJson(res, status, value),
      publicError,
      startSse() {
        res.writeHead(200, {
          'Content-Type': 'text/event-stream; charset=utf-8',
          'Cache-Control': 'no-cache, no-transform',
          Connection: 'keep-alive',
          'X-Accel-Buffering': 'no',
        })
        res.flushHeaders?.()
        sseStarted = true
      },
      sendSse(event, data) {
        if (!res.destroyed && !res.writableEnded) sseSend(res, event, data)
      },
    },
    isSse: () => sseStarted,
  }
}

export function createApiHandler(
  runtime,
  { updates, sponsors, desktopPet, engineVersion = 'unknown' } = {},
) {
  const services = { updates, sponsors, desktopPet, engineVersion }
  return async function handleApi(req, res, url) {
    if (!url.pathname.startsWith('/api/')) return false

    let handlerContext
    try {
      const match = registry.match(req.method, url.pathname)
      if (!match) {
        sendJson(res, 404, { error: '接口不存在。' })
        return true
      }
      handlerContext = createHandlerContext({
        runtime,
        services,
        req,
        res,
        url,
        params: match.params,
      })
      await match.handler(handlerContext.context)
    } catch (error) {
      if (handlerContext?.isSse()) {
        handlerContext.context.sendSse('error', { message: publicError(error) })
      } else {
        sendJson(res, 400, { error: publicError(error) })
      }
    }
    if (handlerContext?.isSse() && !res.writableEnded) res.end()
    return true
  }
}
