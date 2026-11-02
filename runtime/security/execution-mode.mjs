// 执行模式：决定调用工具时是否需要用户确认（默认权限策略），不决定有哪些工具可用。
// 交互会话提供 approval-required（需审批，默认）/ workspace-write（工作区写）/ full-access（完全访问）三种模式；
// 可用工具只由插件页的开关决定，三种模式下一致（子代理也一样），逐次确认由 session-permission-service 负责。
// 工具集合不随模式变化，切换模式也不会让提示缓存失效。

export const EXECUTION_MODES = new Set(['approval-required', 'workspace-write', 'full-access'])
export const DEFAULT_EXECUTION_MODE = 'approval-required'

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
