import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { configSettingsRoutes } from '../http/routes/config-settings.mjs'
import { NotificationSettingsService } from '../services/notification-settings-service.mjs'

test('browser notification setting persists without overwriting other app configuration', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-notifications-'))
  const path = join(directory, 'pisper.json')
  t.after(() => rm(directory, { recursive: true, force: true }))
  await writeFile(path, JSON.stringify({ toolMode: 'workspace', disabledProviders: ['example'] }))
  const channels = {
    getState: () => ({ templates: [{ id: 'schedule.completed' }], connections: {}, scopes: [] }),
  }
  const service = new NotificationSettingsService({ path, channels })
  assert.equal((await service.getState()).browser.enabled, false)
  const updated = await service.updateBrowser({ enabled: true })
  assert.equal(updated.browser.enabled, true)
  const stored = JSON.parse(await readFile(path, 'utf8'))
  assert.equal(stored.toolMode, 'workspace')
  assert.deepEqual(stored.disabledProviders, ['example'])
  assert.equal(stored.notifications.browser.enabled, true)
})

test('notification templates remain delegated to the channel notification service', async () => {
  const calls = []
  const channels = {
    getState: () => ({
      templates: [{ id: 'workflow.completed', enabled: true }],
      connections: {},
      scopes: [],
    }),
    updateTemplate: async (...args) => {
      calls.push(['update', ...args])
    },
    testNotification: async (...args) => {
      calls.push(['test', ...args])
      return { sent: 1 }
    },
    notify: async (...args) => {
      calls.push(['notify', ...args])
      return []
    },
  }
  const service = new NotificationSettingsService({ path: 'unused.json', channels })
  await service.updateTemplate('workflow.completed', 'feishu', { content: 'done' })
  assert.deepEqual(await service.testTemplate('workflow.completed', 'feishu'), { sent: 1 })
  await service.notify(
    'workflow.completed',
    { workflow: { name: 'release' } },
    { platforms: ['feishu'] },
  )
  assert.deepEqual(calls, [
    ['update', 'workflow.completed', 'feishu', { content: 'done' }],
    ['test', 'workflow.completed', 'feishu'],
    ['notify', 'workflow.completed', { workflow: { name: 'release' } }, { platforms: ['feishu'] }],
  ])
})

test('browser notification events use the configured template queue', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-browser-events-'))
  const path = join(directory, 'pisper.json')
  const browserEventsPath = join(directory, 'browser-events.json')
  t.after(() => rm(directory, { recursive: true, force: true }))
  await writeFile(path, JSON.stringify({ notifications: { browser: { enabled: true } } }))
  const channels = {
    getState: () => ({
      templates: [{ id: 'schedule.completed', enabled: true }],
      connections: {},
      scopes: [],
    }),
    notify: async () => [],
    renderNotification: () => ({ title: '定时任务完成', content: '日报已完成' }),
  }
  const service = new NotificationSettingsService({ path, browserEventsPath, channels })
  const bootstrap = await service.getBrowserEvents()
  assert.equal(bootstrap.events.length, 0)
  assert.ok(bootstrap.latestId)
  await service.notify('schedule.completed', { task: { name: '日报' } }, { platforms: ['browser'] })
  const first = await service.getBrowserEvents(bootstrap.latestId)
  assert.equal(first.events.length, 1)
  assert.equal(first.events[0].body, '日报已完成')
  assert.equal((await service.getBrowserEvents(first.latestId)).events.length, 0)
})

test('one-off notification content overrides channel templates and browser rendering', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-notification-override-'))
  const path = join(directory, 'pisper.json')
  const browserEventsPath = join(directory, 'browser-events.json')
  t.after(() => rm(directory, { recursive: true, force: true }))
  await writeFile(path, JSON.stringify({ notifications: { browser: { enabled: true } } }))
  const calls = []
  const channels = {
    getState: () => ({
      templates: [{ id: 'workflow.completed', enabled: true }],
      connections: {},
      scopes: [],
    }),
    notify: async (...args) => {
      calls.push(args)
      return []
    },
    renderNotification: () => ({ title: '工作流完成', content: '全局模板正文' }),
  }
  const service = new NotificationSettingsService({ path, browserEventsPath, channels })

  await service.notify(
    'workflow.completed',
    { workflow: { name: 'release' } },
    {
      platforms: ['feishu', 'browser'],
      title: '发布检查',
      content: '生产环境构建通过',
    },
  )

  assert.deepEqual(calls, [
    [
      'workflow.completed',
      { workflow: { name: 'release' } },
      { platforms: ['feishu'], content: '生产环境构建通过' },
    ],
  ])
  const browser = await service.getBrowserEvents('missing')
  assert.equal(browser.events[0].title, '发布检查')
  assert.equal(browser.events[0].body, '生产环境构建通过')
})

