import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { setImmediate } from 'node:timers/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import { CustomUiService, normalizeComponentManifest } from '../services/custom-ui-service.mjs'

// 把模块边界替换为可观测的领域适配器，不启动 Runtime、不接触个人配置或网络。
const hostCode = transformSync(
  await readFile('src/features/custom-ui/component-bridge.ts', 'utf8'),
  { loader: 'ts', format: 'cjs', supported: { 'dynamic-import': false } },
).code

const gameAssetPermissions = {
  list: 'game-assets.read',
  save: 'game-assets.write',
  run: 'game-assets.run',
  stop: 'game-assets.run',
  uploadImage: 'game-assets.write',
  image: 'game-assets.read',
  process: 'game-assets.write',
  engine: 'game-assets.write',
  export: 'game-assets.read',
  editFrames: 'game-assets.write',
  remove: 'game-assets.write',
}

function hostFixture(t, { permissions = [], preview = false, handler } = {}) {
  const posts = []
  const handlers = new Map()
  const calls = []
  const apiCalls = []
  const notifications = []
  let domainImports = 0
  let disconnected = false
  const iframe = { contentWindow: { postMessage: (message) => posts.push(message) } }
  const module = { exports: {} }
  const root = { lang: 'zh-CN', classList: { contains: () => false } }
  runInNewContext(hostCode, {
    module,
    exports: module.exports,
    AbortController,
    document: { documentElement: root },
    getComputedStyle: () => ({ getPropertyValue: () => '' }),
    MutationObserver: class {
      observe() {}
      disconnect() {
        disconnected = true
      }
    },
    window: {
      addEventListener: (name, listener) => handlers.set(name, listener),
      removeEventListener: (name, listener) => {
        if (handlers.get(name) === listener) handlers.delete(name)
      },
    },
    require(name) {
      if (name === '@/app/i18n') return { translateText: (key) => key }
      if (name === '@/lib/api') {
        return {
          apiJson(path, options) {
            apiCalls.push({ path, options })
            return Promise.resolve({ safeConfig: true })
          },
        }
      }
      assert.equal(name, '@/features/game-assets/component-api')
      domainImports += 1
      return {
        handleGameAssetComponentRequest(method, params, signal) {
          calls.push({ method, params, signal })
          return handler ? handler(method, params, signal) : Promise.resolve({ accepted: method })
        },
      }
    },
  })
  const stop = module.exports.attachComponentBridge(iframe, {
    component: { id: 'fixture', name: 'Fixture', version: '1', permissions },
    preview,
    notify: (message) => notifications.push(message),
  })
  t.after(stop)
  let nextId = 1
  return {
    posts,
    calls,
    apiCalls,
    notifications,
    stop,
    get domainImports() {
      return domainImports
    },
    get disconnected() {
      return disconnected
    },
    send(method, params = {}, options = {}) {
      const id = options.id ?? nextId++
      handlers.get('message')?.({
        source: options.foreign ? {} : iframe.contentWindow,
        data: { pisperBridge: 1, id, method, params },
      })
      return id
    },
    response(id) {
      return posts.find((message) => message.id === id)
    },
  }
}

test('asset permissions survive manifest validation while unknown capabilities are filtered', () => {
  const permissions = ['game-assets.read', 'game-assets.write', 'game-assets.run']
  const manifest = normalizeComponentManifest('fixture', {
    name: 'Fixture',
    permissions: [...permissions, 'gameAssets.read', 'fs.write', 'game-assets.read'],
  })
  assert.deepEqual(manifest.permissions, permissions)
})

test('component bridge exposes asset methods with paired requests and rejects foreign replies', async () => {
  const service = new CustomUiService({ dataDir: '/nonexistent', builtinComponents: [] })
  const sent = []
  let onMessage
  const parent = { postMessage: (message) => sent.push(message) }
  const window = { addEventListener: (_name, listener) => (onMessage = listener) }
  runInNewContext(service.bridgeScript(), { parent, window, document: {}, console })
  const pending = []
  for (const name of Object.keys(gameAssetPermissions)) {
    const params = { operation: name }
    pending.push(window.pisper.gameAssets[name](params))
    const request = sent.at(-1)
    assert.equal(request.method, `gameAssets.${name}`)
    assert.equal(request.params, params)
    onMessage({
      source: {},
      data: { pisperBridge: 1, id: request.id, ok: false, error: 'foreign frame' },
    })
    onMessage({
      source: parent,
      data: { pisperBridge: 1, id: request.id, ok: true, result: name },
    })
  }
  assert.deepEqual(await Promise.all(pending), Object.keys(gameAssetPermissions))
  assert.equal(new Set(sent.map((message) => message.id)).size, sent.length)
})

test('ungranted asset requests never import the asset domain', async (t) => {
  const host = hostFixture(t)
  const ids = Object.keys(gameAssetPermissions).map((name) => host.send(`gameAssets.${name}`))
  const unknownId = host.send('gameAssets.deleteEverything')
  await setImmediate()
  for (const id of [...ids, unknownId]) assert.equal(host.response(id)?.ok, false)
  assert.equal(host.domainImports, 0)
  assert.equal(host.calls.length, 0)
})

