import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { setImmediate } from 'node:timers/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import { createVoiceEndpointAdapter } from '../../src/features/chat/voice-endpoint.ts'
import * as voiceResponse from '../../src/features/chat/voice-response-stream.ts'
import { streamingSpeechSegments } from '../../src/features/chat/speech-stream-text.ts'
import * as abortSignal from '../../src/lib/abort-signal.ts'

const source = await readFile('src/features/chat/use-voice-session.ts', 'utf8')
const compiled = transformSync(source, { loader: 'ts', format: 'cjs' }).code
const stateCompiled = transformSync(
  await readFile('src/features/chat/voice-mode-state.ts', 'utf8'),
  { loader: 'ts', format: 'cjs' },
).code

function deferred() {
  let resolve, reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
function eventHost() {
  const listeners = new Map()
  return {
    addEventListener(type, listener) {
      const list = listeners.get(type) ?? new Set()
      list.add(listener)
      listeners.set(type, list)
    },
    removeEventListener(type, listener) {
      listeners.get(type)?.delete(listener)
    },
    emit(type) {
      for (const listener of listeners.get(type) ?? []) listener()
    },
  }
}

// 执行真实 Hook、状态工具与端点适配器，仅替换 React 调度和外部权限/网络/设备。
function fixture(t, settings = {}) {
  const slots = []
  let cursor = 0,
    effects = [],
    dirty = false,
    mounted = true,
    state
  let now = 0,
    nextTimer = 0
  const timers = new Map()
  const schedule = (callback, delay, interval = false) => {
    const id = ++nextTimer
    timers.set(id, { callback, at: now + delay, interval, delay })
    return id
  }
  const window = {
    ...eventHost(),
    __PISPER_MOBILE_APP__: Boolean(settings.platform || settings.android),
    __PISPER_MOBILE_PLATFORM__: settings.platform ?? (settings.android ? 'android' : undefined),
    setTimeout: (callback, delay) => schedule(callback, delay),
    clearTimeout: (id) => timers.delete(id),
    setInterval: (callback, delay) => schedule(callback, delay, true),
    clearInterval: (id) => timers.delete(id),
  }
  const document = { ...eventHost(), hidden: false }
  const react = {
    useRef(value) {
      const index = cursor++
      slots[index] ??= { current: value }
      return slots[index]
    },
    useState(initial) {
      const index = cursor++
      slots[index] ??= { value: initial }
      return [
        slots[index].value,
        (next) => {
          const value = typeof next === 'function' ? next(slots[index].value) : next
          if (!Object.is(value, slots[index].value)) {
            slots[index].value = value
            dirty = true
          }
        },
      ]
    },
    useCallback(callback, deps) {
      const index = cursor++,
        previous = slots[index]
      if (!previous || deps.some((value, key) => !Object.is(value, previous.deps[key])))
        slots[index] = { deps, callback }
      return slots[index].callback
    },
    useEffect(callback, deps) {
      const index = cursor++,
        previous = slots[index]
      if (!previous || deps.some((value, key) => !Object.is(value, previous.deps[key])))
        effects.push(() => {
          previous?.cleanup?.()
          slots[index] = { deps, cleanup: callback() }
        })
    },
  }
  const permission = deferred(),
    setup = deferred(),
    sending = deferred(),
    playback = deferred()
  if (!settings.permissionPending) permission.resolve()
  if (!settings.setupPending) setup.resolve()
  const recognizers = [],
    captures = [],
    endpoints = [],
    sent = [],
    spoken = [],
    errors = [],
    setupSignals = []
  let aborts = 0
  const props = {
    open: true,
    sessionId: 'one',
    messages: [],
    streaming: false,
    ensureReady(signal) {
      setupSignals.push(signal)
      return setup.promise
    },
    sendPrompt(...args) {
      sent.push(args)
      return sending.promise
    },
    onAbort() {
      aborts += 1
      return settings.abortPending?.promise
    },
    async speakText(source, signal, onSpeaking) {
      for await (const text of streamingSpeechSegments(source, signal)) {
        onSpeaking?.()
        spoken.push({ text, signal })
        await new Promise((resolve, reject) => {
          const abort = () => reject(signal.reason)
          signal.addEventListener('abort', abort, { once: true })
          if (signal.aborted) abort()
          playback.promise
            .then(resolve, reject)
            .finally(() => signal.removeEventListener('abort', abort))
        })
      }
    },
    notifyError: (message) => errors.push(message),
    ...settings.props,
  }
  const microphone = {
    VOICE_MAX_DURATION_SECONDS: 60,
    requestMicrophonePermission: () => permission.promise,
    createSpeechRecognizer() {
      const initializing = deferred(),
        finalizing = deferred(),
        disposing = deferred()
      if (!settings.startPending) initializing.resolve()
      if (!settings.finishPending) finalizing.resolve(settings.transcript ?? 'spoken prompt')
      if (!settings.disposePending) disposing.resolve()
      const recognizer = {
        initializing,
        finalizing,
        disposing,
        finishes: 0,
        disposed: 0,
        samples: 0,
        partial: null,
        start: () => initializing.promise,
        acceptPcm(samples) {
          this.samples += samples.length
          return Boolean(settings.full)
        },
        onPartial(callback) {
          this.partial = callback
          return () => {}
        },
        finish() {
          this.finishes += 1
          return finalizing.promise
        },
        dispose() {
          this.disposed += 1
          return disposing.promise
        },
      }
      recognizers.push(recognizer)
      return recognizer
    },
    startMicrophoneCapture(onPcm, signal) {
      const ready = deferred(),
        stopped = deferred()
      if (!settings.stopPending) stopped.resolve()
      const capture = {
        onPcm,
        signal,
        ready,
        stopped,
        stops: 0,
        stop() {
          this.stops += 1
          return stopped.promise
        },
      }
      captures.push(capture)
      if (!settings.capturePending) ready.resolve(capture)
      return ready.promise
    },
  }
  const modules = {
    react,
    '@/app/use-i18n': { useI18n: () => ({ t: (key) => key }) },
    '@/lib/abort-signal': abortSignal,
    './voice-input': microphone,
    './voice-response-stream': voiceResponse,
    './speech-session': {
      loadSpeechHotwords: async () => ({ terms: [], hotwords: '' }),
      prepareSpeechSession: async (_options, signal) => {
        abortSignal.throwIfAborted(signal)
        return { requestId: 'fixture-session', signal }
      },
    },
    './voice-endpoint': {
      async createVoiceEndpoint() {
        const endpoint = createVoiceEndpointAdapter(() => ({
          processFrame: (frame) => (frame[0] ? 1 : 0),
          destroy() {},
        }))
        endpoints.push(endpoint)
        return endpoint
      },
    },
  }
  const load = (code) => {
    const module = { exports: {} }
    runInNewContext(code, {
      module,
      exports: module.exports,
      require(id) {
        assert.ok(modules[id], id)
        return modules[id]
      },
      window,
      document,
      AbortController: settings.legacyAbort
        ? class extends AbortController {
            constructor() {
              super()
              Object.defineProperties(this.signal, {
                throwIfAborted: { value: undefined },
                reason: { get: () => undefined },
              })
            }
          }
        : AbortController,
      DOMException,
      Error,
      Date: class extends Date {
        static now() {
          return now
        }
      },
      performance: { now: () => now },
    })
    return module.exports
  }
  modules['./voice-mode-state'] = load(stateCompiled)
  const hook = load(compiled).useVoiceSession
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
  async function flush() {
    for (let i = 0; i < 3; i += 1) {
      await setImmediate()
      render()
    }
  }
  async function tick(ms) {
    const until = now + ms
    for (;;) {
      const item = [...timers]
        .filter(([, timer]) => timer.at <= until)
        .sort((a, b) => a[1].at - b[1].at)[0]
      if (!item) break
      const [id, timer] = item
      now = timer.at
      if (timer.interval) timer.at += timer.delay
      else timers.delete(id)
      timer.callback()
      await flush()
    }
    now = until
    await flush()
  }
  function unmount() {
    if (!mounted) return
    mounted = false
    for (const slot of slots) slot?.cleanup?.()
  }
  t.after(unmount)
  render()
  return {
    permission,
    setup,
    sending,
    playback,
    recognizers,
    captures,
    endpoints,
    sent,
    spoken,
    errors,
    setupSignals,
    window,
    document,
    flush,
    tick,
    render,
    unmount,
    update(next) {
      Object.assign(props, next)
      render()
    },
    hidden(value = true) {
      document.hidden = value
      document.emit('visibilitychange')
      render()
    },
    voice() {
      captures.at(-1).onPcm(new Float32Array(320 * 10).fill(0.5))
      render()
    },
    silence() {
      captures.at(-1).onPcm(new Float32Array(320 * 45))
      render()
    },
    get state() {
      render()
      return state
    },
    get aborts() {
      return aborts
    },
    get timerCount() {
      return timers.size
    },
  }
}

async function submit(f) {
  await f.flush()
  f.voice()
  f.silence()
  await f.flush()
}
async function reply(f, text = '**complete** reply', role = 'agent') {
  f.update({ messages: [{ id: 'new', role, text, streaming: false }], streaming: false })
  f.sending.resolve()
  await f.flush()
  await f.tick(50)
}

test('automatic VAD submits once, preserves tail PCM, awaits stream completion and playback before re-listening', async (t) => {
  const f = fixture(t)
  await submit(f)
  assert.equal(f.state.stage, 'thinking')
  assert.equal(f.sent.length, 1)
  assert.deepEqual(
    Array.from(f.sent[0]).map((value) => (Array.isArray(value) ? Array.from(value) : value)),
    ['spoken prompt', [], false, false, null],
  )
  assert.equal(f.recognizers[0].samples, 320 * 55)
  assert.equal(f.recognizers[0].finishes, 1)
  assert.equal(f.captures[0].signal.aborted, true)
  f.silence()
  f.update({
    streaming: true,
    messages: [{ id: 'new', role: 'assistant', text: '**complete**', streaming: true }],
  })
  await f.tick(1000)
  assert.equal(f.spoken.length, 0)
  assert.equal(f.captures.length, 1)
  await reply(f)
  assert.equal(f.state.stage, 'speaking')
  assert.equal(f.spoken[0].text, 'complete reply')
  assert.equal(f.captures.length, 1)
  f.playback.resolve()
  await f.flush()
  assert.equal(f.state.stage, 'listening')
  assert.equal(f.captures.length, 2)
  assert.equal(f.setupSignals.length, 1)
  assert.equal(f.sent.length, 1)
})

for (const platform of ['android', 'ios']) {
  for (const stage of ['listening', 'thinking']) {
    test(`${platform} native interruption invalidates ${stage} before future native requests exist`, async (t) => {
      const f = fixture(t, { platform })
      if (stage === 'thinking') await submit(f)
      else await f.flush()
      assert.equal(f.state.stage, stage)
      f.window.emit('pisper:speech-interrupted')
      await f.flush()
      if (stage === 'thinking') await reply(f, 'old response must stay silent')
      else {
        f.voice()
        f.silence()
        await f.flush()
      }
      assert.equal(f.state.stage, 'idle')
      assert.equal(f.sent.length, stage === 'thinking' ? 1 : 0)
      assert.equal(f.spoken.length, 0)
      assert.equal(f.captures.length, 1)
      assert.equal(f.captures[0].signal.aborted, true)
    })
  }
}

test('setup precedes permissions/capture and callback changes do not restart it', async (t) => {
  const f = fixture(t, { setupPending: true })
  await f.flush()
  assert.equal(f.state.stage, 'requesting')
  assert.equal(f.captures.length, 0)
  f.update({ notifyError() {}, sendPrompt: () => f.sending.promise })
  await f.flush()
  assert.equal(f.setupSignals.length, 1)
  f.setup.resolve()
  await f.flush()
  assert.equal(f.captures.length, 1)
  assert.equal(f.state.stage, 'listening')
})

for (const failure of ['setup', 'initialization', 'permission'])
  test(`${failure} failure surfaces without a hot microphone`, async (t) => {
    const f = fixture(t, {
      setupPending: failure === 'setup',
      startPending: failure === 'initialization',
      permissionPending: failure === 'permission',
    })
    await f.flush()
    if (failure === 'setup') f.setup.reject(new Error('setup failed'))
    if (failure === 'initialization') f.recognizers[0].initializing.reject(new Error('init failed'))
    if (failure === 'permission') f.permission.reject(new Error('microphone_permission_denied'))
    await f.flush()
    assert.equal(f.state.stage, 'error')
    assert.equal(f.captures.length, 0)
    assert.equal(f.sent.length, 0)
    if (failure === 'permission') assert.equal(f.state.error, 'chat:voiceInput.permissionDenied')
  })

for (const reason of ['close', 'session', 'mute', 'blur', 'interruption', 'hidden', 'unmount'])
  test(`${reason} cancels pending recognition and ignores late results`, async (t) => {
    const f = fixture(t, { finishPending: true })
    await submit(f)
    assert.equal(f.state.stage, 'transcribing')
    if (reason === 'close') f.update({ open: false })
    if (reason === 'session') f.update({ sessionId: 'two' })
    if (reason === 'mute') f.state.toggleMute()
    if (reason === 'blur') f.window.emit('blur')
    if (reason === 'interruption') f.window.emit('pisper:speech-interrupted')
    if (reason === 'hidden') f.hidden()
    if (reason === 'unmount') f.unmount()
    f.recognizers[0].finalizing.resolve('late')
    f.recognizers[0].partial('late partial')
    await f.flush()
    assert.equal(f.sent.length, 0)
    assert.equal(f.captures[0].signal.aborted, true)
    assert.equal(f.aborts, 0)
    if (reason !== 'unmount') assert.notEqual(f.state.partial, 'late partial')
  })

for (const platform of ['android', 'ios']) {
  test(
    platform +
      ' mobile permission hidden exception only lasts until grant; hidden grant never captures',
    async (t) => {
      const f = fixture(t, { platform, permissionPending: true })
      await f.flush()
      f.hidden()
      f.window.emit('blur')
      assert.equal(f.state.stage, 'requesting')
      f.permission.resolve()
      await f.flush()
      assert.equal(f.state.stage, 'idle')
      assert.equal(f.captures.length, 0)
      f.hidden(false)
      await f.flush()
      assert.equal(f.captures.length, 0)
    },
  )

  test(
    platform +
      ' mobile visible permission grant starts once and explicit close cancels pending grant',
    async (t) => {
      const f = fixture(t, { platform, permissionPending: true })
      await f.flush()
      f.hidden()
      f.window.emit('blur')
      f.hidden(false)
      f.permission.resolve()
      await f.flush()
      assert.equal(f.captures.length, 1)
      const closed = fixture(t, { platform, permissionPending: true })
      await closed.flush()
      closed.hidden()
      closed.state.hangUp()
      closed.hidden(false)
      closed.permission.resolve()
      await closed.flush()
      assert.equal(closed.captures.length, 0)
    },
  )
}

test('setup hidden and desktop pending permission blur are never exempt', async (t) => {
  const f = fixture(t, { android: true, setupPending: true })
  await f.flush()
  f.hidden()
  assert.equal(f.setupSignals[0].aborted, true)
  f.setup.resolve()
  f.hidden(false)
  await f.flush()
  assert.equal(f.captures.length, 0)
  const desktop = fixture(t, { permissionPending: true })
  await desktop.flush()
  desktop.window.emit('blur')
  desktop.permission.resolve()
  await desktop.flush()
  assert.equal(desktop.captures.length, 0)
})

test('late capture is stopped before a replacement starts and stale initializer cannot release new resources', async (t) => {
  const f = fixture(t, { capturePending: true })
  await f.flush()
  f.state.hangUp()
  f.state.toggleMain()
  await f.flush()
  assert.equal(f.captures.length, 1)
  f.captures[0].onPcm(new Float32Array(320).fill(1))
  assert.equal(f.recognizers[0].samples, 0)
  f.captures[0].ready.resolve(f.captures[0])
  await f.flush()
  assert.equal(f.captures[0].stops, 1)
  assert.equal(f.captures.length, 2)
  f.captures[1].ready.resolve(f.captures[1])
  await f.flush()
  assert.equal(f.state.stage, 'listening')
  assert.equal(f.captures[1].signal.aborted, false)
  const init = fixture(t, { startPending: true })
  await init.flush()
  init.state.hangUp()
  init.state.toggleMain()
  await init.flush()
  init.recognizers[1].initializing.resolve()
  await init.flush()
  init.recognizers[0].initializing.reject(new Error('old init failed'))
  await init.flush()
  assert.equal(init.state.stage, 'listening')
  assert.equal(init.captures[0].signal.aborted, false)
})

test('capture shutdown barrier prevents overlapping audio contexts', async (t) => {
  const f = fixture(t, { stopPending: true })
  await f.flush()
  f.state.hangUp()
  f.state.toggleMain()
  await f.flush()
  assert.equal(f.captures.length, 1)
  assert.equal(f.captures[0].signal.aborted, true)
  f.captures[0].stopped.resolve()
  await f.flush()
  assert.equal(f.captures.length, 2)
})

for (const kind of ['limit', 'manual', 'buffer'])
  test(`silence ${kind} stops without recognition or hallucinated prompt loop`, async (t) => {
    const f = fixture(t, { full: kind === 'buffer', transcript: 'hallucination' })
    await f.flush()
    if (kind === 'limit') await f.tick(60_000)
    if (kind === 'manual') {
      await f.state.commitListening()
      await f.flush()
    }
    if (kind === 'buffer') {
      f.silence()
      await f.flush()
    }
    assert.equal(f.state.stage, 'idle')
    assert.equal(f.recognizers[0].finishes, 0)
    assert.equal(f.sent.length, 0)
    assert.equal(f.captures.length, 1)
    assert.equal(f.timerCount, 0)
  })

for (const kind of ['limit', 'buffer'])
  test(`voiced ${kind} commits once even without a pause`, async (t) => {
    const f = fixture(t, { full: kind === 'buffer' })
    await f.flush()
    f.voice()
    if (kind === 'limit') await f.tick(60_000)
    await f.flush()
    assert.equal(f.sent.length, 1)
    assert.equal(f.recognizers[0].finishes, 1)
  })

test('external streaming prevents capture/send and hangup never aborts that run', async (t) => {
  const f = fixture(t, { props: { streaming: true } })
  await f.flush()
  f.state.toggleMain()
  f.state.hangUp()
  await f.flush()
  assert.equal(f.captures.length, 0)
  assert.equal(f.aborts, 0)
  f.update({ streaming: false })
  f.state.toggleMain()
  await f.flush()
  f.update({ streaming: true })
  f.voice()
  f.silence()
  await f.flush()
  assert.equal(f.sent.length, 0)
  assert.equal(f.aborts, 0)
})

test('hangup aborts only the owned unresolved send and late completion never plays', async (t) => {
  const f = fixture(t)
  await submit(f)
  f.state.hangUp()
  f.state.hangUp()
  assert.equal(f.aborts, 1)
  await reply(f)
  assert.equal(f.spoken.length, 0)
  assert.equal(f.captures.length, 1)
  assert.equal(f.state.stage, 'idle')
})

test('interrupt waits for owned abort settlement, then restarts without old send contamination', async (t) => {
  const abortPending = deferred()
  const f = fixture(t, { abortPending })
  await submit(f)
  f.state.interrupt()
  await f.flush()
  assert.equal(f.aborts, 1)
  assert.equal(f.captures.length, 1)
  abortPending.resolve()
  await f.flush()
  assert.equal(f.captures.length, 2)
  await reply(f)
  assert.equal(f.state.stage, 'listening')
  assert.equal(f.captures[1].signal.aborted, false)
  assert.equal(f.spoken.length, 0)
})

for (const reason of ['hangup', 'hidden', 'mute', 'session'])
  test(`${reason} cancels playback signal; late playback cannot reopen old session`, async (t) => {
    const f = fixture(t)
    await submit(f)
    await reply(f)
    assert.equal(f.spoken.length, 1)
    if (reason === 'hangup') f.state.hangUp()
    if (reason === 'hidden') f.hidden()
    if (reason === 'mute') f.state.toggleMute()
    if (reason === 'session') f.update({ sessionId: 'two' })
    assert.equal(f.spoken[0].signal.aborted, true)
    assert.equal(f.aborts, 0)
    f.playback.resolve()
    await f.flush()
    assert.equal(f.captures.length, reason === 'session' ? 2 : 1)
  })

test('SSE swallow-error messages and rejected TTS stop the loop with visible errors', async (t) => {
  const failed = fixture(t)
  await submit(failed)
  failed.update({
    messages: [{ id: 'new', role: 'agent', text: 'failure text', error: 'run failed' }],
  })
  failed.sending.resolve()
  await failed.flush()
  await failed.tick(50)
  assert.equal(failed.state.stage, 'error')
  assert.equal(failed.state.error, 'run failed')
  assert.equal(failed.spoken.length, 0)
  const tts = fixture(t)
  await submit(tts)
  await reply(tts)
  tts.playback.reject(new Error('speaker failed'))
  await tts.flush()
  assert.equal(tts.state.stage, 'error')
  assert.equal(tts.state.error, 'speaker failed')
  assert.equal(tts.captures.length, 1)
})

test('old replies, tools and system messages are not spoken; missing reply becomes error, not silent success', async (t) => {
  const old = { id: 'old', role: 'assistant', text: 'old answer' }
  const f = fixture(t, { props: { messages: [old] } })
  await submit(f)
  f.update({
    messages: [
      old,
      { id: 'tool', role: 'tool', text: 'tool output' },
      { id: 'system', role: 'system', text: 'system output' },
    ],
  })
  f.sending.resolve()
  await f.flush()
  await f.tick(15_000)
  assert.equal(f.state.stage, 'error')
  assert.equal(f.state.error, 'chat:voiceMode.emptyReply')
  assert.equal(f.spoken.length, 0)
  assert.equal(f.captures.length, 1)
})

test('cancel during an in-flight capture stop still blocks the next microphone', async (t) => {
  const f = fixture(t, { stopPending: true })
  await f.flush()
  f.voice()
  f.silence()
  await f.flush()
  assert.equal(f.state.stage, 'transcribing')
  f.state.hangUp()
  f.state.toggleMain()
  await f.flush()
  assert.equal(f.captures.length, 1)
  f.captures[0].stopped.resolve()
  await f.flush()
  assert.equal(f.captures.length, 2)
  assert.equal(f.sent.length, 0)
})

test('capacity during pending capture aborts immediately and still commits only once', async (t) => {
  const f = fixture(t, { capturePending: true, full: true })
  await f.flush()
  f.voice()
  assert.equal(f.captures[0].signal.aborted, true)
  f.captures[0].ready.resolve(f.captures[0])
  await f.flush()
  assert.equal(f.captures[0].stops, 1)
  assert.equal(f.sent.length, 1)
  assert.equal(f.recognizers[0].finishes, 1)
})

test('interrupt resumes after the owned abort updates real streaming state', async (t) => {
  const abortPending = deferred()
  const f = fixture(t, { abortPending })
  await submit(f)
  f.update({ streaming: true })
  f.state.interrupt()
  await f.flush()
  assert.equal(f.aborts, 1)
  assert.equal(f.captures.length, 1)
  f.update({ streaming: false })
  abortPending.resolve()
  await f.flush()
  assert.equal(f.state.stage, 'listening')
  assert.equal(f.captures.length, 2)
})

test('hangup while interrupt awaits abort prevents a delayed microphone restart', async (t) => {
  const abortPending = deferred()
  const f = fixture(t, { abortPending })
  await submit(f)
  f.state.interrupt()
  f.state.hangUp()
  abortPending.resolve()
  await f.flush()
  assert.equal(f.state.stage, 'idle')
  assert.equal(f.captures.length, 1)
})

test('latest send and speaker callbacks are used without restarting capture', async (t) => {
  const f = fixture(t)
  await f.flush()
  let sends = 0,
    speeches = 0
  f.update({
    sendPrompt() {
      sends += 1
      return f.sending.promise
    },
    speakText() {
      speeches += 1
      return f.playback.promise
    },
  })
  f.voice()
  f.silence()
  await f.flush()
  assert.equal(sends, 1)
  assert.equal(f.captures.length, 1)
  await reply(f)
  assert.equal(speeches, 1)
  assert.equal(f.captures.length, 1)
})

test('a newer external prompt is neither aborted nor mistaken for this round reply', async (t) => {
  const f = fixture(t)
  await submit(f)
  const messages = [
    { id: 'ours', role: 'user', text: 'spoken prompt' },
    { id: 'external', role: 'user', text: 'external prompt' },
    { id: 'external-reply', role: 'assistant', text: 'external answer' },
  ]
  f.update({ messages, streaming: true })
  f.state.hangUp()
  assert.equal(f.aborts, 0)
  const waiting = fixture(t)
  await submit(waiting)
  waiting.update({ messages })
  waiting.sending.resolve()
  await waiting.flush()
  await waiting.tick(50)
  assert.equal(waiting.state.stage, 'error')
  assert.equal(waiting.spoken.length, 0)
})

test('send rejection and empty recognition are visible failures without automatic retries', async (t) => {
  const f = fixture(t)
  await submit(f)
  f.sending.reject(new Error('send failed'))
  await f.flush()
  assert.equal(f.state.stage, 'error')
  assert.equal(f.state.error, 'send failed')
  assert.equal(f.aborts, 0)
  const empty = fixture(t, { transcript: ' ' })
  await submit(empty)
  assert.equal(empty.state.stage, 'error')
  assert.equal(empty.sent.length, 0)
  assert.equal(empty.captures.length, 1)
})

test('React reply hydration and streaming flags are awaited after send resolves', async (t) => {
  const f = fixture(t)
  await submit(f)
  f.sending.resolve()
  await f.flush()
  await f.tick(100)
  assert.equal(f.spoken.length, 0)
  f.update({ messages: [{ id: 'a', role: 'assistant', text: 'finish', streaming: true }] })
  await f.tick(100)
  assert.equal(f.spoken.length, 0)
  f.update({
    messages: [{ id: 'a', role: 'assistant', text: 'finished', streaming: false }],
    streaming: true,
  })
  await f.tick(100)
  assert.equal(f.spoken.length, 0)
  f.update({ streaming: false })
  await f.tick(50)
  assert.equal(f.spoken[0].text, 'finished')
})

test('durable transcript IDs replacing completed SSE bubbles do not cancel pending audio', async (t) => {
  const f = fixture(t)
  await submit(f)
  f.update({
    streaming: true,
    messages: [
      { id: 'optimistic-user', role: 'user', text: 'spoken prompt' },
      { id: 'live', role: 'agent', text: '', streaming: true },
    ],
  })
  publish('第一句。', 'completed')
  await f.flush()
  assert.equal(f.spoken.length, 1)
  const signal = f.spoken[0].signal
  f.update({
    streaming: false,
    messages: [
      { id: 'user-persisted', role: 'user', text: 'spoken prompt' },
      { id: 'assistant-persisted', role: 'agent', text: '第一句。', streaming: false },
    ],
  })
  f.sending.resolve()
  await f.flush()
  await f.tick(50)
  assert.equal(signal.aborted, false)
  assert.equal(f.state.stage, 'speaking')
  assert.equal(f.captures.length, 1)
  assert.deepEqual(f.errors, [])
  f.playback.resolve()
  await f.flush()
  assert.equal(f.state.stage, 'listening')
  assert.equal(f.captures.length, 2)
})

test('legacy signals without throwIfAborted or reason still send and cancel streamed speech', async (t) => {
  const f = fixture(t, { legacyAbort: true })
  await submit(f)
  assert.equal(f.sent.length, 1)
  publish('第一句。后')
  await f.flush()
  assert.equal(f.spoken.length, 1)
  f.state.hangUp()
  await f.flush()
  assert.equal(f.spoken[0].signal.aborted, true)
  assert.equal(f.aborts, 1)
  assert.equal(f.state.stage, 'idle')
  assert.deepEqual(f.errors, [])
})

test('a new user during completed audio stops locally before any new response text', async (t) => {
  const f = fixture(t)
  await submit(f)
  const messages = [
    { id: 'ours', role: 'user', text: 'spoken prompt' },
    { id: 'live', role: 'agent', text: '第一句。尾句', streaming: false },
  ]
  f.update({ messages, streaming: false })
  publish('第一句。尾句', 'completed')
  f.sending.resolve()
  await f.flush()
  await f.tick(50)
  assert.equal(f.state.stage, 'speaking')
  const signal = f.spoken[0].signal
  f.update({
    messages: [...messages, { id: 'foreign', role: 'user', text: 'spoken prompt' }],
    streaming: true,
  })
  assert.equal(signal.aborted, true)
  assert.equal(f.aborts, 0)
  f.playback.resolve()
  await f.flush()
  assert.equal(f.spoken.length, 1)
  assert.equal(f.captures.length, 1)
})

test('completed audio survives a transcript metadata load error', async (t) => {
  const f = fixture(t)
  await submit(f)
  publish('第一句。尾句', 'completed')
  f.update({
    messages: [{ id: 'live', role: 'agent', text: '第一句。尾句', streaming: false }],
    streaming: false,
    error: 'transcript load failed',
  })
  f.sending.resolve()
  await f.flush()
  await f.tick(50)
  assert.equal(f.spoken[0].signal.aborted, false)
  f.playback.resolve()
  await f.flush()
  assert.deepEqual(
    f.spoken.map((item) => item.text),
    ['第一句。', '尾句'],
  )
  assert.equal(f.state.stage, 'listening')
  assert.equal(f.aborts, 0)
})

const runStartedAt = '2026-07-17T01:02:03.456Z'
function snapshot(
  f,
  { text = '第一句。后续尾句', streaming = false, startedAt = runStartedAt, error = '' } = {},
) {
  const messages = [
    {
      id: 'user-1784250123457-0',
      role: 'user',
      text: 'spoken prompt',
      timestamp: 1784250123457,
      error: null,
    },
    {
      id: streaming ? 'live-one' : 'assistant-1784250124457-1',
      role: 'agent',
      text,
      streaming,
      timestamp: 1784250124457,
      error: null,
    },
  ]
  voiceResponse.publishVoiceSnapshot('one', { startedAt, streaming, error, messages })
  f.update({ messages, streaming })
}

async function recoveringReply(f) {
  await submit(f)
  f.update({ streaming: true })
  const identity = { messageId: 'agent-1784250123000', runId: 'run-one', startedAt: runStartedAt }
  publish('', 'started', identity)
  publish('第一句。后', 'streaming', identity)
  await f.flush()
  publish('第一句。后', 'recovering', identity)
  f.sending.resolve()
  await f.flush()
}

test('resync keeps cancellation ownership and waits beyond 15 seconds while its snapshot is active', async (t) => {
  const f = fixture(t)
  await recoveringReply(f)
  snapshot(f, { streaming: true })
  await f.tick(20_000)
  assert.equal(f.state.stage, 'speaking')
  assert.equal(f.spoken[0].signal.aborted, false)
  f.state.hangUp()
  assert.equal(f.aborts, 1)
  assert.equal(f.spoken[0].signal.aborted, true)
  snapshot(f)
  f.playback.resolve()
  await f.flush()
  assert.equal(f.spoken.length, 1)
  assert.equal(f.captures.length, 1)
})

test('a run handed to snapshots before its first text remains cancellable during a long thinking phase', async (t) => {
  const f = fixture(t)
  await submit(f)
  publish('', 'started', { runId: 'run-one', startedAt: runStartedAt })
  publish('', 'recovering', { runId: 'run-one', startedAt: runStartedAt })
  voiceResponse.publishVoiceSnapshot('one', {
    startedAt: runStartedAt,
    streaming: true,
    messages: [{ id: 'message-0', role: 'user', text: 'spoken prompt' }],
  })
  f.sending.resolve()
  await f.flush()
  await f.tick(20_000)
  assert.equal(f.state.stage, 'thinking')
  assert.equal(f.spoken.length, 0)
  f.state.hangUp()
  assert.equal(f.aborts, 1)
  assert.equal(f.captures.length, 1)
})

test('same-run live and durable IDs append and drain the final tail exactly once after resync', async (t) => {
  const f = fixture(t)
  await recoveringReply(f)
  snapshot(f, { streaming: true })
  await f.tick(20_000)
  snapshot(f)
  snapshot(f)
  await f.tick(50)
  assert.equal(f.spoken[0].signal.aborted, false)
  assert.equal(f.captures.length, 1)
  f.playback.resolve()
  await f.flush()
  assert.deepEqual(
    f.spoken.map((item) => item.text),
    ['第一句。', '后续尾句'],
  )
  assert.equal(f.state.stage, 'listening')
  assert.equal(f.aborts, 0)
})

for (const kind of ['foreign-run', 'missing-identity', 'rewrite', 'run-failed']) {
  test(`resync ${kind} cannot supply speech merely by matching the prompt`, async (t) => {
    const f = fixture(t)
    await recoveringReply(f)
    snapshot(f, {
      startedAt:
        kind === 'foreign-run'
          ? '2026-07-17T01:02:04.456Z'
          : kind === 'missing-identity'
            ? null
            : runStartedAt,
      text: kind === 'rewrite' ? '被改写的回答。' : '第一句。后续尾句',
      error: kind === 'run-failed' ? 'model failed' : '',
    })
    await f.flush()
    assert.equal(f.spoken[0].signal.aborted, true)
    assert.equal(f.state.stage, 'error')
    assert.equal(f.aborts, kind === 'rewrite' ? 1 : 0)
    f.playback.resolve()
    await f.flush()
    assert.equal(f.spoken.length, 1)
    assert.equal(f.captures.length, 1)
  })
}

test('a same-run snapshot with a queued identical prompt stops before feeding its foreign tail', async (t) => {
  const f = fixture(t)
  await recoveringReply(f)
  voiceResponse.publishVoiceSnapshot('one', {
    startedAt: runStartedAt,
    streaming: true,
    messages: [
      { id: 'message-0', role: 'user', text: 'spoken prompt' },
      { id: 'message-1', role: 'user', text: 'spoken prompt' },
      { id: 'live-one', role: 'agent', text: '第一句。后续外来回答。', streaming: true },
    ],
  })
  assert.equal(f.spoken[0].signal.aborted, true)
  assert.equal(f.aborts, 0)
  f.playback.resolve()
  await f.flush()
  assert.equal(f.spoken.length, 1)
})

for (const source of ['SSE', 'snapshot']) {
  test(`${source} new run without text stops completed audio even with the same prompt`, async (t) => {
    const f = fixture(t)
    await recoveringReply(f)
    snapshot(f)
    await f.tick(50)
    const signal = f.spoken[0].signal
    assert.equal(signal.aborted, false)
    if (source === 'SSE') publish('', 'started', { messageId: 'another-agent', runId: 'run-two' })
    else snapshot(f, { text: '', streaming: true, startedAt: '2026-07-17T02:00:00.000Z' })
    assert.equal(signal.aborted, true)
    assert.equal(f.aborts, 0)
    f.playback.resolve()
    await f.flush()
    assert.equal(f.spoken.length, 1)
  })
}

function publish(text, status = 'streaming', extra = {}) {
  voiceResponse.publishVoiceResponse({
    sessionId: 'one',
    messageId: 'live',
    prompt: 'spoken prompt',
    text,
    status,
    ...extra,
  })
}

test('direct SSE sentences play before send finishes or the typewriter renders, then EOF tail drains before listening', async (t) => {
  const f = fixture(t)
  await submit(f)
  f.update({
    streaming: true,
    messages: [{ id: 'live', role: 'agent', text: '', streaming: true }],
  })
  publish('第一句。后')
  await f.flush()
  assert.deepEqual(
    f.spoken.map((item) => item.text),
    ['第一句。'],
  )
  assert.equal(f.state.stage, 'speaking')
  assert.equal(f.captures.length, 1)
  f.playback.resolve()
  await f.flush()
  assert.equal(f.captures.length, 1)
  publish('第一句。后续尾句', 'completed')
  await f.flush()
  assert.deepEqual(
    f.spoken.map((item) => item.text),
    ['第一句。', '后续尾句'],
  )
  assert.equal(f.captures.length, 1)
  f.update({
    streaming: false,
    messages: [{ id: 'live', role: 'agent', text: '第一句。后续尾句', streaming: false }],
  })
  f.sending.resolve()
  await f.flush()
  await f.tick(50)
  assert.equal(f.state.stage, 'listening')
  assert.equal(f.captures.length, 2)
})

for (const reason of ['hangup', 'native-interruption', 'error', 'rewrite', 'foreign-prompt']) {
  test(`SSE ${reason} invalidates playback and never reads late text`, async (t) => {
    const f = fixture(t)
    await submit(f)
    f.update({
      streaming: true,
      messages: [{ id: 'live', role: 'agent', text: '', streaming: true }],
    })
    publish('第一句。后')
    await f.flush()
    assert.equal(f.spoken.length, 1)
    const signal = f.spoken[0].signal
    if (reason === 'hangup') f.state.hangUp()
    else if (reason === 'native-interruption') f.window.emit('pisper:speech-interrupted')
    else if (reason === 'error') publish('第一句。后', 'failed', { error: 'stream failed' })
    else if (reason === 'rewrite') publish('changed response')
    else publish('第一句。后', 'streaming', { prompt: 'another user prompt' })
    await f.flush()
    assert.equal(signal.aborted, true)
    f.playback.resolve()
    publish('第一句。后续不能朗读', 'completed')
    f.sending.resolve()
    await f.tick(100)
    assert.equal(f.spoken.length, 1)
    assert.equal(f.captures.length, 1)
    assert.equal(
      f.state.stage,
      ['hangup', 'native-interruption'].includes(reason) ? 'idle' : 'error',
    )
  })
}

test('SSE snapshots deduplicate and unrelated sessions cannot enter the speech source', async (t) => {
  const f = fixture(t)
  await submit(f)
  f.update({
    streaming: true,
    messages: [{ id: 'live', role: 'agent', text: '', streaming: true }],
  })
  publish('Wrong session.', 'completed', { sessionId: 'other' })
  publish('第一句。后')
  publish('第一句。后')
  await f.flush()
  assert.deepEqual(
    f.spoken.map((item) => item.text),
    ['第一句。'],
  )
  f.playback.resolve()
  publish('第一句。后续', 'completed')
  await f.flush()
  assert.deepEqual(
    f.spoken.map((item) => item.text),
    ['第一句。', '后续'],
  )
})
