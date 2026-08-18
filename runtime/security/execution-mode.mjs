// 执行模式：按模式过滤可用工具与默认权限策略。
// 模式：read-only（只读）/ approval-required（需审批）/ workspace-write（工作区写）/ full-access（完全访问）。
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
const FULL_ACCESS_ONLY_TOOLS = new Set(['plugin_create'])
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

// 模式 → 默认权限模式映射：只读 = ignore，审批/工作区写 = ask，完全访问 = auto。
export function permissionModeForExecutionMode(mode) {
  if (mode === 'read-only' || mode === 'approval-required') return 'ask'
  if (mode === 'workspace-write') return 'auto'
  return 'ignore'
}

// 按执行模式过滤工具：高危工具只在更高授权模式下可用；内部工具豁免。
export function filterToolsForExecutionMode(names, mode, getExternalRisk = () => null) {
  const unique = [...new Set(names || [])].filter(
    (name) => mode === 'full-access' || !FULL_ACCESS_ONLY_TOOLS.has(name),
  )
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

// 旧版执行模式迁移（老配置缺省/别名）。
export function migrateLegacyExecutionMode(meta = {}) {
  if (EXECUTION_MODES.has(meta.executionMode)) return meta.executionMode
  return 'full-access'
}
