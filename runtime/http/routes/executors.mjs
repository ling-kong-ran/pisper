// 执行环境与执行节点路由（B6）。两组：
// - 界面用：列出电脑、生成配对码、移除电脑。和其他 Bots 接口一样经过能力与实验开关两道门。
// - 节点用：/api/executor/pair（凭一次性配对码）、/poll 与 /results（凭节点令牌）。这三个在远程监听上
//   不要求设备令牌（remote-auth.mjs 白名单），鉴权在这里做；令牌不对一律 401，不说明原因。
import { bearerToken } from '../../services/remote-access-service.mjs'
import { botRoute } from './bots.mjs'

// 远程访问的地址与证书指纹：配对命令里要用。远程访问没开时节点连不上，界面据此提示先打开。
function remoteInfo(services) {
  try {
    const status = services?.remoteControl?.status?.()
    if (!status) return { enabled: false, endpoints: [], fingerprint: null }
    return {
      enabled: Boolean(status.enabled && status.listening),
      endpoints: (Array.isArray(status.endpoints) ? status.endpoints : [])
        .map((endpoint) => String(endpoint?.url || ''))
        .filter((url) => /^https:\/\//.test(url))
        .slice(0, 8),
      fingerprint: status.fingerprint || null,
    }
  } catch {
    return { enabled: false, endpoints: [], fingerprint: null }
  }
}

// 节点请求：先认令牌，再交给处理器。
function nodeRoute(handler) {
  return botRoute(async (context) => {
    const environment = context.runtime.authenticateExecutor(bearerToken(context.req))
    if (!environment) {
      context.json(401, { error: '执行节点未授权。', code: 'executor_unauthorized' })
      return
    }
    await handler({ ...context, environment })
  })
}

export const executorRoutes = [
  {
    method: 'GET',
    path: '/api/environments',
    handler: botRoute(({ runtime, services, json }) => {
      json(200, { ...runtime.listEnvironments(), remote: remoteInfo(services) })
    }),
  },
  {
    method: 'POST',
    path: '/api/environments/pairing',
    handler: botRoute(({ runtime, services, json }) => {
      json(200, { pairing: runtime.createEnvironmentPairing(), remote: remoteInfo(services) })
    }),
  },
  {
    method: 'DELETE',
    path: '/api/environments/:environmentId',
    handler: botRoute(async ({ runtime, params, json }) => {
      const removed = await runtime.removeEnvironment(params.environmentId)
      if (!removed) json(404, { error: '这台电脑不存在。', code: 'environment_not_found' })
      else json(200, { ok: true })
    }),
  },
  {
    method: 'POST',
    path: '/api/executor/pair',
    handler: botRoute(async ({ runtime, body, json }) => {
      json(200, await runtime.pairExecutor(await body()))
    }),
  },
  {
    // 长轮询：节点上报目录与能力，最多等 25 秒领请求。连接断开时立即结束。
    method: 'POST',
    path: '/api/executor/poll',
    handler: nodeRoute(async ({ runtime, req, body, json, environment }) => {
      const report = await body()
      const controller = new AbortController()
      const abort = () => controller.abort()
      req.once('close', abort)
      try {
        const requests = await runtime.pollExecutor(environment, report, controller.signal)
        json(200, { requests })
      } finally {
        req.off('close', abort)
      }
    }),
  },
  {
    method: 'POST',
    path: '/api/executor/results',
    handler: nodeRoute(async ({ runtime, body, json, environment }) => {
      const input = await body()
      const accepted = runtime.deliverExecutorResults(environment, input?.results)
      json(200, { accepted })
    }),
  },
]
