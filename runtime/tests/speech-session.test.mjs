import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { getEventListeners } from 'node:events'
import { setImmediate } from 'node:timers/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from '@ts-morph/common/dist/typescript.js'
import * as abortSignal from '../../src/lib/abort-signal.ts'
import { consumeEventStream } from '../../src/lib/api.ts'
import { speechHotwords } from '../../shared/speech-terms.mjs'

const code = ts.transpileModule(await readFile('src/features/chat/speech-session.ts', 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText
function deferred() {
  let resolve, reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
async function flush() {
  for (let index = 0; index < 4; index++) await setImmediate()
}
function fixture(t, settings = {}) {
  const calls = [],
    events = [],
    streams = [],
    controllers = [],
    scopes = []
  const modules = {
    '@/lib/abort-signal': {
      ...abortSignal,
      createAbortScope(...args) {
        const scope = abortSignal.createAbortScope(...args)
        scopes.push(scope)
        return scope
      },
    },
    '@/lib/api': {
      consumeEventStream,
      apiJson: async (path, options) => {
        calls.push({ path, options })
        return settings.terms ?? { terms: ['useEffect'] }
      },
    },
    '@shared/speech-terms.mjs': { speechHotwords },
    './speech-text': settings.modules?.promise ?? {},
    './speech-stream-text': settings.modules?.promise ?? {},
    './speech-models': {
      invokeLocalSpeech: async (command, args) => {
        calls.push({ command, args })
        return settings.invoke?.(command, args) ?? { ready: true }
      },
    },
  }
  const module = { exports: {} }
  runInNewContext(code, {
    module,
    exports: module.exports,
    require: (id) => {
      assert.ok(modules[id], id)
      if (id === './speech-text' || id === './speech-stream-text') calls.push({ module: id })
      return modules[id]
    },
    window: {
      __PISPER_MOBILE_APP__: settings.native,
      dispatchEvent: (event) => events.push(event.type),
    },
    crypto: { randomUUID },
    Event,
    Error,
    setTimeout,
    clearTimeout,
    fetch: async (path, options) => {
      calls.push({ path, options, args: JSON.parse(options.body) })
      if (settings.response) return settings.response
      const stream = new ReadableStream({
        start(controller) {
          streams.push(controller)
          options.signal.addEventListener('abort', () => controller.error(options.signal.reason), {
            once: true,
          })
        },
      })
      return new Response(stream, { headers: { 'content-type': 'text/event-stream' } })
    },
  })
  t.after(async () => {
    controllers.forEach((controller) => controller.abort())
    await flush()
  })
  return {
    ...module.exports,
    calls,
    events,
    streams,
    scopes,
    start(options = { kinds: ['asr', 'tts'], hotwords: 'use effect' }) {
      const controller = new AbortController()
      controllers.push(controller)
      return {
        controller,
        preparing: module.exports.prepareSpeechSession(options, controller.signal),
      }
    },
    send(event, data, index = 0) {
      streams[index].enqueue(
        new TextEncoder().encode(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`),
      )
    },
  }
}

test('desktop readiness keeps the SSE lease pinned beyond initialization until explicit abort', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const f = fixture(t)
  const run = f.start()
  let settled = false
  run.preparing.then(() => {
    settled = true
  })
  await flush()
  assert.equal(settled, false)
  f.send('ready', { ready: true })
  const lease = await run.preparing
  assert.equal(f.calls[0].path, '/api/speech/session')
  assert.deepEqual(f.calls[0].args, {
    requestId: lease.requestId,
    kinds: ['asr', 'tts'],
    hotwords: 'use effect',
  })
  t.mock.timers.tick(120_000)
  await flush()
  assert.equal(lease.signal.aborted, false, 'initialization timeout must not expire a ready lease')
  run.controller.abort()
  await flush()
  assert.equal(lease.signal.aborted, true)
  assert.equal(f.calls[0].options.signal.aborted, true)
  assert.deepEqual(f.events, [])
})

test('unexpected desktop EOF after ready aborts the lease and notifies interruption', async (t) => {
  const f = fixture(t)
  const run = f.start()
  await flush()
  f.send('ready', { ready: true })
  const lease = await run.preparing
  f.streams[0].close()
  await flush()
  assert.equal(lease.signal.aborted, true)
  assert.match(lease.signal.reason.message, /interrupted/)
  assert.deepEqual(f.events, ['pisper:speech-interrupted'])
})

for (const phase of ['EOF', 'invalid ready', 'server error', 'HTTP'])
  test(`desktop ${phase} before readiness fails without a live lease`, async (t) => {
    const f = fixture(
      t,
      phase === 'HTTP' ? { response: Response.json({ error: 'failed' }, { status: 503 }) } : {},
    )
    const run = f.start()
    const rejected = assert.rejects(run.preparing)
    await flush()
    if (phase === 'EOF') f.streams[0].close()
    if (phase === 'invalid ready') f.send('ready', { ready: 'true' })
    if (phase === 'server error') f.send('error', { error: 'failed' })
    await rejected
    assert.equal(f.calls[0].options.signal.aborted, true)
    assert.deepEqual(f.events, [])
  })

for (const native of [false, true])
  test(`${native ? 'native' : 'desktop'} prewarm times out rather than hanging`, async (t) => {
    t.mock.timers.enable({ apis: ['setTimeout'] })
    const waiting = deferred()
    const f = fixture(t, { native, invoke: () => waiting.promise })
    const run = f.start()
    const rejected = assert.rejects(run.preparing, { name: 'TimeoutError' })
    await flush()
    t.mock.timers.tick(60_000)
    await rejected
    if (native) {
      const before = f.calls.length
      waiting.resolve({ ready: true })
      await flush()
      assert.ok(f.calls.length > before, 'late preparation must be released again')
    } else assert.equal(f.calls[0].options.signal.aborted, true)
  })

for (const late of ['resolve', 'reject'])
  test(`native cancellation observes late prepare ${late} and release failures`, async (t) => {
    const waiting = deferred()
    const f = fixture(t, {
      native: true,
      invoke: (command) =>
        command === 'mobile_prepare_speech_session'
          ? waiting.promise
          : Promise.reject(new Error('release failure')),
    })
    const run = f.start({ kinds: ['asr'], hotwords: '', voiceId: 'voice' })
    const rejected = assert.rejects(run.preparing, { name: 'AbortError' })
    run.controller.abort()
    await rejected
    const requestId = f.calls[0].args.requestId
    if (late === 'resolve') waiting.resolve({ ready: true })
    else waiting.reject(new Error('late failure'))
    await flush()
    const releases = f.calls.filter((call) => call.command === 'mobile_release_speech_session')
    assert.equal(releases.length, 2)
    assert.ok(releases.every((call) => call.args.requestId === requestId))
    assert.deepEqual(f.events, [])
  })

test('TTS modules load alongside model preparation and readiness waits for both', async (t) => {
  const modules = deferred()
  const f = fixture(t, { native: true, modules })
  const run = f.start()
  let ready = false
  run.preparing.then(() => {
    ready = true
  })
  await flush()
  assert.equal(ready, false)
  assert.equal(f.calls.filter((call) => call.command === 'mobile_prepare_speech_session').length, 1)
  assert.deepEqual(
    f.calls.filter((call) => call.module).map((call) => call.module),
    ['./speech-text', './speech-stream-text'],
  )
  modules.resolve({})
  await run.preparing
  assert.equal(ready, true)
})

for (const failure of ['reject', 'timeout'])
  test(`TTS module ${failure} releases a ready native model instead of hanging initialization`, async (t) => {
    t.mock.timers.enable({ apis: ['setTimeout'] })
    const modules = deferred()
    const f = fixture(t, { native: true, modules })
    const run = f.start()
    const rejected = assert.rejects(run.preparing)
    await flush()
    if (failure === 'reject') modules.reject(new Error('module unavailable'))
    else t.mock.timers.tick(60_000)
    await rejected
    assert.ok(f.calls.some((call) => call.command === 'mobile_release_speech_session'))
    modules.resolve({})
    await flush()
  })

test('ASR-only preparation does not import the playback text modules', async (t) => {
  const f = fixture(t, { native: true })
  await f.start({ kinds: ['asr'] }).preparing
  assert.ok(!f.calls.some((call) => call.module))
})

test('overlapping native leases release only their own request IDs', async (t) => {
  const f = fixture(t, { native: true })
  const first = f.start()
  const second = f.start({ kinds: ['asr'], hotwords: '' })
  const a = await first.preparing
  const b = await second.preparing
  assert.notEqual(a.requestId, b.requestId)
  first.controller.abort()
  await flush()
  assert.equal(b.signal.aborted, false)
  assert.deepEqual(
    f.calls
      .filter((call) => call.command === 'mobile_release_speech_session')
      .map((call) => call.args.requestId),
    [a.requestId],
  )
  second.controller.abort()
  await flush()
  assert.deepEqual(
    f.calls
      .filter((call) => call.command === 'mobile_release_speech_session')
      .map((call) => call.args.requestId),
    [a.requestId, b.requestId],
  )
})

test('pre-aborted warmup does not acquire either platform', async (t) => {
  const f = fixture(t)
  const controller = new AbortController()
  controller.abort()
  await assert.rejects(f.prepareSpeechSession({ kinds: ['asr'] }, controller.signal), {
    name: 'AbortError',
  })
  assert.deepEqual(f.calls, [])
})

for (const native of [false, true])
  for (const ending of ['cancel before ready', 'cancel after ready', 'timeout', 'failure'])
    test(`${native ? 'native' : 'desktop'} ${ending} disposes every real abort-scope listener`, async (t) => {
      t.mock.timers.enable({ apis: ['setTimeout'] })
      const pending = deferred()
      const f = fixture(t, {
        native,
        invoke: (command) => (command === 'mobile_prepare_speech_session' ? pending.promise : {}),
      })
      const run = f.start({ kinds: ['asr'] })
      const result = run.preparing.then(
        (lease) => ({ lease }),
        (error) => ({ error }),
      )
      await flush()
      assert.equal(getEventListeners(run.controller.signal, 'abort').length, 1)
      assert.equal(f.scopes.length, 2)
      if (ending === 'cancel after ready') {
        if (native) pending.resolve({ ready: true })
        else f.send('ready', { ready: true })
        const { lease } = await result
        assert.equal(lease.signal.aborted, false)
        assert.equal(getEventListeners(f.scopes[1].signal, 'abort').length, 0)
        assert.equal(
          getEventListeners(run.controller.signal, 'abort').length,
          1,
          'ready must retain the live owner bridge',
        )
        run.controller.abort()
      } else if (ending === 'timeout') t.mock.timers.tick(60_000)
      else if (ending === 'failure') {
        if (native) pending.reject(new Error('prepare failed'))
        else f.send('error', { error: 'prepare failed' })
      } else run.controller.abort()
      const outcome = await result
      if (ending !== 'cancel after ready') assert.ok(outcome.error)
      await flush()
      assert.equal(getEventListeners(run.controller.signal, 'abort').length, 0)
      for (const scope of f.scopes) assert.equal(getEventListeners(scope.signal, 'abort').length, 0)
      const calls = f.calls.length
      t.mock.timers.tick(120_000)
      await flush()
      assert.equal(f.calls.length, calls, 'disposed timeout must not perform another release')
      pending.resolve({ ready: true })
      await flush()
      assert.equal(getEventListeners(run.controller.signal, 'abort').length, 0)
      for (const scope of f.scopes) assert.equal(getEventListeners(scope.signal, 'abort').length, 0)
    })

test('desktop disconnect after ready removes bridges even when the external owner is not aborted', async (t) => {
  const f = fixture(t)
  const run = f.start({ kinds: ['asr'] })
  await flush()
  f.send('ready', { ready: true })
  const lease = await run.preparing
  f.streams[0].close()
  await flush()
  assert.equal(run.controller.signal.aborted, false)
  assert.equal(lease.signal.aborted, true)
  assert.equal(getEventListeners(run.controller.signal, 'abort').length, 0)
  for (const scope of f.scopes) assert.equal(getEventListeners(scope.signal, 'abort').length, 0)
  assert.deepEqual(f.events, ['pisper:speech-interrupted'])
})

test('hotword loading retains the selected session terms and explicit empty selection', async (t) => {
  const f = fixture(t)
  const controller = new AbortController()
  const result = await f.loadSpeechHotwords('chat /?', controller.signal)
  assert.deepEqual(result.terms, ['useEffect'])
  assert.equal(result.hotwords, 'use effect')
  assert.equal(f.calls[0].path, '/api/speech/terms?sessionId=chat%20%2F%3F')
  const empty = fixture(t, { terms: { terms: [] } })
  assert.equal((await empty.loadSpeechHotwords('', controller.signal)).hotwords, '')
  const bad = fixture(t, { terms: { terms: [null] } })
  await assert.rejects(bad.loadSpeechHotwords('', controller.signal), /语音术语响应无效/)
})
