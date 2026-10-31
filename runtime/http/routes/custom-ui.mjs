// 自定义 UI 组件路由：组件清单、资产服务（沙箱 iframe 加载）与桥接脚本。
// 旧资产路由保留三级路径兼容；凭证预览支持嵌套资源，由服务层限制目录边界。
// A7 新增原生组件（v2）路由：describe、按 rev 服务模块、创建 / 启用 / 删除组件、放置实例。
// 新路由全部走现有鉴权链（没有鉴权前旁路），请求体按小上限读取后当作 unknown 交给服务层校验。
import { CustomUiError } from '../../services/custom-ui-error.mjs'
import { CustomUiImportError } from '../../services/custom-ui-import.mjs'

function customUiUnavailable(services) {
  if (!services.customUi) throw new Error('当前 Runtime 不支持自定义 UI 组件。')
  return services.customUi
}

function serveComponentAsset(services, params, res, json) {
  const customUi = customUiUnavailable(services)
  const path = [params.asset1, params.asset2, params.asset3].filter(Boolean).join('/')
  return customUi.serveAsset({ id: params.componentId, path, res, json })
}

// 结构化错误统一写成 { code, error: code, ...details }；其他异常交给 api-handler 的通用处理。
function nativeRoute(handler) {
  return async (context) => {
    try {
      await handler(context)
    } catch (error) {
      if (!(error instanceof CustomUiError)) throw error
      if (context.res.headersSent) {
        context.res.destroy()
        return
      }
      context.json(error.statusCode, { ...error.details, code: error.code, error: error.code })
    }
  }
}

async function readJsonBody({ bodyBuffer }, maxBytes) {
  let bytes
  try {
    bytes = await bodyBuffer(maxBytes)
  } catch {
    throw new CustomUiError('component_request_too_large', 413)
  }
  if (!bytes.length) return {}
  try {
    return JSON.parse(bytes.toString('utf8'))
  } catch {
    throw new CustomUiError('component_request_invalid', 400)
  }
}

function serveModule({ services, params, res }) {
  return customUiUnavailable(services).serveModuleAsset({
    id: params.componentId,
    rev: params.rev,
    segments: [params.asset1, params.asset2, params.asset3].filter((value) => value !== undefined),
    res,
  })
}

const nativeComponentRoutes = [
  {
    method: 'POST',
    path: '/api/custom-ui/components',
    handler: nativeRoute(async (context) => {
      const input = await readJsonBody(context, 8 * 1024)
      context.json(201, await customUiUnavailable(context.services).createFromTemplate(input))
    }),
  },
  {
    method: 'GET',
    path: '/api/custom-ui/components/:componentId/describe',
    handler: nativeRoute(async ({ services, params, json }) => {
      json(200, await customUiUnavailable(services).describe(params.componentId))
    }),
  },
  {
    method: 'PATCH',
    path: '/api/custom-ui/components/:componentId',
    handler: nativeRoute(async (context) => {
      const input = await readJsonBody(context, 1024)
      context.json(
        200,
        await customUiUnavailable(context.services).setEnabled(context.params.componentId, input),
      )
    }),
  },
  {
    method: 'DELETE',
    path: '/api/custom-ui/components/:componentId',
    handler: nativeRoute(async ({ services, params, json }) => {
      json(200, await customUiUnavailable(services).deleteComponent(params.componentId))
    }),
  },
  {
    method: 'GET',
    path: '/api/custom-ui/components/:componentId/r/:rev/:asset1',
    handler: nativeRoute(serveModule),
  },
  {
    method: 'GET',
    path: '/api/custom-ui/components/:componentId/r/:rev/:asset1/:asset2',
    handler: nativeRoute(serveModule),
  },
  {
    method: 'GET',
    path: '/api/custom-ui/components/:componentId/r/:rev/:asset1/:asset2/:asset3',
    handler: nativeRoute(serveModule),
  },
  {
    method: 'GET',
    path: '/api/custom-ui/instances',
    handler: nativeRoute(async ({ services, url, json }) => {
      const surface = url.searchParams.get('surface') ?? undefined
      json(200, await customUiUnavailable(services).instances.list(surface))
    }),
  },
  {
    method: 'POST',
    path: '/api/custom-ui/instances',
    handler: nativeRoute(async (context) => {
      const input = await readJsonBody(context, 64 * 1024)
      context.json(201, await customUiUnavailable(context.services).instances.create(input))
    }),
  },
  {
    method: 'PUT',
    path: '/api/custom-ui/instances',
    handler: nativeRoute(async (context) => {
      const input = await readJsonBody(context, 64 * 1024)
      context.json(200, await customUiUnavailable(context.services).instances.reorder(input))
    }),
  },
  {
    method: 'PATCH',
    path: '/api/custom-ui/instances/:instanceId',
    handler: nativeRoute(async (context) => {
      const input = await readJsonBody(context, 64 * 1024)
      context.json(
        200,
        await customUiUnavailable(context.services).instances.update(
          context.params.instanceId,
          input,
        ),
      )
    }),
  },
  {
    method: 'DELETE',
    path: '/api/custom-ui/instances/:instanceId',
    handler: nativeRoute(async ({ services, params, json }) => {
      json(200, await customUiUnavailable(services).instances.remove(params.instanceId))
    }),
  },
]

