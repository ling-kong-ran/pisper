import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  calculateNextRun,
  DEFAULT_SCHEDULE_EXECUTION_MODE,
  ScheduleService,
} from '../services/schedule-service.mjs'

async function waitFor(predicate, timeoutMs = 1500) {
  const started = Date.now()
  while (!predicate()) {
    if (Date.now() - started > timeoutMs)
      throw new Error('Timed out waiting for scheduled task execution.')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}

test('service startup reconciles stale running runs with interrupted tasks', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-schedule-recovery-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'schedules.json')
  const startedAt = '2026-08-09T01:00:12.043Z'
  await writeFile(
    path,
    JSON.stringify({
      version: 1,
      tasks: [
        {
          id: 'task-1',
          name: 'Interrupted task',
          prompt: 'test',
          enabled: false,
          lastStatus: 'interrupted',
          lastRunAt: startedAt,
        },
        {
          id: 'task-2',
          name: 'Running task without history',
          prompt: 'test',
          enabled: false,
          lastStatus: 'running',
          lastRunAt: startedAt,
        },
      ],
      runs: [
        {
          id: 'run-1',
          taskId: 'task-1',
          trigger: 'scheduled',
          status: 'running',
          startedAt,
          finishedAt: null,
          durationMs: 0,
          summary: '',
          error: '',
          sessionId: '',
        },
      ],
    }),
  )
  const service = new ScheduleService({
    path,
    cwd: directory,
    tickMs: 60_000,
    agent: { validateDirectory: async () => directory, prompt: async () => ({ text: 'done' }) },
    notifications: { notify: async () => {} },
  })
  await service.init()
  t.after(() => service.dispose())

  const state = service.getState()
  assert.equal(state.tasks[0].targetType, 'prompt')
  assert.equal(state.tasks[0].workflowId, '')
  assert.deepEqual(state.tasks[0].workflowInputs, {})
  assert.equal(state.tasks[0].lastStatus, 'interrupted')
  assert.match(state.tasks[0].lastError, /Pisper 重启/)
  assert.equal(state.tasks[1].lastStatus, 'interrupted')
  assert.match(state.tasks[1].lastError, /Pisper 重启/)
  assert.equal(state.runs[0].status, 'interrupted')
  assert.match(state.runs[0].error, /Pisper 重启/)
  assert.ok(state.runs[0].finishedAt)
  assert.ok(state.runs[0].durationMs > 0)

  const persisted = JSON.parse(await readFile(path, 'utf8'))
  assert.equal(persisted.tasks[0].lastStatus, 'interrupted')
  assert.equal(persisted.tasks[1].lastStatus, 'interrupted')
  assert.equal(persisted.runs[0].status, 'interrupted')
  assert.equal(persisted.runs[0].finishedAt, state.runs[0].finishedAt)
})

test('scheduled task records notification delivery failures without changing the run result', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-schedule-notification-failure-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new ScheduleService({
    path: join(directory, 'schedules.json'),
    cwd: directory,
    tickMs: 60_000,
    agent: { validateDirectory: async () => directory, prompt: async () => ({ text: 'done' }) },
    notifications: {
      notify: async () => {
        throw new Error('通知发送失败：weixin: prepare failed')
      },
    },
  })
  await service.init()
  t.after(() => service.dispose())
  const task = await service.create({
    name: 'daily',
    prompt: 'test',
    frequency: 'daily',
    time: '09:00',
    timezone: 'UTC',
    notifications: ['weixin'],
    notifyOn: 'always',
  })
  await service.runNow(task.id)
  await waitFor(() => Boolean(service.getState().runs[0]?.notificationError))
  await waitFor(() => service.executions.size === 0)
  const state = service.getState()
  assert.equal(state.runs[0].status, 'completed')
  assert.match(state.runs[0].notificationError, /weixin: prepare failed/)
  assert.match(state.tasks[0].lastNotificationError, /weixin: prepare failed/)
})

test('next run calculation supports daily, weekly and monthly schedules', () => {
  const from = new Date('2026-07-18T10:00:00.000Z')
  assert.equal(
    calculateNextRun({ frequency: 'interval', intervalValue: 30, intervalUnit: 'minutes' }, from),
    '2026-07-18T10:30:00.000Z',
  )
  assert.equal(
    calculateNextRun({ frequency: 'interval', intervalValue: 6, intervalUnit: 'hours' }, from),
    '2026-07-18T16:00:00.000Z',
  )
  assert.equal(
    calculateNextRun({ frequency: 'daily', time: '09:00', timezone: 'UTC' }, from),
    '2026-07-19T09:00:00.000Z',
  )
  assert.equal(
    calculateNextRun({ frequency: 'weekly', dayOfWeek: 0, time: '09:00', timezone: 'UTC' }, from),
    '2026-07-19T09:00:00.000Z',
  )
  assert.equal(
    calculateNextRun({ frequency: 'monthly', dayOfMonth: 1, time: '09:00', timezone: 'UTC' }, from),
    '2026-08-01T09:00:00.000Z',
  )
})

