// 计划工具名常量：新命名 + 一次性兼容命名（task_list）。
export const PLAN_TOOL_NAMES = Object.freeze(['get_plan', 'update_plan'])

// One-release compatibility names. Keep these registered but inactive and undiscoverable.
export const PLAN_COMPATIBILITY_TOOL_NAMES = Object.freeze(['get_task_list', 'update_task_list'])

export const PLAN_ALL_TOOL_NAMES = Object.freeze([
  ...PLAN_TOOL_NAMES,
  ...PLAN_COMPATIBILITY_TOOL_NAMES,
])
export const PLAN_READ_TOOL_NAMES = Object.freeze([
  PLAN_TOOL_NAMES[0],
  PLAN_COMPATIBILITY_TOOL_NAMES[0],
])
export const PLAN_WRITE_TOOL_NAMES = Object.freeze([
  PLAN_TOOL_NAMES[1],
  PLAN_COMPATIBILITY_TOOL_NAMES[1],
])
