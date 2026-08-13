import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { WorkflowService } from '../services/workflow-service.mjs'

async function waitFor(check, timeoutMs = 3000) {
  const started = Date.now()
  while (!check()) {
    if (Date.now() - started > timeoutMs)
      throw new Error('Timed out waiting for workflow execution.')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}

test('workflows persist and execute Agent nodes in order with completion notifications', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflows-'))
  const prompts = []
  const notifications = []
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'),
    cwd: directory,
    agent: {
      validateDirectory: async (value) => value,
      abort: async () => true,
      prompt: async (input) => {
        prompts.push(input)
        input.onSession?.('workflow-session')
        return { sessionId: 'workflow-session', text: `完成 ${prompts.length}`, assets: [] }
      },
    },
    notifications: {
      notify: async (...args) => {
        notifications.push(args)
      },
    },
  })
  await service.init()
  const workflow = await service.create({
    name: '发布检查',
    status: 'published',
    cwd: directory,
    notifications: ['browser', 'feishu'],
    nodes: [
      { id: 'trigger', kind: 'trigger', label: '手动触发' },
      { id: 'test', kind: 'prompt', label: '运行测试', prompt: '运行测试' },
      {
        id: 'report',
        kind: 'prompt',
        label: '生成报告',
        prompt: '生成报告',
        executionMode: 'workspace-write',
      },
      { id: 'notify', kind: 'notification', label: '通知' },
    ],
  })
  const run = await service.runNow(workflow.id)
  await waitFor(
    () =>
      service.getState().runs.find((item) => item.id === run.id)?.status === 'completed' &&
      notifications.length === 1,
  )
  const completed = service.getState().runs.find((item) => item.id === run.id)
  assert.equal(prompts.length, 2)
  assert.equal(prompts[0].executionMode, 'full-access')
  assert.equal(prompts[0].isolatedContext, true)
  assert.equal(prompts[1].sessionId, 'workflow-session')
  assert.equal(prompts[1].executionMode, 'workspace-write')
  assert.equal(prompts[1].isolatedContext, true)
  assert.equal(completed.completedNodes, 4)
  assert.equal(completed.summary, '完成 2')
  assert.equal(notifications[0][0], 'workflow.completed')
  assert.deepEqual(notifications[0][2], { platforms: ['browser', 'feishu'] })

  const restored = new WorkflowService({
    path: join(directory, 'workflows.json'),
    cwd: directory,
    agent: service.agent,
    notifications: service.notifications,
  })
  await restored.init()
  assert.equal(restored.getState().workflows[0].name, '发布检查')
  assert.equal(restored.getState().workflows[0].edges.length, 3)
  assert.equal(restored.getState().workflows[0].nodes[1].executionMode, 'full-access')
  assert.equal(restored.getState().workflows[0].nodes[2].executionMode, 'workspace-write')
  await service.dispose()
  await restored.dispose()
  await rm(directory, { recursive: true, force: true })
})

test('notification nodes render configured content from inputs and upstream results', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflow-notification-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const notifications = []
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'),
    cwd: directory,
    agent: {
      validateDirectory: async (value) => value,
      abort: async () => true,
      prompt: async () => ({ sessionId: 'notification-session', text: '构建通过', assets: [] }),
    },
    notifications: {
      notify: async (...args) => notifications.push(args),
    },
  })
  await service.init()
  const workflow = await service.create({
    name: '发布检查',
    inputs: [
      {
        id: 'environment',
        name: 'environment',
        label: '环境',
        type: 'string',
        required: true,
      },
    ],
    nodes: [
      { id: 'trigger', kind: 'trigger', label: '触发器' },
      { id: 'build', kind: 'prompt', label: '构建', prompt: '执行构建' },
      {
        id: 'notify',
        kind: 'notification',
        label: '通知结果',
        notification: {
          title: '{{workflow.name}} · {{inputs.environment}}',
          content:
            '环境：{{inputs.environment}}\n结果：{{previous.summary}}\n输出：{{nodes.build.output}}',
        },
        notificationTargets: ['browser', 'feishu'],
      },
    ],
  })

  const run = await service.runNow(workflow.id, { inputs: { environment: '生产' } })
  await waitFor(() => notifications.length === 1 && service.getRun(run.id)?.status === 'completed')

  assert.equal(notifications[0][0], 'workflow.completed')
  assert.equal(notifications[0][1].workflow.summary, '环境：生产\n结果：构建通过\n输出：构建通过')
  assert.deepEqual(notifications[0][2], {
    platforms: ['browser', 'feishu'],
    title: '发布检查 · 生产',
    content: '环境：生产\n结果：构建通过\n输出：构建通过',
  })
  const completed = service.getRun(run.id)
  assert.equal(
    completed.nodes.find((node) => node.id === 'notify').summary,
    notifications[0][2].content,
  )
  await service.dispose()
})

