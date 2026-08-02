/**
 * @deprecated One-release compatibility wrapper. Use plan.mjs and createPlanTools.
 */
import {
  createPlanTools,
  PLAN_COMPATIBILITY_TOOL_NAMES,
} from './plan.mjs'

export const TASK_LIST_TOOL_NAMES = PLAN_COMPATIBILITY_TOOL_NAMES

export function createTaskListTools({ getTaskList, updateTaskList }) {
  return createPlanTools({ getPlan: getTaskList, updatePlan: updateTaskList })
    .filter((tool) => PLAN_COMPATIBILITY_TOOL_NAMES.includes(tool.name))
}
