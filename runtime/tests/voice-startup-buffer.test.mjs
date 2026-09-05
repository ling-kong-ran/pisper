import assert from 'node:assert/strict'
import { setImmediate } from 'node:timers/promises'
import test from 'node:test'
import {
  createSpeechRecognizer,
  VOICE_MAX_DURATION_SECONDS,
  VOICE_SAMPLE_RATE,
} from '../../src/features/chat/voice-input.ts'

function deferred() {
  let resolve
  let reject
  const promise = new Promise((onResolve, onReject) => {
    resolve = onResolve
    reject = onReject
  })
  return { promise, resolve, reject }
}

function fixture(t) {
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  const previousFetch = globalThis.fetch
  const startResponse = deferred()
  const startRequested = deferred()
  const calls = []
  const timers = new Map()
  let nextTimer = 0
  let timerStarts = 0
  globalThis.window = {
    __PISPER_MOBILE_APP__: false,
    setInterval(callback) {
      const id = ++nextTimer
      timerStarts += 1
      timers.set(id, callback)
      return id
    },
    clearInterval(id) {
      timers.delete(id)
    },
  }
  globalThis.fetch = async (url, options = {}) => {
    calls.push({ url, ...options })
    if (url === '/api/speech/stream/start') {
      startRequested.resolve()
      return startResponse.promise
    }
    if (url === '/api/speech/stream/chunk') return Response.json({ text: 'partial' })
    if (url === '/api/speech/stream/finish') return Response.json({ text: ' final text ' })
    if (url === '/api/speech/transcribe') return Response.json({ text: ' legacy text ' })
    if (url === '/api/speech/stream/cancel') return Response.json({})
    throw new Error(`Unexpected request: ${url}`)
  }
  const recognizer = createSpeechRecognizer({ chatSessionId: 'chat-startup' })
  t.after(async () => {
    try {
      await recognizer.dispose()
    } finally {
      globalThis.fetch = previousFetch
      if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow)
      else delete globalThis.window
    }
  })
  return {
    recognizer,
    calls,
    timers,
    startResponse,
    startRequested: startRequested.promise,
    timerStarts: () => timerStarts,
  }
}

function uploadedSamples(calls, endpoint = '/api/speech/stream/chunk') {
  const uploads = calls.filter((call) => call.url === endpoint)
  for (const upload of uploads) {
    assert.ok(upload.body instanceof ArrayBuffer)
    assert.equal(new Headers(upload.headers).get('Content-Type'), 'application/octet-stream')
    assert.equal(new Headers(upload.headers).get('X-Pisper-Sample-Rate'), String(VOICE_SAMPLE_RATE))
  }
  const chunks = uploads.map((call) => new Float32Array(call.body))
  const samples = new Float32Array(chunks.reduce((length, chunk) => length + chunk.length, 0))
  let offset = 0
  for (const chunk of chunks) {
    samples.set(chunk, offset)
    offset += chunk.length
  }
  return samples
}

function assertStreamingCompletion(calls) {
  assert.deepEqual(
    calls.map((call) => call.url),
    ['/api/speech/stream/start', '/api/speech/stream/chunk', '/api/speech/stream/finish'],
  )
  assert.equal(new Headers(calls[0].headers).get('X-Pisper-Chat-Session'), 'chat-startup')
  for (const call of calls.slice(1)) {
    assert.equal(new Headers(call.headers).get('X-Pisper-Speech-Session'), 'speech-startup')
  }
}

test('speech preserves copied startup PCM before normally accepted streaming chunks', async (t) => {
  const { recognizer, calls, timers, startResponse, startRequested } = fixture(t)
  const starting = recognizer.start()
  const first = new Float32Array([0.125, -0.25])
  assert.equal(recognizer.acceptPcm(first), false)
  first.fill(99)
  await startRequested
  const second = new Float32Array([0.5, -0.75])
  assert.equal(recognizer.acceptPcm(second), false)
  second.fill(99)
  startResponse.resolve(Response.json({ id: 'speech-startup' }))
  await starting
  const third = new Float32Array([1, -1])
  assert.equal(recognizer.acceptPcm(third), false)
  third.fill(99)
  assert.equal(await recognizer.finish(), 'final text')
  assertStreamingCompletion(calls)
  assert.deepEqual(uploadedSamples(calls), new Float32Array([0.125, -0.25, 0.5, -0.75, 1, -1]))
  assert.equal(timers.size, 0)
})

test('speech finish waits for in-flight startup before uploading PCM and finishing', async (t) => {
  const { recognizer, calls, startResponse, startRequested } = fixture(t)
  const starting = recognizer.start()
  await startRequested
  recognizer.acceptPcm(new Float32Array([0.25, 0.5]))
  let settled = false
  const finishing = recognizer.finish().finally(() => {
    settled = true
  })
  const result = finishing.catch((error) => error)
  await setImmediate()
  const settledBeforeStartup = settled
  const urlsBeforeStartup = calls.map((call) => call.url)
  startResponse.resolve(Response.json({ id: 'speech-startup' }))
  await starting
  assert.equal(await result, 'final text')
  assert.equal(settledBeforeStartup, false)
  assert.deepEqual(urlsBeforeStartup, ['/api/speech/stream/start'])
  assertStreamingCompletion(calls)
  assert.deepEqual(uploadedSamples(calls), new Float32Array([0.25, 0.5]))
})

