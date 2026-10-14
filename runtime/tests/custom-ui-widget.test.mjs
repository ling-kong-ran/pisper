import assert from 'node:assert/strict'
import { setImmediate } from 'node:timers/promises'
import test from 'node:test'
import { QueryClient, QueryObserver } from '@tanstack/react-query'
import { listCustomUiComponents } from '../../src/features/custom-ui/custom-ui-api.ts'
import { customUiComponentsQueryOptions } from '../../src/features/custom-ui/useCustomUiComponents.ts'
import { attachComponentBridge } from '../../src/features/custom-ui/component-bridge.ts'
import { startCustomUiView } from '../../src/features/custom-ui/custom-ui-view.ts'
import { customUiComponentLabel } from '../../src/features/custom-ui/custom-ui-labels.ts'

const component = {
  id: 'fixture',
  name: 'Fixture',
  version: '1',
  description: '',
  entry: 'index.html',
  permissions: ['config.read', 'sessions.read', 'notify'],
  entryUrl: '',
  directory: '',
}
const view = {
  id: 'a'.repeat(64),
  entryUrl: `/api/custom-ui/render/${'a'.repeat(64)}/assets/index.html`,
}

function browserFixture(t) {
  const observed = []
  const root = { lang: 'zh-CN', classList: { contains: () => false } }
  const host = Object.assign(new EventTarget(), { location: { origin: 'http://localhost' } })
  const posts = []
  const iframe = { contentWindow: { postMessage: (data) => posts.push(data) } }
  for (const [key, value] of Object.entries({
    window: host,
    document: { documentElement: root },
    getComputedStyle: () => ({ getPropertyValue: () => '' }),
    MutationObserver: class {
      disconnected = false
      constructor(callback) {
        this.callback = callback
        observed.push(this)
      }
      observe() {}
      disconnect() {
        this.disconnected = true
      }
    },
  })) {
    const old = Object.getOwnPropertyDescriptor(globalThis, key)
    Object.defineProperty(globalThis, key, { configurable: true, writable: true, value })
    t.after(() => (old ? Object.defineProperty(globalThis, key, old) : delete globalThis[key]))
  }
  return {
    iframe,
    posts,
    observed,
    send(method, id = 1, params = {}, source = iframe.contentWindow) {
      const event = new Event('message')
      Object.defineProperties(event, {
        source: { value: source },
        data: { value: { pisperBridge: 1, id, method, params } },
      })
      host.dispatchEvent(event)
    },
  }
}

test('component catalog shares one read and only aborts when its final consumer leaves', async (t) => {
  const entered = Promise.withResolvers()
  const aborted = Promise.withResolvers()
  let requests = 0
  let requestSignal
  t.mock.method(globalThis, 'fetch', async (_url, { signal }) => {
    requests += 1
    requestSignal = signal
    if (requests > 1)
      return Response.json({ root: '~/.pisper/agent/custom-ui', components: [component] })
    entered.resolve()
    return new Promise((_resolve, reject) =>
      signal.addEventListener(
        'abort',
        () => {
          aborted.resolve()
          reject(signal.reason)
        },
        { once: true },
      ),
    )
  })
  const client = new QueryClient()
  const a = new QueryObserver(client, customUiComponentsQueryOptions()).subscribe(() => {})
  const b = new QueryObserver(client, customUiComponentsQueryOptions()).subscribe(() => {})
  try {
    await entered.promise
    assert.equal(requests, 1)
    a()
    assert.equal(requestSignal.aborted, false)
    b()
    await aborted.promise
    const next = await client.fetchQuery(customUiComponentsQueryOptions())
    assert.equal(next.components[0].id, 'fixture')
    assert.equal(requests, 2)
  } finally {
    a()
    b()
    await client.cancelQueries()
    client.clear()
  }
})

test('catalog accepts built-in metadata and rejects malformed optional fields', async (t) => {
  const builtIn = { ...component, id: 'pisper-island', builtIn: true }
  let data = { root: 'custom-ui', components: [builtIn] }
  t.mock.method(globalThis, 'fetch', async () => Response.json(data))
  assert.equal((await listCustomUiComponents()).components[0].builtIn, true)
  assert.equal(
    customUiComponentLabel(builtIn, (key) => key),
    'custom-ui:builtIn.islandName',
  )
  assert.equal(
    customUiComponentLabel({ ...builtIn, builtIn: false }, () => 'translated'),
    'Fixture',
  )
  data = { ...data, components: [{ ...builtIn, builtIn: 'yes' }] }
  await assert.rejects(listCustomUiComponents(), /Invalid custom UI component response/)
})

