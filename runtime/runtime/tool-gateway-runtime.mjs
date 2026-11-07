// 工具网关的运行时装配：把应用工具的 Map 包装成 Pi 会话可见的 tool_gateway 工具。
// 可用工具不随执行模式变化；调用前统一走权限审批，模式只决定是否逐次确认。
import { createToolGatewayTool } from '../tools/app/tool-gateway.mjs'

function comparable(value) {
  return String(value || '')
    .toLowerCase()
    .replace(/[\s_-]+/g, ' ')
    .trim()
}

// 模型有时把 discover_tools 列出的显示名（Browser Control [browser_automation]）当工具名传过来。
// 能唯一对应到一个工具才接受，对应不上或有歧义时仍按不可用处理，不猜。
export function resolveGatewayTool(tools, requested) {
  const exact = tools.get(requested)
  if (exact) return exact
  const bracketed = tools.get(/\[([^\]]+)\]\s*$/.exec(requested)?.[1] || '')
  if (bracketed) return bracketed
  const key = comparable(requested)
  if (!key) return null
  const matches = [...tools.values()].filter(
    (tool) => comparable(tool.name) === key || comparable(tool.label) === key,
  )
  return matches.length === 1 ? matches[0] : null
}

export function createRuntimeToolGateway({ tools, authorize, sessionId }) {
  return createToolGatewayTool({
    getTool: (name) => resolveGatewayTool(tools, name),
    authorize: (input) => authorize({ sessionId, ...input }),
  })
}
