// 工具网关的运行时装配：把应用工具的 Map 包装成 Pi 会话可见的 tool_gateway 工具。
// 可用工具不随执行模式变化；调用前统一走权限审批，模式只决定是否逐次确认。
import { createToolGatewayTool } from '../tools/app/tool-gateway.mjs'

export function createRuntimeToolGateway({ tools, authorize, sessionId }) {
  return createToolGatewayTool({
    getTool: (name) => tools.get(name) || null,
    authorize: (input) => authorize({ sessionId, ...input }),
  })
}