test('speech startup buffer caps PCM at sixty seconds and reports capacity', async (t) => {
  const { recognizer, calls, startResponse, startRequested } = fixture(t)
  const starting = recognizer.start()
  await startRequested
  const limit = VOICE_MAX_DURATION_SECONDS * VOICE_SAMPLE_RATE
  assert.equal(limit, 60 * 16_000)
  const first = new Float32Array(limit - 2).fill(0.125)
  const belowLimit = recognizer.acceptPcm(first)
  first.fill(99)
  const reachesLimit = recognizer.acceptPcm(new Float32Array([0.25, 0.5, 0.75, 1]))
  const alreadyFull = recognizer.acceptPcm(new Float32Array([9, 9]))
  startResponse.resolve(Response.json({ id: 'speech-startup' }))
  await starting
  assert.equal(await recognizer.finish(), 'final text')
  assert.equal(belowLimit, false)
  assert.equal(reachesLimit, true)
  assert.equal(alreadyFull, true)
  const expected = new Float32Array(limit).fill(0.125)
  expected.set([0.25, 0.5], limit - 2)
  assert.deepEqual(uploadedSamples(calls), expected)
  assertStreamingCompletion(calls)
})

for (const phase of ['response', 'JSON parsing']) {
  test(`speech cancellation during startup ${phase} cannot revive the stream`, async (t) => {
    const { recognizer, calls, timers, timerStarts, startResponse, startRequested } = fixture(t)
    const starting = recognizer.start().catch((error) => error)
    await startRequested
    recognizer.acceptPcm(new Float32Array([0.25, 0.5]))
    const payload = deferred()
    if (phase === 'JSON parsing') {
      const parsing = deferred()
      startResponse.resolve({
        ok: true,
        json() {
          parsing.resolve()
          return payload.promise
        },
      })
      await parsing.promise
    }
    await recognizer.cancel()
    assert.equal(calls[0].signal.aborted, true)
    assert.equal(recognizer.acceptPcm(new Float32Array([1])), false)
    if (phase === 'response') startResponse.resolve(Response.json({ id: 'speech-startup' }))
    else payload.resolve({ id: 'speech-startup' })
    await starting
    await setImmediate()
    assert.equal(timers.size, 0)
    assert.equal(timerStarts(), 0)
    await assert.rejects(recognizer.finish())
    assert.deepEqual(
      calls.map((call) => call.url),
      ['/api/speech/stream/start', '/api/speech/stream/cancel'],
    )
    assert.equal(new Headers(calls[1].headers).get('X-Pisper-Speech-Session'), 'speech-startup')
  })
}

for (const failure of ['HTTP', 'network', 'JSON parsing', 'missing session ID']) {
  test(`speech ${failure} initialization failure also rejects finish without a hidden fallback`, async (t) => {
    const { recognizer, calls, timers, startResponse, startRequested } = fixture(t)
    const starting = recognizer.start()
    const startRejected = assert.rejects(starting)
    await startRequested
    recognizer.acceptPcm(new Float32Array([0.25]))
    const finishRejected = assert.rejects(recognizer.finish())
    if (failure === 'HTTP') {
      startResponse.resolve(Response.json({ error: 'startup unavailable' }, { status: 503 }))
    } else if (failure === 'network') {
      startResponse.reject(new Error('startup network failure'))
    } else if (failure === 'JSON parsing') {
      startResponse.resolve(new Response('invalid JSON', { status: 200 }))
    } else {
      startResponse.resolve(Response.json({}))
    }
    await Promise.all([startRejected, finishRejected])
    await assert.rejects(recognizer.finish())
    await setImmediate()
    assert.equal(timers.size, 0)
    assert.deepEqual(
      calls.map((call) => call.url),
      ['/api/speech/stream/start'],
    )
  })
}

test('desktop startup 404 retains the existing one-shot fallback and buffered PCM', async (t) => {
  const { recognizer, calls, startResponse, startRequested } = fixture(t)
  const starting = recognizer.start()
  await startRequested
  const source = new Float32Array([0.25, -0.5])
  recognizer.acceptPcm(source)
  source.fill(99)
  startResponse.resolve(new Response('', { status: 404 }))
  await starting
  recognizer.acceptPcm(new Float32Array([0.75]))
  assert.equal(await recognizer.finish(), 'legacy text')
  assert.deepEqual(
    calls.map((call) => call.url),
    ['/api/speech/stream/start', '/api/speech/transcribe'],
  )
  assert.deepEqual(
    uploadedSamples(calls, '/api/speech/transcribe'),
    new Float32Array([0.25, -0.5, 0.75]),
  )
})
