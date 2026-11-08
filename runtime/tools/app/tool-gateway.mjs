// 工具网关：所有可选工具经 call_tool 间接调用（含 schema 校验与权限审批）。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { Check, Errors } from 'typebox/value'
import { CALL_TOOL } from '../../prompts/tools/tool-gateway.mjs'

export const TOOL_GATEWAY_NAME = 'call_tool'

function literalValues(schema) {
  if (schema && Object.prototype.hasOwnProperty.call(schema, 'const')) return [schema.const]
  if (Array.isArray(schema?.enum)) return schema.enum
  if (Array.isArray(schema?.anyOf)) return schema.anyOf.flatMap((item) => literalValues(item))
  return []
}

function validationMessage(schema, args) {
  if (Check(schema, args)) return ''
  const errors = [...Errors(schema, args)]
  const actionValues = literalValues(schema?.properties?.action)
  if (
    actionValues.length &&
    errors.length > 0 &&
    errors.every((error) => error.instancePath === '/action')
  ) {
    return `/action: must be one of ${actionValues.map((value) => JSON.stringify(value)).join(', ')}`
  }
  const messages = errors.slice(0, 5).map((error) => {
    const path = error.instancePath || error.path || '/'
    return `${path}: ${error.message}`
  })
  return messages.join('; ') || 'arguments do not match the tool schema'
}

export function createToolGatewayTool({ getTool, authorize }) {
  return defineTool({
    name: TOOL_GATEWAY_NAME,
    label: 'Call Tool',
    description: CALL_TOOL.description,
    parameters: Type.Object({
      name: Type.String({
        minLength: 1,
        maxLength: 240,
        description: CALL_TOOL.params.name,
      }),
      arguments: Type.Optional(
        Type.Record(Type.String(), Type.Unknown(), {
          description: CALL_TOOL.params.arguments,
        }),
      ),
    }),
    async execute(toolCallId, params, signal) {
      const requested = String(params.name || '').trim()
      const tool = getTool?.(requested)
      // 权限、校验和结果都按工具的真实名字走：别名只是入口，不能绕过按名字的审批与范围检查。
      const name = tool?.name || requested
      if (!tool || name === TOOL_GATEWAY_NAME || name === 'discover_tools')
        throw new Error(
          `Optional tool is unavailable: ${requested}. Use the exact name shown in [brackets] by discover_tools.`,
        )
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
