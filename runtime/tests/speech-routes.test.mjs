import assert from 'node:assert/strict'
import { createHash, randomUUID } from 'node:crypto'
import { EventEmitter } from 'node:events'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import { speechRoutes } from '../http/routes/speech.mjs'
import { SpeechEngineService, speechEngineError } from '../services/speech-engine-service.mjs'
import { SpeechModelDownloadService } from '../services/speech-model-download-service.mjs'

function deferred() {
  let resolve
  let reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

function request(method = 'POST', value) {
  return Object.assign(new EventEmitter(), {
    method,
    aborted: false,
    async *[Symbol.asyncIterator]() {
      if (value !== undefined) yield Buffer.from(JSON.stringify(value))
    },
  })
}

function response() {
  return Object.assign(new EventEmitter(), {
    status: 0,
    headers: {},
    chunks: [],
    heads: [],
    endCount: 0,
    headersSent: false,
    writableEnded: false,
    destroyed: false,
    writeHead(status, headers) {
      this.heads.push(status)
      this.status = status
      this.headers = headers
      this.headersSent = true
    },
    end(value) {
      if (value !== undefined) this.chunks.push(Buffer.from(value))
      this.endCount += 1
      this.writableEnded = true
    },
    bytes() {
      return Buffer.concat(this.chunks)
    },
    json() {
      return JSON.parse(this.bytes().toString('utf8'))
    },
  })
}

function invoke(services, path, value, method = 'POST') {
  const req = request(method, value)
  const res = response()
  const done = createApiHandler({}, services)(req, res, new URL(path, 'http://localhost'))
  return { req, res, done }
}

function route(path) {
  const entry = speechRoutes.find((item) => item.path === path)
  assert.ok(entry, `missing speech route: ${path}`)
  return entry.handler
}

const payload = Buffer.from('test-model-weights')
const digest = createHash('sha256').update(payload).digest('hex')

function catalog() {
  return {
    defaults: { asr: 'asr-a', tts: 'tts-a', voice: 'voice-a' },
    models: ['asr', 'tts'].map((kind) => ({
      id: `${kind}-a`,
      kind,
      engine: kind === 'asr' ? 'online-transducer' : 'vits',
      name: `${kind} model`,
      languages: ['zh', 'en'],
      license: { name: 'Apache-2.0', url: 'https://example.com/license' },
      config:
        kind === 'asr'
          ? { model: '/private/model.onnx' }
          : {
              model: 'weights/model.onnx',
              tokens: 'tokens.txt',
              lexicon: 'lexicon.txt',
              dictDir: 'dict',
            },
      path: '/private/models',
      urls: ['https://cdn.example.com/private'],
      files: (kind === 'asr'
        ? ['weights/model.onnx']
        : ['weights/model.onnx', 'tokens.txt', 'lexicon.txt', 'dict/words.txt']
      ).map((path) => ({
        path,
        bytes: payload.length,
        sha256: digest,
        urls: [`https://cdn.example.com/${kind}/${path}?signature=private`],
      })),
      ...(kind === 'tts'
        ? {
            voices: [
              { id: 'voice-a', name: 'Voice A', language: 'zh', sid: 0, path: '/private/voice' },
            ],
          }
        : {}),
    })),
  }
}

function expectedModel(
  model,
  status,
  downloadedBytes,
  totalBytes = model.files.length * payload.length,
  error,
) {
  return {
    id: model.id,
    kind: model.kind,
    engine: model.engine,
    name: model.name,
    languages: model.languages,
    license: model.license,
    ...(model.kind === 'tts'
      ? { voices: [{ id: 'voice-a', name: 'Voice A', language: 'zh' }] }
      : {}),
    status,
    downloadedBytes,
    totalBytes,
    filesBytes: model.files.length * payload.length,
    ...(error ? { error } : {}),
  }
}

function assertPublic(value) {
  if (!value || typeof value !== 'object') return
  for (const [key, child] of Object.entries(value)) {
    assert.ok(!['files', 'urls', 'path', 'sid', 'config', 'archive', 'sha256'].includes(key), key)
    assertPublic(child)
  }
}

function wavBytes() {
  const wav = Buffer.alloc(52)
  wav.write('RIFF', 0)
  wav.writeUInt32LE(44, 4)
  wav.write('WAVEfmt ', 8)
  wav.writeUInt32LE(16, 16)
  wav.writeUInt16LE(1, 20)
  wav.writeUInt16LE(1, 22)
  wav.writeUInt32LE(24000, 24)
  wav.writeUInt32LE(48000, 28)
  wav.writeUInt16LE(2, 32)
  wav.writeUInt16LE(16, 34)
  wav.write('data', 36)
  wav.writeUInt32LE(8, 40)
  ;[-32768, -1, 0, 32767].forEach((sample, i) => wav.writeInt16LE(sample, 44 + i * 2))
  return wav
}

const speechInput = () => ({ text: 'hello', voiceId: 'voice-a', requestId: randomUUID() })

function assertUnwritten(res) {
  assert.deepEqual(res.heads, [])
  assert.equal(res.endCount, 0)
  assert.equal(res.bytes().length, 0)
}

function trackListeners(req, res) {
  const aborted = () => {}
  const closed = () => {}
  req.on('aborted', aborted)
  res.on('close', closed)
  return () => {
    assert.deepEqual(req.listeners('aborted'), [aborted])
    assert.deepEqual(res.listeners('close'), [closed])
  }
}

test('models returns only public metadata, defaults and service progress mapped by id', async () => {
  const speechCatalog = catalog()
  const states = [
    {
      id: 'tts-a',
      status: 'error',
      downloadedBytes: 5,
      totalBytes: 11,
      error: 'Verification failed.',
      files: [{ path: '/private/state' }],
    },
    {
      id: 'asr-a',
      status: 'downloading',
      downloadedBytes: 3,
      totalBytes: 9,
      path: '/private/install',
    },
  ]
  const calls = []
  const services = {
    speechCatalog,
    speechModels: {
      async list() {
        calls.push('list')
        return states
      },
      async startDownload() {
        assert.fail('listing must not install models')
      },
    },
  }
  const { done, res } = invoke(services, '/api/speech/models', undefined, 'GET')
  await done
  assert.equal(res.status, 200)
  assert.deepEqual(res.json(), {
    defaults: speechCatalog.defaults,
    models: [
      expectedModel(speechCatalog.models[0], 'downloading', 3, 9),
      expectedModel(speechCatalog.models[1], 'error', 5, 11, 'Verification failed.'),
    ],
  })
  assertPublic(res.json())
  assert.deepEqual(calls, ['list'])
})

test('models exposes real downloader progress and explicit cancellation without network or polling', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-speech-routes-'))
  const speechCatalog = catalog()
  const nextRead = deferred()
  const cancelledRead = deferred()
  let fetchCount = 0
  let reads = 0
  const speechModels = new SpeechModelDownloadService({
    dataDir,
    catalog: speechCatalog.models,
    fetchImpl: async () => {
      fetchCount += 1
      return {
        status: 200,
        headers: new Headers({ 'content-length': String(payload.length) }),
        body: {
          getReader() {
            return {
              read() {
                reads += 1
                if (reads === 1)
                  return Promise.resolve({ done: false, value: payload.subarray(0, 4) })
                // 第二次读取意味着第一块已完成落盘和进度更新，无需定时轮询。
                nextRead.resolve()
                return cancelledRead.promise
              },
              async cancel() {
                cancelledRead.resolve({ done: true })
              },
              releaseLock() {},
            }
          },
        },
      }
    },
  })
  t.after(async () => {
    await speechModels.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  const services = { speechCatalog, speechModels }
  const initial = invoke(services, '/api/speech/models', undefined, 'GET')
  await initial.done
  assert.equal(fetchCount, 0)
  assert.deepEqual(
    initial.res.json().models,
    speechCatalog.models.map((model) => expectedModel(model, 'not-installed', 0)),
  )
  const download = invoke(services, '/api/speech/models/download', { modelId: 'asr-a' })
  await download.done
  assert.equal(download.res.status, 200)
  assert.equal(download.res.json().status, 'downloading')
  await nextRead.promise
  const progress = invoke(services, '/api/speech/models', undefined, 'GET')
  await progress.done
  assert.deepEqual(
    progress.res.json().models[0],
    expectedModel(speechCatalog.models[0], 'downloading', 4),
  )
  assertPublic(progress.res.json())
  const cancel = invoke(services, '/api/speech/models/cancel', { modelId: 'asr-a' })
  await cancel.done
  assert.deepEqual(cancel.res.json(), expectedModel(speechCatalog.models[0], 'cancelled', 4))
  assertPublic(cancel.res.json())
  assert.equal(speechModels.inflight.size, 0)
  assert.equal(fetchCount, 1)
})

test('model operations reject unknown or malformed modelId through the real download service', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-speech-route-ids-'))
  const speechCatalog = catalog()
  const speechModels = new SpeechModelDownloadService({
    dataDir,
    catalog: speechCatalog.models,
    fetchImpl: async () => assert.fail('invalid modelId must not access the CDN'),
  })
  t.after(async () => {
    await speechModels.dispose()
    await rm(dataDir, { recursive: true, force: true })
  })
  for (const action of ['download', 'cancel']) {
    for (const modelId of [undefined, null, 42, {}, [], '', '../tts-a', 'unknown']) {
      const { done, res } = invoke(
        { speechCatalog, speechModels },
        `/api/speech/models/${action}`,
        { modelId },
      )
      await done
      assert.deepEqual(res.heads, [404], JSON.stringify(modelId))
      assert.deepEqual(res.json(), { error: 'Unknown speech model.' })
      assert.equal(speechModels.inflight.size, 0)
    }
  }
})

for (const [action, method, status] of [
  ['download', 'startDownload', 'downloading'],
  ['cancel', 'cancelDownload', 'cancelled'],
]) {
  test(`${action} passes exactly modelId and projects the returned state`, async () => {
    const speechCatalog = catalog()
    const calls = []
    const serviceState = {
      id: 'tts-a',
      status,
      downloadedBytes: 6,
      totalBytes: 10,
      path: '/private',
      urls: ['https://private'],
    }
    const services = {
      speechCatalog,
      speechModels: {
        async [method](...args) {
          calls.push(args)
          return serviceState
        },
      },
    }
    const { done, res } = invoke(
      services,
      `/api/speech/models/${action}?modelId=asr-a&url=https://untrusted`,
      { modelId: 'tts-a' },
    )
    await done
    assert.equal(res.status, 200)
    assert.deepEqual(calls, [['tts-a']])
    assert.deepEqual(res.json(), expectedModel(speechCatalog.models[1], status, 6, 10))
    assertPublic(res.json())
  })

  test(`${action} rejects extra URL, path and body fields before service invocation`, async () => {
    let calls = 0
    const services = {
      speechCatalog: catalog(),
      speechModels: {
        async [method]() {
          calls += 1
          assert.fail('invalid body reached service')
        },
      },
    }
    for (const body of [
      null,
      [],
      'tts-a',
      1,
      true,
      ...['url', 'urls', 'path', 'files', 'body', 'requestId', '__proto__'].map((key) => ({
        modelId: 'tts-a',
        [key]: 'untrusted',
      })),
    ]) {
      const { done, res } = invoke(services, `/api/speech/models/${action}`, body)
      await done
      assert.equal(res.status, 400, JSON.stringify(body))
      assert.deepEqual(res.json(), { error: 'Invalid speech request.' })
    }
    assert.equal(calls, 0)
  })

  test(`${action} preserves service errors and never writes a successful response`, async () => {
    const error = Object.assign(new Error('Unknown speech model.'), {
      code: 'unknown',
      statusCode: 404,
    })
    const services = {
      speechCatalog: catalog(),
      speechModels: {
        async [method]() {
          throw error
        },
      },
    }
    await assert.rejects(
      route(`/api/speech/models/${action}`)({
        services,
        body: async () => ({ modelId: 'unknown' }),
        json: () => assert.fail('unexpected response'),
      }),
      (actual) => actual === error,
    )
    const { done, res } = invoke(services, `/api/speech/models/${action}`, { modelId: 'unknown' })
    await done
    assert.deepEqual(res.heads, [404])
    assert.deepEqual(res.json(), { error: error.message })
  })
}

for (const [path, method] of [
  ['/api/speech/models', 'GET'],
  ['/api/speech/models/download', 'POST'],
  ['/api/speech/models/cancel', 'POST'],
]) {
  test(`${path} returns 503 when either model dependency is absent`, async () => {
    for (const services of [{}, { speechCatalog: catalog() }, { speechModels: {} }]) {
      const { done, res } = invoke(services, path, { modelId: 'tts-a' }, method)
      await done
      assert.deepEqual(res.heads, [503])
      assert.deepEqual(res.json(), { error: 'Local speech models are unavailable.' })
    }
  })
}

for (const path of ['/api/speech/synthesize', '/api/speech/cancel']) {
  test(`${path} returns 503 without a speech service`, async () => {
    const { done, res } = invoke({}, path, speechInput())
    await done
    assert.deepEqual(res.heads, [503])
    assert.deepEqual(res.json(), { error: 'Local speech synthesis is unavailable.' })
  })
}

test('synthesis returns complete binary WAV, exact length and security headers only after fulfillment', async () => {
  const input = speechInput()
  const wav = wavBytes()
  const started = deferred()
  const result = deferred()
  const calls = []
  const { req, res, done } = invoke(
    {
      speech: {
        synthesize(value) {
          calls.push(value)
          started.resolve()
          return result.promise
        },
        async cancelSpeech() {
          assert.fail('completed request must not be cancelled')
        },
      },
    },
    '/api/speech/synthesize',
    input,
  )
  const assertClean = trackListeners(req, res)
  await started.promise
  assertUnwritten(res)
  result.resolve({ wav, sampleRate: 24000, durationMs: 4 / 24 })
  await done
  assert.deepEqual(calls, [input])
  assert.deepEqual(res.heads, [200])
  assert.deepEqual(res.headers, {
    'Content-Type': 'audio/wav',
    'Content-Length': wav.length,
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
  })
  assert.equal(res.endCount, 1)
  assert.deepEqual(res.bytes(), wav)
  assertClean()
  req.emit('aborted')
  res.emit('close')
})

for (const code of ['busy', 'missing', 'cancelled']) {
  test(`asynchronous synthesis ${code} rejection preserves service HTTP error without premature 200`, async () => {
    const started = deferred()
    const result = deferred()
    const error = speechEngineError(code)
    const { req, res, done } = invoke(
      {
        speech: {
          synthesize() {
            started.resolve()
            return result.promise
          },
          async cancelSpeech() {
            assert.fail('rejected request must not be cancelled later')
          },
        },
      },
      '/api/speech/synthesize',
      speechInput(),
    )
    const assertClean = trackListeners(req, res)
    await started.promise
    assertUnwritten(res)
    result.reject(error)
    await done
    assert.deepEqual(res.heads, [error.statusCode])
    assert.deepEqual(res.json(), { error: error.message })
    assertClean()
    req.emit('aborted')
    res.emit('close')
  })
}

for (const event of ['aborted', 'close']) {
  for (const outcome of ['resolve', 'reject']) {
    test(`${event} cancels only its requestId and ignores late synthesis ${outcome}`, async () => {
      const started = deferred()
      const result = deferred()
      const input = speechInput()
      const foreignId = randomUUID()
      const active = new Set([input.requestId, foreignId])
      const cancelled = []
      const { req, res, done } = invoke(
        {
          speech: {
            synthesize() {
              started.resolve()
              return result.promise
            },
            async cancelSpeech(id) {
              cancelled.push(id)
              active.delete(id)
              return { cancelled: true }
            },
          },
        },
        '/api/speech/synthesize',
        input,
      )
      const assertClean = trackListeners(req, res)
      await started.promise
      if (event === 'aborted') {
        // aborted 可先于响应 close；不能依赖响应 destroyed 才丢弃迟到音频。
        req.aborted = true
        req.emit('aborted')
      } else {
        res.destroyed = true
        res.emit('close')
      }
      assert.deepEqual(cancelled, [input.requestId])
      assert.deepEqual([...active], [foreignId])
      if (outcome === 'resolve') result.resolve({ wav: wavBytes() })
      else result.reject(speechEngineError('cancelled'))
      await done
      assertClean()
      assertUnwritten(res)
      req.emit('aborted')
      res.emit('close')
      assert.deepEqual(cancelled, [input.requestId])
    })
  }

  test(`already ${event} before body parsing finishes does not start synthesis or leak listeners`, async () => {
    const body = deferred()
    const req = request()
    const res = response()
    const assertClean = trackListeners(req, res)
    const done = route('/api/speech/synthesize')({
      services: {
        speech: {
          synthesize() {
            assert.fail('disconnected request started synthesis')
          },
          async cancelSpeech() {
            assert.fail('no synthesis belongs to this request')
          },
        },
      },
      req,
      res,
      body: () => body.promise,
    })
    if (event === 'aborted') {
      req.aborted = true
      req.emit('aborted')
    } else {
      res.destroyed = true
      res.emit('close')
    }
    body.resolve(speechInput())
    await done
    assertUnwritten(res)
    assertClean()
  })
}

test('normal request close after its body is read does not cancel pending synthesis', async () => {
  const started = deferred()
  const result = deferred()
  const { req, res, done } = invoke(
    {
      speech: {
        synthesize() {
          started.resolve()
          return result.promise
        },
        async cancelSpeech() {
          assert.fail('normal request completion is not client disconnection')
        },
      },
    },
    '/api/speech/synthesize',
    speechInput(),
  )
  const assertClean = trackListeners(req, res)
  await started.promise
  req.emit('close')
  assertUnwritten(res)
  result.resolve({ wav: wavBytes() })
  await done
  assert.deepEqual(res.heads, [200])
  assert.deepEqual(res.bytes(), wavBytes())
  assertClean()
})

test('disconnect cancellation rejection is consumed and both listeners are cleaned', async () => {
  const started = deferred()
  const result = deferred()
  const cancellation = deferred()
  const input = speechInput()
  const ids = []
  const { req, res, done } = invoke(
    {
      speech: {
        synthesize() {
          started.resolve()
          return result.promise
        },
        cancelSpeech(id) {
          ids.push(id)
          return cancellation.promise
        },
      },
    },
    '/api/speech/synthesize',
    input,
  )
  const assertClean = trackListeners(req, res)
  await started.promise
  res.destroyed = true
  res.emit('close')
  cancellation.reject(speechEngineError('worker'))
  result.resolve({ wav: wavBytes() })
  await done
  assert.deepEqual(ids, [input.requestId])
  assertUnwritten(res)
  assertClean()
})

test('synthesis and cancellation reject unknown body fields without touching the engine', async () => {
  const services = {
    speech: {
      synthesize() {
        assert.fail('invalid synthesis input reached engine')
      },
      cancelSpeech() {
        assert.fail('invalid cancellation input reached engine')
      },
    },
  }
  for (const path of ['/api/speech/synthesize', '/api/speech/cancel']) {
    const base = path.endsWith('/cancel') ? { requestId: randomUUID() } : speechInput()
    for (const body of [
      null,
      [],
      'invalid',
      ...['url', 'path', 'sid', 'modelId', 'body'].map((key) => ({ ...base, [key]: 'untrusted' })),
    ]) {
      const { req, res, done } = invoke(services, path, body)
      await done
      assert.deepEqual(res.heads, [400])
      assert.deepEqual(res.json(), { error: 'Invalid speech request.' })
      assert.equal(req.listenerCount('aborted'), 0)
      assert.equal(res.listenerCount('close'), 0)
    }
  }
})

test('cancel passes exactly the UUID and returns the awaited service result', async () => {
  const requestId = randomUUID()
  const started = deferred()
  const result = deferred()
  const calls = []
  const { done, res } = invoke(
    {
      speech: {
        cancelSpeech(...args) {
          calls.push(args)
          started.resolve()
          return result.promise
        },
      },
    },
    '/api/speech/cancel',
    { requestId },
  )
  await started.promise
  assertUnwritten(res)
  result.resolve({ cancelled: true })
  await done
  assert.deepEqual(calls, [[requestId]])
  assert.deepEqual(res.heads, [200])
  assert.deepEqual(res.json(), { cancelled: true })
})

test('cancel uses the real engine UUID validation and preserves foreign request ownership', async (t) => {
  const engine = new SpeechEngineService({ catalog: catalog(), modelDownloads: {} })
  t.after(() => engine.dispose())
  const requestId = randomUUID()
  const controller = new AbortController()
  const task = { requestId, controller, completion: Promise.resolve() }
  engine.activeSpeech = task
  t.after(() => {
    engine.activeSpeech = null
  })
  for (const invalid of [
    undefined,
    null,
    '',
    42,
    {},
    [],
    'not-uuid',
    ` ${requestId}`,
    `${requestId}\n`,
    `prefix-${requestId}`,
    '00000000-0000-0000-0000-000000000000',
  ]) {
    const { done, res } = invoke({ speech: engine }, '/api/speech/cancel', { requestId: invalid })
    await done
    assert.deepEqual(res.heads, [400], JSON.stringify(invalid))
    assert.deepEqual(res.json(), { error: 'Invalid speech input.' })
    assert.equal(controller.signal.aborted, false)
  }
  const foreign = invoke({ speech: engine }, '/api/speech/cancel', { requestId: randomUUID() })
  await foreign.done
  assert.deepEqual(foreign.res.json(), { cancelled: false })
  assert.equal(controller.signal.aborted, false)
  assert.equal(engine.activeSpeech, task)
  const own = invoke({ speech: engine }, '/api/speech/cancel', { requestId })
  await own.done
  assert.deepEqual(own.res.json(), { cancelled: true })
  assert.equal(controller.signal.aborted, true)
})

test('cancel propagates the exact service rejection and HTTP status', async () => {
  const error = Object.assign(new Error('Cancellation unavailable.'), { statusCode: 503 })
  const services = {
    speech: {
      async cancelSpeech() {
        throw error
      },
    },
  }
  const input = { requestId: randomUUID() }
  await assert.rejects(
    route('/api/speech/cancel')({
      services,
      body: async () => input,
      json: () => assert.fail('unexpected response'),
    }),
    (actual) => actual === error,
  )
  const { done, res } = invoke(services, '/api/speech/cancel', input)
  await done
  assert.deepEqual(res.heads, [503])
  assert.deepEqual(res.json(), { error: error.message })
})
