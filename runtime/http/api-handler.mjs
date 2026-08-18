// API 处理器：注册所有路由，统一封装请求上下文（JSON/SSE/错误脱敏），
// 并把请求分发给对应的路由处理器。
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

// 错误信息对外统一脱敏，避免把密钥/令牌泄漏到响应体。
function publicError(error) {
  return redactSecretText(error instanceof Error ? error.message : String(error))
}

// 构建路由处理上下文：提供 JSON 响应、SSE 启动/发送、请求体解析等工具。
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
