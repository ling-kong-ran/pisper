import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
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

test('scheduled tasks preserve an explicit workspace execution mode', async (t) => {
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
    name: '沙箱任务',
    prompt: 'test',
    frequency: 'daily',
    time: '09:00',
    timezone: 'UTC',
    executionMode: 'workspace',
  })
  await service.runNow(task.id)
  await waitFor(() => service.executions.size === 0)
  assert.equal(service.getState().tasks[0].executionMode, 'workspace')
  assert.equal(prompts[0].executionMode, 'workspace')
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