test('scheduled tasks persist, execute with the selected model and notify multiple targets', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-schedules-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const prompts = []
  const notifications = []
  const service = new ScheduleService({
    path: join(directory, 'schedules.json'),
    cwd: directory,
    tickMs: 60_000,
    agent: {
      validateDirectory: async (cwd) => cwd || directory,
      prompt: async (input) => {
        prompts.push(input)
        return { sessionId: 'session-1', text: '检查完成，没有发现失败测试。' }
      },
    },
    notifications: {
      notify: async (...args) => {
        notifications.push(args)
      },
    },
  })
  await service.init()
  t.after(() => service.dispose())
  const task = await service.create({
    name: '每日检查',
    prompt: '运行测试',
    frequency: 'daily',
    time: '09:00',
    timezone: 'UTC',
    cwd: directory,
    model: { provider: 'openai', model: 'gpt-5.4' },
    notifications: ['browser', 'feishu', 'weixin'],
    notifyOn: 'always',
  })
  await service.runNow(task.id)
  await waitFor(() => notifications.length === 1)
  await waitFor(() => service.executions.size === 0)
  const state = service.getState()
  assert.equal(state.tasks[0].lastStatus, 'completed')
  assert.equal(state.runs[0].status, 'completed')
  assert.equal(state.tasks[0].executionMode, DEFAULT_SCHEDULE_EXECUTION_MODE)
  assert.equal(prompts[0].cwd, directory)
  assert.equal(prompts[0].executionMode, 'full-access')
  assert.equal(prompts[0].isolatedContext, true)
  assert.deepEqual(prompts[0].model, { provider: 'openai', model: 'gpt-5.4' })
  assert.deepEqual(notifications[0][2], { platforms: ['browser', 'feishu', 'weixin'] })
  assert.equal(notifications[0][0], 'schedule.completed')
  assert.match(notifications[0][1].task.summary, /检查完成/)
})

