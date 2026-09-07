import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createPisperRuntime } from '../app-runtime.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { createAppTools } from '../tools/registry.mjs'
import { mobileBaseInitialization, resolveRuntimeCapabilities } from '../runtime-capabilities.mjs'
import { createStartupLogObserver, createStartupObserver } from '../startup-observer.mjs'

function deferred() {
  let resolve
  let reject
  const promise = new Promise((accept, fail) => {
    resolve = accept
    reject = fail
  })
  promise.catch(() => {})
  return { promise, resolve, reject }
}

async function fixture(
  t,
  { profile = 'mobile-embedded', initializationMode = 'mobile-base' } = {},
) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-mobile-startup-'))
  await writeFile(join(root, 'package.json'), JSON.stringify({ version: '0.0.0-test' }))
  const modules = deferred()
  const memory = deferred()
  const stages = []
  const capabilities = await resolveRuntimeCapabilities({
    environment: { PISPER_RUNTIME_PROFILE: profile },
    moduleSupport: { childProcess: false, workerThreads: false, sqlite: true, wasm: false },
  })
  class ControlledRuntime extends AgentRuntimeService {
    constructor(options) {
      super(options)
      const initializeMemory = this.memory.init.bind(this.memory)
      this.memory.init = async () => {
        await memory.promise
        await initializeMemory()
      }
    }
  }
  const app = await createPisperRuntime({
    root,
    runtimeCwd: root,
    dataDir: join(root, 'agent'),
    production: true,
    port: 0,
    deferRuntimeInitialization: true,
    desktopAuthToken: 'startup-test-token',
    initializationMode,
    runtimeCapabilities: capabilities,
    startupObserver: (stage, timing) => stages.push({ stage, timing }),
    runtimeModuleLoader: async () => {
      await modules.promise
      return { AgentRuntimeService: ControlledRuntime }
    },
  })
  t.after(async () => {
    modules.resolve()
    memory.resolve()
    await app.close()
    await rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 })
  })
  const bootstrap = await fetch(`${app.url}/_pisper/desktop/bootstrap?token=startup-test-token`, {
    redirect: 'manual',
  })
  assert.equal(bootstrap.status, 302)
  const cookie = bootstrap.headers.get('set-cookie').split(';')[0]
  const request = (path, options = {}) =>
    fetch(`${app.url}${path}`, {
      signal: AbortSignal.timeout(10_000),
      ...options,
      headers: { cookie, ...options.headers },
    })
  return { app, modules, memory, request, stages }
}

test('startup timing is opt-in, monotonic, compatible with stage observers, and best-effort', () => {
  const records = []
  let time = 10
  const stage = createStartupObserver((name, timing) => records.push({ name, ...timing }), {
    now: () => time,
  })
  time = 15
  stage('runtime-modules-loading')
  time = 23
  stage('runtime-modules-loaded')
  assert.deepEqual(records, [
    { name: 'runtime-modules-loading', elapsedMs: 5, durationMs: 5 },
    { name: 'runtime-modules-loaded', elapsedMs: 13, durationMs: 8 },
  ])
  createStartupObserver(null, { now: () => assert.fail('disabled observer read clock') })('idle')
  assert.doesNotThrow(() =>
    createStartupObserver(() => {
      throw new Error('observer failed')
    })('stage'),
  )
  assert.equal(createStartupLogObserver({}), null)
  const logs = []
  createStartupLogObserver({ PISPER_STARTUP_TRACE: '1' }, (line) => logs.push(JSON.parse(line)))(
    'runtime-complete',
    { elapsedMs: 23, durationMs: 8 },
  )
  assert.deepEqual(logs, [
    { event: 'pisper-startup', stage: 'runtime-complete', elapsedMs: 23, durationMs: 8 },
  ])
})

test('mobile base startup requires an explicit valid mobile profile', () => {
  assert.equal(mobileBaseInitialization('full', 'desktop'), false)
  assert.equal(mobileBaseInitialization('mobile-base', 'mobile-embedded'), true)
  assert.equal(mobileBaseInitialization('mobile-base', 'mobile-store'), true)
  assert.throws(() => mobileBaseInitialization('mobile-base', 'desktop'), /requires a mobile/)
  assert.throws(
    () => mobileBaseInitialization('invented', 'mobile-store'),
    /Unknown initialization/,
  )
})

