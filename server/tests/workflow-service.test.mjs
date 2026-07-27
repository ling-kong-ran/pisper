import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { WorkflowService } from '../services/workflow-service.mjs'

async function waitFor(check, timeoutMs = 3000) {
  const started = Date.now()
  while (!check()) {
    if (Date.now() - started > timeoutMs) throw new Error('Timed out waiting for workflow execution.')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}

test('workflows persist and execute Agent nodes in order with completion notifications', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflows-'))
  const prompts = []
  const notifications = []
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'), cwd: directory,
    agent: {
      validateDirectory: async (value) => value,
      abort: async () => true,
      prompt: async (input) => {
        prompts.push(input)
        input.onSession?.('workflow-session')
        return { sessionId: 'workflow-session', text: `完成 ${prompts.length}`, assets: [] }
      },
    },
    notifications: { notify: async (...args) => { notifications.push(args) } },
  })
  await service.init()
  const workflow = await service.create({
    name: '发布检查', status: 'published', cwd: directory, notifications: ['browser', 'feishu'],
    nodes: [
      { id: 'trigger', kind: 'trigger', label: '手动触发' },
      { id: 'test', kind: 'prompt', label: '运行测试', prompt: '运行测试' },
      { id: 'report', kind: 'prompt', label: '生成报告', prompt: '生成报告' },
      { id: 'notify', kind: 'notification', label: '通知' },
    ],
  })
  const run = await service.runNow(workflow.id)
  await waitFor(() => service.getState().runs.find((item) => item.id === run.id)?.status === 'completed' && notifications.length === 1)
  const completed = service.getState().runs.find((item) => item.id === run.id)
  assert.equal(prompts.length, 2)
  assert.equal(prompts[1].sessionId, 'workflow-session')
  assert.equal(completed.completedNodes, 4)
  assert.equal(completed.summary, '完成 2')
  assert.equal(notifications[0][0], 'workflow.completed')
  assert.deepEqual(notifications[0][2], { platforms: ['browser', 'feishu'] })

  const restored = new WorkflowService({ path: join(directory, 'workflows.json'), cwd: directory, agent: service.agent, notifications: service.notifications })
  await restored.init()
  assert.equal(restored.getState().workflows[0].name, '发布检查')
  assert.equal(restored.getState().workflows[0].edges.length, 3)
  await service.dispose()
  await restored.dispose()
  await rm(directory, { recursive: true, force: true })
})

test('workflow edges determine execution order independently from canvas node order', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflow-graph-'))
  const prompts = []
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'), cwd: directory,
    agent: {
      validateDirectory: async (value) => value,
      abort: async () => true,
      prompt: async (input) => {
        prompts.push(input)
        input.onSession?.('graph-session')
        return { sessionId: 'graph-session', text: input.message.includes('第一步') ? 'first' : 'second', assets: [] }
      },
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  const workflow = await service.create({
    name: '图顺序', status: 'published',
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
  await waitFor(() => service.getState().runs.find((item) => item.id === run.id)?.status === 'completed')
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
    path: join(directory, 'workflows.json'), cwd: directory,
    agent: { validateDirectory: async (value) => value, abort: async () => true, prompt: async () => ({ text: '' }) },
    notifications: { notify: async () => {} },
  })
  await service.init()
  await assert.rejects(() => service.create({
    name: '循环图', status: 'published',
    nodes: [
      { id: 'a', kind: 'prompt', label: 'A', prompt: 'A' },
      { id: 'b', kind: 'prompt', label: 'B', prompt: 'B' },
    ],
    edges: [
      { id: 'a-b', source: 'a', target: 'b' },
      { id: 'b-a', source: 'b', target: 'a' },
    ],
  }), /循环连接/)
  await assert.rejects(() => service.create({
    name: '断开图', status: 'published',
    nodes: [
      { id: 'trigger', kind: 'trigger', label: '触发器' },
      { id: 'a', kind: 'prompt', label: 'A', prompt: 'A' },
      { id: 'b', kind: 'prompt', label: 'B', prompt: 'B' },
    ],
    edges: [{ id: 'trigger-a', source: 'trigger', target: 'a' }],
  }), /尚未连接/)
  await service.dispose()
  await rm(directory, { recursive: true, force: true })
})

test('failed nodes can retry and skip without terminating the workflow', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflow-skip-'))
  let calls = 0
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'), cwd: directory,
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
  const workflow = await service.create({ name: '容错流程', nodes: [
    { id: 'unstable', kind: 'prompt', label: '不稳定节点', prompt: '执行', retries: 1, failurePolicy: 'skip' },
    { id: 'next', kind: 'prompt', label: '后续节点', prompt: '继续' },
  ] })
  const run = await service.runNow(workflow.id)
  await waitFor(() => service.getState().runs.find((item) => item.id === run.id)?.status !== 'running')
  const completed = service.getState().runs.find((item) => item.id === run.id)
  assert.equal(completed.status, 'completed')
  assert.equal(completed.nodes[0].status, 'skipped')
  assert.equal(completed.nodes[0].attempts, 2)
  assert.equal(completed.nodes[1].status, 'completed')
  await service.dispose()
  await rm(directory, { recursive: true, force: true })
})

test('active workflow runs can abort their Agent session', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-workflow-stop-'))
  let rejectPrompt
  const aborted = []
  const service = new WorkflowService({
    path: join(directory, 'workflows.json'), cwd: directory,
    agent: {
      validateDirectory: async (value) => value,
      prompt: ({ onSession }) => new Promise((_resolve, reject) => { onSession?.('active-session'); rejectPrompt = reject }),
      abort: async (sessionId) => { aborted.push(sessionId); rejectPrompt?.(new Error('aborted')); return true },
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  const workflow = await service.create({ name: '可停止流程', nodes: [{ id: 'wait', kind: 'prompt', label: '长任务', prompt: '等待' }] })
  const run = await service.runNow(workflow.id)
  await waitFor(() => service.getState().runs.find((item) => item.id === run.id)?.sessionId === 'active-session')
  await service.stop(run.id)
  await waitFor(() => service.getState().runs.find((item) => item.id === run.id)?.status === 'cancelled')
  assert.deepEqual(aborted, ['active-session'])
  await service.dispose()
  await rm(directory, { recursive: true, force: true })
})
