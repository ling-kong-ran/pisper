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