test('scheduled tasks run published workflows with structured inputs', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-schedules-workflow-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const workflowCalls = []
  const workflows = [
    {
      id: 'workflow-1',
      name: 'Release checks',
      status: 'published',
      inputs: [{ name: 'branch', type: 'string', required: true, defaultValue: '' }],
    },
  ]
  const service = new ScheduleService({
    path: join(directory, 'schedules.json'),
    cwd: directory,
    tickMs: 60_000,
    agent: { validateDirectory: async () => directory, prompt: async () => ({ text: 'unused' }) },
    workflows: {
      list: () => workflows,
      run: async (id, options) => {
        workflowCalls.push([id, options])
        return { id: 'workflow-run-1', status: 'running' }
      },
      getRun: () => ({
        id: 'workflow-run-1',
        status: 'completed',
        summary: 'Release checks passed.',
        sessionId: 'hidden-workflow-session',
      }),
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  t.after(() => service.dispose())

  const task = await service.create({
    name: 'Nightly release checks',
    targetType: 'workflow',
    workflowId: 'workflow-1',
    workflowInputs: { branch: 'release', retries: 2 },
    frequency: 'daily',
    time: '09:00',
    timezone: 'UTC',
  })
  await service.runNow(task.id)
  await waitFor(() => service.executions.size === 0)

  const state = service.getState()
  assert.equal(state.tasks[0].targetType, 'workflow')
  assert.equal(state.tasks[0].workflowId, 'workflow-1')
  assert.deepEqual(state.tasks[0].workflowInputs, { branch: 'release', retries: 2 })
  assert.deepEqual(workflowCalls, [
    [
      'workflow-1',
      {
        trigger: 'schedule',
        sourceMessage: 'Nightly release checks',
        inputs: { branch: 'release', retries: 2 },
      },
    ],
  ])
  assert.equal(state.runs[0].status, 'completed')
  assert.equal(state.runs[0].workflowRunId, 'workflow-run-1')
  assert.equal(state.runs[0].sessionId, 'hidden-workflow-session')
  assert.equal(state.runs[0].summary, 'Release checks passed.')
})

test('scheduled workflow failures propagate to the schedule run', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-schedules-workflow-failure-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new ScheduleService({
    path: join(directory, 'schedules.json'),
    cwd: directory,
    tickMs: 60_000,
    agent: { validateDirectory: async () => directory, prompt: async () => ({ text: 'unused' }) },
    workflows: {
      list: () => [{ id: 'workflow-1', name: 'Failure', status: 'published', inputs: [] }],
      run: async () => ({ id: 'workflow-run-failed', status: 'running' }),
      getRun: () => ({
        id: 'workflow-run-failed',
        status: 'failed',
        error: 'Workflow node failed.',
      }),
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  t.after(() => service.dispose())
  const task = await service.create({
    name: 'Failing workflow',
    targetType: 'workflow',
    workflowId: 'workflow-1',
    frequency: 'daily',
    time: '09:00',
    timezone: 'UTC',
  })
  await service.runNow(task.id)
  await waitFor(() => service.executions.size === 0)

  const state = service.getState()
  assert.equal(state.tasks[0].lastStatus, 'failed')
  assert.equal(state.tasks[0].lastError, 'Workflow node failed.')
  assert.equal(state.runs[0].workflowRunId, 'workflow-run-failed')
  assert.equal(state.runs[0].status, 'failed')
  assert.equal(state.runs[0].error, 'Workflow node failed.')
})

test('disposing scheduled workflow polling interrupts the schedule run', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-schedules-workflow-dispose-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new ScheduleService({
    path: join(directory, 'schedules.json'),
    cwd: directory,
    tickMs: 60_000,
    agent: { validateDirectory: async () => directory, prompt: async () => ({ text: 'unused' }) },
    workflows: {
      list: () => [{ id: 'workflow-1', name: 'Approval', status: 'published', inputs: [] }],
      run: async () => ({ id: 'workflow-run-1', status: 'running' }),
      getRun: () => ({ id: 'workflow-run-1', status: 'waiting_approval' }),
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  const task = await service.create({
    name: 'Approval workflow',
    targetType: 'workflow',
    workflowId: 'workflow-1',
    frequency: 'daily',
    time: '09:00',
    timezone: 'UTC',
  })
  await service.runNow(task.id)
  await service.dispose()

  const state = service.getState()
  assert.equal(state.tasks[0].lastStatus, 'interrupted')
  assert.match(state.tasks[0].lastError, /Pisper 关闭/)
  assert.equal(state.runs[0].workflowRunId, 'workflow-run-1')
  assert.equal(state.runs[0].status, 'interrupted')
})

test('scheduled workflow targets must exist and remain published', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-schedules-workflow-validation-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const workflows = [{ id: 'draft-1', name: 'Draft', status: 'draft' }]
  const service = new ScheduleService({
    path: join(directory, 'schedules.json'),
    cwd: directory,
    tickMs: 60_000,
    agent: { validateDirectory: async () => directory, prompt: async () => ({ text: 'unused' }) },
    workflows: {
      list: () => workflows,
      run: async () => null,
      getRun: () => null,
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  t.after(() => service.dispose())

  await assert.rejects(
    service.create({
      name: 'Missing workflow',
      targetType: 'workflow',
      workflowId: 'missing',
      frequency: 'daily',
      time: '09:00',
      timezone: 'UTC',
    }),
    /不存在/,
  )
  await assert.rejects(
    service.create({
      name: 'Draft workflow',
      targetType: 'workflow',
      workflowId: 'draft-1',
      frequency: 'daily',
      time: '09:00',
      timezone: 'UTC',
    }),
    /已发布/,
  )
  workflows.push({
    id: 'published-1',
    name: 'Published',
    status: 'published',
    inputs: [{ name: 'branch', label: 'Branch', required: true, defaultValue: '' }],
  })
  await assert.rejects(
    service.create({
      name: 'Missing input',
      targetType: 'workflow',
      workflowId: 'published-1',
      workflowInputs: {},
      frequency: 'daily',
      time: '09:00',
      timezone: 'UTC',
    }),
    /Branch.*不能为空/,
  )
})

test('scheduled tasks preserve an explicit read-only execution mode', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-schedules-execution-mode-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const prompts = []
  const service = new ScheduleService({
    path: join(directory, 'schedules.json'),
    cwd: directory,
    tickMs: 60_000,
    agent: {
      validateDirectory: async () => directory,
      prompt: async (input) => {
        prompts.push(input)
        return { text: 'ok' }
      },
    },
    notifications: { notify: async () => {} },
  })
  await service.init()
  t.after(() => service.dispose())
  const task = await service.create({
    name: '只读任务',
    prompt: 'test',
    frequency: 'daily',
    time: '09:00',
    timezone: 'UTC',
    executionMode: 'read-only',
  })
  await service.runNow(task.id)
  await waitFor(() => service.executions.size === 0)
  assert.equal(service.getState().tasks[0].executionMode, 'read-only')
  assert.equal(prompts[0].executionMode, 'read-only')
})

test('failure-only tasks suppress success notifications and send failure templates', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-schedules-failure-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const notifications = []
  let shouldFail = false
  const service = new ScheduleService({
    path: join(directory, 'schedules.json'),
    cwd: directory,
    tickMs: 60_000,
    agent: {
      validateDirectory: async () => directory,
      prompt: async () => {
        if (shouldFail) throw new Error('测试超时')
        return { text: 'ok' }
      },
    },
    notifications: {
      notify: async (...args) => {
        notifications.push(args)
      },
    },
  })
  await service.init()
  t.after(() => service.dispose())
  const task = await service.create({
    name: '失败通知',
    prompt: 'test',
    frequency: 'daily',
    time: '09:00',
    timezone: 'UTC',
    notifications: ['browser'],
    notifyOn: 'failure',
  })
  await service.runNow(task.id)
  await waitFor(() => service.getState().tasks[0].lastStatus === 'completed')
  assert.equal(notifications.length, 0)
  shouldFail = true
  await service.runNow(task.id)
  await waitFor(() => notifications.length === 1)
  await waitFor(() => service.executions.size === 0)
  assert.equal(notifications[0][0], 'schedule.failed')
  assert.equal(notifications[0][1].task.error, '测试超时')
})
