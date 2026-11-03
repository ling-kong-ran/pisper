import assert from 'node:assert/strict'
import test from 'node:test'

import { resolveRuntimeCapabilities } from '../runtime-capabilities.mjs'
import { ToolActivation } from '../runtime/tool-activation.mjs'
import { permissionRequirement } from '../services/session-permission-service.mjs'
import { MULTI_AGENT_TOOL_NAMES } from '../services/multi-agent-service.mjs'
import { TOOL_PRESETS } from '../tools/builtin-catalog.mjs'
import { APP_TOOL_CATALOG } from '../tools/app/index.mjs'
import {
  SCHEDULE_TOOL_NAMES,
  createScheduleListTool,
  createScheduleManageTool,
} from '../tools/app/schedules.mjs'
import {
  WORKFLOW_TOOL_NAMES,
  createWorkflowListTool,
  createWorkflowRunTool,
} from '../tools/app/workflows.mjs'

function fakeRuntime() {
  const calls = []
  const task = (input) => ({
    id: 'task-1',
    name: 'n',
    targetType: 'prompt',
    prompt: 'p',
    enabled: true,
    frequency: 'daily',
    time: '09:00',
    timezone: 'UTC',
    notifications: [],
    lastStatus: 'idle',
    createdBy: { kind: 'agent', sessionId: 's' },
    ...input,
  })
  return {
    calls,
    list: () => ({ tasks: [task({ prompt: 'x'.repeat(2_000) })] }),
    create: async (input) => {
      calls.push(['create', input])
      return task(input)
    },
    update: async (id, input) => {
      calls.push(['update', id, input])
      return id === 'task-1' ? task(input) : null
    },
    remove: async (id) => {
      calls.push(['remove', id])
      return id === 'task-1'
    },
    runNow: async (id) => {
      calls.push(['runNow', id])
      return id === 'task-1' ? task({}) : null
    },
  }
}

