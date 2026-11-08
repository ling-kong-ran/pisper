// get_plan / update_plan 及兼容别名 get_task_list / update_task_list 的说明。

export const GET_PLAN = Object.freeze({
  description: "Read this session's plan and progress. Subagents can read it but cannot change it.",
})

export const GET_TASK_LIST = Object.freeze({
  description: 'Legacy alias of get_plan.',
})

export const UPDATE_PLAN = Object.freeze({
  description: "Set this session's plan (primary Agent only).",
  guidelines: [
    'Use update_plan for work with several concrete steps or when asked for a plan. Keep items outcome-level, not one per tool call.',
    'Reuse item ids when updating and keep unfinished items unless they left the scope. Set in_progress before substantive work, completed only after verification, blocked only on a concrete blocker.',
    'For a temporary side request, send entirely new ids with mode auto: the unfinished plan is suspended and restored when the new items complete. After resumed: true, continue the restored plan unless the user cancelled or redirected it.',
    'Use mode replace only when the user abandons all earlier work. An empty items array clears the active and suspended plans.',
  ],
  params: {
    mode: 'Default auto',
    id: 'Stable id',
    assignee: 'Agent canonical name, or empty',
    dependsOn: 'Item ids that must complete first',
    items: 'The complete plan',
  },
})

export const UPDATE_TASK_LIST = Object.freeze({
  description: 'Legacy alias of update_plan.',
})

// 暂停的计划恢复后追加在计划结果后面，提醒模型接着做。
export const PLAN_RESUMED =
  'Runtime continuation: a previously unfinished plan is active again; continue it unless the user explicitly cancelled or redirected it.'
