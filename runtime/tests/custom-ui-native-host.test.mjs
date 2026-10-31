// A7 S3：原生组件前端 SDK 与宿主核心的纯逻辑（计划 §4.2–§4.3、§6 S3）。
// tsx 直接导入 TS：熔断、提交计时、安全模式哨兵、尺寸换算、能力检查与 CapabilityError、
// 模块加载缓存与 20 版上限（注入 importer）、v2 与旧形状的 API 解析器、component_changed 的解析与 '*'。
// 另用 htm + react-dom/server 真正渲染一遍内置模板，确认模板源码与 ctx v1 契约对得上。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import test from 'node:test'
import htm from 'htm'
import * as React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { CTX_V1_KEYS, UI_KIT_V1_KEYS } from '../../shared/custom-ui-manifest.mjs'
import { normalizeNativeManifest } from '../../shared/custom-ui-manifest.mjs'
import { buildComponentTemplate } from '../services/custom-ui-templates.mjs'
import {
  emitComponentChange,
  notifyComponentFeedEvent,
  parseComponentChange,
  subscribeComponentChanges,
} from '../../src/features/board/component-changes.ts'
import {
  componentChangeQueryKeys,
  createComponentChangeSync,
} from '../../src/features/custom-ui/change-sync.ts'
import {
  componentModuleUrl,
  parseComponentAgentSession,
  parseComponentCatalog,
  parseComponentErrors,
  parseComponentInstances,
  parseComponentRow,
} from '../../src/features/custom-ui/custom-ui-api.ts'
import { customUiQueryKeys } from '../../src/features/custom-ui/query-keys.ts'
import {
  createCircuitBreaker,
  recordCrash,
} from '../../src/features/custom-ui/native/circuit-breaker.ts'
import { createCommitTimer } from '../../src/features/custom-ui/native/commit-timer.ts'
import {
  componentFixSummary,
  createComponentLogger,
  createErrorReporter,
  createLogBuffer,
  formatLogArguments,
} from '../../src/features/custom-ui/native/log.ts'
import {
  ComponentLoadError,
  MODULE_VERSION_LIMIT,
  createModuleLoader,
  createStyleRegistry,
} from '../../src/features/custom-ui/native/module-loader.ts'
import {
  base64ToBytes,
  buildFetchRequest,
  createFetchResponse,
} from '../../src/features/custom-ui/native/net.ts'
import {
  SAFE_MODE_CLEAR_DELAY_MS,
  SAFE_MODE_PREFERENCE_KEY,
  createSafeModeController,
  parseSafeModeSentinel,
  safeModeRequested,
} from '../../src/features/custom-ui/native/safe-mode.ts'
import {
  CapabilityError,
  SDK_VERSION,
  componentDisplayName,
  createComponentRuntime,
  translateComponentString,
} from '../../src/features/custom-ui/native/sdk.ts'
import {
  createSessionFeed,
  filterComponentSessions,
  normalizeSessionFilter,
  projectComponentSessions,
} from '../../src/features/custom-ui/native/sessions.ts'
import { computeComponentSize } from '../../src/features/custom-ui/native/size.ts'
import { MIN_INTERVAL_MS, createManagedTimers } from '../../src/features/custom-ui/native/timers.ts'
import { BROWSER_PREFERENCE_KEYS } from '../../shared/browser-preferences.mjs'

const root = resolve(import.meta.dirname, '../..')
const REV = 'a'.repeat(24)
const REV2 = 'b'.repeat(24)

function manifestFor(id, capabilities = {}, extra = {}) {
  const { manifest, errors } = normalizeNativeManifest(id, {
    runtime: 'native',
    sdk: 1,
    name: '测试',
    version: '1.0.0',
    entry: 'component.js',
    capabilities,
    i18n: {
      'zh-CN': { name: '测试组件', strings: { hello: '你好 {name}', only: '仅中文' } },
      'en-US': { name: 'Test widget', strings: { hello: 'Hello {name}' } },
    },
    ...extra,
  })
  assert.deepEqual(errors, [])
  return manifest
}

function fakeTimers() {
  let next = 1
  const pending = new Map()
  return {
    pending,
    host: {
      setTimeout: (handler, ms) => {
        const id = next++
        pending.set(id, { handler, ms, kind: 'timeout' })
        return id
      },
      setInterval: (handler, ms) => {
        const id = next++
        pending.set(id, { handler, ms, kind: 'interval' })
        return id
      },
      clearTimeout: (id) => pending.delete(id),
      clearInterval: (id) => pending.delete(id),
    },
    fire(id) {
      const entry = pending.get(id)
      if (entry?.kind === 'timeout') pending.delete(id)
      entry?.handler()
    },
  }
}

