import assert from 'node:assert/strict'
import test from 'node:test'
import {
  notificationApi,
  parseNotificationSettings,
  supportsNotificationChannelPreferences,
} from '../../src/features/config/notification-api.ts'

const settings = () => ({
  browser: { enabled: true },
  connections: { feishu: { enabled: true, status: 'connected' }, qq: null },
  scopes: [{ platform: 'feishu' }],
  templates: [
    {
      id: 'chat.completed',
      name: 'Completed',
      description: 'Response finished',
      enabled: true,
      variables: ['chat.title'],
      channels: Object.fromEntries(
        ['browser', 'feishu', 'weixin', 'qq', 'telegram'].map((platform) => [
          platform,
          { content: 'Saved body', enabled: true },
        ]),
      ),
    },
  ],
})
test('notification protocol keeps old Runtime behavior without advertising unsupported channel writes', () => {
  const legacy = settings()
  for (const channel of Object.values(legacy.templates[0].channels)) delete channel.enabled
  const parsed = parseNotificationSettings(legacy)
  assert.equal(supportsNotificationChannelPreferences(parsed), false)
  assert.equal(parsed.templates[0].enabled, true)
  assert.equal(parsed.templates[0].channels.browser.content, 'Saved body')
  assert.equal(supportsNotificationChannelPreferences(parseNotificationSettings(settings())), true)
})
test('notification boundary rejects malformed booleans, duplicate IDs, and invalid channel shapes', () => {
  for (const mutate of [
    (data) => {
      data.browser.enabled = 'false'
    },
    (data) => {
      data.templates[0].channels.browser.enabled = 'false'
    },
    (data) => {
      data.templates.push(structuredClone(data.templates[0]))
    },
    (data) => {
      data.templates[0].channels.browser.content = {}
    },
    (data) => {
      data.scopes[0].platform = 'email'
    },
    (data) => {
      data.connections.feishu.enabled = 'true'
    },
  ]) {
    const data = settings()
    mutate(data)
    assert.throws(
      () => parseNotificationSettings(data),
      (error) => error.kind === 'protocol' && error.data.code === 'INVALID_RESPONSE',
    )
  }
})
test('per-channel writes use the additive field and reject a legacy server silently ignoring it', async (t) => {
  const previous = globalThis.fetch
  t.after(() => {
    globalThis.fetch = previous
  })
  let captured
  globalThis.fetch = async (url, options) => {
    captured = { url, body: JSON.parse(options.body) }
    return new Response(JSON.stringify(settings()), { status: 200 })
  }
  await assert.rejects(
    notificationApi.setChannelEnabled('chat.completed', 'browser', false),
    (error) => error.kind === 'protocol',
  )
  assert.deepEqual(captured, {
    url: '/api/settings/notifications/templates/chat.completed/browser',
    body: { channelEnabled: false },
  })
  globalThis.fetch = async () => {
    const response = settings()
    response.templates[0].channels.browser.enabled = false
    return new Response(JSON.stringify(response), { status: 200 })
  }
  assert.equal(
    (await notificationApi.setChannelEnabled('chat.completed', 'browser', false)).templates[0]
      .channels.browser.enabled,
    false,
  )
})
