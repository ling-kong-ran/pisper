import { isLocalAppRequest } from '../local-request.mjs'
import { pisperError } from '../../../shared/errors.mjs'

// 内置 MCP 服务只能从本机应用管理；配对移动端和被重绑定的浏览器请求都不能读取令牌或改动开关。
function requireLocalManagement(req, json) {
  if (isLocalAppRequest(req)) return true
  json(403, {
    error: 'The built-in MCP server can only be managed from this computer.',
    code: 'mcp_host_local_only',
  })
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
        throw pisperError('invalid', 'mcp', 'enabled must be a boolean')
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
        json(409, {
          error: 'The built-in MCP server is not ready.',
          code: 'mcp_host_not_listening',
        })
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
        json(503, {
          error: 'The built-in MCP server is not ready.',
          code: 'mcp_host_not_listening',
        })
        return
      }
      json(200, credentials)
    },
  },
]