function runtimeFor(manifest, overrides = {}) {
  const buffer = createLogBuffer()
  const sent = []
  const reporter = createErrorReporter({
    send: async (id, report) => {
      sent.push({ id, report })
    },
  })
  const logger = createComponentLogger({
    componentId: manifest.id,
    instanceId: 'inst-1',
    surface: 'feature-page',
    buffer,
    reporter,
  })
  const calls = { notify: [], open: [], fetch: [], storage: [] }
  const callbackErrors = []
  const timers = fakeTimers()
  let now = 1_000_000
  const runtime = createComponentRuntime({
    manifest,
    getLanguage: () => overrides.language ?? 'zh-CN',
    kit: {
      React,
      html: htm.bind(React.createElement),
      ui: Object.freeze({ Textarea: 'textarea', Button: 'button' }),
      useTheme: () => ({
        mode: 'light',
        density: 'comfortable',
        fontSize: '14px',
        radius: 'default',
        token: () => '',
      }),
    },
    services: {
      notify: (text) => calls.notify.push(text),
      openSession: (id) => calls.open.push(id),
      useSessions: () => overrides.sessions ?? [],
      subscribeSessions: overrides.subscribeSessions ?? (() => () => {}),
      fetch: async (request, signal) => {
        calls.fetch.push({ request, signal })
        if (overrides.fetchPending)
          return new Promise((_, reject) =>
            signal.addEventListener('abort', () => reject(signal.reason), { once: true }),
          )
        return (
          overrides.fetchResult ?? {
            status: 200,
            statusText: 'OK',
            headers: { 'content-type': 'application/json' },
            body: '{"ok":true}',
            bodyEncoding: 'utf8',
            url: request.url,
            redirected: false,
          }
        )
      },
      storageGet: async (key) => {
        calls.storage.push(['get', key])
        return undefined
      },
      storageSet: async (key, value) => {
        calls.storage.push(['set', key, value])
      },
      storageRemove: async (key) => {
        calls.storage.push(['remove', key])
      },
    },
    logger,
    onCallbackError: (error) => callbackErrors.push(error),
    timers: timers.host,
    now: () => now,
  })
  const ctx = runtime.context({
    size: computeComponentSize({ surface: 'feature-page', width: 640, height: 400 }),
    placement: { surface: 'feature-page', instanceId: 'inst-1', title: '', preview: false },
  })
  return {
    runtime,
    ctx,
    calls,
    callbackErrors,
    timers,
    buffer,
    sent,
    advance: (ms) => {
      now += ms
    },
  }
}

test('熔断：60 秒内 3 次崩溃暂停，窗口外的旧记录不计，恢复后清零', () => {
  let state = recordCrash(undefined, 0)
  state = recordCrash(state, 30_000)
  assert.equal(state.paused, false)
  // 第一次已经出了 60 秒窗口：只剩两次。
  state = recordCrash(state, 61_000)
  assert.equal(state.paused, false)
  assert.equal(state.crashes.length, 2)
  state = recordCrash(state, 62_000)
  assert.equal(state.paused, true)

  let now = 0
  const breaker = createCircuitBreaker({ now: () => now })
  let notified = 0
  const off = breaker.subscribe(() => notified++)
  breaker.record('x')
  now += 1000
  breaker.record('x')
  now += 1000
  assert.equal(breaker.isPaused('x'), false)
  breaker.record('x')
  assert.equal(breaker.isPaused('x'), true)
  assert.equal(breaker.isPaused('y'), false)
  const version = breaker.version()
  breaker.resume('x')
  assert.equal(breaker.isPaused('x'), false)
  assert.ok(breaker.version() > version)
  // 恢复后的第一次崩溃不会立刻再暂停。
  breaker.record('x')
  assert.equal(breaker.isPaused('x'), false)
  breaker.pause('y')
  assert.equal(breaker.isPaused('y'), true)
  off()
  assert.equal(notified, 6)
})

test('提交计时：连续 3 次超过 200ms 才警告，中间一次快提交会打断连击', () => {
  const timer = createCommitTimer()
  assert.equal(timer.commit(10), null, '没有开始就提交不产生样本')
  const sample = (duration) => {
    timer.start(1000)
    // 同一次提交里的重放渲染不覆盖开始时间。
    timer.start(1000 + duration / 2)
    return timer.commit(1000 + duration)
  }
  assert.deepEqual(sample(250), { duration: 250, slow: true, warning: false })
  assert.equal(sample(300).warning, false)
  assert.equal(sample(50).slow, false)
  assert.equal(sample(201).warning, false)
  assert.equal(sample(201).warning, false)
  const third = sample(400)
  assert.equal(third.warning, true)
  assert.equal(timer.last(), 400)
  // 警告一旦出现就保持，直到 reset。
  assert.equal(sample(10).warning, true)
  timer.reset()
  assert.equal(timer.warning(), false)
})

test('尺寸换算：总览 S/M 记 1/2 个单位，宽度 < 300 为 compact，非法尺寸按 0', () => {
  assert.deepEqual(
    computeComponentSize({ surface: 'overview-card', cardSize: 'S', width: 320, height: 120 }),
    { units: 1, width: 320, height: 120, compact: false },
  )
  assert.deepEqual(
    computeComponentSize({ surface: 'overview-card', cardSize: 'M', width: 299.6, height: 248.2 }),
    { units: 2, width: 300, height: 248, compact: false },
  )
  assert.equal(computeComponentSize({ surface: 'floating', width: 260, height: 96 }).compact, true)
  assert.deepEqual(computeComponentSize({ surface: 'session-panel', width: NaN, height: -1 }), {
    units: 1,
    width: 0,
    height: 0,
    compact: true,
  })
})

