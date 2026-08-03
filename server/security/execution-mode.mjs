import { PLAN_READ_TOOL_NAMES } from '../tools/app/plan-tool-names.mjs'
import { TOOL_CATALOG } from '../tools/registry.mjs'

export const EXECUTION_MODES = new Set(['read-only', 'workspace', 'full-access'])
export const DEFAULT_EXECUTION_MODE = 'workspace'

const TOOL_RISK = new Map(TOOL_CATALOG.map((tool) => [tool.id, tool.risk]))
const INTERNAL_READ_ONLY_TOOLS = new Set([
  'discover_tools',
  'get_goal',
  ...PLAN_READ_TOOL_NAMES,
  'list_agents',
  'send_message',
  'wait_agent',
])

export function normalizeExecutionMode(value, fallback = DEFAULT_EXECUTION_MODE) {
  const mode = String(value || '')
  return EXECUTION_MODES.has(mode) ? mode : fallback
}

export function permissionModeForExecutionMode(mode) {
  if (mode === 'full-access') return 'ignore'
  if (mode === 'read-only') return 'ask'
  return 'auto'
}

export function filterToolsForExecutionMode(names, mode, getExternalRisk = () => null) {
  const unique = [...new Set(names || [])]
  if (mode !== 'read-only') return unique
  return unique.filter((name) => {
    if (INTERNAL_READ_ONLY_TOOLS.has(name)) return true
    const risk = TOOL_RISK.get(name) || getExternalRisk(name)
    return risk === 'low' || risk === '低风险'
  })
}

export function migrateLegacyExecutionMode(meta = {}) {
  if (EXECUTION_MODES.has(meta.executionMode)) return meta.executionMode
  return meta.permissionMode === 'ignore' ? 'full-access' : DEFAULT_EXECUTION_MODE
}
