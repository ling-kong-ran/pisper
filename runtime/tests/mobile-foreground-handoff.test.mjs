import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { createContext, runInContext } from 'node:vm'
import { transformSync } from 'esbuild'

const shell = await readFile(new URL('../../src-tauri/src/mobile/mod.rs', import.meta.url), 'utf8')
const initializationScript = shell.match(
  /const MOBILE_CLIENT_INITIALIZATION_SCRIPT: &str = r#"([\s\S]*?)"#;/,
)?.[1]
assert.ok(initializationScript, '移动壳初始化脚本必须来自实际 Rust 字符串')
const recoverySource = await readFile(
  new URL('../../src/lib/mobile-runtime-recovery.ts', import.meta.url),
  'utf8',
)
const recoveryScript = transformSync(recoverySource, {
  loader: 'ts',
  target: 'es2022',
  format: 'cjs',
}).code
const handoffKey = '__PISPER_MOBILE_FOREGROUND_RECOVERY_INSTALLED__'
const flush = () => new Promise((resolve) => setImmediate(resolve))

test('manual route reload waits for shared recovery and navigates even when recovery rejects', async () => {
  const source = await readFile(
    new URL('../../src/app/RouteErrorBoundary.tsx', import.meta.url),
    'utf8',
  )
  let rejectRecovery
  let waits = 0
  const recovery = new Promise((_, reject) => {
    rejectRecovery = reject
  })
  const navigations = []
  const module = { exports: {} }
  const context = createContext({
    module,
    exports: module.exports,
    URL,
    window: {
      __PISPER_MOBILE_APP__: true,
      location: {
        href: 'http://127.0.0.1:41873/?existing=1#/chat?session=retained',
        replace: (url) => navigations.push(url),
      },
    },
    require: (name) =>
      name === '@/lib/http'
        ? {
            waitForMobileRuntimeReady: () => {
              waits += 1
              return recovery
            },
          }
        : {},
  })
  runInContext(
    transformSync(`${source}\nexport { recoverRoute }`, {
      loader: 'tsx',
      target: 'es2022',
      format: 'cjs',
    }).code,
    context,
  )
  const pending = module.exports.recoverRoute()
  assert.equal(waits, 1)
  assert.deepEqual(navigations, [])
  rejectRecovery(new Error('bounded recovery timed out'))
  await pending
  assert.equal(navigations.length, 1)
  const destination = new URL(navigations[0])
  assert.equal(destination.origin, 'http://127.0.0.1:41873')
  assert.equal(destination.hash, '#/chat?session=retained')
  assert.equal(destination.searchParams.get('existing'), '1')
  assert.ok(destination.searchParams.has('_pisper_recovery'))
})

// 两套真实脚本共享同一页面；独立 VM 避免模块单例、全局对象与定时器污染其他测试。
function createPage({
  inject = true,
  healthy = true,
  resume,
  recover,
  href = 'http://127.0.0.1:41873/#/chat',
} = {}) {
  let now = 1_000
  let nextTimer = 0
  const timers = new Map()
  const commands = []
  const probes = []
  const navigations = []
  const registrations = []
  const location = Object.assign(new URL(href), {
    replace: (url) => navigations.push(url),
  })
  const window = { location }
  function eventTarget(name) {
    const listeners = new Map()
    return {
      addEventListener(type, callback) {
        registrations.push({ target: name, type, handoff: window[handoffKey] })
        const callbacks = listeners.get(type) || []
        callbacks.push(callback)
        listeners.set(type, callbacks)
      },
      dispatch(type, properties = {}) {
        const event = { type, preventDefault() {}, ...properties }
        for (const callback of listeners.get(type) || []) callback(event)
      },
    }
  }
  const document = { ...eventTarget('document'), visibilityState: 'visible' }
  const setTimeout = (callback, ms) => {
    const id = ++nextTimer
    timers.set(id, { callback, at: now + ms })
    return id
  }
  const clearTimeout = (id) => timers.delete(id)
  Object.assign(window, eventTarget('window'), {
    setTimeout,
    clearTimeout,
    __TAURI_INTERNALS__: {
      invoke: async (command) => {
        commands.push(command)
        if (command === 'mobile_resume_local_runtime') await resume?.()
        if (command === 'mobile_recover_application') await recover?.()
      },
    },
  })
  const context = createContext({
    window,
    document,
    location,
    URL,
    AbortController,
    setTimeout,
    clearTimeout,
    Date: class extends Date {
      static now() {
        return now
      }
    },
    fetch: async (url) => {
      probes.push(String(url))
      return {
        ok: healthy,
        headers: { get: () => 'text/html; charset=utf-8' },
        text: async () => '<div id="root"></div>',
      }
    },
    module: { exports: {} },
  })
  if (inject) runInContext(initializationScript, context)
  runInContext(recoveryScript, context)
  return {
    window,
    document,
    commands,
    probes,
    navigations,
    registrations,
    install: context.module.exports.installMobileRuntimeForegroundRecovery,
    waitUntilReady: context.module.exports.waitForMobileRuntimeReady,
    visibility(state) {
      document.visibilityState = state
      document.dispatch('visibilitychange')
    },
    advance(ms) {
      now += ms
      for (const [id, timer] of timers) {
        if (timer.at > now) continue
        timers.delete(id)
        timer.callback()
      }
    },
  }
}