test('安全模式：URL 参数、哨兵解析、写入时机、3 秒后清除与上次残留时的询问', () => {
  assert.equal(safeModeRequested('?safe-mode=components'), true)
  assert.equal(safeModeRequested('?x=1&safe-mode=components'), true)
  assert.equal(safeModeRequested('?safe-mode=all'), false)
  assert.equal(parseSafeModeSentinel(null), null)
  assert.equal(parseSafeModeSentinel('{bad'), null)
  assert.equal(parseSafeModeSentinel('{"loading":{"at":"nope","instanceIds":[]}}'), null)
  assert.deepEqual(
    parseSafeModeSentinel(
      JSON.stringify({
        loading: { at: '2026-10-31T00:00:00.000Z', instanceIds: ['ok-1', 'bad id', 3] },
      }),
    ),
    { loading: { at: '2026-10-31T00:00:00.000Z', instanceIds: ['ok-1'] } },
  )
  assert.ok(BROWSER_PREFERENCE_KEYS.includes(SAFE_MODE_PREFERENCE_KEY))

  const values = new Map()
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: (key) => values.delete(key),
  }
  const scheduled = []
  const controller = createSafeModeController({
    storage,
    search: '',
    now: () => Date.parse('2026-10-31T08:00:00.000Z'),
    schedule: (callback, ms) => {
      scheduled.push({ callback, ms })
      return scheduled.length
    },
    cancel: () => {},
  })
  assert.equal(controller.mode(), 'normal')
  controller.beginLoad('a')
  controller.beginLoad('b')
  assert.deepEqual(JSON.parse(values.get(SAFE_MODE_PREFERENCE_KEY)), {
    loading: { at: '2026-10-31T08:00:00.000Z', instanceIds: ['a', 'b'] },
  })
  controller.endLoad('a')
  assert.equal(scheduled.length, 0, '还有实例没提交时不清除')
  controller.endLoad('b')
  assert.equal(scheduled.length, 1)
  assert.equal(scheduled[0].ms, SAFE_MODE_CLEAR_DELAY_MS)
  scheduled[0].callback()
  assert.equal(values.has(SAFE_MODE_PREFERENCE_KEY), false)
  // 初始阶段结束后挂载的实例不再写哨兵。
  controller.beginLoad('c')
  assert.equal(values.has(SAFE_MODE_PREFERENCE_KEY), false)

  // 上次加载时卡死：哨兵还在 → pending，先不加载，等用户选择。
  values.set(
    SAFE_MODE_PREFERENCE_KEY,
    JSON.stringify({ loading: { at: '2026-10-31T07:00:00.000Z', instanceIds: ['a'] } }),
  )
  const next = createSafeModeController({ storage, search: '' })
  assert.equal(next.mode(), 'pending')
  assert.deepEqual(next.staleSentinel()?.loading.instanceIds, ['a'])
  next.beginLoad('a')
  assert.deepEqual(
    JSON.parse(values.get(SAFE_MODE_PREFERENCE_KEY)).loading.at,
    '2026-10-31T07:00:00.000Z',
    'pending 期间不覆盖旧哨兵',
  )
  let changes = 0
  next.subscribe(() => changes++)
  next.resolve('safe')
  assert.equal(next.mode(), 'safe')
  assert.equal(values.has(SAFE_MODE_PREFERENCE_KEY), false)
  assert.equal(changes, 1)
  next.dispose()

  // URL 参数优先：不读哨兵，也不写。
  values.set(
    SAFE_MODE_PREFERENCE_KEY,
    '{"loading":{"at":"2026-10-31T07:00:00.000Z","instanceIds":[]}}',
  )
  const forced = createSafeModeController({ storage, search: '?safe-mode=components' })
  assert.equal(forced.mode(), 'safe')
  forced.resolve('normal')
  assert.equal(forced.mode(), 'safe', '只有 pending 才能被选择改变')

  // 存储抛错时退化为正常模式，不阻止组件加载。
  const broken = createSafeModeController({
    storage: {
      getItem: () => {
        throw new Error('blocked')
      },
      setItem: () => {
        throw new Error('blocked')
      },
      removeItem: () => {},
    },
    search: '',
  })
  assert.equal(broken.mode(), 'normal')
  assert.doesNotThrow(() => broken.beginLoad('a'))

  // 评审返修：初始加载的 3 秒内正常刷新 / 关窗（卸载处理器跑得到）清掉哨兵，下次启动不误报；
  // 阶段结束后卸载监听撤掉，之后卸载不再写存储。
  values.clear()
  const unloads = []
  let stops = 0
  const timers = []
  const reloaded = createSafeModeController({
    storage,
    search: '',
    schedule: (callback) => timers.push(callback),
    cancel: () => {},
    listenUnload: (handler) => {
      unloads.push(handler)
      return () => stops++
    },
  })
  reloaded.beginLoad('x')
  reloaded.beginLoad('y')
  assert.equal(unloads.length, 1, '整个初始阶段只挂一次卸载监听')
  assert.ok(values.has(SAFE_MODE_PREFERENCE_KEY))
  unloads[0]()
  assert.equal(values.has(SAFE_MODE_PREFERENCE_KEY), false)
  assert.equal(createSafeModeController({ storage, search: '' }).mode(), 'normal')
  const settled = createSafeModeController({
    storage,
    search: '',
    schedule: (callback) => timers.push(callback),
    cancel: () => {},
    listenUnload: (handler) => {
      unloads.push(handler)
      return () => stops++
    },
  })
  settled.beginLoad('z')
  settled.endLoad('z')
  timers.at(-1)()
  assert.equal(stops, 1, '初始阶段结束时撤掉卸载监听')
  values.set(SAFE_MODE_PREFERENCE_KEY, 'kept')
  unloads.at(-1)()
  assert.equal(values.get(SAFE_MODE_PREFERENCE_KEY), 'kept')
  reloaded.dispose()
  assert.equal(stops, 2, 'dispose 也撤掉卸载监听')
})

