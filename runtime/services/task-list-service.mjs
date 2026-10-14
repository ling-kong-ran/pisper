// 任务列表兼容层：为旧版 TaskListService 命名提供一次性兼容包装，
// 实际实现已迁移到 plan-service.mjs。新代码请直接使用 PlanService。
/**
 * @deprecated One-release compatibility wrapper. Use plan-service.mjs and PlanService.
 */
export {
  PlanService as TaskListService,
  PLAN_STATUSES as TASK_LIST_STATUSES,
  MAX_PLAN_ITEMS as MAX_TASK_LIST_ITEMS,
  MAX_PLAN_TITLE_CHARS as MAX_TASK_TITLE_CHARS,
  MAX_PLAN_NOTE_CHARS as MAX_TASK_NOTE_CHARS,
  MAX_PLAN_ASSIGNEE_CHARS as MAX_TASK_ASSIGNEE_CHARS,
  MAX_PLAN_DEPENDS_ON as MAX_TASK_DEPENDS_ON,
} from './plan-service.mjs'
