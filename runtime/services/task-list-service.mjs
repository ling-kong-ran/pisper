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