test('ctx 的成员与 CTX_V1_KEYS 一致，ui 子集与 UI_KIT_V1_KEYS 一致', async () => {
  const { ctx, runtime } = runtimeFor(manifestFor('demo'))
  assert.deepEqual(Object.keys(ctx), [...CTX_V1_KEYS])
  assert.equal(ctx.sdk, SDK_VERSION)
  assert.equal(ctx.session, undefined, '只有会话面板放置才有 session')
  assert.ok(Object.isFrozen(ctx))
  assert.equal(ctx.CapabilityError, CapabilityError)
  const panel = runtime.context({
    size: computeComponentSize({ surface: 'session-panel', width: 420, height: 600 }),
    placement: { surface: 'session-panel', instanceId: 'p', title: '', preview: true },
    session: { id: 's1', title: 'T', cwd: '/w', status: 'running' },
  })
  assert.deepEqual(panel.session, { id: 's1', title: 'T', cwd: '/w', status: 'running' })
  assert.equal(panel.placement.preview, true)

  // ui-kit.ts 的冻结对象按 UI_KIT_V1_KEYS 的顺序列出全部成员（读源码，不在 Node 里加载基础组件）。
  const source = await readFile(resolve(root, 'src/features/custom-ui/native/ui-kit.ts'), 'utf8')
  const body = /Object\.freeze\(\{([\s\S]*?)\}\)/.exec(source)?.[1] ?? ''
  const keys = body
    .split('\n')
    .map((line) => /^\s*([A-Za-z]+)(?::|,)/.exec(line)?.[1])
    .filter(Boolean)
  assert.deepEqual(keys, [...UI_KIT_V1_KEYS])
})

test('未声明的能力抛 CapabilityError 并记一条 warn 日志；异步能力返回被拒绝的 Promise', async () => {
  const { ctx, buffer, sent, calls } = runtimeFor(manifestFor('bare'))
  assert.throws(
    () => ctx.notify('hi'),
    (error) => error instanceof CapabilityError && error.capability === 'notify',
  )
  assert.throws(
    () => ctx.sessions.useList(),
    (error) => error.capability === 'sessions.read',
  )
  assert.throws(
    () => ctx.sessions.subscribe(() => {}),
    (error) => error.capability === 'sessions.read',
  )
  assert.throws(
    () => ctx.sessions.open('s'),
    (error) => error.capability === 'sessions.open',
  )
  await assert.rejects(
    ctx.net.fetch('https://api.example.com/'),
    (error) => error.capability === 'network',
  )
  await assert.rejects(ctx.storage.get('k'), (error) => error.capability === 'storage')
  await assert.rejects(ctx.storage.set('k', 1), CapabilityError)
  await assert.rejects(ctx.storage.remove('k'), CapabilityError)
  assert.equal(calls.notify.length + calls.fetch.length + calls.storage.length, 0)
  const entries = buffer.read('bare')
  assert.equal(entries.length, 8)
  assert.ok(entries.every((entry) => entry.level === 'warn' && entry.kind === 'log'))
  assert.match(entries[0].message, /CapabilityError: Capability "notify"/)
  assert.equal(sent.length, 8)
  assert.equal(sent[0].report.instanceId, 'inst-1')
  assert.equal(sent[0].report.surface, 'feature-page')
})

test('声明了的能力：notify 截断与限速、会话筛选与打开、存储键校验、网络请求转换与 signal', async () => {
  const manifest = manifestFor('full', {
    notify: { reason: 'r' },
    'sessions.read': { reason: 'r' },
    'sessions.open': { reason: 'r' },
    storage: { reason: 'r' },
    network: { reason: 'r', hosts: ['api.example.com'] },
  })
  const sessions = [
    { id: 'a', title: 'A', cwd: '/w/one', status: 'running', updatedAt: '2026-10-31T01:00:00Z' },
    { id: 'b', title: 'B', cwd: '/w/two/', status: 'done', updatedAt: '2026-10-31T00:00:00Z' },
  ]
  const { ctx, calls, runtime, advance, buffer } = runtimeFor(manifest, { sessions })
  ctx.notify('x'.repeat(300))
  assert.equal(calls.notify[0].length, 200)
  for (let index = 0; index < 8; index++) ctx.notify(`n${index}`)
  assert.equal(calls.notify.length, 6, '每分钟最多 6 条')
  assert.ok(buffer.read('full').some((entry) => /rate limited/.test(entry.message)))
  advance(60_000)
  ctx.notify('later')
  assert.equal(calls.notify.at(-1), 'later')

  assert.deepEqual(
    ctx.sessions.useList({ cwd: '/w/two', limit: 5 }).map((item) => item.id),
    ['b'],
  )
  assert.deepEqual(
    ctx.sessions.useList({ status: 'running' }).map((item) => item.id),
    ['a'],
  )
  ctx.sessions.open('a')
  ctx.sessions.open(42)
  assert.deepEqual(calls.open, ['a'])

  await ctx.storage.set('note:1', { text: 'hi' })
  assert.equal(await ctx.storage.get('note:1'), undefined)
  await ctx.storage.remove('note:1')
  await assert.rejects(
    ctx.storage.get('../etc'),
    (error) => error.code === 'component_storage_key_invalid',
  )
  assert.deepEqual(calls.storage, [
    ['set', 'note:1', { text: 'hi' }],
    ['get', 'note:1'],
    ['remove', 'note:1'],
  ])

  const response = await ctx.net.fetch('https://api.example.com/x', {
    method: 'post',
    headers: { 'Content-Type': 'application/json' },
    body: new Uint8Array([1, 2, 255]),
  })
  assert.deepEqual(calls.fetch[0].request, {
    url: 'https://api.example.com/x',
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: 'AQL/',
    bodyEncoding: 'base64',
  })
  assert.equal(response.ok, true)
  assert.deepEqual(await response.json(), { ok: true })
  assert.equal(calls.fetch[0].signal.aborted, false)
  // 进行中的请求在卸载时被中止（已完成的请求早已释放监听，不受影响）。
  const pending = runtimeFor(manifest, { fetchPending: true })
  const inflight = pending.ctx.net.fetch('https://api.example.com/slow')
  await Promise.resolve()
  assert.equal(pending.calls.fetch[0].signal.aborted, false)
  pending.runtime.dispose()
  assert.equal(pending.calls.fetch[0].signal.aborted, true, '卸载时中止进行中的请求')
  await assert.rejects(inflight, (error) => error.code === 'component_request_aborted')
  runtime.dispose()
  assert.equal(runtime.disposed, true)
})