test('workflow edges determine execution order independently from canvas node order', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflow-graph-'))
  const prompts = []
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'),
    cwd: directory,
    agent: {
      validateDirectory: async (value) => value,
      abort: async () => true,
      prompt: async (input) => {
        prompts.push(input)
        input.onSession?.('graph-session')
        return {
          sessionId: 'graph-session',
          text: input.message.includes('第一步') ? 'first' : 'second',
          assets: [],
        }
      },
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  const workflow = await service.create({
    name: '图顺序',
    status: 'published',
    nodes: [
      { id: 'second', kind: 'prompt', label: '第二步', prompt: '第二步' },
      { id: 'trigger', kind: 'trigger', label: '触发器' },
      { id: 'first', kind: 'prompt', label: '第一步', prompt: '第一步' },
    ],
    edges: [
      { id: 'edge-a', source: 'trigger', target: 'first' },
      { id: 'edge-b', source: 'first', target: 'second' },
    ],
  })
  const run = await service.runNow(workflow.id)
  await waitFor(
    () => service.getState().runs.find((item) => item.id === run.id)?.status === 'completed',
  )
  assert.match(prompts[0].message, /第一步/)
  assert.match(prompts[1].message, /第二步/)
  assert.match(prompts[1].message, /第一步：first/)
  assert.equal(prompts[1].sessionId, 'graph-session')
  await service.dispose()
  await rm(directory, { recursive: true, force: true })
})

test('published workflows reject cycles and disconnected nodes', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflow-invalid-'))
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'),
    cwd: directory,
    agent: {
      validateDirectory: async (value) => value,
      abort: async () => true,
      prompt: async () => ({ text: '' }),
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  await assert.rejects(
    () =>
      service.create({
        name: '循环图',
        status: 'published',
        nodes: [
          { id: 'a', kind: 'prompt', label: 'A', prompt: 'A' },
          { id: 'b', kind: 'prompt', label: 'B', prompt: 'B' },
        ],
        edges: [
          { id: 'a-b', source: 'a', target: 'b' },
          { id: 'b-a', source: 'b', target: 'a' },
        ],
      }),
    /循环连接/,
  )
  await assert.rejects(
    () =>
      service.create({
        name: '断开图',
        status: 'published',
        nodes: [
          { id: 'trigger', kind: 'trigger', label: '触发器' },
          { id: 'a', kind: 'prompt', label: 'A', prompt: 'A' },
          { id: 'b', kind: 'prompt', label: 'B', prompt: 'B' },
        ],
        edges: [{ id: 'trigger-a', source: 'trigger', target: 'a' }],
      }),
    /尚未连接/,
  )
  await service.dispose()
  await rm(directory, { recursive: true, force: true })
})

test('failed nodes can retry and skip without terminating the workflow', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflow-skip-'))
  let calls = 0
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'),
    cwd: directory,
    agent: {
      validateDirectory: async (value) => value,
      abort: async () => true,
      prompt: async ({ onSession }) => {
        onSession?.('skip-session')
        calls += 1
        if (calls <= 2) throw new Error('暂时失败')
        return { sessionId: 'skip-session', text: '后续节点完成', assets: [] }
      },
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  const workflow = await service.create({
    name: '容错流程',
    nodes: [
      {
        id: 'unstable',
        kind: 'prompt',
        label: '不稳定节点',
        prompt: '执行',
        retries: 1,
        failurePolicy: 'skip',
      },
      { id: 'next', kind: 'prompt', label: '后续节点', prompt: '继续' },
    ],
  })
  const run = await service.runNow(workflow.id)
  await waitFor(
    () => service.getState().runs.find((item) => item.id === run.id)?.status !== 'running',
  )
  const completed = service.getState().runs.find((item) => item.id === run.id)
  assert.equal(completed.status, 'completed')
  assert.equal(completed.nodes[0].status, 'skipped')
  assert.equal(completed.nodes[0].attempts, 2)
  assert.equal(completed.nodes[1].status, 'completed')
  await service.dispose()
  await rm(directory, { recursive: true, force: true })
})

