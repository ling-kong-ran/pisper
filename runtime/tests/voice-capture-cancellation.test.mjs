import assert from 'node:assert/strict'
import { setImmediate } from 'node:timers/promises'
import test from 'node:test'
import { startMicrophoneCapture } from '../../src/features/chat/voice-input.ts'

function deferred() {
  let resolve
  let reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

function fixture(t, options = {}) {
  const previous = new Map(
    ['window', 'navigator', 'AudioWorkletNode'].map((key) => [
      key,
      Object.getOwnPropertyDescriptor(globalThis, key),
    ]),
  )
  const controller = new AbortController()
  const media = deferred()
  const worklet = deferred()
  const resume = deferred()
  const closing = deferred()
  const contexts = []
  const processors = []
  const samples = []
  const interruptions = []
  let requests = 0
  const tracks = Array.from({ length: 2 }, (_, index) =>
    Object.assign(new EventTarget(), {
      stops: 0,
      readyState: options.endedInitially && index === 0 ? 'ended' : 'live',
      stop() {
        this.stops += 1
      },
    }),
  )
  const stream = { getTracks: () => tracks }
  const node = () => ({
    connections: 0,
    disconnections: 0,
    connect() {
      this.connections += 1
    },
    disconnect() {
      this.disconnections += 1
    },
  })
  class FakeAudioContext {
    constructor() {
      if (options.constructorFailure) throw new Error('context creation failed')
      this.closes = 0
      this.resumes = 0
      this.modules = 0
      this.sources = []
      this.gains = []
      this.destination = {}
      this.audioWorklet = {
        addModule: () => {
          this.modules += 1
          return worklet.promise
        },
      }
      contexts.push(this)
    }
    createMediaStreamSource(value) {
      assert.equal(value, stream)
      const source = node()
      this.sources.push(source)
      return source
    }
    createGain() {
      const gain = { ...node(), gain: { value: 1 } }
      this.gains.push(gain)
      return gain
    }
    resume() {
      this.resumes += 1
      return resume.promise
    }
    close() {
      this.closes += 1
      return closing.promise
    }
  }
  class FakeAudioWorkletNode {
    constructor() {
      Object.assign(this, node())
      this.port = {
        onmessage: null,
        closes: 0,
        close() {
          this.closes += 1
        },
      }
      processors.push(this)
    }
  }
  Object.defineProperties(globalThis, {
    window: {
      configurable: true,
      value: {
        AudioContext: FakeAudioContext,
        dispatchEvent: (event) => {
          interruptions.push(event.type)
          return true
        },
      },
    },
    navigator: {
      configurable: true,
      value: {
        mediaDevices: {
          getUserMedia() {
            requests += 1
            return media.promise
          },
        },
      },
    },
    AudioWorkletNode: { configurable: true, value: FakeAudioWorkletNode },
  })
  if (!options.mediaPending) media.resolve(stream)
  if (!options.workletPending) worklet.resolve()
  if (!options.resumePending) resume.resolve()
  if (!options.closePending) closing.resolve()
  t.after(() => {
    controller.abort()
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor)
      else delete globalThis[key]
    }
  })
  return {
    controller,
    media,
    worklet,
    resume,
    closing,
    contexts,
    processors,
    tracks,
    stream,
    samples,
    interruptions,
    onPcm: (pcm) => samples.push(Array.from(pcm)),
    requests: () => requests,
  }
}

function assertStopped(f) {
  for (const track of f.tracks) assert.equal(track.stops, 1)
  for (const context of f.contexts) {
    assert.equal(context.closes, 1)
    for (const node of [...context.sources, ...context.gains]) {
      assert.equal(node.disconnections, 1)
    }
  }
  for (const processor of f.processors) {
    assert.equal(processor.disconnections, 1)
    assert.equal(processor.port.closes, 1)
    assert.equal(processor.port.onmessage, null)
  }
}

for (const reason of ['ended', 'mute']) {
  for (const phase of ['worklet', 'resume', 'ready']) {
    test(`${reason} during ${phase} stops hardware and invalidates the shared round even without an external signal`, async (t) => {
      const f = fixture(t, { [`${phase}Pending`]: phase !== 'ready' })
      const starting = startMicrophoneCapture(f.onPcm)
      await setImmediate()
      const rejection = phase === 'ready' ? null : assert.rejects(starting, { name: 'AbortError' })
      f.tracks[0].dispatchEvent(new Event(reason))
      assertStopped(f)
      assert.deepEqual(f.interruptions, ['pisper:speech-interrupted'])
      f.tracks[1].dispatchEvent(new Event(reason))
      assert.equal(f.interruptions.length, 1)
      if (phase === 'ready') await (await starting).stop()
      else {
        f[phase].resolve()
        await rejection
      }
      assertStopped(f)
      assert.deepEqual(f.samples, [])
    })
  }
}

test('an already ended stream cannot initialize an audio context or leak listeners on remaining tracks', async (t) => {
  const f = fixture(t, { endedInitially: true })
  await assert.rejects(startMicrophoneCapture(f.onPcm), { name: 'AbortError' })
  assertStopped(f)
  assert.equal(f.contexts.length, 0)
  f.tracks[1].dispatchEvent(new Event('mute'))
  assert.deepEqual(f.interruptions, ['pisper:speech-interrupted'])
})