test('网络请求与响应的形状转换', async () => {
  assert.throws(
    () => buildFetchRequest(42),
    (error) => error.code === 'component_fetch_invalid',
  )
  assert.throws(
    () => buildFetchRequest('https://a.example', { body: { not: 'allowed' } }),
    (error) => error.code === 'component_fetch_invalid',
  )
  assert.deepEqual(buildFetchRequest('https://a.example', { body: 'hi' }), {
    url: 'https://a.example',
    body: 'hi',
    bodyEncoding: 'utf8',
  })
  const binary = createFetchResponse({
    status: 404,
    statusText: 'Not Found',
    headers: {},
    body: Buffer.from('héllo').toString('base64'),
    bodyEncoding: 'base64',
    url: 'https://a.example/',
    redirected: true,
  })
  assert.equal(binary.ok, false)
  assert.equal(await binary.text(), 'héllo')
  assert.deepEqual(
    new Uint8Array(await binary.arrayBuffer()),
    base64ToBytes(Buffer.from('héllo').toString('base64')),
  )
  assert.equal(binary.redirected, true)
})

test('托管定时器：回调异常归属实例，卸载时全部清理，只清自己建的 id', () => {
  const { ctx, runtime, timers, callbackErrors, buffer } = runtimeFor(manifestFor('timers'))
  const fired = []
  const first = ctx.setTimeout(() => fired.push('t'), 10)
  const interval = ctx.setInterval(() => {
    throw new Error('boom')
  }, 0)
  assert.equal(timers.pending.get(interval).ms, MIN_INTERVAL_MS, '间隔有下限')
  timers.fire(first)
  timers.fire(interval)
  assert.deepEqual(fired, ['t'])
  assert.equal(callbackErrors.length, 1)
  assert.ok(
    buffer.read('timers').some((entry) => entry.kind === 'callback' && /boom/.test(entry.message)),
  )
  ctx.clearTimeout(9999)
  assert.equal(timers.pending.size, 1)
  ctx.setTimeout(() => fired.push('never'), 5)
  runtime.dispose()
  assert.equal(timers.pending.size, 0)
  assert.equal(
    ctx.setTimeout(() => {}, 1),
    0,
    '卸载后不再建定时器',
  )

  const managed = createManagedTimers({ host: fakeTimers().host, onError: () => {} })
  assert.equal(managed.setTimeout('not a function', 1), 0)
})

test('ctx.t：当前语言 → 另一种语言 → 键名，{name} 替换；显示名按语言选择', () => {
  const manifest = manifestFor('i18n')
  assert.equal(translateComponentString(manifest, 'en-US', 'hello', { name: 'Ann' }), 'Hello Ann')
  assert.equal(translateComponentString(manifest, 'en-US', 'only'), '仅中文')
  assert.equal(translateComponentString(manifest, 'zh-CN', 'missing'), 'missing')
  assert.equal(translateComponentString(manifest, 'zh-CN', 'hello', { other: 1 }), '你好 {name}')
  assert.equal(translateComponentString(manifest, 'zh-CN', 42), '')
  assert.equal(componentDisplayName(manifest, 'en-US'), 'Test widget')
  const { ctx } = runtimeFor(manifest, { language: 'en-US' })
  assert.equal(ctx.t('hello', { name: 'Bo' }), 'Hello Bo')
})

test('模块加载：按 id@rev 缓存、default 必须是函数、失败不缓存、20 个版本后建议刷新', async () => {
  const imported = []
  const modules = new Map()
  const loader = createModuleLoader({
    importer: async (url) => {
      imported.push(url)
      const module = modules.get(url)
      if (!module) throw new Error('404')
      return module
    },
  })
  const url = componentModuleUrl('demo', REV, 'component.js')
  assert.equal(url, `/api/custom-ui/components/demo/r/${REV}/component.js`)
  assert.throws(() => componentModuleUrl('demo', REV, '../x.js'))
  assert.throws(() => componentModuleUrl('Demo', REV, 'x.js'))
  modules.set(url, { default: () => null })
  const [one, two] = await Promise.all([
    loader.load('demo', REV, 'component.js'),
    loader.load('demo', REV, 'component.js'),
  ])
  assert.equal(one, two)
  assert.equal(imported.length, 1, '同一版本只导入一次')

  modules.set(componentModuleUrl('demo', REV2, 'component.js'), { default: 'nope' })
  await assert.rejects(
    loader.load('demo', REV2, 'component.js'),
    (error) => error instanceof ComponentLoadError && error.code === 'default_export_invalid',
  )
  await assert.rejects(
    loader.load('missing', REV, 'component.js'),
    (error) => error instanceof ComponentLoadError && error.code === 'module_load_failed',
  )
  await assert.rejects(loader.load('missing', REV, 'component.js'))
  assert.equal(
    imported.filter((entry) => entry.includes('/missing/')).length,
    2,
    '失败的导入不缓存',
  )
  assert.equal(loader.versions('demo'), 1)

  let notified = 0
  loader.subscribe(() => notified++)
  for (let index = 1; index < MODULE_VERSION_LIMIT; index++) {
    const rev = index.toString(16).padStart(24, '0')
    modules.set(componentModuleUrl('demo', rev, 'component.js'), { default: () => null })
    assert.equal(loader.reloadRecommended('demo'), false)
    await loader.load('demo', rev, 'component.js')
  }
  assert.equal(loader.versions('demo'), MODULE_VERSION_LIMIT)
  assert.equal(loader.reloadRecommended('demo'), true)
  assert.equal(notified, MODULE_VERSION_LIMIT - 1)
})