test('workflow inputs, JSON output, and condition ports select only the matching branch', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflow-condition-'))
  const prompts = []
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'),
    cwd: directory,
    agent: {
      validateDirectory: async (value) => value,
      abort: async () => true,
      prompt: async (input) => {
        prompts.push(input.message)
        return { sessionId: `condition-${prompts.length}`, text: '{"accepted":true}', assets: [] }
      },
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  const workflow = await service.create({
    name: '条件流程',
    status: 'published',
    inputs: [
      { id: 'approved', name: 'approved', label: '是否批准', type: 'boolean', required: true },
    ],
    nodes: [
      { id: 'trigger', kind: 'trigger', label: '触发器' },
      {
        id: 'condition',
        kind: 'condition',
        label: '检查批准状态',
        condition: { source: 'inputs.approved', operator: 'equals', value: true },
      },
      {
        id: 'accepted',
        kind: 'prompt',
        label: '批准分支',
        prompt: '执行批准分支',
        outputFormat: 'json',
      },
      { id: 'rejected', kind: 'prompt', label: '拒绝分支', prompt: '执行拒绝分支' },
    ],
    edges: [
      { id: 'trigger-condition', source: 'trigger', target: 'condition' },
      { id: 'condition-accepted', source: 'condition', sourcePort: 'true', target: 'accepted' },
      { id: 'condition-rejected', source: 'condition', sourcePort: 'false', target: 'rejected' },
    ],
  })
  const run = await service.runNow(workflow.id, { inputs: { approved: true } })
  await waitFor(() => service.getRun(run.id)?.status === 'completed')
  const completed = service.getRun(run.id)
  assert.equal(prompts.length, 1)
  assert.match(prompts[0], /执行批准分支/)
  assert.deepEqual(completed.inputs, { approved: true })
  assert.equal(completed.nodes.find((node) => node.id === 'condition').selectedPort, 'true')
  assert.deepEqual(completed.nodes.find((node) => node.id === 'accepted').output, {
    accepted: true,
  })
  assert.equal(completed.nodes.find((node) => node.id === 'rejected').status, 'skipped')
  assert.equal(
    completed.nodes.find((node) => node.id === 'rejected').skipReason,
    'branch_not_selected',
  )
  await service.dispose()
  await rm(directory, { recursive: true, force: true })
})

test('parallel branches start together and join after both complete', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflow-parallel-'))
  const started = []
  const releases = new Map()
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'),
    cwd: directory,
    agent: {
      validateDirectory: async (value) => value,
      abort: async () => true,
      prompt: ({ message, onSession }) =>
        new Promise((resolve) => {
          const branch = message.includes('分支 A') ? 'a' : 'b'
          started.push(branch)
          onSession?.(`parallel-${branch}`)
          releases.set(branch, () =>
            resolve({ sessionId: `parallel-${branch}`, text: branch, assets: [] }),
          )
        }),
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  const workflow = await service.create({
    name: '并行流程',
    nodes: [
      { id: 'trigger', kind: 'trigger', label: '触发器' },
      { id: 'parallel', kind: 'parallel', label: '并行启动' },
      { id: 'a', kind: 'prompt', label: '分支 A', prompt: '分支 A' },
      { id: 'b', kind: 'prompt', label: '分支 B', prompt: '分支 B' },
      { id: 'join', kind: 'notification', label: '汇总' },
    ],
    edges: [
      { id: 'trigger-parallel', source: 'trigger', target: 'parallel' },
      { id: 'parallel-a', source: 'parallel', target: 'a' },
      { id: 'parallel-b', source: 'parallel', target: 'b' },
      { id: 'a-join', source: 'a', target: 'join' },
      { id: 'b-join', source: 'b', target: 'join' },
    ],
  })
  const run = await service.runNow(workflow.id)
  await waitFor(() => started.length === 2)
  assert.deepEqual(new Set(started), new Set(['a', 'b']))
  assert.equal(service.getRun(run.id).status, 'running')
  releases.get('a')()
  await new Promise((resolve) => setTimeout(resolve, 20))
  assert.equal(service.getRun(run.id).status, 'running')
  releases.get('b')()
  await waitFor(() => service.getRun(run.id)?.status === 'completed')
  assert.equal(service.getRun(run.id).nodes.find((node) => node.id === 'join').status, 'completed')
  await service.dispose()
  await rm(directory, { recursive: true, force: true })
})

