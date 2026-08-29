// 执行模式：按模式过滤可用工具与默认权限策略。
// 交互会话提供 approval-required（需审批，默认）/ workspace-write（工作区写）/ full-access（完全访问）三种模式。
import { PLAN_READ_TOOL_NAMES } from '../tools/app/plan-tool-names.mjs'
import { TOOL_CATALOG } from '../tools/registry.mjs'

export const EXECUTION_MODES = new Set(['approval-required', 'workspace-write', 'full-access'])
export const DEFAULT_EXECUTION_MODE = 'approval-required'

const TOOL_RISK = new Map(TOOL_CATALOG.map((tool) => [tool.id, tool.risk]))
// 审批模式下仍可见的高危工具：调用时逐次审批，由用户决定是否放行。
// 生图是按次计费的高危操作，但在默认模式（approval-required）下必须可被发现，否则用户根本用不到。
const APPROVAL_TOOLS = new Set([
  'edit',
  'write',
  'bash',
  'skill_create',
  'plugin_create',
  'mobile_device',
  'generate_visual',
])
const INTERNAL_APPROVAL_TOOLS = new Set(['update_plan'])
const INTERNAL_SAFE_TOOLS = new Set([
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

// 模式 → 默认权限模式映射：审批 = ask，工作区写 = auto，完全访问 = ignore。
export function permissionModeForExecutionMode(mode) {
  if (mode === 'approval-required') return 'ask'
  if (mode === 'workspace-write') return 'auto'
  return 'ignore'
}

// 按执行模式过滤工具：审批模式保留低风险工具及逐次审批工具。
export function filterToolsForExecutionMode(names, mode, getExternalRisk = () => null) {
  const unique = [...new Set(names || [])]
  if (mode !== 'approval-required') return unique
  return unique.filter((name) => {
    if (APPROVAL_TOOLS.has(name) || INTERNAL_APPROVAL_TOOLS.has(name)) return true
    if (INTERNAL_SAFE_TOOLS.has(name)) return true
    const risk = TOOL_RISK.get(name) || getExternalRisk(name)
    return risk === 'low' || risk === '低风险'
  })
}
