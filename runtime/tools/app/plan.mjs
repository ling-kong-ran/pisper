// 计划工具：get_plan / update_plan（含兼容命名的 task_list 版本）。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import {
  MAX_PLAN_ASSIGNEE_CHARS,
  MAX_PLAN_DEPENDS_ON,
  MAX_PLAN_ITEMS,
  MAX_PLAN_NOTE_CHARS,
  MAX_PLAN_TITLE_CHARS,
  PLAN_STATUSES,
} from '../../services/plan-service.mjs'
import { PLAN_COMPATIBILITY_TOOL_NAMES, PLAN_TOOL_NAMES } from './plan-tool-names.mjs'

export {
  PLAN_ALL_TOOL_NAMES,
  PLAN_COMPATIBILITY_TOOL_NAMES,
  PLAN_READ_TOOL_NAMES,
  PLAN_TOOL_NAMES,
  PLAN_WRITE_TOOL_NAMES,
} from './plan-tool-names.mjs'

const statusSchema = Type.String({ enum: PLAN_STATUSES })
const updateParameters = Type.Object({
  mode: Type.Optional(Type.String({ enum: ['auto', 'replace'], description: 'Default auto' })),
  items: Type.Array(
    Type.Object({
      id: Type.Optional(Type.String({ minLength: 1, maxLength: 80, description: 'Stable id' })),
      title: Type.String({ minLength: 1, maxLength: MAX_PLAN_TITLE_CHARS }),
      status: statusSchema,
      note: Type.Optional(Type.String({ maxLength: MAX_PLAN_NOTE_CHARS })),
      assignee: Type.Optional(
        Type.String({
          maxLength: MAX_PLAN_ASSIGNEE_CHARS,
          description: 'Agent canonical name, or empty',
        }),
      ),
      dependsOn: Type.Optional(
        Type.Array(Type.String({ minLength: 1, maxLength: 80 }), {
          maxItems: MAX_PLAN_DEPENDS_ON,
          description: 'Item ids that must complete first',
        }),
      ),
    }),
    {
      maxItems: MAX_PLAN_ITEMS,
      description: 'The complete plan',
    },
  ),
})

function planResult(plan) {
  const continuation = plan?.resumed
    ? '\n\nRuntime continuation: a previously unfinished plan is active again; continue it unless the user explicitly cancelled or redirected it.'
    : ''
  return {
    content: [{ type: 'text', text: `${JSON.stringify({ plan }, null, 2)}${continuation}` }],
    details: { plan },
  }
}

export function createPlanTools({ getPlan, updatePlan }) {
  const get = async () => planResult(await getPlan?.())
  const update = async (_toolCallId, params) =>
    planResult(await updatePlan?.(params.items, { mode: params.mode || 'auto' }))
  return [
    defineTool({
      name: PLAN_TOOL_NAMES[0],
      label: 'Get Plan',
      description:
        "Read this session's plan and progress. Subagents can read it but cannot change it.",
      parameters: Type.Object({}),
      execute: get,
    }),
    defineTool({
      name: PLAN_TOOL_NAMES[1],
      label: 'Update Plan',
      description: "Set this session's plan (primary Agent only).",
      promptGuidelines: [
        'Use update_plan for work with several concrete steps or when asked for a plan. Keep items outcome-level, not one per tool call.',
        'Reuse item ids when updating and keep unfinished items unless they left the scope. Set in_progress before substantive work, completed only after verification, blocked only on a concrete blocker.',
        'For a temporary side request, send entirely new ids with mode auto: the unfinished plan is suspended and restored when the new items complete. After resumed: true, continue the restored plan unless the user cancelled or redirected it.',
        'Use mode replace only when the user abandons all earlier work. An empty items array clears the active and suspended plans.',
      ],
      parameters: updateParameters,
      execute: update,
    }),
    defineTool({
      name: PLAN_COMPATIBILITY_TOOL_NAMES[0],
      label: 'Get Plan (Legacy Alias)',
      description:
        'One-release compatibility alias for get_plan. Read the current primary Agent session execution plan.',
      parameters: Type.Object({}),
      execute: get,
    }),
    defineTool({
      name: PLAN_COMPATIBILITY_TOOL_NAMES[1],
      label: 'Update Plan (Legacy Alias)',
      description:
        'One-release compatibility alias for update_plan. Update the current primary Agent session execution plan with automatic suspend and resume behavior.',
      parameters: updateParameters,
      execute: update,
    }),
  ]
}
