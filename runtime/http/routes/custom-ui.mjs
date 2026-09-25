// 自定义 UI 组件路由：组件清单、资产服务（沙箱 iframe 加载）与桥接脚本。
// 旧资产路由保留三级路径兼容；凭证预览支持嵌套资源，由服务层限制目录边界。

function customUiUnavailable(services) {
  if (!services.customUi) throw new Error('当前 Runtime 不支持自定义 UI 组件。')
  return services.customUi
}

function serveComponentAsset(services, params, res, json) {
  const customUi = customUiUnavailable(services)
  const path = [params.asset1, params.asset2, params.asset3].filter(Boolean).join('/')
  return customUi.serveAsset({ id: params.componentId, path, res, json })
}

export const customUiRoutes = [
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