test('canvas preview disables every asset permission even when declared', async (t) => {
  const host = hostFixture(t, {
    permissions: ['game-assets.read', 'game-assets.write', 'game-assets.run'],
    preview: true,
  })
  const readyId = host.send('ready')
  const ids = Object.keys(gameAssetPermissions).map((name) => host.send(`gameAssets.${name}`))
  await setImmediate()
  assert.equal(host.response(readyId)?.result.component.permissions.length, 0)
  for (const id of ids) assert.equal(host.response(id)?.ok, false)
  assert.equal(host.domainImports, 0)
})

test('each asset method requires its own declared capability', async (t) => {
  for (const permission of ['game-assets.read', 'game-assets.write', 'game-assets.run']) {
    await t.test(permission, async (t) => {
      const host = hostFixture(t, { permissions: [permission] })
      for (const [name, required] of Object.entries(gameAssetPermissions)) {
        const params = { fixture: name }
        const id = host.send(`gameAssets.${name}`, params)
        await setImmediate()
        assert.equal(host.response(id)?.ok, permission === required, name)
        if (permission === required) {
          assert.equal(host.calls.at(-1).params, params)
          assert.equal(host.calls.at(-1).signal.aborted, false)
        }
      }
      assert.deepEqual(
        host.calls.map((call) => call.method),
        Object.entries(gameAssetPermissions)
          .filter(([, required]) => required === permission)
          .map(([name]) => `gameAssets.${name}`),
      )
    })
  }
})

test('foreign iframe and invalid request identifiers cannot invoke asset operations', async (t) => {
  const host = hostFixture(t, { permissions: ['game-assets.run'] })
  host.send('gameAssets.run', {}, { foreign: true })
  for (const id of [NaN, Infinity, 1.5, '1', Number.MAX_SAFE_INTEGER + 1]) {
    host.send('gameAssets.run', {}, { id })
  }
  await setImmediate()
  assert.equal(host.domainImports, 0)
  assert.equal(host.posts.filter((message) => 'id' in message).length, 0)
})

test('only four asset requests run at once and busy requests do not block ready or notify', async (t) => {
  const active = []
  const host = hostFixture(t, {
    permissions: ['game-assets.write', 'notify'],
    handler: () => {
      const pending = Promise.withResolvers()
      active.push(pending)
      return pending.promise
    },
  })
  const ids = Array.from({ length: 5 }, () => host.send('gameAssets.process', { image: 'fixture' }))
  const readyId = host.send('ready')
  const notifyId = host.send('notify', { message: 'processing' })
  await setImmediate()
  assert.equal(host.calls.length, 4)
  assert.equal(host.domainImports, 4)
  assert.equal(host.response(ids[4]).ok, false)
  assert.equal(host.response(ids[4]).error, 'custom-ui:bridge.gameAssetBusy')
  assert.equal(host.response(readyId).ok, true)
  assert.equal(host.response(notifyId).ok, true)
  assert.deepEqual(host.notifications, ['processing'])
  active[0].resolve({ finished: true })
  await setImmediate()
  assert.equal(host.response(ids[0]).ok, true)
  const nextId = host.send('gameAssets.process')
  await setImmediate()
  assert.equal(host.calls.length, 5)
  for (const pending of active) pending.resolve({ finished: true })
  await setImmediate()
  assert.equal(host.response(nextId).ok, true)
})

test('unmount aborts exclusive asset requests and suppresses late results without stopping jobs', async (t) => {
  const pending = Promise.withResolvers()
  const host = hostFixture(t, { permissions: ['game-assets.read'], handler: () => pending.promise })
  host.send('gameAssets.image')
  await setImmediate()
  assert.equal(host.calls.length, 1)
  const postsBeforeUnmount = host.posts.length
  host.stop()
  assert.equal(host.calls[0].signal.aborted, true)
  assert.equal(host.disconnected, true)
  pending.resolve({ data: new ArrayBuffer(4) })
  host.send('gameAssets.list')
  await setImmediate()
  assert.equal(host.posts.length, postsBeforeUnmount)
  assert.equal(host.calls.length, 1)
})

test('unmount before lazy import resolves never starts a asset mutation', async (t) => {
  const host = hostFixture(t, { permissions: ['game-assets.write'] })
  host.send('gameAssets.save')
  host.stop()
  await setImmediate()
  assert.equal(host.calls.length, 0)
  assert.equal(host.posts.filter((message) => 'id' in message).length, 0)
})

test('config, sessions and notifications retain their existing bridge contract without loading assets', async (t) => {
  const host = hostFixture(t, { permissions: ['config.read', 'sessions.read', 'notify'] })
  const configId = host.send('getConfig')
  const sessionsId = host.send('listSessions', { limit: 500 })
  const notifyId = host.send('notify', { message: '  hello  ' })
  await setImmediate()
  assert.equal(host.response(configId).result.safeConfig, true)
  assert.equal(host.response(sessionsId).ok, true)
  assert.equal(host.response(notifyId).ok, true)
  assert.deepEqual(
    host.apiCalls.map((call) => call.path),
    ['/api/config', '/api/sessions?limit=200'],
  )
  assert.deepEqual(host.notifications, ['hello'])
  assert.equal(host.domainImports, 0)
})
