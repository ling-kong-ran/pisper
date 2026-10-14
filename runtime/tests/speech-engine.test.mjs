import assert from 'node:assert/strict'
import { fork } from 'node:child_process'
import { createHash, randomUUID } from 'node:crypto'
import { mkdir, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { once } from 'node:events'
import { dirname, join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import test from 'node:test'
import { SpeechEngineService, speechWorkerEnvironment } from '../services/speech-engine-service.mjs'
import { createSpeechInferenceHandler, encodeWav } from '../workers/speech-inference-worker.mjs'

const fixture = new URL('./fixtures/speech-worker.mjs', import.meta.url)
const request = (text = 'hello', requestId = randomUUID()) => ({
  text,
  requestId,
  voiceId: 'voice-a',
})
const catalog = (config = {}) => ({
  version: 1,
  defaults: { asr: 'asr-a', tts: 'vits-melo-tts-zh_en', voice: 'voice-a' },
  models: [
    { id: 'asr-a', kind: 'asr', engine: 'online-transducer', config },
    {
      id: 'vits-melo-tts-zh_en',
      kind: 'tts',
      engine: 'vits',
      config,
      voices: [{ id: 'voice-a', sid: 0, sourceId: 0 }],
    },
  ],
})

function harness(t, { config = {}, ...options } = {}) {
  const children = []
  const calls = []
  const lifecycle = []
  const service = new SpeechEngineService({
    catalog: catalog(config),
    modelDownloads: { modelDirectory: async (id) => resolve('fixture-models', id) },
    workerUrl: fixture,
    idleUnloadMs: 0,
    startupTimeoutMs: 3000,
    inferenceTimeoutMs: 3000,
    shutdownGraceMs: 1000,
    ...options,
    forkProcess(url, args, settings) {
      assert.deepEqual(args, ['--pisper-speech-worker'])
      assert.deepEqual(settings.execArgv, [])
      assert.equal(settings.serialization, 'advanced')
      assert.equal(settings.env.NODE_OPTIONS, undefined)
      const child = fork(url, args, settings)
      children.push(child)
      lifecycle.push(['spawn', child.pid])
      child.on('exit', () => lifecycle.push(['exit', child.pid]))
      child.on('message', (message) => {
        if (message.event) calls.push({ pid: child.pid, ...message })
      })
      return child
    },
  })
  t.after(async () => {
    await service.dispose()
    for (const child of children) {
      assert.ok(
        child.exitCode !== null || child.signalCode !== null,
        `worker ${child.pid} must exit`,
      )
      assert.throws(() => process.kill(child.pid, 0), { code: 'ESRCH' })
    }
  })
  return { service, children, calls, lifecycle }
}

async function until(predicate, timeoutMs = 3000) {
  const deadline = Date.now() + timeoutMs
  while (!predicate()) {
    assert.ok(Date.now() < deadline, 'condition timed out')
    await delay(5)
  }
}

function rejected(promise, code) {
  return assert.rejects(promise, (error) => {
    assert.equal(error.code, code)
    assert.doesNotMatch(error.message, /private|https:|fixture-models/)
    return true
  })
}

test('real IPC preserves samples and terms, reuses same kind, and ignores late replies', async (t) => {
  const { service, children, calls } = harness(t, { config: { lateReply: true } })
  const samples = new Float32Array([0.25, 0.5])
  const terms = ['TypeScript']
  const first = service.transcribe(samples, { terms })
  samples[0] = 9
  terms[0] = 'changed'
  const result = JSON.parse(await first)
  assert.deepEqual(result.samples, [0.25, 0.5])
  assert.deepEqual(result.terms, ['TypeScript'])
  await delay(35)
  assert.equal(JSON.parse(await service.transcribe(new Float32Array([1]))).pid, result.pid)
  assert.equal(children.length, 1)
  assert.equal(calls.filter((item) => item.method === 'init').length, 1)
})

test('voice mode warms real worker engines and holds both beyond the idle window', async (t) => {
  const { service, children, calls } = harness(t, { idleUnloadMs: 40 })
  const mode = new AbortController()
  assert.deepEqual(
    await service.prepareSpeechSession(
      { requestId: randomUUID(), kinds: ['asr', 'tts'], hotwords: 'type script' },
      mode.signal,
    ),
    { ready: true },
  )
  assert.equal(children.length, 2)
  assert.deepEqual(calls.find((call) => call.method === 'warmup').params.terms, ['type script'])
  assert.equal(
    calls.some((call) => call.method === 'synthesize' || call.method === 'transcribe'),
    false,
  )
  await delay(100)
  assert.equal(service.idleTimer, null)
  assert.equal(service.workers.size, 2)
  const { id } = await service.startSession()
  await service.finishSession(id)
  await service.synthesize(request())
  await delay(100)
  assert.equal(children.length, 2)
  assert.equal(service.idleTimer, null)
  mode.abort()
  assert.ok(service.idleTimer)
  await until(() => service.workers.size === 0)
})

test('ASR and TTS initialize concurrently while synthesis stays behind the warmup barrier', async (t) => {
  const { service, children, calls } = harness(t, { config: { initBarrier: true } })
  const mode = new AbortController()
  const warming = service.prepareSpeechSession(
    { requestId: randomUUID(), kinds: ['asr', 'tts'] },
    mode.signal,
  )
  await until(() => calls.filter((call) => call.method === 'init').length === 2)
  assert.equal(children.length, 2)
  const synthesis = service.synthesize(request())
  await delay(20)
  assert.equal(
    calls.some((call) => call.method === 'synthesize'),
    false,
  )
  children.forEach((child) => child.send({ method: 'releaseInit' }))
  assert.deepEqual(await warming, { ready: true })
  await synthesis
  assert.equal(children.length, 2)
  mode.abort()
})

test('only the last voice session leaving starts a fresh idle window', async (t) => {
  const { service } = harness(t, { idleUnloadMs: 80 })
  const modes = [new AbortController(), new AbortController()]
  for (const mode of modes)
    await service.prepareSpeechSession({ requestId: randomUUID(), kinds: ['asr'] }, mode.signal)
  modes[0].abort()
  await delay(120)
  assert.equal(service.voiceSessions.size, 1)
  assert.equal(service.idleTimer, null)
  assert.equal(service.workers.size, 1)
  modes[1].abort()
  await delay(20)
  assert.equal(service.workers.size, 1)
  await until(() => service.workers.size === 0)
})

test('voice warmup cancellation during model verification cannot leave a late pin or worker', async (t) => {
  let installed
  const { service, children } = harness(t, {
    modelDownloads: {
      modelDirectory: () =>
        new Promise((resolve) => {
          installed = resolve
        }),
    },
  })
  const mode = new AbortController()
  const warming = rejected(
    service.prepareSpeechSession({ requestId: randomUUID(), kinds: ['asr', 'tts'] }, mode.signal),
    'cancelled',
  )
  await until(() => installed)
  mode.abort()
  installed('fixture-models')
  await warming
  assert.equal(service.voiceSessions.size, 0)
  assert.equal(service.warmupOperations, 0)
  assert.equal(children.length, 0)
})

test('failed warmup releases its own pin and disposal clears remaining sessions', async (t) => {
  const { service } = harness(t, { config: { initError: true } })
  await rejected(
    service.prepareSpeechSession(
      { requestId: randomUUID(), kinds: ['tts'] },
      new AbortController().signal,
    ),
    'inference',
  )
  assert.equal(service.voiceSessions.size, 0)
  assert.equal(service.warmupOperations, 0)
  const healthy = harness(t).service
  await healthy.prepareSpeechSession(
    { requestId: randomUUID(), kinds: ['asr'] },
    new AbortController().signal,
  )
  await healthy.dispose()
  assert.equal(healthy.voiceSessions.size, 0)
  assert.equal(healthy.workers.size, 0)
})

test('warmup rejects invalid kinds, duplicate ids, and hotword injection without disturbing an active session', async (t) => {
  const { service } = harness(t)
  for (const kinds of [[], ['asr', 'asr'], ['other'], ['asr', 'tts', 'asr'], null])
    await rejected(
      service.prepareSpeechSession(
        { requestId: randomUUID(), kinds },
        new AbortController().signal,
      ),
      'invalid',
    )
  for (const hotwords of ['bad/word', 'bad\u0000word', '\ud800', 'a'.repeat(129)])
    await rejected(
      service.prepareSpeechSession(
        { requestId: randomUUID(), kinds: ['asr'], hotwords },
        new AbortController().signal,
      ),
      'invalid',
    )
  const requestId = randomUUID()
  const mode = new AbortController()
  await service.prepareSpeechSession({ requestId, kinds: ['asr'] }, mode.signal)
  await rejected(
    service.prepareSpeechSession({ requestId, kinds: ['tts'] }, new AbortController().signal),
    'busy',
  )
  assert.equal(service.voiceSessions.size, 1)
  mode.abort()
})

test('kind switches reuse loaded engines across turns while TTS maps whitelisted sid', async (t) => {
  const { service, children, calls, lifecycle } = harness(t)
  await service.transcribe(new Float32Array([1]))
  const asrPid = children[0].pid
  const wav = await service.synthesize(request())
  assert.equal(wav.wav.toString('ascii', 0, 4), 'RIFF')
  assert.equal(wav.sampleRate, 24000)
  const ttsPid = children[1].pid
  for (let turn = 0; turn < 3; turn++) {
    await service.transcribe(new Float32Array([1]))
    await service.synthesize(request('next'))
  }
  assert.equal(children.length, 2)
  assert.deepEqual(lifecycle, [
    ['spawn', asrPid],
    ['spawn', ttsPid],
  ])
  assert.equal(calls.filter((item) => item.method === 'init').length, 2)
  assert.ok(
    calls.filter((item) => item.method === 'transcribe').every((item) => item.pid === asrPid),
  )
  assert.ok(
    calls
      .filter((item) => item.method === 'synthesize')
      .every((item) => item.pid === ttsPid && item.params.sid === 0),
  )
})

test('ASR reservations serialize concurrent start/finish while TTS proceeds independently', async (t) => {
  const { service, children } = harness(t, { config: { delayMs: 40 } })
  const starting = service.startSession({ terms: ['project'] })
  await service.synthesize(request())
  const { id } = await starting
  const another = service.startSession()
  const finish = service.finishSession(id)
  await service.synthesize(request())
  const second = await another
  assert.deepEqual(await finish, { text: 'finished' })
  assert.deepEqual(await service.acceptChunk(second.id, new Float32Array([1])), { text: 'partial' })
  await service.synthesize(request())
  assert.equal(children.length, 2)
  assert.deepEqual(await service.cancelSession(second.id), { ok: true })
  await service.synthesize(request())
  assert.equal(children.length, 2)
})

test('a blocked ASR operation does not block TTS and later ASR still queues behind it', async (t) => {
  const { service, children, calls } = harness(t)
  const first = rejected(service.transcribe(new Float32Array([-1])), 'disposed')
  const second = rejected(service.transcribe(new Float32Array([1])), 'disposed')
  await until(() => calls.some((call) => call.method === 'transcribe'))
  await service.synthesize(request())
  assert.equal(calls.filter((call) => call.method === 'transcribe').length, 1)
  assert.equal(children.length, 2)
  await service.dispose()
  await Promise.all([first, second])
})

test('TTS cancellation is isolated by UUID, stops matching worker, and permits recovery', async (t) => {
  const { service, children, calls } = harness(t)
  const input = request('hang')
  const pending = rejected(service.synthesize(input), 'cancelled')
  await until(() => calls.some((item) => item.method === 'synthesize'))
  assert.deepEqual(await service.cancelSpeech(randomUUID()), { cancelled: false })
  await rejected(service.synthesize(input), 'busy')
  const { id } = await service.startSession()
  await service.cancelSession(id)
  assert.equal(children[0].exitCode, null)
  assert.deepEqual(await service.cancelSpeech(input.requestId), { cancelled: true })
  await pending
  assert.ok(children[0].signalCode || children[0].exitCode !== null)
  assert.deepEqual(await service.cancelSpeech(input.requestId), { cancelled: false })
  await service.synthesize(request())
  assert.equal(children.length, 3)
})

test('TTS has a FIFO queue; cancelling a queued segment cannot kill the active one or ASR', async (t) => {
  const { service, calls, children } = harness(t)
  const firstInput = request('hang')
  const secondInput = request('skip')
  const first = rejected(service.synthesize(firstInput), 'cancelled')
  const second = rejected(service.synthesize(secondInput), 'cancelled')
  const third = service.synthesize(request('third'))
  await until(() => calls.some((call) => call.method === 'synthesize'))
  assert.deepEqual(
    calls.filter((call) => call.method === 'synthesize').map((call) => call.params.text),
    ['hang'],
  )
  await service.cancelSpeech(secondInput.requestId)
  assert.equal(children[0].exitCode, null)
  assert.equal(JSON.parse(await service.transcribe(new Float32Array([1]))).samples[0], 1)
  await service.cancelSpeech(firstInput.requestId)
  await Promise.all([first, second, third])
  assert.deepEqual(
    calls.filter((call) => call.method === 'synthesize').map((call) => call.params.text),
    ['hang', 'third'],
  )
  await Promise.all(['fourth', 'fifth'].map((text) => service.synthesize(request(text))))
  assert.deepEqual(
    calls
      .filter((call) => call.method === 'synthesize')
      .slice(-2)
      .map((call) => call.params.text),
    ['fourth', 'fifth'],
  )
  assert.equal(service.speechTasks.size, 0)
})

test('TTS queue is bounded and disposal rejects every queued request', async (t) => {
  const { service, calls } = harness(t)
  const queued = Array.from({ length: 16 }, (_, index) =>
    rejected(service.synthesize(request(index ? 'queued' : 'hang')), 'disposed'),
  )
  await rejected(service.synthesize(request('overflow')), 'busy')
  await until(() => calls.some((call) => call.method === 'synthesize'))
  await service.dispose()
  await Promise.all(queued)
  assert.equal(service.speechTasks.size, 0)
})

test('cancel during installation cannot spawn late and does not cancel another request', async (t) => {
  let release
  let waiting = true
  const installed = new Promise((resolveInstall) => {
    release = resolveInstall
  })
  const { service, children } = harness(t, {
    modelDownloads: {
      modelDirectory: () => (waiting ? installed : Promise.resolve(resolve('fixture-models'))),
    },
  })
  const input = request()
  const pending = rejected(service.synthesize(input), 'cancelled')
  await delay(10)
  assert.deepEqual(await service.cancelSpeech(input.requestId), { cancelled: true })
  await pending
  waiting = false
  release(resolve('old-fixture-model'))
  await service.synthesize(request())
  assert.equal(children.length, 1)
})

test('cancel during startup kills loading child rather than waiting for init', async (t) => {
  const { service, children, calls } = harness(t, { config: { initHang: true } })
  const input = request()
  const pending = rejected(service.synthesize(input), 'cancelled')
  await until(() => calls.some((item) => item.method === 'init'))
  await service.cancelSpeech(input.requestId)
  await pending
  assert.equal(children.length, 1)
})

test('session cancel/error/finish and synchronous TTL sweeping release ownership', async (t) => {
  const { service } = harness(t, { sessionTtlMs: 10 })
  const one = await service.startSession()
  await rejected(service.acceptChunk(one.id, new Float32Array([-3])), 'inference')
  await rejected(service.finishSession(one.id), 'session')
  const two = await service.startSession()
  assert.equal(service.sweepExpiredSessions(Date.now() + 100), undefined)
  await until(() => service.sessions.size === 0 && service.asrOperations === 0)
  await rejected(service.acceptChunk(two.id, new Float32Array([1])), 'session')
  assert.deepEqual(await service.cancelSession(two.id), { ok: true })
  const three = await service.startSession()
  await service.finishSession(three.id)
  await service.synthesize(request())
})

test('startup and inference timeouts terminate actual children and clear streams', async (t) => {
  const start = harness(t, { config: { initHang: true }, startupTimeoutMs: 150 })
  await rejected(start.service.startSession(), 'timeout')
  await until(() => start.children[0].signalCode !== null)
  assert.equal(start.service.sessions.size, 0)
  const inference = harness(t, { inferenceTimeoutMs: 80 })
  const stream = await inference.service.startSession()
  await rejected(inference.service.transcribe(new Float32Array([-1])), 'timeout')
  await until(() => inference.children[0].signalCode !== null)
  await rejected(inference.service.finishSession(stream.id), 'session')
  await inference.service.synthesize(request())
})

test('unexpected exit/error reject RPC safely and permit a fresh process', async (t) => {
  const { service, children, calls } = harness(t)
  await rejected(service.transcribe(new Float32Array([-2])), 'worker')
  await rejected(service.transcribe(new Float32Array([-3])), 'inference')
  const pending = rejected(service.transcribe(new Float32Array([-1])), 'worker')
  await until(() => calls.filter((item) => item.method === 'transcribe').length === 3)
  children[1].emit('error', new Error('/private/model'))
  await pending
  await service.transcribe(new Float32Array([1]))
  assert.equal(children.length, 3)
})

test('idle recycle and dispose wait for child exit; queued and running operations reject', async (t) => {
  const idle = harness(t, { idleUnloadMs: 20, config: { exitDelayMs: 40 } })
  await idle.service.transcribe(new Float32Array([1]))
  await until(() => idle.children[0].exitCode !== null)
  await idle.service.transcribe(new Float32Array([1]))
  assert.equal(idle.children.length, 2)
  const { service, calls } = harness(t)
  const first = rejected(service.transcribe(new Float32Array([-1])), 'disposed')
  const second = rejected(service.startSession(), 'disposed')
  await until(() => calls.some((item) => item.method === 'transcribe'))
  const disposing = service.dispose()
  assert.equal(service.dispose(), disposing)
  await disposing
  await Promise.all([first, second])
  await rejected(service.startSession(), 'disposed')
})

test('dispose during installation prevents late spawning', async (t) => {
  let release
  const installed = new Promise((resolveInstall) => {
    release = resolveInstall
  })
  const { service, children } = harness(t, { modelDownloads: { modelDirectory: () => installed } })
  const pending = rejected(service.startSession(), 'disposed')
  await delay(10)
  await service.dispose()
  release(resolve('fixture-models'))
  await pending
  await delay(10)
  assert.equal(children.length, 0)
})

test('worker exits when IPC parent disconnects without running app or HTTP', async () => {
  for (const url of [fixture, new URL('../workers/speech-inference-worker.mjs', import.meta.url)]) {
    const child = fork(url, ['--pisper-speech-worker'], {
      execArgv: [],
      serialization: 'advanced',
      stdio: ['ignore', 'ignore', 'ignore', 'ipc'],
    })
    try {
      const exit = once(child, 'exit')
      await once(child, 'spawn')
      await delay(80)
      child.disconnect()
      const result = await Promise.race([
        exit,
        delay(3000).then(() => {
          throw new Error('worker remained alive')
        }),
      ])
      assert.equal(result[0], 0)
      assert.throws(() => process.kill(child.pid, 0), { code: 'ESRCH' })
    } finally {
      if (child.exitCode === null && child.signalCode === null) {
        child.kill('SIGKILL')
        await once(child, 'exit')
      }
    }
  }
})

test('text, UUID, voice and PCM limits reject before forking; malformed output rejects', async (t) => {
  const { service, children } = harness(t)
  for (const text of ['', '   ', '\ud800'])
    await rejected(service.synthesize(request(text)), 'invalid')
  await rejected(service.synthesize(request('x'.repeat(401))), 'limit')
  await rejected(service.synthesize({ ...request(), requestId: 'not-uuid' }), 'invalid')
  await rejected(service.synthesize({ ...request(), voiceId: '../model' }), 'invalid')
  await rejected(service.transcribe(new Float32Array()), 'invalid')
  await rejected(service.transcribe(new Float32Array([NaN])), 'invalid')
  await rejected(service.transcribe(new Float32Array(16_000 * 600 + 1)), 'limit')
  assert.equal(children.length, 0)
  await service.synthesize(request('x'.repeat(400)))
  await rejected(service.synthesize(request('oversized')), 'limit')
  await until(() => children[0].signalCode !== null)
})

test('initialization and installation errors are sanitized without leaving workers', async (t) => {
  const failed = harness(t, { config: { initError: true } })
  await rejected(failed.service.startSession(), 'inference')
  assert.ok(failed.children[0].signalCode)
  const missing = harness(t, {
    modelDownloads: {
      modelDirectory: async () => {
        throw new Error('/private/model https://secret.invalid')
      },
    },
  })
  await rejected(missing.service.startSession(), 'missing')
  assert.equal(missing.children.length, 0)
})

test('idle expiry releases both cached engines and force-kills an uncooperative shutdown', async (t) => {
  const { service, children } = harness(t, {
    config: { exitDelayMs: 10000 },
    shutdownGraceMs: 20,
  })
  await service.transcribe(new Float32Array([1]))
  await service.synthesize(request())
  service.idleUnloadMs = 20
  service.scheduleIdle()
  await until(() => children.every((child) => child.signalCode === 'SIGKILL'))
  assert.equal(service.workers.size, 0)
  assert.equal(service.worker, null)
})

test('cancelling TTS preserves the cached ASR worker and dispose releases both kinds', async (t) => {
  const { service, children, calls } = harness(t)
  await service.transcribe(new Float32Array([1]))
  const asrPid = children[0].pid
  const input = request('hang')
  const pending = rejected(service.synthesize(input), 'cancelled')
  await until(() => calls.some((item) => item.method === 'synthesize'))
  await service.cancelSpeech(input.requestId)
  await pending
  assert.equal(JSON.parse(await service.transcribe(new Float32Array([1]))).pid, asrPid)
  await service.synthesize(request())
  assert.equal(children.length, 3)
  await service.dispose()
  assert.equal(service.workers.size, 0)
  assert.ok(children.every((child) => child.exitCode !== null || child.signalCode !== null))
})

test('malformed WAV and synthesis errors recycle TTS without accepting invalid audio', async (t) => {
  const { service, children } = harness(t)
  await rejected(service.synthesize(request('bad-wav')), 'limit')
  assert.ok(children[0].signalCode)
  await rejected(service.synthesize(request('error')), 'inference')
  assert.ok(children[1].signalCode)
  await service.synthesize(request())
  assert.equal(children.length, 3)
})

test('worker environment strips credentials, Node options and unrelated Pisper state', () => {
  assert.deepEqual(
    speechWorkerEnvironment({
      Path: 'native-bin',
      SystemRoot: 'windows',
      TEMP: 'tmp',
      PISPER_APP_ROOT: 'runtime',
      NODE_OPTIONS: '--inspect',
      OPENAI_API_KEY: 'secret',
      ANTHROPIC_API_KEY: 'secret',
      PISPER_PARENT_PID: '42',
      PISPER_AGENT_DIR: 'personal',
      RANDOM_SECRET: 'secret',
    }),
    { Path: 'native-bin', SystemRoot: 'windows', TEMP: 'tmp', PISPER_APP_ROOT: 'runtime' },
  )
})

test('WAV encoder produces bounded 16-bit mono RIFF with clipping and rejects invalid PCM', () => {
  const { wav, sampleRate, durationMs } = encodeWav(new Float32Array([-2, -1, 0, 0.5, 1, 2]), 24000)
  assert.equal(sampleRate, 24000)
  assert.equal(durationMs, 0.25)
  assert.equal(wav.length, 56)
  assert.equal(wav.toString('ascii', 0, 4), 'RIFF')
  assert.equal(wav.readUInt32LE(4), 48)
  assert.equal(wav.toString('ascii', 8, 16), 'WAVEfmt ')
  assert.equal(wav.readUInt16LE(20), 1)
  assert.equal(wav.readUInt16LE(22), 1)
  assert.equal(wav.readUInt32LE(24), 24000)
  assert.equal(wav.readUInt32LE(28), 48000)
  assert.equal(wav.readUInt16LE(32), 2)
  assert.equal(wav.readUInt16LE(34), 16)
  assert.equal(wav.toString('ascii', 36, 40), 'data')
  assert.equal(wav.readUInt32LE(40), 12)
  assert.deepEqual(
    Array.from({ length: 6 }, (_, i) => wav.readInt16LE(44 + i * 2)),
    [-32768, -32768, 0, 16384, 32767, 32767],
  )
  assert.equal(encodeWav(new Float32Array(8000 * 45), 8000).durationMs, 45000)
  for (const [samples, rate] of [
    [new Float32Array(), 24000],
    [new Float32Array([1]), 192000],
    [new Float32Array(8000 * 45 + 1), 8000],
    [new Float32Array([Infinity]), 24000],
  ]) {
    assert.throws(() => encodeWav(samples, rate))
  }
})

async function nativeHarness(
  t,
  {
    samples = new Float32Array([0, 1]),
    sampleRate = 24000,
    progressSamples,
    defaultExport = false,
    constructorFallback = false,
    maxCodePoints = 400,
    ruleFsts = ['date.fst', 'number.fst', 'phone.fst'],
  } = {},
) {
  const modelDir = await mkdtemp(join(await realpath(tmpdir()), 'pisper-melo-engine-'))
  t.after(() => rm(modelDir, { recursive: true, force: true }))
  const model = {
    id: 'vits-melo-tts-zh_en',
    kind: 'tts',
    engine: 'vits',
    voices: [{ id: 'voice-a', sid: 0, sourceId: 0 }],
    config: {
      model: 'model.onnx',
      maxTextCodePoints: maxCodePoints,
      tokens: 'tokens.txt',
      lexicon: 'lexicon.txt',
      dictDir: 'dict',
      ...(ruleFsts ? { ruleFsts } : {}),
    },
    files: [],
  }
  // 只替换原生推理，安装树及清单仍交给生产 worker 完整校验。
  for (const path of [
    'model.onnx',
    'tokens.txt',
    'lexicon.txt',
    'dict/jieba.dict.utf8',
    'dict/hmm_model.utf8',
    'dict/user.dict.utf8',
    'dict/idf.utf8',
    'dict/stop_words.utf8',
    ...(ruleFsts || []),
  ]) {
    const bytes = Buffer.from(`fixture:${path}`)
    const target = join(modelDir, path)
    await mkdir(dirname(target), { recursive: true })
    await writeFile(target, bytes)
    model.files.push({
      path,
      bytes: bytes.length,
      sha256: createHash('sha256').update(bytes).digest('hex'),
    })
  }
  let nativeConfig
  let generation
  class OfflineTts {
    constructor(config) {
      nativeConfig = config
      this.sampleRate = sampleRate
      this.numSpeakers = 0
    }
    async generateAsync(options) {
      generation = options
      if (progressSamples) assert.equal(options.onProgress({ samples: progressSamples }), false)
      return { samples, sampleRate }
    }
  }
  const nativeModule = {
    OfflineTts: constructorFallback
      ? OfflineTts
      : { createAsync: async (config) => new OfflineTts(config) },
  }
  const handler = createSpeechInferenceHandler({
    nativeModule: defaultExport ? { default: nativeModule } : nativeModule,
  })
  const init = ({ model: modelOverrides, ...overrides } = {}) =>
    handler('init', {
      kind: 'tts',
      modelDir,
      model: {
        ...model,
        ...modelOverrides,
        config: { ...model.config, ...modelOverrides?.config },
      },
      ...overrides,
    })
  return { handler, init, modelDir, config: () => nativeConfig, generation: () => generation }
}

test('ASR warmup constructs the native recognizer without decoding audio and reuses it', async (t) => {
  const modelDir = await mkdtemp(join(tmpdir(), 'pisper-asr-warmup-'))
  t.after(() => rm(modelDir, { recursive: true, force: true }))
  for (const file of [
    'encoder.int8.onnx',
    'decoder.onnx',
    'joiner.int8.onnx',
    'tokens.txt',
    'bpe.model',
  ])
    await writeFile(join(modelDir, file), 'fixture')
  let created = 0
  let streams = 0
  class OnlineRecognizer {
    constructor() {
      created += 1
    }
    createStream() {
      streams += 1
      return {}
    }
  }
  const handler = createSpeechInferenceHandler({ nativeModule: { OnlineRecognizer } })
  await handler('init', {
    kind: 'asr',
    modelDir,
    model: { id: 'asr', engine: 'online-transducer', config: {} },
  })
  assert.equal(created, 0)
  assert.deepEqual(await handler('warmup', { terms: [] }), { ready: true })
  assert.equal(created, 1)
  assert.equal(streams, 0)
  await handler('warmup', { terms: [] })
  const { id } = await handler('startSession', { terms: [] })
  assert.equal(created, 1)
  assert.equal(streams, 1)
  await handler('cancelSession', { id })
})

test('Melo VITS uses real async API shape, correct paths and fixed CPU/sentence settings', async (t) => {
  const native = await nativeHarness(t)
  await native.init({ numThreads: 3 })
  assert.deepEqual(native.config(), {
    model: {
      vits: {
        model: join(native.modelDir, 'model.onnx'),
        tokens: join(native.modelDir, 'tokens.txt'),
        lexicon: join(native.modelDir, 'lexicon.txt'),
        dictDir: join(native.modelDir, 'dict'),
        noiseScale: 0.667,
        noiseScaleW: 0.8,
        lengthScale: 1,
      },
      numThreads: 3,
      provider: 'cpu',
    },
    ruleFsts: ['date.fst', 'number.fst', 'phone.fst']
      .map((path) => join(native.modelDir, path))
      .join(','),
    maxNumSentences: 1,
  })
  const result = await native.handler('synthesize', { text: 'hello', sid: 0 })
  assert.equal(result.wav.length, 48)
  assert.equal(native.generation().text, 'hello')
  assert.equal(native.generation().sid, 0)
  assert.equal(native.generation().speed, 1)
  await rejected(native.handler('synthesize', { text: 'hello', sid: 10 }), 'invalid')
  await rejected(native.handler('synthesize', { text: '\ud800', sid: 0 }), 'invalid')
})

test('Melo VITS accepts the actual CommonJS default export and enforces catalog codepoint limits', async (t) => {
  const native = await nativeHarness(t, { defaultExport: true, maxCodePoints: 16 })
  await native.init()
  const result = await native.handler('synthesize', { text: 'Hello.', sid: 0 })
  assert.equal(result.wav.length, 48)
  assert.equal(native.generation().text, 'Hello.')
  await rejected(native.handler('synthesize', { text: 'a'.repeat(17), sid: 0 }), 'limit')
  const unicode = '\u{20000}'.repeat(16)
  assert.equal((await native.handler('synthesize', { text: unicode, sid: 0 })).wav.length, 48)
  assert.equal(native.generation().text, unicode)
  await rejected(native.handler('synthesize', { text: '\u{20000}'.repeat(17), sid: 0 }), 'limit')
})

test('Melo VITS retains the synchronous native constructor fallback', async (t) => {
  const native = await nativeHarness(t, { constructorFallback: true })
  await native.init()
  assert.equal(native.config().model.provider, 'cpu')
  assert.equal((await native.handler('synthesize', { text: 'hello', sid: 0 })).wav.length, 48)
  assert.equal(native.generation().sid, 0)
  await rejected(native.handler('synthesize', { text: 'hello', sid: 1 }), 'invalid')
})

test('Melo VITS allows an installed tree without optional FSTs', async (t) => {
  const native = await nativeHarness(t, { ruleFsts: null })
  await native.init()
  assert.equal(Object.hasOwn(native.config(), 'ruleFsts'), false)
  assert.equal(native.config().maxNumSentences, 1)
  assert.equal((await native.handler('synthesize', { text: 'hello', sid: 0 })).wav.length, 48)
})

test('worker rejects unsafe catalog paths and invalid initialization settings', async (t) => {
  for (const modelPath of [
    '../outside.onnx',
    resolve('outside.onnx'),
    'a,b.onnx',
    'bad\u0000.onnx',
  ]) {
    const native = await nativeHarness(t)
    await rejected(native.init({ model: { config: { model: modelPath } } }), 'config')
    assert.equal(native.config(), undefined)
    // 拒绝后同一安装树必须能初始化，避免其他无效字段造成路径测试假阳性。
    await native.init()
    assert.equal(native.config().model.vits.model, join(native.modelDir, 'model.onnx'))
  }
  for (const overrides of [{ maxTextLength: 401 }, { maxOutputSeconds: 46 }, { numThreads: 0 }]) {
    const native = await nativeHarness(t)
    await rejected(native.init(overrides), 'config')
    assert.equal(native.config(), undefined)
    await native.init()
    assert.equal(native.config().model.provider, 'cpu')
  }
})

test('native progress and final audio both enforce configurable output limits', async (t) => {
  const overflow = await nativeHarness(t, { progressSamples: new Float32Array(24001) })
  await overflow.init({ maxOutputSeconds: 1 })
  await rejected(overflow.handler('synthesize', { text: 'hello', sid: 0 }), 'limit')
  const missingProgress = await nativeHarness(t, { samples: new Float32Array(24001) })
  await missingProgress.init({ maxOutputSeconds: 1 })
  await rejected(missingProgress.handler('synthesize', { text: 'hello', sid: 0 }), 'limit')
  const shortText = await nativeHarness(t)
  await shortText.init({ maxTextLength: 2 })
  await rejected(shortText.handler('synthesize', { text: 'abc', sid: 0 }), 'limit')
})

test('orphaned ASR sessions expire via periodic sweep without holding model ownership forever', async (t) => {
  // 不经由 start 路由触发：周期 sweep 自行回收过期会话，避免孤儿 stream 永久持有模型。
  const { service } = harness(t, { sessionTtlMs: 40, sessionSweepMs: 20 })
  const { id } = await service.startSession()
  assert.ok(service.sessions.has(id))
  await until(() => !service.sessions.has(id) && service.asrOperations === 0)
  const wav = await service.synthesize(request())
  assert.equal(wav.wav.toString('ascii', 0, 4), 'RIFF')
})