test('canvas preview renders a bridge with no config, session or notification permissions', async (t) => {
  const fixture = browserFixture(t)
  let notifications = 0
  t.mock.method(globalThis, 'fetch', () => assert.fail('preview must not access application data'))
  const stop = attachComponentBridge(fixture.iframe, {
    component,
    preview: true,
    locale: 'en-US',
    notify: () => notifications++,
  })
  try {
    fixture.send('ready', 1)
    fixture.send('getConfig', 2)
    fixture.send('listSessions', 3)
    fixture.send('notify', 4, { message: 'not sent' })
    fixture.send('ready', 5, {}, {})
    await setImmediate()
    const responses = fixture.posts.filter((entry) => entry.id)
    assert.deepEqual(
      responses.map((entry) => [entry.id, entry.ok]),
      [
        [1, true],
        [2, false],
        [3, false],
        [4, false],
      ],
    )
    assert.deepEqual(responses[0].result.component.permissions, [])
    assert.equal(responses[0].result.locale, 'en-US')
    assert.equal(responses[0].result.theme.locale, 'en-US')
    assert.equal(notifications, 0)
  } finally {
    stop()
  }
  assert.equal(fixture.observed[0].disconnected, true)
})

test('unmount aborts bridge reads and suppresses late replies, themes and notifications', async (t) => {
  const fixture = browserFixture(t)
  const entered = Promise.withResolvers()
  const response = Promise.withResolvers()
  let requestSignal
  let notifications = 0
  t.mock.method(globalThis, 'fetch', async (_url, { signal }) => {
    requestSignal = signal
    entered.resolve()
    return response.promise
  })
  const stop = attachComponentBridge(fixture.iframe, { component, notify: () => notifications++ })
  fixture.send('getConfig')
  await entered.promise
  stop()
  assert.equal(requestSignal.aborted, true)
  const count = fixture.posts.length
  response.resolve(Response.json({ example: 'late data' }))
  fixture.send('notify', 2, { message: 'not sent after removal' })
  fixture.observed[0].callback()
  await setImmediate()
  assert.equal(fixture.posts.length, count)
  assert.equal(notifications, 0)
})

test('mounted widget view renews then releases its lease exactly once on removal', async (t) => {
  browserFixture(t)
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const methods = []
  t.mock.method(globalThis, 'fetch', async (_url, options) => {
    methods.push(options.method)
    return Response.json(options.method === 'POST' ? view : {})
  })
  const loaded = Promise.withResolvers()
  const stop = startCustomUiView(component.id, loaded.resolve, () =>
    assert.fail('view should load'),
  )
  assert.deepEqual(await loaded.promise, view)
  t.mock.timers.tick(60_000)
  await setImmediate()
  assert.deepEqual(methods, ['POST', 'PUT'])
  stop()
  stop()
  await setImmediate()
  t.mock.timers.tick(120_000)
  await setImmediate()
  assert.deepEqual(methods, ['POST', 'PUT', 'DELETE'])
})

test('a view created after removal is revoked without displaying or renewing it', async (t) => {
  browserFixture(t)
  const entered = Promise.withResolvers()
  const response = Promise.withResolvers()
  const released = Promise.withResolvers()
  let signal
  t.mock.method(globalThis, 'fetch', async (_url, options) => {
    if (options.method === 'POST') {
      signal = options.signal
      entered.resolve()
      return response.promise
    }
    assert.equal(options.method, 'DELETE')
    released.resolve()
    return Response.json({})
  })
  const stop = startCustomUiView(
    component.id,
    () => assert.fail('removed view must not render'),
    () => assert.fail('removed view must not report failure'),
  )
  await entered.promise
  stop()
  assert.equal(signal.aborted, true)
  response.resolve(Response.json(view))
  await released.promise
})

test('renewal failure stops a widget and revokes its lease before retry', async (t) => {
  browserFixture(t)
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const methods = []
  t.mock.method(globalThis, 'fetch', async (_url, options) => {
    methods.push(options.method)
    if (options.method === 'PUT') return Response.json({ error: 'expired' }, { status: 404 })
    return Response.json(options.method === 'POST' ? view : {})
  })
  const loaded = Promise.withResolvers()
  let failures = 0
  const stop = startCustomUiView(component.id, loaded.resolve, () => failures++)
  await loaded.promise
  t.mock.timers.tick(60_000)
  await setImmediate()
  assert.equal(failures, 1)
  assert.deepEqual(methods, ['POST', 'PUT', 'DELETE'])
  stop()
  t.mock.timers.tick(120_000)
  await setImmediate()
  assert.deepEqual(methods, ['POST', 'PUT', 'DELETE'])
})
