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
  items: Type.Array(
    Type.Object({
      id: Type.Optional(
        Type.String({
          minLength: 1,
          maxLength: 80,
          description: 'Stable plan item id reused across updates',
        }),
      ),
      title: Type.String({ minLength: 1, maxLength: MAX_PLAN_TITLE_CHARS }),
      status: statusSchema,
      note: Type.Optional(Type.String({ maxLength: MAX_PLAN_NOTE_CHARS })),
      assignee: Type.Optional(
        Type.String({
          maxLength: MAX_PLAN_ASSIGNEE_CHARS,
          description: 'Who owns this plan item: an agent canonical name, or empty when unassigned',
        }),
      ),
      dependsOn: Type.Optional(
        Type.Array(Type.String({ minLength: 1, maxLength: 80 }), {
          maxItems: MAX_PLAN_DEPENDS_ON,
          description:
            'Existing plan item ids that must complete first; self-references and cycles are rejected',
        }),
      ),
    }),
    { maxItems: MAX_PLAN_ITEMS },
  ),
})

function planResult(plan) {
  return { content: [{ type: 'text', text: JSON.stringify({ plan }, null, 2) }], details: { plan } }
}

export function createPlanTools({ getPlan, updatePlan }) {
  const get = async () => planResult(await getPlan?.())
  const update = async (_toolCallId, params) => planResult(await updatePlan?.(params.items))
  return [
    defineTool({
      name: PLAN_TOOL_NAMES[0],
      label: 'Get Plan',
      description: 'Read the current primary Agent session execution plan and progress counts.',
      promptSnippet: 'Read the structured execution plan for the current primary Agent session',
      promptGuidelines: [
        'Use get_plan when the current execution plan is needed and has not already been returned by update_plan.',
        'This plan belongs to the primary Agent session. Subagents may read it for coordination but cannot modify it.',
      ],
      parameters: Type.Object({}),
      execute: get,
    }),
    defineTool({
      name: PLAN_TOOL_NAMES[1],
      label: 'Update Plan',
      description:
        'Replace the current primary Agent session plan with a structured progress snapshot.',
      promptSnippet: 'Create and maintain a concise structured execution plan for multi-step work',
      promptGuidelines: [
        'Use update_plan for work with multiple concrete steps or when the user explicitly asks for a plan.',
        'Keep stable plan item ids when updating status. Preserve unfinished items unless they are genuinely removed from scope.',
        'Set status to in_progress before substantive work, completed only after verification, and blocked only when a concrete blocker exists.',
        'Keep the plan concise and outcome-oriented. Do not create a plan item for trivial narration or every individual tool call.',
        'An empty items array clears the plan.',
        'Only the primary Agent may modify this plan. Subagents have read-only access for coordination.',
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
        'One-release compatibility alias for update_plan. Replace the current primary Agent session execution plan.',
      parameters: updateParameters,
      execute: update,
    }),
  ]
}