export const customUiRoutes = [
  ...nativeComponentRoutes,
  {
    method: 'POST',
    path: '/api/custom-ui/import',
    async handler({ runtime, services, bodyBuffer, json }) {
      try {
        const bytes = await bodyBuffer(16 * 1024 * 1024)
        const imported = await customUiUnavailable(services).importBundle(bytes)
        // 第 1 期组件仍是 custom-ui：安装 / 覆盖更新后通知看板事件流（component_changed）。
        if (typeof imported?.id === 'string') runtime?.boardEvents?.componentChanged(imported.id)
        json(201, imported)
      } catch (error) {
        if (!(error instanceof CustomUiImportError)) throw error
        json(error.statusCode, { code: error.code, error: error.code })
      }
    },
  },
  {
    method: 'POST',
    path: '/api/custom-ui/components/:componentId/views',
    async handler({ services, params, req, body, json }) {
      const input = await body()
      if (
        !input ||
        typeof input.origin !== 'string' ||
        Object.keys(input).some((key) => key !== 'origin')
      ) {
        throw Object.assign(new Error('组件来源无效。'), { statusCode: 400 })
      }
      json(
        200,
        await customUiUnavailable(services).createView(
          params.componentId,
          req.pisperDevice?.id || 'local',
          input.origin,
        ),
      )
    },
  },
  {
    method: 'PUT',
    path: '/api/custom-ui/views/:viewId',
    handler({ services, params, req, json }) {
      customUiUnavailable(services).renewView(params.viewId, req.pisperDevice?.id || 'local')
      json(200, { ok: true })
    },
  },
  {
    method: 'DELETE',
    path: '/api/custom-ui/views/:viewId',
    handler({ services, params, req, json }) {
      customUiUnavailable(services).revokeView(params.viewId, req.pisperDevice?.id || 'local')
      json(200, { ok: true })
    },
  },
  {
    method: 'GET',
    path: '/api/custom-ui/components',
    async handler({ services, json }) {
      json(200, await customUiUnavailable(services).listComponents())
    },
  },
  {
    method: 'GET',
    path: '/api/custom-ui/bridge.js',
    handler({ services, res }) {
      const script = customUiUnavailable(services).bridgeScript()
      res.writeHead(200, {
        'Content-Type': 'text/javascript; charset=utf-8',
        'Content-Length': Buffer.byteLength(script),
        'Cache-Control': 'no-cache',
        'X-Content-Type-Options': 'nosniff',
      })
      res.end(script)
    },
  },
  {
    method: 'GET',
    path: '/api/custom-ui/components/:componentId/assets/:asset1',
    async handler({ services, params, res, json }) {
      await serveComponentAsset(services, params, res, json)
    },
  },
  {
    method: 'GET',
    path: '/api/custom-ui/components/:componentId/assets/:asset1/:asset2',
    async handler({ services, params, res, json }) {
      await serveComponentAsset(services, params, res, json)
    },
  },
  {
    method: 'GET',
    path: '/api/custom-ui/components/:componentId/assets/:asset1/:asset2/:asset3',
    async handler({ services, params, res, json }) {
      await serveComponentAsset(services, params, res, json)
    },
  },
]

// 此入口位于 Cookie/Bearer 鉴权前，但只接受短期组件凭证和 GET；不能分发其他 API。
export async function handleCustomUiResource(
  req,
  res,
  url,
  { customUi, remoteAccess, remote = false, json },
) {
  if (!url.pathname.startsWith('/api/custom-ui/render/')) return false
  const match = /^\/api\/custom-ui\/render\/([a-f0-9]{64})\/(bridge\.js|assets\/(.+))$/.exec(
    url.pathname,
  )
  const view = match && customUi.getView(match[1])
  const device = view && view.owner !== 'local' ? remoteAccess?.getDevice(view.owner) : null
  if (
    req.method !== 'GET' ||
    !view ||
    (remote && view.owner === 'local') ||
    (view.owner !== 'local' && (!device || device.revokedAt))
  ) {
    json(404, { error: '组件预览不存在或已过期。' })
    return true
  }
  const resourceBase = `${view.origin}/api/custom-ui/render/${match[1]}/`
  if (match[2] === 'bridge.js') {
    const script = customUi.bridgeScript()
    res.writeHead(200, {
      ...customUi.assetHeaders(resourceBase),
      'Content-Type': 'text/javascript; charset=utf-8',
    })
    res.end(script)
  } else {
    let path
    try {
      path = decodeURIComponent(match[3])
    } catch {
      json(404, { error: '组件资源不存在。' })
      return true
    }
    if (device) remoteAccess.trackResponse(device.id, res)
    await customUi.serveAsset({ id: view.componentId, path, res, json, resourceBase })
  }
  return true
}
