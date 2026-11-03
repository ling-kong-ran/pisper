// 工作流工具：workflow_list（只读）+ workflow_run（启动 / 停止已发布的工作流）。
// 工作流节点无人值守、以完全访问权限运行，所以启动在非完全访问会话里逐次确认且不受持久允许规则影响
// （见 session-permission-service 的 workflow_run 分支）；子代理与无人值守的运行拿不到这两个工具，
// 避免工作流节点再启动工作流。新建与编辑工作流仍在工作流页里完成。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'

export const WORKFLOW_TOOL_NAMES = Object.freeze(['workflow_list', 'workflow_run'])
const RECENT_RUNS = 5

export const manifests = [
  {
    id: 'workflow_list',
    name: 'Workflow List',
    category: 'workflows',
    risk: 'low',
    description: 'List Pisper workflows with their inputs, and recent runs of one workflow.',
    scope: 'Application-level workflows',
    capability: 'Read workflows and run results without changing them',
    source: 'app',
  },
  {
    id: 'workflow_run',
    name: 'Workflow Run',
    category: 'workflows',
    risk: 'high',
    description: 'Start or stop a published Pisper workflow.',
    scope: 'Application-level workflows',
    capability:
      'Start workflows whose nodes run unattended with full access; starting needs confirmation unless the session already has full access',
    source: 'app',
  },
]

function publicWorkflow(workflow) {
  return {
    id: workflow.id,
    name: workflow.name,
    description: workflow.description || undefined,
    published: workflow.status === 'published',
    inputs: (workflow.inputs || []).map((input) => ({
      name: input.name,
      label: input.label,
      type: input.type,
      required: input.required,
      defaultValue: input.defaultValue,
    })),
    lastRunAt: workflow.lastRunAt,
    lastStatus: workflow.lastStatus,
  }
}

function publicRun(run) {
  return {
    id: run.id,
    workflowId: run.workflowId,
    status: run.status,
    trigger: run.trigger,
    startedAt: run.startedAt,
    finishedAt: run.finishedAt,
    progress: `${run.completedNodes}/${run.totalNodes}`,
    currentNode: run.currentNodeLabel || undefined,
    summary: run.summary ? String(run.summary).slice(0, 1_200) : undefined,
    error: run.error ? String(run.error).slice(0, 500) : undefined,
  }
}

function resultContent(value) {
  return { content: [{ type: 'text', text: JSON.stringify(value, null, 2) }], details: value }
}

export function createWorkflowListTool({ workflowRuntime }) {
  return defineTool({
    name: manifests[0].id,
    label: manifests[0].name,
    description: manifests[0].description,
    promptSnippet: 'List Pisper workflows, their inputs and recent runs',
    promptGuidelines: [
      'Call workflow_list to find the workflow id and its required inputs before workflow_run.',
      'Pass workflowId to also get its recent runs; use that to check on a run you started.',
    ],
    parameters: Type.Object({
      workflowId: Type.Optional(Type.String({ minLength: 1, maxLength: 200 })),
    }),
    async execute(_toolCallId, params) {
      const state = await workflowRuntime.list()
      const workflows = state.workflows.map(publicWorkflow)
      if (!params.workflowId) return resultContent({ workflows })
      if (!workflows.some((workflow) => workflow.id === params.workflowId))
        throw new Error('Workflow does not exist.')
      const runs = state.runs
        .filter((run) => run.workflowId === params.workflowId)
        .slice(-RECENT_RUNS)
        .reverse()
        .map(publicRun)
      return resultContent({ workflows, runs })
    },
  })
}

export function createWorkflowRunTool({ workflowRuntime }) {
  return defineTool({
    name: manifests[1].id,
    label: manifests[1].name,
    description: manifests[1].description,
    promptSnippet: 'Start or stop a published Pisper workflow',
    promptGuidelines: [
      'Only published workflows can be started. Provide every required input; omitted optional inputs use their defaults.',
      'Workflow nodes run later without the user present and with full access. Starting one asks the user to confirm unless the session already has full access.',
      'workflow_run returns right away; use workflow_list with the workflowId to check progress instead of waiting in a loop.',
      'To run a workflow on a schedule, use schedule_manage with workflowId instead.',
    ],
    parameters: Type.Union([
      Type.Object({
        action: Type.Literal('start'),
        workflowId: Type.String({ minLength: 1, maxLength: 200 }),
        inputs: Type.Optional(Type.Record(Type.String({ maxLength: 120 }), Type.Unknown())),
      }),
      Type.Object({
        action: Type.Literal('stop'),
        runId: Type.String({ minLength: 1, maxLength: 200 }),
      }),
    ]),
    async execute(_toolCallId, params, signal) {
      if (signal?.aborted) throw new Error('Workflow action was cancelled.')
      if (params.action === 'stop') {
        const run = await workflowRuntime.stop(params.runId)
        if (!run) throw new Error('Workflow run is not running.')
        return resultContent({ run: publicRun(run) })
      }
      const workflow = (await workflowRuntime.list()).workflows.find(
        (item) => item.id === params.workflowId,
      )
      if (!workflow) throw new Error('Workflow does not exist.')
      if (workflow.status !== 'published')
        throw new Error('Only published workflows can be started from an agent.')
      const run = await workflowRuntime.start(params.workflowId, params.inputs || {})
      if (!run) throw new Error('Workflow does not exist.')
      return resultContent({ run: publicRun(run) })
    },
  })
}

export const factories = {
  workflow_list: createWorkflowListTool,
  workflow_run: createWorkflowRunTool,
}
