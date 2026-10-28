import { isLocalAppRequest } from '../local-request.mjs'

// 内置 MCP 服务只能从本机应用管理；配对移动端和被重绑定的浏览器请求都不能读取令牌或改动开关。
function requireLocalManagement(req, json) {
  if (isLocalAppRequest(req)) return true
  json(403, { error: '只能在本机 Pisper 中管理内置 MCP 服务。', code: 'mcp_host_local_only' })
  return false
}

export const mcpHostRoutes = [
  {
    method: 'GET',
    path: '/api/mcp-host',
    handler({ services, req, json }) {
      if (!requireLocalManagement(req, json)) return
      json(200, services.mcpHost.status())
    },
  },
  {
    method: 'PATCH',
    path: '/api/mcp-host',
    async handler({ services, body, req, json }) {
      if (!requireLocalManagement(req, json)) return
      const input = await body()
      if (typeof input?.enabled !== 'boolean')
        throw Object.assign(new Error('MCP 服务开关必须是布尔值。'), { statusCode: 400 })
      json(200, await services.mcpHost.setEnabled(input.enabled))
    },
  },
  {
    method: 'POST',
    path: '/api/mcp-host/credentials',
    handler({ services, req, json }) {
      if (!requireLocalManagement(req, json)) return
      const credentials = services.mcpHost.getCredentials()
      if (!credentials) {
        json(409, { error: '内置 MCP 服务尚未就绪。', code: 'mcp_host_not_listening' })
        return
      }
      json(200, credentials)
    },
  },
  {
    method: 'POST',
    path: '/api/mcp-host/rotate-token',
    async handler({ services, req, json }) {
      if (!requireLocalManagement(req, json)) return
      const credentials = await services.mcpHost.rotateToken()
      if (!credentials) {
        json(503, { error: '内置 MCP 服务尚未就绪。', code: 'mcp_host_not_listening' })
        return
      }
      json(200, credentials)
    },
  },
]
