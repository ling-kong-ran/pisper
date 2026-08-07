import { MULTI_AGENT_TOOL_NAMES } from '../services/multi-agent-service.mjs'
import { GOAL_TOOL_NAMES } from '../tools/app/goal.mjs'
import { PLAN_ALL_TOOL_NAMES, PLAN_COMPATIBILITY_TOOL_NAMES } from '../tools/app/plan.mjs'
import { TOOL_DISCOVERY_NAME } from '../tools/app/tool-discovery.mjs'
import { TOOL_GATEWAY_NAME } from '../tools/app/tool-gateway.mjs'
import { selectedToolNames } from '../tools/tool-activation.mjs'
import { filterToolsForExecutionMode } from '../security/execution-mode.mjs'
import { applyPisperSystemPrompt } from '../prompts/pisper-system-prompt.mjs'

export class ToolActivation {
  constructor({
    getExecutionMode,
    getToolRisk,
    getSessionMeta,
    saveSessionMeta,
    getGoal,
    promoteTools,
  }) {
    this.getExecutionMode = getExecutionMode
    this.getToolRisk = getToolRisk
    this.getSessionMeta = getSessionMeta
    this.saveSessionMeta = saveSessionMeta
    this.getGoal = getGoal
    this.promoteTools = promoteTools
  }

  optionalToolNames(value) {
    const blockedToolNames = new Set(value?.blockedToolNames || [])
    return [
      ...(value?.baseToolNames || []),
      ...PLAN_COMPATIBILITY_TOOL_NAMES,
      ...MULTI_AGENT_TOOL_NAMES,
    ].filter((name) => !blockedToolNames.has(name))
  }

  syncGoalTools(value, goal) {
    if (!value?.session) return
    const mode = this.getExecutionMode(value.session.sessionId)
    const blockedToolNames = new Set(value.blockedToolNames || [])
    const availableToolNames = [
      ...(value.baseToolNames || []),
      TOOL_DISCOVERY_NAME,
      TOOL_GATEWAY_NAME,
      ...PLAN_ALL_TOOL_NAMES,
      ...MULTI_AGENT_TOOL_NAMES,
      ...GOAL_TOOL_NAMES,
    ].filter((name) => !blockedToolNames.has(name))
    const names = selectedToolNames({
      availableToolNames,
      promotedToolNames: [],
      requestedToolNames: [],
      goalToolNames: GOAL_TOOL_NAMES,
      goalActive: goal?.status === 'active',
    })
    value.session.setActiveToolsByName(filterToolsForExecutionMode(names, mode, this.getToolRisk))
  }

  async promoteSessionTools(value, toolNames = []) {
    if (!value?.session) return { routedToolNames: [] }
    const availableToolNames = this.optionalToolNames(value)
    const routedToolNames = filterToolsForExecutionMode(
      toolNames.filter((name) => availableToolNames.includes(name)),
      this.getExecutionMode(value.session.sessionId),
      this.getToolRisk,
    )
    this.syncGoalTools(value, this.getGoal(value.session.sessionId))
    applyPisperSystemPrompt(value.session, value.session.model)
    return { routedToolNames }
  }

  async selectToolsForMessage(
    value,
    _message,
    { requestedToolNames = [], preserveRequested = false } = {},
  ) {
    if (!value?.session) return []
    const requested = [
      ...new Set(
        (Array.isArray(requestedToolNames) ? requestedToolNames : [])
          .map((name) => String(name || '').trim())
          .filter(Boolean),
      ),
    ]
    const routed = await this.promoteTools(value, requested)
    value.requestedToolNames = preserveRequested
      ? [...new Set([...(value.requestedToolNames || []), ...(routed.routedToolNames || [])])]
      : routed.routedToolNames || []
    return value.session.getActiveToolNames()
  }
}