test('normal capture cleanup does not publish a hardware interruption', async (t) => {
  const f = fixture(t)
  const capture = await startMicrophoneCapture(f.onPcm)
  await capture.stop()
  for (const track of f.tracks) track.dispatchEvent(new Event('ended'))
  assert.deepEqual(f.interruptions, [])
  assertStopped(f)
})

test('pre-aborted capture never requests microphone access', async (t) => {
  const f = fixture(t)
  f.controller.abort()
  await assert.rejects(startMicrophoneCapture(f.onPcm, f.controller.signal), { name: 'AbortError' })
  assert.equal(f.requests(), 0)
  assert.equal(f.contexts.length, 0)
})

test('abort during getUserMedia stops a late stream before creating an audio context', async (t) => {
  const f = fixture(t, { mediaPending: true })
  const starting = startMicrophoneCapture(f.onPcm, f.controller.signal)
  const rejected = assert.rejects(starting, { name: 'AbortError' })
  assert.equal(f.requests(), 1)
  f.controller.abort()
  await setImmediate()
  assert.equal(f.contexts.length, 0)
  // 系统尚未返回流时没有可关闭的 tracks；授权结果一到达就必须释放，不能继续初始化。
  assert.deepEqual(
    f.tracks.map((track) => track.stops),
    [0, 0],
  )
  f.media.resolve(f.stream)
  await rejected
  assertStopped(f)
  assert.equal(f.contexts.length, 0)
  assert.deepEqual(f.samples, [])
})

for (const phase of ['worklet', 'resume']) {
  test(`abort while ${phase} is pending closes tracks and context before initialization settles`, async (t) => {
    const f = fixture(t, { [`${phase}Pending`]: true })
    const starting = startMicrophoneCapture(f.onPcm, f.controller.signal)
    const rejected = assert.rejects(starting, { name: 'AbortError' })
    await setImmediate()
    assert.equal(f.contexts.length, 1)
    const context = f.contexts[0]
    assert.equal(context.modules, 1)
    assert.equal(context.resumes, phase === 'resume' ? 1 : 0)
    const lateMessage = f.processors[0]?.port.onmessage
    f.controller.abort()
    assertStopped(f)
    lateMessage?.({ data: new Float32Array([0.5]).buffer })
    assert.deepEqual(f.samples, [])
    f[phase].resolve()
    await rejected
    assertStopped(f)
    assert.equal(context.resumes, phase === 'resume' ? 1 : 0)
    assert.equal(context.sources.length, phase === 'resume' ? 1 : 0)
  })
}

test('abort of an already running capture stops immediately and rejects queued PCM', async (t) => {
  const f = fixture(t)
  const capture = await startMicrophoneCapture(f.onPcm, f.controller.signal)
  const message = f.processors[0].port.onmessage
  message({ data: new Float32Array([0.25, 0.5]).buffer })
  assert.deepEqual(f.samples, [[0.25, 0.5]])
  f.controller.abort()
  assertStopped(f)
  message({ data: new Float32Array([1]).buffer })
  await capture.stop()
  await capture.stop()
  assertStopped(f)
  assert.deepEqual(f.samples, [[0.25, 0.5]])
})

test('normal stop without a signal preserves capture behavior and is idempotent', async (t) => {
  const f = fixture(t)
  const capture = await startMicrophoneCapture(f.onPcm)
  const context = f.contexts[0]
  assert.equal(context.resumes, 1)
  assert.equal(context.closes, 0)
  assert.equal(context.gains[0].gain.value, 0)
  f.processors[0].port.onmessage({ data: new Float32Array([-0.25]).buffer })
  assert.deepEqual(f.samples, [[-0.25]])
  const stopping = capture.stop()
  assertStopped(f)
  await stopping
  await capture.stop()
  assertStopped(f)
})

test('normal stop detaches the abort listener and later abort does not close twice', async (t) => {
  const f = fixture(t)
  const signal = f.controller.signal
  const removeEventListener = signal.removeEventListener.bind(signal)
  let removals = 0
  signal.removeEventListener = (type, callback, options) => {
    if (type === 'abort') removals += 1
    removeEventListener(type, callback, options)
  }
  const capture = await startMicrophoneCapture(f.onPcm, signal)
  await capture.stop()
  assert.equal(removals, 1)
  f.controller.abort()
  assert.equal(removals, 1)
  assertStopped(f)
})

test('abort does not wait for AudioContext.close before stopping all microphone tracks', async (t) => {
  const f = fixture(t, { closePending: true })
  const capture = await startMicrophoneCapture(f.onPcm, f.controller.signal)
  f.controller.abort()
  assertStopped(f)
  let settled = false
  const stopping = capture.stop().then(() => {
    settled = true
  })
  await setImmediate()
  assert.equal(settled, false)
  f.closing.resolve()
  await stopping
  assertStopped(f)
})

test('AudioContext constructor failure still releases the acquired microphone', async (t) => {
  const f = fixture(t, { constructorFailure: true })
  await assert.rejects(
    startMicrophoneCapture(f.onPcm, f.controller.signal),
    /context creation failed/,
  )
  assertStopped(f)
  assert.equal(f.contexts.length, 0)
})

for (const phase of ['worklet', 'resume']) {
  test(`${phase} setup failure releases capture resources without a signal`, async (t) => {
    const f = fixture(t, { [`${phase}Pending`]: true })
    const starting = startMicrophoneCapture(f.onPcm)
    const rejected = assert.rejects(starting, /initialization failed/)
    await setImmediate()
    f[phase].reject(new Error('initialization failed'))
    await rejected
    assertStopped(f)
  })
}