test('组件样式按 href 引用计数注入与撤下', () => {
  const head = {
    children: [],
    appendChild(element) {
      this.children.push(element)
      element.remove = () => {
        head.children = head.children.filter((child) => child !== element)
      }
    },
  }
  const registry = createStyleRegistry({
    head,
    createElement: () => ({ dataset: {} }),
  })
  const releaseA = registry.retain('demo', REV, ['styles.css', '../bad.css'])
  const releaseB = registry.retain('demo', REV, ['styles.css'])
  assert.equal(head.children.length, 1, '非法路径跳过，同一 href 只注入一次')
  assert.equal(head.children[0].href, `/api/custom-ui/components/demo/r/${REV}/styles.css`)
  assert.equal(head.children[0].rel, 'stylesheet')
  releaseA()
  releaseA()
  assert.equal(head.children.length, 1)
  releaseB()
  assert.equal(head.children.length, 0)
  assert.equal(registry.size(), 0)
})

test('日志：参数格式化、按组件限速上报（20 条/分钟）、过长实例 id 不上报', async () => {
  assert.equal(formatLogArguments(['a', 1, { b: 2 }, new Error('x')]), 'a 1 {"b":2} Error: x')
  const circular = {}
  circular.self = circular
  assert.equal(formatLogArguments([circular]), '[object Object]')
  assert.equal(formatLogArguments(['x'.repeat(900)]).length, 500)

  let now = 0
  const sent = []
  const reporter = createErrorReporter({
    send: async (id, report) => sent.push({ id, report }),
    now: () => now,
  })
  for (let index = 0; index < 25; index++)
    reporter.report('a', { kind: 'log', level: 'warn', message: `m${index}` })
  assert.equal(sent.length, 20)
  assert.equal(reporter.dropped('a'), 5)
  assert.equal(reporter.report('b', { kind: 'log', level: 'warn', message: 'other' }), true)
  now += 60_000
  assert.equal(reporter.report('a', { kind: 'log', level: 'error', message: '' }), true)
  assert.equal(sent.at(-1).report.message, 'log', '空消息用 kind 兜底')
  reporter.report('a', { kind: 'render', level: 'error', message: 'x', instanceId: 'i'.repeat(70) })
  assert.equal(sent.at(-1).report.instanceId, undefined)
  const failing = createErrorReporter({ send: async () => Promise.reject(new Error('offline')) })
  assert.equal(failing.report('a', { kind: 'log', level: 'warn', message: 'x' }), true)

  const buffer = createLogBuffer(3)
  for (let index = 0; index < 5; index++)
    buffer.append({
      at: index,
      componentId: 'c',
      kind: 'log',
      level: 'info',
      message: String(index),
    })
  assert.deepEqual(
    buffer.read('c').map((entry) => entry.message),
    ['2', '3', '4'],
  )
})

test('会话投影：未归档会话按泳道标状态，筛选与 limit 夹取，订阅源只在内容变化时通知', () => {
  const projected = projectComponentSessions(
    [
      { id: 'old', title: 'Old', cwd: '/w', modified: '2026-10-29T00:00:00Z' },
      { id: 'run', title: 'Run', cwd: '/w/', modified: '2026-10-31T00:00:00Z' },
      { id: 'arch', title: 'Arch', modified: '2026-10-31T00:00:00Z', archived: true },
    ],
    [{ id: 'run', lane: 'running', totalTokens: 1200 }],
  )
  assert.deepEqual(projected, [
    {
      id: 'run',
      title: 'Run',
      cwd: '/w/',
      status: 'running',
      updatedAt: '2026-10-31T00:00:00Z',
      usage: { totalTokens: 1200 },
    },
    { id: 'old', title: 'Old', cwd: '/w', status: 'idle', updatedAt: '2026-10-29T00:00:00Z' },
  ])
  assert.deepEqual(
    filterComponentSessions(projected, { cwd: '/w' }).map((item) => item.id),
    ['run', 'old'],
  )
  assert.deepEqual(
    filterComponentSessions(projected, { limit: 1 }).map((item) => item.id),
    ['run'],
  )
  assert.deepEqual(normalizeSessionFilter({ limit: 999, status: 'bogus' }), { limit: 100 })
  assert.deepEqual(normalizeSessionFilter('x'), { limit: 50 })

  const feed = createSessionFeed()
  const seen = []
  const off = feed.subscribe((list) => seen.push(list.length))
  feed.set(projected)
  feed.set([...projected])
  feed.set([])
  off()
  feed.set(projected)
  assert.deepEqual(seen, [2, 0])
})