test('schedule_manage creates prompt tasks in the current workspace and local timezone', async () => {
  const runtime = fakeRuntime()
  const tool = createScheduleManageTool({ scheduleRuntime: runtime, cwd: '/workspace' })
  const result = await tool.execute('call', {
    action: 'create',
    name: '提醒开会',
    prompt: '提醒我 10 点开周会',
    frequency: 'once',
    date: '2026-11-04',
    time: '09:50',
    notifications: ['feishu'],
  })
  const [[kind, input]] = runtime.calls
  assert.equal(kind, 'create')
  assert.equal(input.targetType, 'prompt')
  assert.equal(input.cwd, '/workspace')
  assert.equal(input.enabled, true)
  assert.equal(input.frequency, 'once')
  assert.equal(input.date, '2026-11-04')
  assert.equal(input.timezone, Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC')
  assert.deepEqual(input.notifications, ['feishu'])
  assert.equal(result.details.task.createdBy, 'agent')
  assert.match(result.details.now.iso, /^\d{4}-\d{2}-\d{2}T/)
})

test('schedule_manage edits only the given fields and maps pause/resume/delete/run_now', async () => {
  const runtime = fakeRuntime()
  const tool = createScheduleManageTool({ scheduleRuntime: runtime, cwd: '/workspace' })
  await tool.execute('call', { action: 'update', id: 'task-1', time: '10:00' })
  await tool.execute('call', { action: 'pause', id: 'task-1' })
  await tool.execute('call', { action: 'resume', id: 'task-1' })
  await tool.execute('call', { action: 'run_now', id: 'task-1' })
  const deleted = await tool.execute('call', { action: 'delete', id: 'task-1' })
  assert.deepEqual(runtime.calls, [
    ['update', 'task-1', { time: '10:00' }],
    ['update', 'task-1', { enabled: false }],
    ['update', 'task-1', { enabled: true }],
    ['runNow', 'task-1'],
    ['remove', 'task-1'],
  ])
  assert.equal(deleted.details.deleted, 'task-1')
  await assert.rejects(tool.execute('call', { action: 'delete', id: 'nope' }), /does not exist/)
  await assert.rejects(tool.execute('call', { action: 'run_now', id: 'nope' }), /does not exist/)
})

test('schedule_list returns the current time and trimmed task summaries', async () => {
  const tool = createScheduleListTool({ scheduleRuntime: fakeRuntime() })
  const result = await tool.execute('call', {})
  assert.ok(result.details.now.local)
  assert.equal(result.details.tasks[0].prompt.length, 500)
  assert.equal(result.details.tasks[0].time, '09:00')
})

test('creating, editing, resuming or running a schedule always needs confirmation outside full access', () => {
  const base = { cwd: '/workspace', toolName: 'schedule_manage' }
  for (const [mode, executionMode] of [
    ['ask', 'approval-required'],
    ['auto', 'workspace-write'],
  ]) {
    for (const action of ['create', 'update', 'resume', 'run_now']) {
      const requirement = permissionRequirement({ ...base, mode, executionMode, args: { action } })
      assert.equal(requirement?.guarded, true, `${executionMode} ${action}`)
      assert.match(requirement.reason, /无人值守、以完全访问权限运行/)
    }
  }
  // 暂停与删除只会停掉工作：工作区写模式直接放行，审批模式按普通高风险确认（可被允许规则记住）。
  for (const action of ['pause', 'delete']) {
    assert.equal(
      permissionRequirement({
        ...base,
        mode: 'auto',
        executionMode: 'workspace-write',
        args: { action },
      }),
      null,
    )
    const ask = permissionRequirement({
      ...base,
      mode: 'ask',
      executionMode: 'approval-required',
      args: { action },
    })
    assert.equal(ask?.guarded, undefined)
    assert.equal(ask?.risk, 'high')
  }
  assert.equal(
    permissionRequirement({
      ...base,
      mode: 'ignore',
      executionMode: 'full-access',
      args: { action: 'create' },
    }),
    null,
  )
})

test('schedule tools are on by default, desktop-only, parent-only and hidden from unattended runs', async () => {
  for (const name of SCHEDULE_TOOL_NAMES) {
    assert.ok(TOOL_PRESETS.full.includes(name), name)
    assert.ok(
      APP_TOOL_CATALOG.some((tool) => tool.id === name),
      name,
    )
    assert.equal(MULTI_AGENT_TOOL_NAMES.includes(name), false)
  }
  const desktop = await resolveRuntimeCapabilities({
    environment: { PISPER_RUNTIME_PROFILE: 'desktop' },
    moduleSupport: { childProcess: true, workerThreads: true, sqlite: true, wasm: true },
  })
  assert.ok(desktop.tools.includes('schedule_manage'))
  const mobile = await resolveRuntimeCapabilities({
    environment: { PISPER_RUNTIME_PROFILE: 'mobile-embedded' },
    moduleSupport: { childProcess: true, workerThreads: true, sqlite: true, wasm: true },
  })
  assert.equal(mobile.tools.includes('schedule_manage'), false)

  const activation = new ToolActivation({
    getSessionMeta: () => ({}),
    saveSessionMeta: async () => {},
    getGoal: () => null,
    promoteTools: async () => ({ routedToolNames: [] }),
  })
  const value = {
    session: { getAllTools: () => [{ name: 'schedule_manage' }, { name: 'schedule_list' }] },
    baseToolNames: ['read'],
  }
  assert.ok(activation.optionalToolNames(value).includes('schedule_manage'))
  value.unattendedRun = true
  const unattended = activation.optionalToolNames(value)
  assert.equal(unattended.includes('schedule_manage'), false)
  assert.equal(unattended.includes('schedule_list'), false)
})

// —— 工作流工具 ——

function fakeWorkflows() {
  const calls = []
  const runs = [
    {
      id: 'r1',
      workflowId: 'wf-pub',
      status: 'completed',
      trigger: 'manual',
      completedNodes: 2,
      totalNodes: 2,
      summary: 'ok',
    },
    {
      id: 'r2',
      workflowId: 'wf-draft',
      status: 'failed',
      trigger: 'manual',
      completedNodes: 0,
      totalNodes: 1,
      error: 'x',
    },
  ]
  return {
    calls,
    list: () => ({
      workflows: [
        {
          id: 'wf-pub',
          name: '日报',
          status: 'published',
          inputs: [{ name: 'day', label: '日期', type: 'string', required: true }],
        },
        { id: 'wf-draft', name: '草稿', status: 'draft', inputs: [] },
      ],
      runs,
    }),
    start: async (id, inputs) => {
      calls.push(['start', id, inputs])
      return {
        id: 'r3',
        workflowId: id,
        status: 'running',
        trigger: 'manual',
        completedNodes: 0,
        totalNodes: 2,
      }
    },
    stop: async (runId) => {
      calls.push(['stop', runId])
      return runId === 'r3'
        ? { id: 'r3', workflowId: 'wf-pub', status: 'cancelled', completedNodes: 1, totalNodes: 2 }
        : null
    },
  }
}

test('workflow_list shows inputs and the recent runs of one workflow', async () => {
  const tool = createWorkflowListTool({ workflowRuntime: fakeWorkflows() })
  const all = await tool.execute('call', {})
  assert.deepEqual(
    all.details.workflows.map((workflow) => [workflow.id, workflow.published]),
    [
      ['wf-pub', true],
      ['wf-draft', false],
    ],
  )
  assert.equal(all.details.runs, undefined)
  const one = await tool.execute('call', { workflowId: 'wf-pub' })
  assert.deepEqual(
    one.details.runs.map((run) => [run.id, run.progress]),
    [['r1', '2/2']],
  )
  await assert.rejects(tool.execute('call', { workflowId: 'nope' }), /does not exist/)
})

test('workflow_run starts only published workflows and can stop a run', async () => {
  const runtime = fakeWorkflows()
  const tool = createWorkflowRunTool({ workflowRuntime: runtime })
  const started = await tool.execute('call', {
    action: 'start',
    workflowId: 'wf-pub',
    inputs: { day: '2026-11-03' },
  })
  assert.equal(started.details.run.status, 'running')
  await assert.rejects(
    tool.execute('call', { action: 'start', workflowId: 'wf-draft' }),
    /Only published/,
  )
  const stopped = await tool.execute('call', { action: 'stop', runId: 'r3' })
  assert.equal(stopped.details.run.status, 'cancelled')
  await assert.rejects(tool.execute('call', { action: 'stop', runId: 'r9' }), /not running/)
  assert.deepEqual(runtime.calls, [
    ['start', 'wf-pub', { day: '2026-11-03' }],
    ['stop', 'r3'],
    ['stop', 'r9'],
  ])
})

test('starting a workflow needs confirmation outside full access; stopping does not escalate', () => {
  const base = { cwd: '/workspace', toolName: 'workflow_run' }
  for (const [mode, executionMode] of [
    ['ask', 'approval-required'],
    ['auto', 'workspace-write'],
  ]) {
    const start = permissionRequirement({ ...base, mode, executionMode, args: { action: 'start' } })
    assert.equal(start?.guarded, true, executionMode)
    assert.match(start.reason, /无人值守、以完全访问权限运行/)
  }
  assert.equal(
    permissionRequirement({
      ...base,
      mode: 'auto',
      executionMode: 'workspace-write',
      args: { action: 'stop' },
    }),
    null,
  )
})

test('schedule_manage can schedule a published workflow instead of a prompt', async () => {
  const runtime = fakeRuntime()
  const tool = createScheduleManageTool({ scheduleRuntime: runtime, cwd: '/workspace' })
  await tool.execute('call', {
    action: 'create',
    name: '每天跑日报',
    workflowId: 'wf-pub',
    workflowInputs: { day: 'today' },
    frequency: 'daily',
    time: '18:00',
  })
  const [[, input]] = runtime.calls
  assert.equal(input.targetType, 'workflow')
  assert.equal(input.workflowId, 'wf-pub')
  assert.deepEqual(input.workflowInputs, { day: 'today' })
  assert.equal(input.prompt, undefined)
  await assert.rejects(
    tool.execute('call', {
      action: 'create',
      name: 'x',
      prompt: 'p',
      workflowId: 'wf-pub',
      frequency: 'daily',
    }),
    /either prompt or workflowId/,
  )
  await assert.rejects(
    tool.execute('call', { action: 'create', name: 'x', frequency: 'daily' }),
    /either prompt or workflowId/,
  )
})

test('workflow tools are on by default, parent-only, gated by the workflows capability and hidden from unattended runs', async () => {
  for (const name of WORKFLOW_TOOL_NAMES) {
    assert.ok(TOOL_PRESETS.full.includes(name), name)
    assert.ok(
      APP_TOOL_CATALOG.some((tool) => tool.id === name),
      name,
    )
  }
  const mobile = await resolveRuntimeCapabilities({
    environment: { PISPER_RUNTIME_PROFILE: 'mobile-embedded' },
    moduleSupport: { childProcess: true, workerThreads: true, sqlite: true, wasm: true },
  })
  assert.equal(mobile.tools.includes('workflow_run'), false)
  const activation = new ToolActivation({
    getSessionMeta: () => ({}),
    saveSessionMeta: async () => {},
    getGoal: () => null,
    promoteTools: async () => ({ routedToolNames: [] }),
  })
  const value = {
    session: { getAllTools: () => [{ name: 'workflow_run' }, { name: 'workflow_list' }] },
    baseToolNames: [],
    unattendedRun: true,
  }
  const unattended = activation.optionalToolNames(value)
  for (const name of WORKFLOW_TOOL_NAMES) assert.equal(unattended.includes(name), false, name)
})
