// workflow_list / workflow_run 的说明。

export const WORKFLOW_LIST = Object.freeze({
  description: 'List Pisper workflows with their inputs, and recent runs of one workflow.',
  snippet: 'List Pisper workflows, their inputs and recent runs',
  guidelines: [
    'Call workflow_list to find the workflow id and its required inputs before workflow_run.',
    'Pass workflowId to also get its recent runs; use that to check on a run you started.',
  ],
})

export const WORKFLOW_RUN = Object.freeze({
  description: 'Start or stop a published Pisper workflow.',
  snippet: 'Start or stop a published Pisper workflow',
  guidelines: [
    'Only published workflows can be started. Provide every required input; omitted optional inputs use their defaults.',
    'Workflow nodes run later without the user present and with full access. Starting one asks the user to confirm unless the session already has full access.',
    'workflow_run returns right away; use workflow_list with the workflowId to check progress instead of waiting in a loop.',
    'To run a workflow on a schedule, use schedule_manage with workflowId instead.',
  ],
})