test('API 解析：v2 目录行、旧形状一律 legacy、ready 缺 rev 降为 invalid', () => {
  const manifest = manifestFor('notes', { storage: { reason: 'r' } })
  const catalog = parseComponentCatalog({
    root: '~/.pisper/custom-ui',
    features: { import: false, versions: false, export: false, trash: 'move' },
    components: [
      {
        id: 'notes',
        kind: 'native',
        status: 'ready',
        name: '便签',
        version: '1.0.0',
        description: '',
        directory: '~/x',
        enabled: true,
        source: 'template',
        rev: REV,
        entry: 'component.js',
        manifest,
        errors: [],
        updatedAt: '2026-10-31T00:00:00.000Z',
      },
      { id: 'notes', kind: 'native' },
      { id: 'Bad Id', kind: 'native' },
      { id: 'old', kind: 'legacy', status: 'unsupported', name: 'Old', enabled: false },
      {
        id: 'broken',
        kind: 'broken',
        errors: [{ path: 'manifest.json', code: 'manifest_missing' }],
      },
      {
        id: 'norev',
        kind: 'native',
        status: 'ready',
        enabled: true,
        manifest: manifestFor('norev'),
      },
    ],
  })
  assert.equal(catalog.legacyShape, false)
  assert.deepEqual(catalog.features, {
    import: false,
    versions: false,
    export: false,
    trash: 'move',
  })
  assert.deepEqual(
    catalog.components.map((row) => row.id),
    ['notes', 'old', 'broken', 'norev'],
  )
  const [notes, old, broken, norev] = catalog.components
  assert.equal(notes.status, 'ready')
  assert.equal(notes.rev, REV)
  assert.equal(notes.manifest.capabilities.storage.quotaKb, 256)
  assert.equal(notes.source, 'template')
  assert.equal(old.kind, 'legacy')
  assert.equal(old.enabled, false)
  assert.equal(broken.status, 'invalid')
  assert.equal(broken.errors[0].code, 'manifest_missing')
  assert.equal(norev.status, 'invalid')
  assert.equal(norev.rev, undefined)
  assert.equal(norev.errors.at(-1).code, 'component_response_invalid')

  // manifest 的 id 与行不一致、入口不安全：manifest 丢弃，ready 行降为 invalid。
  const mismatched = parseComponentRow({
    id: 'a',
    kind: 'native',
    status: 'ready',
    rev: REV,
    enabled: true,
    manifest: { ...manifest, id: 'b' },
  })
  assert.equal(mismatched.status, 'invalid')
  assert.equal(mismatched.manifest, undefined)

  // 旧 Runtime（第 2–4 期）的形状：没有 kind / features，全部按 legacy，删除按钮隐藏。
  const legacy = parseComponentCatalog({
    root: '~/.pisper/custom-ui',
    components: [
      {
        id: 'pisper-island',
        name: '灵动岛',
        version: '1.0.0',
        description: '',
        entry: 'index.html',
        entryUrl: '/api/custom-ui/components/pisper-island/assets/index.html',
        permissions: [],
        directory: '内置',
        builtIn: true,
      },
    ],
  })
  assert.equal(legacy.legacyShape, true)
  assert.equal(legacy.features.trash, 'none')
  assert.equal(legacy.components[0].kind, 'legacy')
  assert.equal(legacy.components[0].status, 'unsupported')
  assert.throws(() => parseComponentCatalog({ components: [] }))
})

test('API 解析：实例逐条校验并按 order 排序，错误日志与网络统计', () => {
  const data = parseComponentInstances({
    revision: 4,
    instances: [
      {
        instanceId: '3f1d7a2e-0000-4000-8000-000000000002',
        componentId: 'notes',
        surface: 'overview-card',
        size: 'X',
        props: { a: 1 },
        order: 2,
      },
      {
        instanceId: '3f1d7a2e-0000-4000-8000-000000000001',
        componentId: 'notes',
        surface: 'session-panel',
        scope: { type: 'project', cwd: '/w' },
        enabled: false,
        order: 1,
      },
      { instanceId: 'bad_id!', componentId: 'notes', surface: 'floating' },
      { instanceId: 'x1', componentId: 'notes', surface: 'board-card' },
    ],
  })
  assert.equal(data.revision, 4)
  assert.deepEqual(
    data.instances.map((instance) => instance.order),
    [1, 2],
  )
  assert.equal(data.instances[1].size, 'S', '未知尺寸回到 S')
  assert.deepEqual(data.instances[0].scope, { type: 'project', cwd: '/w' })
  assert.equal(data.instances[0].enabled, false)
  assert.equal(data.instances[0].size, null)
  assert.throws(() => parseComponentInstances({ revision: 'x', instances: [] }))

  const errors = parseComponentErrors({
    entries: [
      {
        at: '2026-10-31T00:00:00Z',
        kind: 'render',
        level: 'error',
        message: 'boom',
        stack: 's',
        instanceId: 'i',
      },
      { message: 42 },
    ],
    dropped: 3,
    network: {
      count: 2,
      lastAt: '2026-10-31T00:00:00Z',
      hosts: { 'api.example.com': 2, bad: 'x' },
    },
  })
  assert.equal(errors.entries.length, 1)
  assert.equal(errors.dropped, 3)
  assert.deepEqual(errors.network, {
    count: 2,
    lastAt: '2026-10-31T00:00:00Z',
    hosts: { 'api.example.com': 2 },
  })
  assert.deepEqual(parseComponentErrors({ entries: [] }).network, {
    count: 0,
    lastAt: null,
    hosts: {},
  })
})