function externalActivityRoundTrip(page, ms = 70_000) {
  page.visibility('hidden')
  page.advance(ms)
  page.visibility('visible')
}

test('before listener installation a long external Activity still uses native fallback', async () => {
  const page = createPage()
  externalActivityRoundTrip(page, 59_999)
  await flush()
  assert.deepEqual(page.commands, [])
  externalActivityRoundTrip(page, 60_000)
  await flush()
  assert.deepEqual(page.commands, ['mobile_recover_application'])
  assert.equal(page.window[handoffKey], undefined)
})

test('handoff is published only after all listeners are installed and installation is idempotent', () => {
  const page = createPage()
  page.registrations.length = 0
  page.install()
  assert.deepEqual(page.registrations, [
    { target: 'document', type: 'visibilitychange', handoff: undefined },
    { target: 'window', type: 'pagehide', handoff: undefined },
    { target: 'window', type: 'pageshow', handoff: undefined },
    { target: 'window', type: 'online', handoff: undefined },
  ])
  assert.equal(page.window[handoffKey], true)
  page.install()
  assert.equal(page.registrations.length, 4)
})

test('failed listener installation does not publish handoff', () => {
  const page = createPage()
  page.window.addEventListener = () => {
    throw new Error('listener registration failed')
  }
  assert.throws(page.install, /listener registration failed/)
  assert.equal(page.window[handoffKey], undefined)
})

test('healthy external Activity return shares one recovery gate without navigating', async () => {
  let releaseResume
  const pendingResume = new Promise((resolve) => {
    releaseResume = resolve
  })
  const page = createPage({ resume: () => pendingResume })
  page.install()
  externalActivityRoundTrip(page)
  const firstGate = page.waitUntilReady()
  const secondGate = page.waitUntilReady()
  assert.strictEqual(firstGate, secondGate)
  assert.deepEqual(page.commands, ['mobile_resume_local_runtime'])
  assert.equal(page.probes.length, 0)
  releaseResume()
  await firstGate
  assert.equal(page.probes.length, 1)
  assert.match(page.probes[0], /_pisper_resume_probe=/)
  assert.deepEqual(page.navigations, [])
  await page.waitUntilReady()
  assert.deepEqual(page.commands, ['mobile_resume_local_runtime'])
})

test('unhealthy external Activity return still reloads through the bounded shared gate', async () => {
  const page = createPage({ healthy: false })
  page.install()
  externalActivityRoundTrip(page)
  const gate = page.waitUntilReady()
  await flush()
  assert.deepEqual(page.commands, ['mobile_resume_local_runtime'])
  assert.equal(page.probes.length, 1)
  assert.equal(page.navigations.length, 1)
  const url = new URL(page.navigations[0])
  assert.equal(url.hash, '#/chat')
  assert.ok(url.searchParams.has('_pisper_recovery'))
  // 页面导航在事件桩中不会卸载 VM，推进原有 reload 兜底时限释放闸门。
  page.advance(8_000)
  await gate
  await page.waitUntilReady()
  assert.equal(page.navigations.length, 1)
})

test('installing while an external Activity is open preserves the pending foreground check', async () => {
  const page = createPage()
  page.visibility('hidden')
  page.advance(70_000)
  page.install()
  page.visibility('visible')
  await page.waitUntilReady()
  assert.deepEqual(page.commands, ['mobile_resume_local_runtime'])
  assert.equal(page.probes.length, 1)
  assert.deepEqual(page.navigations, [])
})