test('browser template tests return a system-notification payload without queueing a duplicate event', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-browser-template-test-'))
  const path = join(directory, 'pisper.json')
  const browserEventsPath = join(directory, 'browser-events.json')
  t.after(() => rm(directory, { recursive: true, force: true }))
  await writeFile(path, JSON.stringify({ notifications: { browser: { enabled: true } } }))
  const channels = {
    getState: () => ({
      templates: [{ id: 'schedule.completed', enabled: true }],
      connections: {},
      scopes: [],
    }),
    renderNotification: () => ({ title: '定时任务完成', content: '日报已完成' }),
  }
  const service = new NotificationSettingsService({ path, browserEventsPath, channels })

  assert.deepEqual(await service.testTemplate('schedule.completed', 'browser'), {
    sent: 1,
    title: '定时任务完成',
    body: '日报已完成',
    preview: '日报已完成',
  })
  assert.equal((await service.getBrowserEvents('missing')).events.length, 0)
})

test('TUI chat completion reports system-notification state and sends active channels', async () => {
  const route = configSettingsRoutes.find(
    (item) => item.path === '/api/settings/notifications/chat-completed',
  )
  const calls = []
  let response
  await route.handler({
    runtime: {
      getNotificationSettings: async () => ({ browser: { enabled: true } }),
      notifyChannels: async (...args) => calls.push(args),
    },
    body: async () => ({
      title: 'TUI session',
      summary: 'Finished from the terminal',
      model: 'provider/model',
    }),
    json: (status, value) => {
      response = { status, value }
    },
  })

  assert.deepEqual(calls, [
    [
      'chat.completed',
      {
        chat: {
          title: 'TUI session',
          summary: 'Finished from the terminal',
          model: 'provider/model',
        },
      },
      { platforms: ['feishu', 'weixin'] },
    ],
  ])
  assert.deepEqual(response, {
    status: 202,
    value: { accepted: true, systemNotificationEnabled: true, channelError: '' },
  })
})

test('TUI approval waiting notification includes context and does not fail on channel errors', async () => {
  const route = configSettingsRoutes.find(
    (item) => item.path === '/api/settings/notifications/chat-waiting',
  )
  const calls = []
  let response
  await route.handler({
    runtime: {
      getNotificationSettings: async () => ({ browser: { enabled: false } }),
      notifyChannels: async (...args) => {
        calls.push(args)
        throw new Error('weixin unavailable')
      },
    },
    body: async () => ({
      title: 'Release audit',
      tool: 'bash',
      reason: 'Runs a command outside the workspace.',
      model: 'provider/model',
    }),
    json: (status, value) => {
      response = { status, value }
    },
  })

  assert.deepEqual(calls, [
    [
      'chat.waiting',
      {
        chat: {
          title: 'Release audit',
          tool: 'bash',
          reason: 'Runs a command outside the workspace.',
          model: 'provider/model',
        },
      },
      { platforms: ['feishu', 'weixin'] },
    ],
  ])
  assert.deepEqual(response, {
    status: 202,
    value: {
      accepted: true,
      systemNotificationEnabled: false,
      channelError: 'weixin unavailable',
    },
  })
})

test('channel notification failures are reported after other targets are attempted', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-notification-failure-'))
  const path = join(directory, 'pisper.json')
  const browserEventsPath = join(directory, 'browser-events.json')
  t.after(() => rm(directory, { recursive: true, force: true }))
  await writeFile(path, JSON.stringify({ notifications: { browser: { enabled: true } } }))
  const channels = {
    getState: () => ({
      templates: [{ id: 'schedule.completed', enabled: true }],
      connections: {},
      scopes: [],
    }),
    notify: async () => [{ platform: 'weixin', status: 'rejected', error: 'prepare failed' }],
    renderNotification: () => ({ title: 'Schedule completed', content: 'Daily task completed' }),
  }
  const service = new NotificationSettingsService({ path, browserEventsPath, channels })
  await assert.rejects(
    service.notify(
      'schedule.completed',
      { task: { name: 'daily' } },
      { platforms: ['weixin', 'browser'] },
    ),
    /weixin: prepare failed/,
  )
  assert.equal((await service.getBrowserEvents('missing')).events.length, 1)
})