test("component_changed：解析不可信帧，接受后通知监听者，快照通知 '*'，并映射到查询键", () => {
  assert.deepEqual(parseComponentChange({ id: 'notes', rev: REV, reason: 'files', revision: 9 }), {
    id: 'notes',
    rev: REV,
    reason: 'files',
  })
  assert.deepEqual(parseComponentChange({ id: 'notes', rev: 'short', reason: 'weird' }), {
    id: 'notes',
  })
  assert.equal(parseComponentChange({ id: '../x' }), null)
  assert.equal(parseComponentChange(null), null)

  const seen = []
  const off = subscribeComponentChanges((change) => seen.push(change))
  const offThrowing = subscribeComponentChanges(() => {
    throw new Error('listener bug')
  })
  notifyComponentFeedEvent('component_changed', { id: 'notes', reason: 'instances' }, true)
  notifyComponentFeedEvent('component_changed', { id: 'notes' }, false)
  notifyComponentFeedEvent('component_changed', { id: 'NOPE' }, true)
  notifyComponentFeedEvent('snapshot', { revision: 1, items: [] }, true)
  notifyComponentFeedEvent('heartbeat', { revision: 2 }, true)
  emitComponentChange({ id: 'other', reason: 'deleted' })
  off()
  offThrowing()
  notifyComponentFeedEvent('snapshot', {}, true)
  assert.deepEqual(seen, [
    { id: 'notes', reason: 'instances' },
    { id: '*' },
    { id: 'other', reason: 'deleted' },
  ])

  assert.deepEqual(componentChangeQueryKeys({ id: '*' }), [customUiQueryKeys.all])
  assert.deepEqual(componentChangeQueryKeys({ id: 'notes', reason: 'instances' }), [
    ['custom-ui', 'instances'],
  ])
  assert.deepEqual(componentChangeQueryKeys({ id: 'notes', reason: 'files', rev: REV }), [
    ['custom-ui', 'catalog'],
    ['custom-ui', 'describe', 'notes'],
    ['custom-ui', 'errors', 'notes'],
    ['custom-ui', 'instances'],
  ])

  // 多个放置 hook 共用一条订阅：引用计数归零才退订。
  let listener = null
  let subscriptions = 0
  const sync = createComponentChangeSync((next) => {
    subscriptions++
    listener = next
    return () => {
      listener = null
    }
  })
  const invalidated = []
  const client = { invalidateQueries: ({ queryKey }) => invalidated.push(queryKey) }
  const releaseA = sync.retain(client)
  const releaseB = sync.retain(client)
  assert.equal(subscriptions, 1)
  listener({ id: '*' })
  assert.deepEqual(invalidated, [['custom-ui']])
  releaseA()
  releaseA()
  assert.ok(listener, '还有使用者时不退订')
  releaseB()
  assert.equal(listener, null)
  assert.equal(sync.users(), 0)
})

test('内置模板经 htm + ctx v1 真实渲染（服务端渲染，不跑 effect）', async () => {
  const sessions = [
    {
      id: 'a',
      title: 'A',
      cwd: '/w',
      status: 'running',
      updatedAt: new Date().toISOString(),
      usage: { totalTokens: 1500 },
    },
    { id: 'b', title: 'B', cwd: '/w', status: 'done', updatedAt: new Date().toISOString() },
  ]
  for (const [template, surface, width, expected] of [
    ['usage-today', 'overview-card', 320, /1\.5k/],
    ['usage-today', 'floating', 260, /<div[^>]*>2<\/div>/],
    ['notes', 'feature-page', 640, /<textarea[^>]*disabled=""/],
    ['blank', 'feature-page', 640, /<strong[^>]*>Blank<\/strong>/],
  ]) {
    const built = buildComponentTemplate(template, template === 'blank' ? { name: 'Blank' } : {})
    const { manifest, errors } = normalizeNativeManifest(template, built.manifest)
    assert.deepEqual(errors, [], template)
    const source = built.files.find((file) => file.path === 'component.js').content
    const module = await import(
      `data:text/javascript;base64,${Buffer.from(source).toString('base64')}`
    )
    const { runtime } = runtimeFor(manifest, { sessions })
    const size = computeComponentSize({ surface, cardSize: 'M', width, height: 200 })
    const placement = { surface, instanceId: 'inst-1', title: '', preview: false }
    const ctx = runtime.context({ size, placement })
    const markup = renderToStaticMarkup(
      React.createElement(module.default, {
        props: {},
        size: ctx.size,
        placement: ctx.placement,
        ctx,
      }),
    )
    assert.match(markup, expected, `${template} @ ${surface}`)
    runtime.dispose()
  }
})

test('绑定会话响应按不可信数据解析：会话 id 与技能命令有格式要求，摘要只在 id 一致时采用', () => {
  assert.deepEqual(
    parseComponentAgentSession({
      sessionId: 's-1',
      created: true,
      command: '/skill:pisper-component',
      session: { id: 's-1', cwd: '/data/custom-ui/ci' },
    }),
    {
      sessionId: 's-1',
      created: true,
      command: '/skill:pisper-component',
      session: { id: 's-1', cwd: '/data/custom-ui/ci' },
    },
  )
  // 旧 Runtime 没有 session 字段；摘要的 id 对不上就不用；命令不像技能命令就当没有。
  assert.deepEqual(
    parseComponentAgentSession({ sessionId: 's-2', created: false, command: '/skill:x\nrm' }),
    { sessionId: 's-2', created: false, command: '', session: null },
  )
  assert.equal(
    parseComponentAgentSession({ sessionId: 's-3', created: 1, session: { id: 'other' } }).session,
    null,
  )
  for (const value of [
    null,
    {},
    { sessionId: '' },
    { sessionId: 'a b' },
    { sessionId: 'x'.repeat(129) },
  ])
    assert.throws(() => parseComponentAgentSession(value), JSON.stringify(value))
})

test('[让代理修复] 的错误摘要是消息加调用栈前 1 KB，调用栈开头重复的消息不再写一遍', () => {
  assert.equal(componentFixSummary('TypeError: x', undefined), 'TypeError: x')
  assert.equal(
    componentFixSummary('TypeError: x', 'TypeError: x\n    at Card (r/abc/component.js:3:9)'),
    'TypeError: x\nat Card (r/abc/component.js:3:9)',
  )
  const long = componentFixSummary('Error: y', `at ${'a'.repeat(3000)}`)
  assert.equal(long.length, 'Error: y\n'.length + 1024)
})