for (const profile of ['mobile-embedded', 'mobile-store']) {
  test(`${profile}: authenticated readiness and real chat APIs do not wait for optional memory`, async (t) => {
    const { app, modules, memory, request, stages } = await fixture(t, { profile })
    assert.equal((await fetch(`${app.url}/api/ready`)).status, 401)
    const pending = await request('/api/ready')
    assert.equal(pending.status, 503)
    assert.deepEqual(await pending.json(), {
      version: 1,
      ready: false,
      client: 'mobile-app',
      profile,
      initialization: { base: 'pending', background: 'pending', services: {} },
    })
    modules.resolve()
    await app.baseReady
    let fullReady = false
    app.initialized.then(() => {
      fullReady = true
    })
    const ready = await request('/api/ready')
    assert.equal(ready.status, 200)
    assert.equal(ready.headers.get('cache-control'), 'no-store')
    assert.deepEqual(await ready.json(), {
      version: 1,
      ready: true,
      client: 'mobile-app',
      profile,
      initialization: {
        base: 'ready',
        background: 'pending',
        services: { memory: 'initializing' },
      },
    })
    for (const path of [
      '/api/config',
      '/api/sessions',
      '/api/client-info',
      '/api/runtime/capabilities',
    ]) {
      const response = await request(path)
      assert.equal(response.status, 200, path)
      assert.ok(await response.json(), path)
    }
    const created = await request('/api/sessions', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: 'First chat' }),
    })
    assert.equal(created.status, 201)
    const session = await created.json()
    const value = await app.runtime.getOrCreateSession(session.id)
    assert.equal(value.session.sessionId, session.id)
    assert.equal(fullReady, false)
    let executed = false
    const [tool] = createAppTools({
      enabledTools: ['memory_search'],
      waitForInitialization: (service) => app.runtime.waitForInitialization(service),
      memoryRuntime: {
        searchRelevant: () => {
          executed = true
          return []
        },
      },
    })
    const toolResult = tool.execute('test-search', { query: 'startup' })
    let memoryResponded = false
    const memoryResponse = request('/api/memory').then((response) => {
      memoryResponded = true
      return response
    })
    await new Promise((resolve) => setImmediate(resolve))
    assert.equal(executed, false)
    assert.equal(memoryResponded, false)
    memory.resolve()
    await app.initialized
    assert.equal((await toolResult).details.count, 0)
    assert.equal(executed, true)
    assert.equal((await memoryResponse).status, 200)
    const complete = await (await request('/api/ready')).json()
    assert.equal(complete.initialization.background, 'ready')
    assert.equal(complete.initialization.services.memory, 'ready')
    for (const name of [
      'modules-loading',
      'modules-loaded',
      'created',
      'filesystem',
      'session-state',
      'providers',
      'skills',
      'mcp',
      'default-tools',
      'model-runtime',
      'memory',
      'complete',
    ]) {
      const record = stages.find(({ stage }) => stage === `runtime-${name}`)
      assert.ok(record, name)
      assert.ok(record.timing.elapsedMs >= 0, name)
      assert.ok(record.timing.durationMs >= 0, name)
    }
  })
}

test('failed background memory is observable without invalidating base readiness or session APIs', async (t) => {
  const { app, modules, memory, request } = await fixture(t)
  modules.resolve()
  await app.baseReady
  memory.reject(new Error('injected memory failure'))
  await assert.rejects(app.initialized, /injected memory failure/)
  await app.baseReady
  const response = await request('/api/ready')
  assert.equal(response.status, 200)
  assert.deepEqual((await response.json()).initialization, {
    base: 'ready',
    background: 'failed',
    services: { memory: 'failed' },
  })
  assert.equal((await request('/api/config')).status, 200)
  assert.equal((await request('/api/sessions')).status, 200)
  assert.equal((await request('/api/memory')).status, 503)
  const [tool] = createAppTools({
    enabledTools: ['memory_remember'],
    waitForInitialization: (service) => app.runtime.waitForInitialization(service),
    memoryRuntime: { ensureWorkspaceSpace: () => assert.fail('uninitialized memory was exposed') },
  })
  await assert.rejects(
    tool.execute('test-remember', { title: 'startup', content: 'test' }),
    /injected memory failure/,
  )
})

test('full initialization keeps the desktop readiness boundary unchanged', async (t) => {
  const { app, modules, memory, request } = await fixture(t, {
    profile: 'desktop',
    initializationMode: 'full',
  })
  modules.resolve()
  while (!app.runtime) await new Promise((resolve) => setImmediate(resolve))
  const response = await request('/api/ready', { headers: { 'x-pisper-client': 'mobile-app' } })
  assert.equal(response.status, 503)
  assert.equal((await response.json()).client, 'web')
  memory.resolve()
  await app.initialized
  await app.baseReady
  assert.equal((await request('/api/ready')).status, 200)
})

test('base initialization failure rejects both readiness promises and reports non-ready', async (t) => {
  const { app, modules, request } = await fixture(t)
  modules.reject(new Error('injected module failure'))
  await assert.rejects(app.baseReady, /injected module failure/)
  await assert.rejects(app.initialized, /injected module failure/)
  const response = await request('/api/ready')
  assert.equal(response.status, 503)
  assert.deepEqual((await response.json()).initialization, {
    base: 'failed',
    background: 'failed',
    services: {},
  })
  assert.equal((await request('/api/config')).status, 503)
})
