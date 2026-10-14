// 工具网关的运行时装配：把应用工具的 Map 包装成 Pi 会话可见的 tool_gateway 工具。
// 关键点：按执行模式过滤工具，未获准的工具对模型不可见；调用前再走权限审批。
import { createToolGatewayTool } from '../tools/app/tool-gateway.mjs'
import { filterToolsForExecutionMode } from '../security/execution-mode.mjs'

export function createRuntimeToolGateway({
  tools,
  getExecutionMode,
  getToolRisk,
  authorize,
  sessionId,
}) {
  return createToolGatewayTool({
    getTool: (name) => {
      const permitted = filterToolsForExecutionMode(
        [name],
        getExecutionMode(sessionId),
        getToolRisk,
      )
      return permitted.includes(name) ? tools.get(name) : null
    },
    authorize: (input) => authorize({ sessionId, ...input }),
  })
}
