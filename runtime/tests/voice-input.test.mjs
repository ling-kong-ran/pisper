import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { setImmediate } from 'node:timers/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import ts from '@ts-morph/common/dist/typescript.js'
import * as abortSignal from '../../src/lib/abort-signal.ts'
import * as speechTerms from '../../shared/speech-terms.mjs'

const compile = async (path) =>
  ts.transpileModule(await readFile(path, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText
const inputCode = await compile('src/features/chat/voice-input.ts')
const hookCode = await compile('src/features/chat/use-voice-session.ts')
function deferred() {
  let resolve, reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
async function flush() {
  for (let i = 0; i < 4; i++) await setImmediate()
}
function load(code, modules, globals) {
  const module = { exports: {} }
  runInNewContext(code, {
    module,
    exports: module.exports,
    require: (id) => {
      assert.ok(modules[id], id)
      return modules[id]
    },
    AbortController,
    DOMException,
    Error,
    Float32Array,
    Uint8Array,
    ArrayBuffer,
    ...globals,
  })
  return module.exports
}
function inputFixture(t, settings = {}) {
  const calls = [],
    timers = new Map()
  const ready = deferred()
  const lease = new AbortController()
  let leaseParent
  const modules = {
    '@/lib/http': { waitForMobileRuntimeReady: async () => {} },
    '@/lib/abort-signal': abortSignal,
    '@shared/speech-terms.mjs': speechTerms,
    './speech-session': {
      loadSpeechHotwords: async (sessionId, signal) => {
        calls.push({ type: 'terms', sessionId, signal })
        if (settings.termsError) throw settings.termsError
        const terms = settings.terms ?? ['useEffect']
        return { terms, hotwords: speechTerms.speechHotwords(terms) }
      },
      prepareSpeechSession: async (options, signal) => {
        calls.push({ type: 'prepare', options, signal })
        leaseParent = signal
        signal.addEventListener('abort', () => lease.abort(signal.reason), { once: true })
        await ready.promise
        abortSignal.throwIfAborted(signal)
        return { requestId: 'lease', signal: lease.signal }
      },
    },
  }
  const { createSpeechRecognizer } = load(inputCode, modules, {
    crypto: { randomUUID: () => 'recognition-request' },
    btoa,
    window: {
      __PISPER_MOBILE_APP__: settings.native,
      __TAURI__: {
        core: {
          invoke: async (command, args) => {
            calls.push({ type: command, args })
            return { text: 'use effect' }
          },
        },
      },
      setInterval: (callback) => {
        const id = timers.size + 1
        timers.set(id, callback)
        return id
      },
      clearInterval: (id) => timers.delete(id),
    },
    fetch: async (path, options) => {
      calls.push({ type: path, options })
      if (path === '/api/speech/stream/start') {
        if (settings.startError) throw settings.startError
        return settings.legacy
          ? new Response('', { status: 404 })
          : Response.json({ id: 'asr-stream' })
      }
      return Response.json({ text: 'final text' })
    },
  })
  const recognizer = createSpeechRecognizer({ chatSessionId: 'chat-selected' })
  t.after(() => recognizer.dispose())
  return { recognizer, calls, timers, ready, lease, parent: () => leaseParent }
}

for (const native of [false, true])
  test(`${native ? 'native' : 'desktop'} start awaits ASR readiness with actual hotwords and holds until dispose`, async (t) => {
    const f = inputFixture(t, { native })
    let started = false
    const starting = f.recognizer.start().then(() => {
      started = true
    })
    await flush()
    assert.equal(started, false)
    assert.deepEqual(
      f.calls.map((call) => call.type),
      ['terms', 'prepare'],
    )
    assert.equal(f.calls[0].sessionId, 'chat-selected')
    assert.equal(f.calls[1].options.hotwords, 'use effect')
    assert.deepEqual([...f.calls[1].options.kinds], ['asr'])
    f.ready.resolve()
    await starting
    f.recognizer.acceptPcm(new Float32Array([0.25]))
    assert.equal(await f.recognizer.finish(), native ? 'useEffect' : 'final text')
    assert.equal(
      f.parent().aborted,
      false,
      'finishing inference is not disposal of the voice session',
    )
    if (native)
      assert.equal(
        f.calls.find((call) => call.type === 'mobile_transcribe_pcm').args.hotwords,
        'use effect',
      )
    await f.recognizer.dispose()
    assert.equal(f.parent().aborted, true)
  })

test('ordinary input preserves an explicitly empty hotword selection', async (t) => {
  const f = inputFixture(t, { native: true, terms: [] })
  const starting = f.recognizer.start()
  await flush()
  assert.equal(f.calls[1].options.hotwords, '')
  f.ready.resolve()
  await starting
})

test('cancelling during prewarm cannot start a late desktop stream', async (t) => {
  const f = inputFixture(t)
  const starting = f.recognizer.start()
  const rejected = assert.rejects(starting, { name: 'AbortError' })
  await flush()
  await f.recognizer.cancel()
  assert.equal(f.parent().aborted, true)
  f.ready.resolve()
  await rejected
  assert.deepEqual(
    f.calls.map((call) => call.type),
    ['terms', 'prepare'],
  )
  assert.equal(f.timers.size, 0)
})

test('ASR startup failures release an already acquired prewarm lease', async (t) => {
  const f = inputFixture(t, { startError: new Error('stream unavailable') })
  const starting = f.recognizer.start()
  const rejected = assert.rejects(starting, /stream unavailable/)
  f.ready.resolve()
  await rejected
  assert.equal(f.parent().aborted, true)
})

test('unexpected lease interruption cancels recognition and its existing desktop ASR stream', async (t) => {
  const f = inputFixture(t)
  const starting = f.recognizer.start()
  f.ready.resolve()
  await starting
  f.lease.abort(new Error('connection lost'))
  await flush()
  assert.equal(f.parent().aborted, true)
  assert.ok(f.calls.some((call) => call.type === '/api/speech/stream/cancel'))
  await assert.rejects(f.recognizer.finish())
})

test('the legacy one-shot fallback still owns an ASR lease until cancellation', async (t) => {
  const f = inputFixture(t, { legacy: true })
  const starting = f.recognizer.start()
  f.ready.resolve()
  await starting
  f.recognizer.acceptPcm(new Float32Array([0.5]))
  assert.equal(await f.recognizer.finish(), 'final text')
  assert.equal(f.parent().aborted, false)
  assert.ok(f.calls.some((call) => call.type === '/api/speech/transcribe'))
  await f.recognizer.cancel()
  assert.equal(f.parent().aborted, true)
})

function eventHost() {
  const listeners = new Map()
  return {
    addEventListener(type, listener) {
      const set = listeners.get(type) ?? new Set()
      set.add(listener)
      listeners.set(type, set)
    },
    removeEventListener(type, listener) {
      listeners.get(type)?.delete(listener)
    },
    emit(type) {
      for (const listener of listeners.get(type) ?? []) listener()
    },
  }
}

// 执行生产 Hook；只替换 React 调度、外部语音设备及模型服务边界。
function hookFixture(t, settings = {}) {
  const slots = [],
    leases = [],
    calls = [],
    timers = new Map()
  let cursor = 0,
    effects = [],
    dirty = false,
    mounted = true,
    state,
    nextTimer = 0
  const react = {
    useRef(value) {
      const i = cursor++
      slots[i] ??= { current: value }
      return slots[i]
    },
    useState(value) {
      const i = cursor++
      slots[i] ??= { value }
      return [
        slots[i].value,
        (next) => {
          const value = typeof next === 'function' ? next(slots[i].value) : next
          if (!Object.is(value, slots[i].value)) {
            slots[i].value = value
            dirty = true
          }
        },
      ]
    },
    useCallback(callback, deps) {
      const i = cursor++,
        previous = slots[i]
      if (!previous || deps.some((value, key) => !Object.is(value, previous.deps[key])))
        slots[i] = { callback, deps }
      return slots[i].callback
    },
    useEffect(callback, deps) {
      const i = cursor++,
        previous = slots[i]
      if (!previous || deps.some((value, key) => !Object.is(value, previous.deps[key])))
        effects.push(() => {
          previous?.cleanup?.()
          slots[i] = { deps, cleanup: callback() }
        })
    },
  }
  const window = {
    ...eventHost(),
    __PISPER_MOBILE_APP__: Boolean(settings.native),
    setTimeout: (callback) => {
      const id = ++nextTimer
      timers.set(id, callback)
      return id
    },
    clearTimeout: (id) => timers.delete(id),
    setInterval: (callback) => {
      const id = ++nextTimer
      timers.set(id, callback)
      return id
    },
    clearInterval: (id) => timers.delete(id),
  }
  const document = { ...eventHost(), hidden: false }
  const modules = {
    react,
    '@/app/use-i18n': { useI18n: () => ({ t: (key) => key }) },
    '@/lib/abort-signal': abortSignal,
    './speech-session': {
      loadSpeechHotwords: async (sessionId, signal) => {
        calls.push({ type: 'terms', sessionId, signal })
        return { hotwords: 'use effect' }
      },
      prepareSpeechSession: async (options, signal) => {
        calls.push({ type: 'prepare', options, signal })
        leases.push({ options, signal })
        await settings.preparing?.promise
        abortSignal.throwIfAborted(signal)
        return { requestId: `lease-${leases.length}`, signal }
      },
    },
    './voice-input': {
      VOICE_MAX_DURATION_SECONDS: 60,
      requestMicrophonePermission: async () => {
        calls.push({ type: 'permission' })
      },
      createSpeechRecognizer: () => ({
        start: async () => {
          calls.push({ type: 'recognizer-start' })
        },
        onPartial: () => () => {},
        finish: async () => 'recognized prompt',
        dispose: async () => {
          calls.push({ type: 'recognizer-dispose' })
        },
      }),
      startMicrophoneCapture: async () => {
        calls.push({ type: 'microphone' })
        return { stop: async () => {} }
      },
    },
    './voice-endpoint': { createVoiceEndpoint: async () => ({ hasSpeech: true, dispose() {} }) },
    './voice-response-stream': {
      createVoiceTextStream: () => ({ update() {}, finish() {} }),
      subscribeVoiceResponse: () => () => {},
    },
    './voice-mode-state': {
      createLevelSmoother: () => ({ push: (value) => value }),
      pcmLevel: () => 0,
    },
  }
  const { useVoiceSession: hook } = load(hookCode, modules, {
    window,
    document,
    performance: { now: () => 0 },
  })
  let props = {
    open: true,
    sessionId: 'chat-a',
    messages: [],
    streaming: settings.streaming,
    sendPrompt: async () => {},
    onAbort: async () => {},
    ensureReady: async (signal) => {
      calls.push({ type: 'install', signal })
      await settings.installing?.promise
      abortSignal.throwIfAborted(signal)
    },
    speakText: (_text, signal) =>
      new Promise((resolve) => signal.addEventListener('abort', resolve, { once: true })),
  }
  function render() {
    if (!mounted) return
    do {
      dirty = false
      cursor = 0
      effects = []
      state = hook(props)
      for (const effect of effects) effect()
    } while (dirty)
  }
  async function settle() {
    for (let i = 0; i < 5; i++) {
      await flush()
      render()
    }
  }
  function unmount() {
    mounted = false
    for (const slot of slots) slot?.cleanup?.()
  }
  t.after(async () => {
    unmount()
    await flush()
  })
  render()
  return {
    calls,
    leases,
    window,
    document,
    settle,
    unmount,
    get state() {
      return state
    },
    update(next) {
      props = { ...props, ...next }
      render()
    },
  }
}

for (const native of [false, true])
  test(`${native ? 'native' : 'desktop'} conversation warms both models after installation and before the first microphone`, async (t) => {
    const preparing = deferred(),
      installing = deferred()
    const f = hookFixture(t, { native, preparing, installing })
    await f.settle()
    assert.deepEqual(
      f.calls.map((call) => call.type),
      ['install'],
    )
    installing.resolve()
    await f.settle()
    assert.deepEqual(
      f.calls.map((call) => call.type),
      ['install', 'terms', 'prepare'],
    )
    assert.deepEqual([...f.leases[0].options.kinds], ['asr', 'tts'])
    assert.equal(f.leases[0].options.hotwords, 'use effect')
    preparing.resolve()
    await f.settle()
    assert.equal(f.state.stage, 'listening')
    assert.deepEqual(
      f.calls.map((call) => call.type),
      ['install', 'terms', 'prepare', 'permission', 'recognizer-start', 'microphone'],
    )
    f.unmount()
    assert.equal(f.leases[0].signal.aborted, true)
  })

test('entering during an external text run still warms both models without opening the microphone', async (t) => {
  const f = hookFixture(t, { streaming: true })
  await f.settle()
  assert.equal(f.leases.length, 1)
  assert.equal(f.leases[0].signal.aborted, false)
  assert.equal(f.state.stage, 'idle')
  assert.ok(!f.calls.some((call) => call.type === 'permission' || call.type === 'microphone'))
  f.update({ streaming: false })
  f.state.toggleMain()
  await f.settle()
  assert.equal(f.state.stage, 'listening')
  assert.equal(f.leases.length, 1)
})

test('conversation lease survives mute, resumed listening, thinking and round interruption', async (t) => {
  const f = hookFixture(t)
  await f.settle()
  const lease = f.leases[0]
  f.state.toggleMute()
  await f.settle()
  assert.equal(f.state.stage, 'idle')
  assert.equal(lease.signal.aborted, false)
  f.state.toggleMute()
  await f.settle()
  assert.equal(f.state.stage, 'listening')
  assert.equal(f.leases.length, 1)
  const committing = f.state.commitListening()
  await f.settle()
  assert.equal(f.state.stage, 'thinking')
  assert.equal(lease.signal.aborted, false)
  f.state.interrupt()
  await f.settle()
  await committing
  assert.equal(f.state.stage, 'listening')
  assert.equal(f.leases.length, 1)
  assert.equal(lease.signal.aborted, false)
})

for (const ending of ['hangUp', 'background', 'interruption', 'close', 'session switch', 'unmount'])
  test(`conversation ${ending} releases its lease and later starts can reacquire`, async (t) => {
    const f = hookFixture(t)
    await f.settle()
    const first = f.leases[0]
    if (ending === 'hangUp') f.state.hangUp()
    if (ending === 'background') {
      f.document.hidden = true
      f.document.emit('visibilitychange')
    }
    if (ending === 'interruption') f.window.emit('pisper:speech-interrupted')
    if (ending === 'close') f.update({ open: false })
    if (ending === 'session switch') f.update({ sessionId: 'chat-b' })
    if (ending === 'unmount') f.unmount()
    await f.settle()
    assert.equal(first.signal.aborted, true)
    if (ending === 'unmount') return
    if (ending === 'close') f.update({ open: true })
    else if (ending !== 'session switch') {
      f.document.hidden = false
      f.state.toggleMain()
    }
    await f.settle()
    assert.equal(f.state.stage, 'listening')
    assert.equal(f.leases.length, 2)
    assert.equal(f.leases[1].signal.aborted, false)
    if (ending === 'session switch')
      assert.equal(f.calls.filter((call) => call.type === 'terms').at(-1).sessionId, 'chat-b')
  })

test('background during preparation cancels the session owner and cannot open a late microphone', async (t) => {
  const preparing = deferred()
  const f = hookFixture(t, { preparing })
  await f.settle()
  f.document.hidden = true
  f.document.emit('visibilitychange')
  assert.equal(f.leases[0].signal.aborted, true)
  preparing.resolve()
  await f.settle()
  assert.equal(f.state.stage, 'idle')
  assert.ok(!f.calls.some((call) => call.type === 'microphone'))
  f.document.hidden = false
  f.state.toggleMain()
  await f.settle()
  assert.equal(f.leases.length, 2)
  assert.equal(f.state.stage, 'listening')
})