test('approval nodes pause a run and resume after approval', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflow-approval-'))
  let promptCalls = 0
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'),
    cwd: directory,
    agent: {
      validateDirectory: async (value) => value,
      abort: async () => true,
      prompt: async () => {
        promptCalls += 1
        return { sessionId: 'approval-session', text: '已发布', assets: [] }
      },
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  const workflow = await service.create({
    name: '审批流程',
    nodes: [
      { id: 'trigger', kind: 'trigger', label: '触发器' },
      {
        id: 'approval',
        kind: 'approval',
        label: '发布审批',
        approval: { message: '确认发布？', timeoutMinutes: 1 },
      },
      { id: 'publish', kind: 'prompt', label: '发布', prompt: '执行发布' },
    ],
    edges: [
      { id: 'trigger-approval', source: 'trigger', target: 'approval' },
      { id: 'approval-publish', source: 'approval', target: 'publish' },
    ],
  })
  const run = await service.runNow(workflow.id)
  await waitFor(() => service.getRun(run.id)?.status === 'waiting_approval')
  assert.equal(promptCalls, 0)
  assert.equal(
    service.getRun(run.id).nodes.find((node) => node.id === 'approval').status,
    'waiting_approval',
  )
  assert.ok(await service.resolveApproval(run.id, 'approval', true, '可以发布'))
  await waitFor(() => service.getRun(run.id)?.status === 'completed')
  assert.equal(promptCalls, 1)
  assert.deepEqual(service.getRun(run.id).nodes.find((node) => node.id === 'approval').output, {
    approved: true,
    comment: '可以发布',
  })
  await service.dispose()
  await rm(directory, { recursive: true, force: true })
})

test('workflow revisions remain attached to runs and duplicate preserves graph connections', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflow-version-'))
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'),
    cwd: directory,
    agent: {
      validateDirectory: async (value) => value,
      abort: async () => true,
      prompt: async () => ({ sessionId: 'version-session', text: 'done', assets: [] }),
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  const workflow = await service.create({
    name: '版本流程',
    status: 'published',
    visibility: 'shared',
    tags: ['release'],
    nodes: [
      { id: 'trigger', kind: 'trigger', label: '触发器' },
      { id: 'task', kind: 'prompt', label: '任务', prompt: '执行任务' },
    ],
    edges: [{ id: 'trigger-task', source: 'trigger', target: 'task' }],
  })
  const firstRun = await service.runNow(workflow.id)
  await waitFor(() => service.getRun(firstRun.id)?.status === 'completed')
  const updated = await service.update(workflow.id, { description: '第二版' })
  assert.equal(updated.revision, 2)
  assert.equal(service.getRun(firstRun.id).workflowRevision, 1)

  const duplicate = await service.duplicate(workflow.id)
  assert.equal(duplicate.revision, 1)
  assert.equal(duplicate.status, 'draft')
  assert.equal(duplicate.nodes.length, 2)
  assert.equal(duplicate.edges.length, 1)
  assert.equal(duplicate.edges[0].source, duplicate.nodes[0].id)
  assert.equal(duplicate.edges[0].target, duplicate.nodes[1].id)

  const exported = service.exportWorkflow(workflow.id)
  assert.equal(exported.format, 'pisper-workflow')
  assert.equal(exported.workflow.visibility, 'shared')
  assert.equal(exported.workflow.id, undefined)
  const imported = await service.importWorkflow(exported)
  assert.equal(imported.status, 'draft')
  assert.equal(imported.visibility, 'private')
  assert.equal(imported.nodes.length, 2)
  assert.equal(imported.edges.length, 1)
  await assert.rejects(() => service.importWorkflow({ format: 'other' }), /有效的 Pisper 工作流/)
  await service.dispose()
  await rm(directory, { recursive: true, force: true })
})

test('active workflow runs can abort their Agent session', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflow-stop-'))
  let rejectPrompt
  const aborted = []
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'),
    cwd: directory,
    agent: {
      validateDirectory: async (value) => value,
      prompt: ({ onSession }) =>
        new Promise((_resolve, reject) => {
          onSession?.('active-session')
          rejectPrompt = reject
        }),
      abort: async (sessionId) => {
        aborted.push(sessionId)
        rejectPrompt?.(new Error('aborted'))
        return true
      },
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  const workflow = await service.create({
    name: '可停止流程',
    nodes: [{ id: 'wait', kind: 'prompt', label: '长任务', prompt: '等待' }],
  })
  const run = await service.runNow(workflow.id)
  await waitFor(
    () =>
      service.getState().runs.find((item) => item.id === run.id)?.sessionId === 'active-session',
  )
  await service.stop(run.id)
  await waitFor(
    () => service.getState().runs.find((item) => item.id === run.id)?.status === 'cancelled',
  )
  assert.deepEqual(aborted, ['active-session'])
  await service.dispose()
  await rm(directory, { recursive: true, force: true })
})