test('module failures still invoke native recovery after foreground handoff', async () => {
  for (const [type, properties] of [
    ['error', { target: { tagName: 'SCRIPT' } }],
    ['error', { message: 'Failed to fetch dynamically imported module' }],
    ['unhandledrejection', { reason: new Error('module script load failed') }],
    ['unhandledrejection', { reason: new Error('Load route script failed') }],
    ['error', { message: 'ChunkLoadError: Loading chunk 42 failed' }],
    ['vite:preloadError', { payload: new Error('Unable to preload CSS') }],
  ]) {
    const page = createPage()
    page.install()
    page.window.dispatch(type, properties)
    await flush()
    assert.deepEqual(page.commands, ['mobile_recover_application'])
    assert.deepEqual(page.navigations, [])
  }
})

test('native module recovery rejection or stalled navigation falls back once on the same route', async () => {
  for (const recover of [
    () => Promise.reject(new Error('native failure')),
    () => new Promise(() => undefined),
    () => Promise.resolve(),
  ]) {
    const page = createPage({ recover })
    page.window.dispatch('vite:preloadError')
    await flush()
    page.advance(20_000)
    await flush()
    assert.equal(page.navigations.length, 1)
    assert.equal(new URL(page.navigations[0]).hash, '#/chat')
    assert.ok(new URL(page.navigations[0]).searchParams.has('_pisper_recovery'))
    page.window.dispatch('error', { target: { tagName: 'SCRIPT' } })
    page.advance(20_000)
    assert.equal(page.navigations.length, 1)
  }
})

test('a repeated broken module remains visible instead of triggering a reload loop', () => {
  const page = createPage({ href: 'http://127.0.0.1:41873/?_pisper_recovery=999#/chat' })
  let prevented = false
  page.window.dispatch('vite:preloadError', { preventDefault: () => (prevented = true) })
  assert.equal(prevented, false)
  assert.deepEqual(page.commands, [])
  page.advance(60_000)
  page.window.dispatch('vite:preloadError', { preventDefault: () => (prevented = true) })
  assert.equal(prevented, true)
  assert.deepEqual(page.commands, ['mobile_recover_application'])
})

test('ordinary network failures do not reload the app or suppress the original error', () => {
  const page = createPage()
  let prevented = false
  page.window.dispatch('unhandledrejection', {
    reason: new Error('Load failed'),
    preventDefault: () => (prevented = true),
  })
  assert.equal(prevented, false)
  assert.deepEqual(page.commands, [])
})

test('network restoration shares the foreground gate and probes again', async () => {
  const page = createPage()
  page.install()
  page.window.dispatch('online')
  await page.waitUntilReady()
  assert.deepEqual(page.commands, ['mobile_resume_local_runtime'])
  assert.equal(page.probes.length, 1)
})

test('bfcache before handoff retains native fallback and ignores ordinary pageshow', async () => {
  const page = createPage()
  page.window.dispatch('pageshow', { persisted: false })
  assert.deepEqual(page.commands, [])
  page.window.dispatch('pagehide')
  page.advance(70_000)
  page.window.dispatch('pageshow', { persisted: true })
  await flush()
  assert.deepEqual(page.commands, ['mobile_recover_application'])
})

test('bfcache after handoff shares the healthy foreground recovery without native navigation', async () => {
  const page = createPage()
  page.install()
  page.visibility('hidden')
  page.window.dispatch('pagehide')
  page.advance(70_000)
  page.window.dispatch('pageshow', { persisted: true })
  const gate = page.waitUntilReady()
  page.visibility('visible')
  assert.strictEqual(page.waitUntilReady(), gate)
  await gate
  assert.deepEqual(page.commands, ['mobile_resume_local_runtime'])
  assert.equal(page.probes.length, 1)
  assert.deepEqual(page.navigations, [])
})

test('desktop pages and non-loopback mobile startup pages never claim foreground handoff', () => {
  for (const options of [
    { inject: false },
    { inject: false, href: 'https://example.com/#/chat' },
    { href: 'http://tauri.localhost/mobile-startup.html' },
  ]) {
    const page = createPage(options)
    page.registrations.length = 0
    page.install()
    assert.equal(page.window[handoffKey], undefined)
    assert.deepEqual(page.registrations, [])
    externalActivityRoundTrip(page)
    assert.deepEqual(page.commands, [])
  }
})
