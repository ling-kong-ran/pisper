// 原生组件（v2）路由：组件目录、describe、按 rev 服务模块、创建 / 启用 / 删除组件、放置实例、
// 组件存储、网络代理、错误日志与绑定会话。全部走现有鉴权链（没有鉴权前旁路），
// 请求体按小上限读取后当作 unknown 交给服务层校验。
// A7 S4 删除旧 iframe 机制：视图凭证、桥脚本与沙箱资产路由改为 410 兼容桩（旧前端拿到结构化错误而不是 404），
// 鉴权前的 /api/custom-ui/render/* 入口整个删除（Runtime 不再有鉴权前旁路）；导入一律结构化拒绝。
import { normalizeSessionOrganization } from '../../services/chat-session-organization.mjs'
import { CustomUiError } from '../../services/custom-ui-error.mjs'

function customUiUnavailable(services) {
  if (!services.customUi) throw new Error('当前 Runtime 不支持自定义 UI 组件。')
  return services.customUi
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
  // ── S2：组件存储、网络代理、错误日志、绑定会话 ──
  {
    method: 'GET',
    path: '/api/custom-ui/components/:componentId/storage/:key',
    handler: nativeRoute(async ({ services, params, json }) => {
      json(200, await customUiUnavailable(services).storageGet(params.componentId, params.key))
    }),
  },
  {
    method: 'PUT',
    path: '/api/custom-ui/components/:componentId/storage/:key',
    handler: nativeRoute(async (context) => {
      const input = await readJsonBody(context, 1024 * 1024)
      context.json(
        200,
        await customUiUnavailable(context.services).storageSet(
          context.params.componentId,
          context.params.key,
          input,
        ),
      )
    }),
  },
  {
    method: 'DELETE',
    path: '/api/custom-ui/components/:componentId/storage/:key',
    handler: nativeRoute(async ({ services, params, json }) => {
      json(200, await customUiUnavailable(services).storageRemove(params.componentId, params.key))
    }),
  },
  {
    method: 'POST',
    path: '/api/custom-ui/components/:componentId/fetch',
    handler: nativeRoute(async (context) => {
      const input = await readJsonBody(context, 512 * 1024)
      context.json(
        200,
        await customUiUnavailable(context.services).fetchForComponent(
          context.params.componentId,
          input,
        ),
      )
    }),
  },
  {
    method: 'POST',
    path: '/api/custom-ui/components/:componentId/errors',
    handler: nativeRoute(async (context) => {
      const input = await readJsonBody(context, 16 * 1024)
      context.json(
        202,
        await customUiUnavailable(context.services).reportError(context.params.componentId, input),
      )
    }),
  },
  {
    method: 'GET',
    path: '/api/custom-ui/components/:componentId/errors',
    handler: nativeRoute(async ({ services, params, json }) => {
      json(200, await customUiUnavailable(services).listErrors(params.componentId))
    }),
  },
  {
    method: 'POST',
    path: '/api/custom-ui/components/:componentId/session',
    handler: nativeRoute(async (context) => {
      const input = await readJsonBody(context, 1024)
      if (!input || typeof input !== 'object' || Array.isArray(input) || Object.keys(input).length)
        throw new CustomUiError('component_request_invalid', 400)
      const { runtime } = context
      if (!runtime?.createSession || !runtime?.findSessionInfo)
        throw new Error('当前 Runtime 不支持代理会话。')
      context.json(
        200,
        await customUiUnavailable(context.services).ensureAgentSession(context.params.componentId, {
          createSession: (name, cwd) => runtime.createSession(name, cwd),
          // 归档的绑定会话不再复用：用户已经把它收起来了，新生成从一个新会话开始。
          findSession: async (sessionId) => {
            const info = await runtime.findSessionInfo(sessionId)
            if (!info) return null
            return {
              archived: normalizeSessionOrganization(runtime.sessionMeta?.[sessionId]).archived,
            }
          },
        }),
      )
    }),
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

// 旧 iframe 机制的兼容桩：至少保留到 A8 之后（计划 §2.3）。不读请求体、不碰组件目录。
function legacyRemoved({ json }) {
  json(410, { code: 'custom_ui_legacy_removed', error: 'custom_ui_legacy_removed' })
}

const legacyRemovedRoutes = [
  ['POST', '/api/custom-ui/components/:componentId/views'],
  ['PUT', '/api/custom-ui/views/:viewId'],
  ['DELETE', '/api/custom-ui/views/:viewId'],
  ['GET', '/api/custom-ui/bridge.js'],
  ['GET', '/api/custom-ui/components/:componentId/assets/:asset1'],
  ['GET', '/api/custom-ui/components/:componentId/assets/:asset1/:asset2'],
  ['GET', '/api/custom-ui/components/:componentId/assets/:asset1/:asset2/:asset3'],
].map(([method, path]) => ({ method, path, handler: legacyRemoved }))

export const customUiRoutes = [
  ...nativeComponentRoutes,
  {
    method: 'GET',
    path: '/api/custom-ui/components',
    handler: nativeRoute(async ({ services, json }) => {
      json(200, await customUiUnavailable(services).catalog())
    }),
  },
  {
    // 5a 不安装任何包：只解包判断格式后结构化拒绝（服务层 rejectImport），不写文件、不发 component_changed。
    method: 'POST',
    path: '/api/custom-ui/import',
    handler: nativeRoute(async ({ services, bodyBuffer }) => {
      const customUi = customUiUnavailable(services)
      let bytes
      try {
        bytes = await bodyBuffer(16 * 1024 * 1024)
      } catch {
        throw new CustomUiError('component_archive_too_large', 413)
      }
      await customUi.rejectImport(bytes)
    }),
  },
  ...legacyRemovedRoutes,
]
