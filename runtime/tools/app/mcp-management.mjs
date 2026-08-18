// MCP 管理工具：mcp_list（只读）+ mcp_manage（增删改/测试/启停）。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'

export const manifests = [
  {
    id: 'mcp_list',
    name: 'MCP List',
    category: 'mcp',
    risk: 'low',
    description: 'Read configured MCP services, connection status, and available tools.',
    scope: 'Application-level MCP configuration',
    capability: 'Read redacted MCP service information without changing configuration',
    source: 'app',
  },
  {
    id: 'mcp_manage',
    name: 'MCP Manage',
    category: 'mcp',
    risk: 'high',
    description:
      'Add, update, delete, test, enable, or disable MCP services and tools with structured parameters.',
    scope: 'Application-level MCP configuration',
    capability:
      'Modify MCP service configuration without exposing sensitive headers or environment values',
    source: 'app',
  },
]

const stringMap = Type.Record(Type.String({ maxLength: 200 }), Type.String({ maxLength: 8_000 }))
const timeout = Type.Optional(Type.Integer({ minimum: 1_000, maximum: 600_000 }))
const enabled = Type.Optional(Type.Boolean())
const args = Type.Optional(Type.Array(Type.String({ maxLength: 4_000 }), { maxItems: 100 }))

const stdioConfig = Type.Object({
  name: Type.String({ minLength: 1, maxLength: 120 }),
  transport: Type.Literal('stdio'),
  command: Type.String({ minLength: 1, maxLength: 2_000 }),
  args,
  cwd: Type.Optional(Type.String({ maxLength: 4_000 })),
  env: Type.Optional(stringMap),
  enabled,
  requestTimeoutMs: timeout,
})

const remoteConfig = (transport) =>
  Type.Object({
    name: Type.String({ minLength: 1, maxLength: 120 }),
    transport: Type.Literal(transport),
    url: Type.String({ minLength: 1, maxLength: 8_000 }),
    headers: Type.Optional(stringMap),
    enabled,
    requestTimeoutMs: timeout,
  })

const updateConfig = Type.Object({
  name: Type.Optional(Type.String({ minLength: 1, maxLength: 120 })),
  transport: Type.Optional(
    Type.Union([Type.Literal('stdio'), Type.Literal('http'), Type.Literal('sse')]),
  ),
  command: Type.Optional(Type.String({ minLength: 1, maxLength: 2_000 })),
  args,
  cwd: Type.Optional(Type.String({ maxLength: 4_000 })),
  env: Type.Optional(stringMap),
  url: Type.Optional(Type.String({ minLength: 1, maxLength: 8_000 })),
  headers: Type.Optional(stringMap),
  enabled,
  requestTimeoutMs: timeout,
})

function publicDashboard(dashboard) {
  return {
    metrics: dashboard?.metrics || {},
    services: (dashboard?.services || []).map((service) => ({
      id: service.id,
      name: service.name,
      transport: service.transport,
      enabled: service.enabled,
      status: service.status,
      endpoint: service.endpoint,
      command: service.command,
      workingDirectory: service.workingDirectory,
      toolCount: service.toolCount,
      enabledToolCount: service.enabledToolCount,
      error: service.error,
      tools: (service.tools || []).map((tool) => ({
        name: tool.name,
        enabled: tool.enabled,
        risk: tool.risk,
      })),
    })),
  }
}

function resultContent(value) {
  return { content: [{ type: 'text', text: JSON.stringify(value, null, 2) }], details: value }
}

export function createMcpListTool({ mcpRuntime }) {
  return defineTool({
    name: manifests[0].id,
    label: manifests[0].name,
    description: manifests[0].description,
    promptSnippet: 'List configured MCP services and their current tool availability',
    promptGuidelines: [
      'Use mcp_list before changing an existing MCP service so you have its exact service ID and current transport.',
      'The result is already credential-safe; never try to read MCP credential files directly.',
    ],
    parameters: Type.Object({
      refresh: Type.Optional(
        Type.Boolean({ description: 'Reconnect and refresh tool discovery before returning.' }),
      ),
    }),
    async execute(_toolCallId, params) {
      return resultContent(
        publicDashboard(await mcpRuntime.list({ refresh: params.refresh === true })),
      )
    },
  })
}

export function createMcpManageTool({ mcpRuntime }) {
  return defineTool({
    name: manifests[1].id,
    label: manifests[1].name,
    description: manifests[1].description,
    promptSnippet:
      'Manage MCP services with typed fields instead of shell commands or local HTTP requests',
    promptGuidelines: [
      'Always use mcp_manage for MCP configuration. Never call the application MCP API through bash, PowerShell, curl, fetch, or an inline script.',
      'Use Windows paths exactly as ordinary strings in the command or cwd field; do not add JavaScript, JSON, or shell escaping beyond the structured argument encoding.',
      'Call mcp_list first when updating, deleting, testing, or toggling an existing service.',
      'Do not read credential files. Put credentials only in the structured env or headers object supplied by the user or an approved onboarding flow.',
    ],
    parameters: Type.Union([
      Type.Object({
        action: Type.Literal('add'),
        config: Type.Union([stdioConfig, remoteConfig('http'), remoteConfig('sse')]),
      }),
      Type.Object({
        action: Type.Literal('update'),
        id: Type.String({ minLength: 1, maxLength: 100 }),
        config: updateConfig,
      }),
      Type.Object({
        action: Type.Literal('delete'),
        id: Type.String({ minLength: 1, maxLength: 100 }),
      }),
      Type.Object({
        action: Type.Literal('test'),
        id: Type.String({ minLength: 1, maxLength: 100 }),
      }),
      Type.Object({
        action: Type.Literal('set_enabled'),
        id: Type.String({ minLength: 1, maxLength: 100 }),
        enabled: Type.Boolean(),
      }),
      Type.Object({
        action: Type.Literal('set_tool_enabled'),
        id: Type.String({ minLength: 1, maxLength: 100 }),
        toolName: Type.String({ minLength: 1, maxLength: 300 }),
        enabled: Type.Boolean(),
      }),
    ]),
    async execute(_toolCallId, params, signal) {
      if (signal?.aborted) throw new Error('MCP management was cancelled.')
      let dashboard
      if (params.action === 'add') dashboard = await mcpRuntime.add(params.config)
      else if (params.action === 'update')
        dashboard = await mcpRuntime.update(params.id, params.config)
      else if (params.action === 'delete') {
        const deleted = await mcpRuntime.remove(params.id)
        if (!deleted) throw new Error('MCP service does not exist.')
        dashboard = await mcpRuntime.list({ refresh: false })
      } else if (params.action === 'test') dashboard = await mcpRuntime.test(params.id, { signal })
      else if (params.action === 'set_enabled')
        dashboard = await mcpRuntime.update(params.id, { enabled: params.enabled })
      else if (params.action === 'set_tool_enabled')
        dashboard = await mcpRuntime.setToolEnabled(params.id, params.toolName, params.enabled)
      if (!dashboard) throw new Error('MCP service or tool does not exist.')
      return resultContent(publicDashboard(dashboard))
    },
  })
}

export const factories = {
  mcp_list: createMcpListTool,
  mcp_manage: createMcpManageTool,
}
