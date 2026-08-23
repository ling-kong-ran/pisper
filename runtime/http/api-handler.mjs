// API 处理器：注册所有路由，统一封装请求上下文（JSON/SSE/错误脱敏），
// 并把请求分发给对应的路由处理器。
import { redactSecretText } from '../security/secret-redaction.mjs'
import { bodyJson, json as sendJson, sseSend } from './response.mjs'
import { createRouteRegistry } from './route-registry.mjs'
import { configSettingsRoutes } from './routes/config-settings.mjs'
import { desktopRoutes } from './routes/desktop.mjs'
import { integrationRoutes } from './routes/integrations.mjs'
import { memoryAssetRoutes } from './routes/memory-assets.mjs'
import { remoteRoutes } from './routes/remote.mjs'
import { runRoutes } from './routes/runs.mjs'
import { sessionRuntimeRoutes } from './routes/sessions-runtime.mjs'
import { workflowScheduleRoutes } from './routes/workflows-schedules.mjs'
import { RunRegistry } from '../services/run-registry.mjs'

const registry = createRouteRegistry([
  ...sessionRuntimeRoutes,
  ...configSettingsRoutes,
  ...workflowScheduleRoutes,
  ...memoryAssetRoutes,
  ...remoteRoutes,
  ...runRoutes,
  ...integrationRoutes,
  ...desktopRoutes,
])

// 错误信息对外统一脱敏，避免把密钥/令牌泄漏到响应体。
function publicError(error) {
  return redactSecretText(error instanceof Error ? error.message : String(error))
}

const CAPABILITY_ROUTES = [
  { pattern: /^\/api\/memory(?:\/|$)|^\/api\/settings\/memory$/, feature: 'memory' },
  { pattern: /^\/api\/mcp(?:\/|$)/, feature: 'mcp' },
  { pattern: /^\/api\/plugins\/web-search\/test$/, feature: 'webSearch' },
  { pattern: /^\/api\/plugins(?:\/|$)/, feature: 'plugins' },
  { pattern: /^\/api\/channels(?:\/|$)/, feature: 'channels' },
  { pattern: /^\/api\/schedules(?:\/|$)/, feature: 'schedules' },
  { pattern: /^\/api\/(?:workflows|workflow-runs)(?:\/|$)/, feature: 'workflows' },
  { pattern: /^\/api\/remote(?:\/|$)/, feature: 'remoteAccess' },
  { pattern: /^\/api\/desktop-pet(?:\/|$)/, feature: 'desktopPet' },
  { pattern: /^\/api\/sessions\/[^/]+\/(?:git|vcs)(?:\/|$)/, feature: 'vcs' },
  { pattern: /^\/api\/sessions\/[^/]+\/goal(?:\/|$)/, feature: 'goals' },
  { pattern: /^\/api\/sessions\/[^/]+\/workflow-runs(?:\/|$)/, feature: 'workflows' },
]

export function requiredRuntimeFeature(pathname, method = 'GET') {
  // 工具目录和策略保存不依赖第三方插件 Worker，降级宿主仍需配置可用内置工具。
  if (pathname === '/api/plugins' && ['GET', 'PUT'].includes(method)) return ''
  return CAPABILITY_ROUTES.find(({ pattern }) => pattern.test(pathname))?.feature || ''
}

// 构建路由处理上下文：提供 JSON 响应、SSE 启动/发送、请求体解析等工具。
// startRun 把当前 SSE 流登记为可重挂的 run：发送 run 头帧（游标 0，不入缓冲），
// 之后的 sendSse 自动带上游标并写入环形缓冲，供断线重挂补发。
function createHandlerContext({ runtime, services, req, res, url, params }) {
  let sseStarted = false
  let activeRun = null
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
        // 先入缓冲再写出：客户端断开时写出为空操作，但缓冲继续累积，保证可重挂。
        let cursor = null
        if (activeRun) cursor = services.runs.record(activeRun, event, data)
        if (!res.destroyed && !res.writableEnded) sseSend(res, event, data, cursor)
      },
      startRun(meta = {}) {
        activeRun = services.runs.begin(meta)
        if (!res.destroyed && !res.writableEnded) {
          sseSend(res, 'run', { runId: activeRun.id, ...meta, cursor: 0 })
        }
        return { runId: activeRun.id }
      },
      endRun() {
        if (activeRun) services.runs.close(activeRun)
        activeRun = null
      },
    },
    isSse: () => sseStarted,
  }
}

export function createApiHandler(
  runtime,
  {
    updates,
    sponsors,
    desktopPet,
    engineVersion = 'unknown',
    remoteAccess,
    remoteControl,
    runs,
  } = {},
) {
  const services = {
    updates,
    sponsors,
    desktopPet,
    engineVersion,
    remoteAccess,
    remoteControl,
    runs: runs || new RunRegistry(),
  }
  return async function handleApi(req, res, url) {
    if (!url.pathname.startsWith('/api/')) return false

    let handlerContext
    try {
      const requiredFeature = requiredRuntimeFeature(url.pathname, req.method)
      if (requiredFeature && runtime.capabilities?.features?.[requiredFeature] === false) {
        sendJson(res, 409, {
          error: `当前 Runtime 不支持 ${requiredFeature} 能力。`,
          capability: requiredFeature,
        })
        return true
      }
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
    if (handlerContext?.isSse()) {
      // SSE 结束前关闭 run：终态帧（done/error）已在上方记录，此后进入重放保留期。
      handlerContext.context.endRun?.()
      if (!res.writableEnded) res.end()
    }
    return true
  }
}
