import { PLAN_READ_TOOL_NAMES } from '../tools/app/plan-tool-names.mjs'
import { TOOL_CATALOG } from '../tools/registry.mjs'

export const EXECUTION_MODES = new Set([
  'read-only',
  'approval-required',
  'workspace-write',
  'full-access',
])
export const DEFAULT_EXECUTION_MODE = 'approval-required'

const TOOL_RISK = new Map(TOOL_CATALOG.map((tool) => [tool.id, tool.risk]))
const APPROVAL_TOOLS = new Set(['edit', 'write', 'bash', 'skill_create'])
const INTERNAL_APPROVAL_TOOLS = new Set(['update_plan'])
const INTERNAL_READ_ONLY_TOOLS = new Set([
  'discover_tools',
  'call_tool',
  'get_goal',
  ...PLAN_READ_TOOL_NAMES,
  'list_agents',
  'send_message',
  'wait_agent',
])

export function normalizeExecutionMode(value, fallback = DEFAULT_EXECUTION_MODE) {
  const mode = String(value || '')
  if (mode === 'workspace') return 'workspace-write'
  return EXECUTION_MODES.has(mode) ? mode : fallback
}

export function permissionModeForExecutionMode(mode) {
  if (mode === 'read-only' || mode === 'approval-required') return 'ask'
  if (mode === 'workspace-write') return 'auto'
  return 'ignore'
}

export function filterToolsForExecutionMode(names, mode, getExternalRisk = () => null) {
  const unique = [...new Set(names || [])]
  if (mode !== 'read-only' && mode !== 'approval-required') return unique
  return unique.filter((name) => {
    if (
      mode === 'approval-required' &&
      (APPROVAL_TOOLS.has(name) || INTERNAL_APPROVAL_TOOLS.has(name))
    )
      return true
    if (INTERNAL_READ_ONLY_TOOLS.has(name)) return true
    const risk = TOOL_RISK.get(name) || getExternalRisk(name)
    return risk === 'low' || risk === '低风险'
  })
}

export function migrateLegacyExecutionMode(meta = {}) {
  if (EXECUTION_MODES.has(meta.executionMode)) return meta.executionMode
  return 'full-access'
}
