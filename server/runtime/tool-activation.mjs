import { MULTI_AGENT_TOOL_NAMES } from '../services/multi-agent-service.mjs'
import { GOAL_TOOL_NAMES } from '../tools/app/goal.mjs'
import {
  PLAN_ALL_TOOL_NAMES,
  PLAN_COMPATIBILITY_TOOL_NAMES,
} from '../tools/app/plan.mjs'
import { TOOL_DISCOVERY_NAME } from '../tools/app/tool-discovery.mjs'
import {
  mergePromotedToolNames,
  selectedToolNames,
} from '../tools/tool-activation.mjs'
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
      ...PLAN_ALL_TOOL_NAMES,
      ...MULTI_AGENT_TOOL_NAMES,
      ...GOAL_TOOL_NAMES,
    ].filter((name) => !blockedToolNames.has(name))
    const names = selectedToolNames({
      availableToolNames,
      promotedToolNames: value.promotedToolNames || [],
      requestedToolNames: value.requestedToolNames || [],
      goalToolNames: GOAL_TOOL_NAMES,
      goalActive: goal?.status === 'active',
    })
    value.session.setActiveToolsByName(filterToolsForExecutionMode(
      names,
      mode,
      this.getToolRisk,
    ))
  }

  async promoteSessionTools(value, toolNames = []) {
    if (!value?.session) return { activatedToolNames: [], promotedToolNames: [] }
    const availableToolNames = this.optionalToolNames(value)
    const permittedToolNames = filterToolsForExecutionMode(
      toolNames.filter((name) => availableToolNames.includes(name)),
      this.getExecutionMode(value.session.sessionId),
      this.getToolRisk,
    )
    const previousPromotedToolNames = value.promotedToolNames || []
    const promotedToolNames = mergePromotedToolNames({
      availableToolNames: availableToolNames.filter(
        (name) => !PLAN_COMPATIBILITY_TOOL_NAMES.includes(name),
      ),
      promotedToolNames: previousPromotedToolNames,
      requestedToolNames: permittedToolNames,
    })
    let promotionWrite = null
    if (promotedToolNames.join('\0') !== previousPromotedToolNames.join('\0')) {
      value.promotedToolNames = promotedToolNames
      const sessionId = value.session.sessionId
      const sessionMeta = this.getSessionMeta()
      sessionMeta[sessionId] = { ...(sessionMeta[sessionId] || {}), promotedToolNames }
      promotionWrite = this.saveSessionMeta()
    }
    this.syncGoalTools(value, this.getGoal(value.session.sessionId))
    applyPisperSystemPrompt(value.session, value.session.model)
    if (promotionWrite) await promotionWrite
    const active = new Set(value.session.getActiveToolNames())
    return {
      activatedToolNames: permittedToolNames.filter((name) => active.has(name)),
      promotedToolNames,
    }
  }

  async selectToolsForMessage(
    value,
    _message,
    { requestedToolNames = [], preserveRequested = false } = {},
  ) {
    if (!value?.session) return []
    const requested = [...new Set((Array.isArray(requestedToolNames) ? requestedToolNames : [])
      .map((name) => String(name || '').trim())
      .filter(Boolean))]
    value.requestedToolNames = preserveRequested
      ? [...new Set([...(value.requestedToolNames || []), ...requested])]
      : requested
    await this.promoteTools(value, requested)
    return value.session.getActiveToolNames()
  }
}
