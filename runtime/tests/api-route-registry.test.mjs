import assert from 'node:assert/strict'
import test from 'node:test'
import { createRouteRegistry } from '../http/route-registry.mjs'
import { configSettingsRoutes } from '../http/routes/config-settings.mjs'
import { desktopRoutes } from '../http/routes/desktop.mjs'
import { integrationRoutes } from '../http/routes/integrations.mjs'
import { memoryAssetRoutes } from '../http/routes/memory-assets.mjs'
import { sessionRuntimeRoutes } from '../http/routes/sessions-runtime.mjs'
import { workflowScheduleRoutes } from '../http/routes/workflows-schedules.mjs'

const handler = () => {}

test('route registry matches methods and prefers static paths independent of registration order', () => {
  const definitions = [
    { method: 'GET', path: '/api/items/:itemId', handler },
    { method: 'GET', path: '/api/items/new', handler },
  ]

  for (const routes of [definitions, [...definitions].reverse()]) {
    const registry = createRouteRegistry(routes)
    assert.equal(registry.match('GET', '/api/items/new').path, '/api/items/new')
    assert.deepEqual(registry.match('GET', '/api/items/item%201').params, { itemId: 'item 1' })
    assert.equal(registry.match('POST', '/api/items/new'), null)
  }
})

test('route registry decodes parameters and enforces explicit parameter constraints', () => {
  const registry = createRouteRegistry([
    {
      method: 'POST',
      path: '/api/items/:itemId/:action',
      where: { action: ['accept', 'reject'] },
      handler,
    },
  ])

  assert.deepEqual(registry.match('POST', '/api/items/folder%2Fitem%201/accept').params, {
    itemId: 'folder/item 1',
    action: 'accept',
  })
  assert.equal(registry.match('POST', '/api/items/item-1/archive'), null)
  assert.equal(registry.match('POST', '/api/items//accept'), null)
  assert.throws(() => registry.match('POST', '/api/items/%E0%A4%A/accept'), URIError)
})

test('route registry rejects conflicting and order-ambiguous registrations', () => {
  assert.throws(
    () =>
      createRouteRegistry([
        { method: 'GET', path: '/api/items/:itemId', handler },
        { method: 'GET', path: '/api/items/:name', handler },
      ]),
    /Conflicting route registration/,
  )
  assert.throws(
    () =>
      createRouteRegistry([
        { method: 'GET', path: '/api/:group/fixed', handler },
        { method: 'GET', path: '/api/fixed/:itemId', handler },
      ]),
    /Ambiguous route registration/,
  )
})

test('resource route groups expose representative static and parameter routes', () => {
  const registry = createRouteRegistry([
    ...sessionRuntimeRoutes,
    ...configSettingsRoutes,
    ...workflowScheduleRoutes,
    ...memoryAssetRoutes,
    ...integrationRoutes,
    ...desktopRoutes,
  ])
  const cases = [
    [
      'PUT',
      '/api/sessions/session%201/model',
      '/api/sessions/:sessionId/model',
      { sessionId: 'session 1' },
    ],
    [
      'POST',
      '/api/providers/provider%201/import',
      '/api/providers/:providerId/import',
      { providerId: 'provider 1' },
    ],
    [
      'POST',
      '/api/schedules/daily%20build/run',
      '/api/schedules/:scheduleId/run',
      { scheduleId: 'daily build' },
    ],
    [
      'GET',
      '/api/assets/folder%2Fasset/content',
      '/api/assets/:assetId/content',
      { assetId: 'folder/asset' },
    ],
    [
      'POST',
      '/api/channels/feishu/onboarding',
      '/api/channels/:channel/onboarding',
      { channel: 'feishu' },
    ],
    [
      'GET',
      '/api/sponsors/settings%20updates',
      '/api/sponsors/:placement',
      { placement: 'settings updates' },
    ],
  ]

  for (const [method, pathname, path, params] of cases) {
    const match = registry.match(method, pathname)
    assert.equal(match.path, path)
    assert.deepEqual(match.params, params)
  }
  assert.equal(registry.match('POST', '/api/channels/slack/onboarding'), null)
})
