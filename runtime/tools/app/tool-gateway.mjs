// 工具网关：所有可选工具经 call_tool 间接调用（含 schema 校验与权限审批）。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { Check, Errors } from 'typebox/value'

export const TOOL_GATEWAY_NAME = 'call_tool'

function validationMessage(schema, args) {
  if (Check(schema, args)) return ''
  const errors = [...Errors(schema, args)].slice(0, 5).map((error) => {
    const path = error.path || '/'
    return `${path}: ${error.message}`
  })
  return errors.join('; ') || 'arguments do not match the tool schema'
}

export function createToolGatewayTool({ getTool, authorize }) {
  return defineTool({
    name: TOOL_GATEWAY_NAME,
    label: 'Call Tool',
    description: 'Call an optional tool by exact name with schema-valid arguments.',
    promptSnippet: 'Call an optional tool by exact name',
    promptGuidelines: ['Use the exact discovered name and only schema-supported arguments.'],
    parameters: Type.Object({
      name: Type.String({
        minLength: 1,
        maxLength: 240,
        description: 'Exact tool name',
      }),
      arguments: Type.Optional(
        Type.Record(Type.String(), Type.Unknown(), {
          description: 'Arguments validated against the selected tool schema',
        }),
      ),
    }),
    async execute(toolCallId, params, signal) {
      const name = String(params.name || '').trim()
      const tool = getTool?.(name)
      if (!tool || name === TOOL_GATEWAY_NAME || name === 'discover_tools')
        throw new Error(`Optional tool is unavailable: ${name}`)
      const args = params.arguments || {}
      const validationError = validationMessage(tool.parameters || {}, args)
      if (validationError) throw new Error(`Invalid arguments for ${name}: ${validationError}`)
      const authorization = await authorize?.({
        toolName: name,
        toolCallId: `${toolCallId}:${name}`,
        args,
        signal,
      })
      if (authorization?.block) throw new Error(authorization.reason || `Tool blocked: ${name}`)
      const result = await tool.execute(`${toolCallId}:${name}`, args, signal)
      return {
        ...result,
        details: {
          ...(result?.details || {}),
          gatewayToolName: name,
        },
      }
    },
  })
}
