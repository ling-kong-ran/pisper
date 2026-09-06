import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import { SpeechRecognitionService } from '../services/speech-recognition-service.mjs'
import { BUILTIN_SPEECH_TERMS, SpeechTermsService } from '../services/speech-terms-service.mjs'
import { formatSpeechTerms, speechHotwords, spokenTerm } from '../../shared/speech-terms.mjs'

async function workspace(t) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-speech-hotwords-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const cwd = join(root, 'trusted')
  const dataDir = join(root, 'data')
  await mkdir(cwd)
  return { root, cwd, dataDir, speechTerms: new SpeechTermsService({ dataDir }) }
}

async function request(handler, method, path, { body, bytes, headers = {} } = {}) {
  const req = {
    method,
    headers,
    async *[Symbol.asyncIterator]() {
      if (bytes) yield bytes
      else if (body !== undefined) yield Buffer.from(JSON.stringify(body))
    },
  }
  const res = {
    status: 0,
    headers: {},
    body: '',
    writableEnded: false,
    destroyed: false,
    writeHead(status, responseHeaders) {
      this.status = status
      this.headers = responseHeaders
    },
    end(value = '') {
      this.body += value
      this.writableEnded = true
    },
  }
  assert.equal(await handler(req, res, new URL(path, 'http://localhost')), true)
  assert.equal(res.headers['Content-Type'], 'application/json; charset=utf-8')
  assert.equal(res.headers['Cache-Control'], 'no-store')
  return { status: res.status, data: JSON.parse(res.body) }
}

function pcm() {
  return Buffer.from(new Float32Array([0.25, -0.5, 0.75]).buffer)
}

async function nativeService(
  t,
  { vocabulary = true, hotwordsDirectory = true, idleUnloadMs = 0, text = 'use effect' } = {},
) {
  const { root } = await workspace(t)
  const modelDir = join(root, 'model')
  const hotwordsDir = join(root, 'hotwords')
  await mkdir(modelDir)
  for (const name of [
    'encoder.int8.onnx',
    'decoder.onnx',
    'joiner.int8.onnx',
    'tokens.txt',
    'bpe.model',
    ...(vocabulary ? ['bpe.vocab'] : []),
  ]) {
    await writeFile(join(modelDir, name), 'fake')
  }
  const instances = []
  class FakeRecognizer {
    constructor(config) {
      this.config = structuredClone(config)
      this.hotwords = config.hotwordsFile ? readFileSync(config.hotwordsFile, 'utf8') : ''
      this.streams = []
      instances.push(this)
    }

    createStream() {
      const stream = {
        owner: this,
        ready: false,
        totalSamples: 0,
        acceptWaveform({ samples, sampleRate }) {
          assert.equal(sampleRate, 16000)
          this.totalSamples += samples.length
          this.ready = true
        },
        inputFinished() {
          this.ready = true
        },
      }
      this.streams.push(stream)
      return stream
    }

    isReady(stream) {
      assert.equal(stream.owner, this)
      return stream.ready
    }

    decode(stream) {
      assert.equal(stream.owner, this)
      stream.ready = false
    }

    getResult(stream) {
      assert.equal(stream.owner, this)
      return { text: stream.totalSamples ? text : '' }
    }
  }
  const service = new SpeechRecognitionService({
    modelDir,
    hotwordsDir: hotwordsDirectory ? hotwordsDir : '',
    nativeModule: { OnlineRecognizer: FakeRecognizer },
    idleUnloadMs,
  })
  t.after(() => service.markActive())
  return { service, instances, modelDir, hotwordsDir }
}

test('speech settings routes serialize defaults and persist partial updates across service instances', async (t) => {
  const { speechTerms, dataDir } = await workspace(t)
  const handler = createApiHandler({}, { speechTerms })
  const defaults = await request(handler, 'GET', '/api/settings/speech')
  assert.deepEqual(defaults, {
    status: 200,
    data: { projectTermsEnabled: true, builtinTerms: [...BUILTIN_SPEECH_TERMS] },
  })
  const saved = await request(handler, 'PATCH', '/api/settings/speech', {
    body: {},
  })
  assert.equal(saved.status, 200)
  const disabled = await request(handler, 'PATCH', '/api/settings/speech', {
    body: { projectTermsEnabled: false },
  })
  assert.equal(disabled.status, 200)
  assert.deepEqual(Object.keys(disabled.data).sort(), ['builtinTerms', 'projectTermsEnabled'])
  assert.equal(disabled.data.projectTermsEnabled, false)
  const restarted = createApiHandler({}, { speechTerms: new SpeechTermsService({ dataDir }) })
  assert.deepEqual(await request(restarted, 'GET', '/api/settings/speech'), disabled)
  assert.deepEqual(JSON.parse(await readFile(join(dataDir, 'speech-settings.json'), 'utf8')), {
    projectTermsEnabled: false,
  })
})

test('speech settings GET ignores legacy disk terms and PATCH removes them without exposing the field', async (t) => {
  const { dataDir } = await workspace(t)
  await mkdir(dataDir)
  const path = join(dataDir, 'speech-settings.json')
  for (const customTerms of [['LegacyTerm'], 'LegacyTerm', null, { invalid: true }]) {
    const original = JSON.stringify({ projectTermsEnabled: false, customTerms })
    await writeFile(path, original)
    const handler = createApiHandler({}, { speechTerms: new SpeechTermsService({ dataDir }) })
    assert.deepEqual(await request(handler, 'GET', '/api/settings/speech'), {
      status: 200,
      data: { projectTermsEnabled: false, builtinTerms: [...BUILTIN_SPEECH_TERMS] },
    })
    assert.equal(await readFile(path, 'utf8'), original)
    assert.deepEqual(
      await request(handler, 'PATCH', '/api/settings/speech', {
        body: { projectTermsEnabled: true },
      }),
      {
        status: 200,
        data: { projectTermsEnabled: true, builtinTerms: [...BUILTIN_SPEECH_TERMS] },
      },
    )
    assert.deepEqual(JSON.parse(await readFile(path, 'utf8')), { projectTermsEnabled: true })
  }
})

test('speech settings routes serialize concurrent patches and reject invalid updates without persistence changes', async (t) => {
  const { speechTerms } = await workspace(t)
  const handler = createApiHandler({}, { speechTerms })
  const outputs = await Promise.all([
    request(handler, 'PATCH', '/api/settings/speech', { body: { projectTermsEnabled: false } }),
    request(handler, 'PATCH', '/api/settings/speech', { body: {} }),
    request(handler, 'PATCH', '/api/settings/speech', { body: { projectTermsEnabled: true } }),
  ])
  assert.ok(outputs.every((output) => output.status === 200))
  const before = await request(handler, 'GET', '/api/settings/speech')
  assert.equal(outputs[0].data.projectTermsEnabled, false)
  assert.deepEqual(outputs[1], outputs[0])
  assert.deepEqual(before, outputs[2])
  assert.equal(Object.hasOwn(before.data, 'customTerms'), false)
  assert.equal(before.data.projectTermsEnabled, true)
  for (const body of [
    { customTerms: [] },
    { customTerms: ['ValidTerm'] },
    { customTerms: ['inject:99'] },
    { projectTermsEnabled: false, customTerms: null },
    { projectTermsEnabled: 'false' },
    { builtinTerms: [] },
  ]) {
    const rejected = await request(handler, 'PATCH', '/api/settings/speech', { body })
    assert.equal(rejected.status, 400)
    assert.equal(typeof rejected.data.error, 'string')
  }
  assert.deepEqual(await request(handler, 'GET', '/api/settings/speech'), before)
})

test('terms endpoint resolves the session cwd and ignores arbitrary client cwd parameters', async (t) => {
  const { speechTerms, cwd, root, dataDir } = await workspace(t)
  await writeFile(join(cwd, 'package.json'), '{"name":"trusted-project"}')
  const untrusted = join(root, 'untrusted')
  await mkdir(untrusted)
  await writeFile(join(untrusted, 'package.json'), '{"name":"untrusted-project"}')
  await mkdir(dataDir)
  await writeFile(
    join(dataDir, 'speech-settings.json'),
    JSON.stringify({ customTerms: ['LegacyTerm'] }),
  )
  const ids = []
  const handler = createApiHandler(
    {
      sessionWorkspaceCwd: async (id) => {
        ids.push(id)
        return cwd
      },
    },
    { speechTerms },
  )
  const query = new URLSearchParams({ sessionId: '  chat-1  ', cwd: untrusted })
  const result = await request(handler, 'GET', `/api/speech/terms?${query}`)
  assert.equal(result.status, 200)
  assert.deepEqual(ids, ['chat-1'])
  assert.deepEqual(result.data.terms, [...BUILTIN_SPEECH_TERMS, 'trusted project'])
  const withoutSession = await request(
    handler,
    'GET',
    `/api/speech/terms?cwd=${encodeURIComponent(untrusted)}`,
  )
  assert.deepEqual(withoutSession.data.terms, [...BUILTIN_SPEECH_TERMS])
  assert.deepEqual(ids, ['chat-1'])
  await speechTerms.updateSettings({ projectTermsEnabled: false })
  const disabled = await request(handler, 'GET', '/api/speech/terms?sessionId=chat-1')
  assert.deepEqual(disabled.data.terms, [...BUILTIN_SPEECH_TERMS])
})

test('transcribe and stream start pass only server-resolved terms with unchanged binary PCM', async (t) => {
  const { speechTerms, cwd, dataDir } = await workspace(t)
  await writeFile(join(cwd, 'package.json'), '{"name":"voice-project"}')
  await mkdir(dataDir)
  await writeFile(
    join(dataDir, 'speech-settings.json'),
    JSON.stringify({ customTerms: ['LegacyTerm'] }),
  )
  const ids = []
  const calls = []
  const handler = createApiHandler(
    {
      sessionWorkspaceCwd: async (id) => {
        ids.push(id)
        return cwd
      },
    },
    {
      speechTerms,
      speech: {
        sweepExpiredSessions() {
          calls.push(['sweep'])
        },
        async transcribe(samples, options) {
          calls.push(['transcribe', Array.from(samples), options])
          return '中文 useEffect'
        },
        async startSession(options) {
          calls.push(['start', options])
          return { id: 'speech-uuid' }
        },
      },
    },
  )
  const headers = {
    'x-pisper-sample-rate': '16000',
    'x-pisper-chat-session': ' chat-2 ',
    'x-pisper-cwd': '/untrusted',
    'x-pisper-speech-session': 'not-the-chat-id',
  }
  assert.deepEqual(
    await request(handler, 'POST', '/api/speech/transcribe?cwd=/untrusted', {
      bytes: pcm(),
      headers,
    }),
    { status: 200, data: { text: '中文 useEffect' } },
  )
  assert.deepEqual(
    await request(handler, 'POST', '/api/speech/stream/start?cwd=/untrusted', {
      headers,
      body: { cwd: '/untrusted', terms: ['Injected'] },
    }),
    { status: 200, data: { id: 'speech-uuid' } },
  )
  const terms = [...BUILTIN_SPEECH_TERMS, 'voice project']
  assert.deepEqual(ids, ['chat-2', 'chat-2'])
  assert.deepEqual(calls, [
    ['transcribe', [0.25, -0.5, 0.75], { terms }],
    ['sweep'],
    ['start', { terms }],
  ])
})

test('legacy routes remain usable without a terms service while missing services return JSON errors', async () => {
  const termsCalls = []
  const handler = createApiHandler(
    {
      sessionWorkspaceCwd() {
        throw new Error('Must not resolve without terms service')
      },
    },
    {
      speech: {
        sweepExpiredSessions() {},
        async transcribe(samples, { terms }) {
          termsCalls.push(terms)
          return 'legacy'
        },
        async startSession({ terms }) {
          termsCalls.push(terms)
          return { id: 'legacy-stream' }
        },
      },
    },
  )
  assert.deepEqual(await request(handler, 'GET', '/api/speech/terms?sessionId=chat-1'), {
    status: 200,
    data: { terms: [] },
  })
  assert.equal(
    (
      await request(handler, 'POST', '/api/speech/transcribe', {
        bytes: pcm(),
        headers: { 'x-pisper-sample-rate': '16000' },
      })
    ).status,
    200,
  )
  assert.equal((await request(handler, 'POST', '/api/speech/stream/start')).status, 200)
  assert.deepEqual(termsCalls, [[], []])
  const missing = createApiHandler({})
  for (const [method, path, options] of [
    ['GET', '/api/settings/speech'],
    ['PATCH', '/api/settings/speech', { body: {} }],
    ['POST', '/api/speech/stream/start'],
    [
      'POST',
      '/api/speech/transcribe',
      { bytes: pcm(), headers: { 'x-pisper-sample-rate': '16000' } },
    ],
  ]) {
    const result = await request(missing, method, path, options)
    assert.equal(result.status, 400)
    assert.equal(typeof result.data.error, 'string')
  }
})

test('native hotword configuration uses the text BPE vocabulary and modified beam search', async (t) => {
  const { service, instances, modelDir, hotwordsDir } = await nativeService(t)
  const terms = ['useEffect', 'TypeScript', 'use effect']
  assert.equal(await service.transcribe(new Float32Array([0.5]), { terms }), 'useEffect')
  assert.equal(instances.length, 1)
  const { config, hotwords } = instances[0]
  assert.equal(config.decodingMethod, 'modified_beam_search')
  assert.equal(config.modelConfig.modelingUnit, 'bpe')
  assert.equal(config.modelConfig.bpeVocab, join(modelDir, 'bpe.vocab'))
  assert.equal(config.modelConfig.transducer.encoder, join(modelDir, 'encoder.int8.onnx'))
  assert.equal(config.maxActivePaths, 2)
  assert.equal(config.hotwordsScore, 1.5)
  assert.equal(config.hotwordsFile, join(hotwordsDir, 'active-terms.txt'))
  assert.equal(hotwords, 'use effect\ntype script\n')
  assert.equal(await readFile(config.hotwordsFile, 'utf8'), hotwords)
})

test('idle recognizer is reused for matching terms and rebuilt for another context', async (t) => {
  const { service, instances } = await nativeService(t)
  const samples = new Float32Array([0.5])
  await service.transcribe(samples, { terms: ['useEffect'] })
  await service.transcribe(samples, { terms: ['useEffect'] })
  assert.equal(instances.length, 1)
  await service.transcribe(samples, { terms: ['TypeScript'] })
  assert.equal(instances.length, 2)
  assert.equal(instances[0].hotwords, 'use effect\n')
  assert.equal(instances[1].hotwords, 'type script\n')
  assert.equal(await readFile(instances[1].config.hotwordsFile, 'utf8'), 'type script\n')
})

test('idle unload rebuilds the native model on the next hotword request', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const { service, instances } = await nativeService(t, { idleUnloadMs: 20 })
  await service.transcribe(new Float32Array([0.5]), { terms: ['useEffect'] })
  assert.equal(instances.length, 1)
  t.mock.timers.tick(20)
  assert.equal(service.recognizer, null)
  await service.transcribe(new Float32Array([0.5]), { terms: ['useEffect'] })
  assert.equal(instances.length, 2)
  assert.equal(instances[1].hotwords, 'use effect\n')
})

test('different hotword contexts cannot replace an active recognizer or overwrite its hotword file', async (t) => {
  const { service, instances } = await nativeService(t)
  const { id } = await service.startSession({ terms: ['useEffect'] })
  const active = service.recognizer
  await assert.rejects(service.startSession({ terms: ['TypeScript'] }), /另一个语音会话/)
  await assert.rejects(
    service.transcribe(new Float32Array([0.5]), { terms: ['TypeScript'] }),
    /另一个语音会话/,
  )
  assert.equal(service.recognizer, active)
  assert.equal(service.sessions.size, 1)
  assert.equal(instances.length, 1)
  assert.equal(await readFile(instances[0].config.hotwordsFile, 'utf8'), 'use effect\n')
  await service.acceptChunk(id, new Float32Array([0.5]))
  assert.deepEqual(await service.finishSession(id), { text: 'useEffect' })
  await service.transcribe(new Float32Array([0.5]), { terms: ['TypeScript'] })
  assert.equal(instances.length, 2)
})

test('simultaneous sessions with the same context reuse the model and hold independent streams', async (t) => {
  const { service, instances } = await nativeService(t)
  const sessions = await Promise.all([
    service.startSession({ terms: ['useEffect'] }),
    service.startSession({ terms: ['useEffect'] }),
  ])
  assert.equal(instances.length, 1)
  assert.equal(instances[0].streams.length, 2)
  assert.notEqual(sessions[0].id, sessions[1].id)
  for (const { id } of sessions) await service.acceptChunk(id, new Float32Array([0.5]))
  assert.deepEqual(await Promise.all(sessions.map(({ id }) => service.finishSession(id))), [
    { text: 'useEffect' },
    { text: 'useEffect' },
  ])
})

test('active streaming terms are snapshotted before asynchronous model initialization', async (t) => {
  const { service, instances } = await nativeService(t)
  const terms = ['useEffect']
  const starting = service.startSession({ terms })
  terms[0] = 'TypeScript'
  terms.push('DifferentTerm')
  const { id } = await starting
  assert.equal(instances[0].hotwords, 'use effect\n')
  await service.acceptChunk(id, new Float32Array([0.5]))
  assert.deepEqual(await service.finishSession(id), { text: 'useEffect' })
})

test('missing bpe.vocab keeps legacy greedy decoding without treating bpe.model as text vocabulary', async (t) => {
  const { service, instances } = await nativeService(t, { vocabulary: false })
  assert.equal(
    await service.transcribe(new Float32Array([0.5]), { terms: ['useEffect'] }),
    'useEffect',
  )
  const { config } = instances[0]
  assert.equal(config.decodingMethod, 'greedy_search')
  assert.equal(config.modelConfig.bpeVocab, undefined)
  assert.equal(config.modelConfig.modelingUnit, undefined)
  assert.equal(config.hotwordsFile, undefined)
  assert.equal(config.hotwordsScore, undefined)
  assert.equal(config.maxActivePaths, undefined)
})

test('no terms or no configured hotword directory retain greedy backwards compatibility', async (t) => {
  const first = await nativeService(t)
  assert.equal(await first.service.transcribe(new Float32Array([0.5])), 'use effect')
  assert.equal(first.instances[0].config.decodingMethod, 'greedy_search')
  assert.equal(first.instances[0].config.hotwordsFile, undefined)
  const second = await nativeService(t, { hotwordsDirectory: false })
  assert.equal(
    await second.service.transcribe(new Float32Array([0.5]), { terms: ['useEffect'] }),
    'useEffect',
  )
  assert.equal(second.instances[0].config.decodingMethod, 'greedy_search')
  assert.equal(second.instances[0].config.hotwordsFile, undefined)
})

test('spoken terms normalize camel case, acronyms and separators with stable hotword deduplication', () => {
  assert.equal(spokenTerm('useEffect'), 'use effect')
  assert.equal(spokenTerm('HTTPClient'), 'http client')
  assert.equal(spokenTerm('  npm_install-test  '), 'npm install test')
  assert.equal(spokenTerm('Pi Agent'), 'pi agent')
  assert.equal(spokenTerm('中文术语'), '中文术语')
  assert.equal(
    speechHotwords(['useEffect', 'use effect', 'TypeScript', 'type-script', '', '  ']),
    'use effect\ntype script',
  )
})

test('formatting restores known technical spellings without confusing py, pi, Python or paths', () => {
  const terms = [...BUILTIN_SPEECH_TERMS, 'Py', 'Pi', 'projectFile']
  assert.equal(
    formatSpeechTerms('use effect, TYPE SCRIPT; javascript and react', terms),
    'useEffect, TypeScript; JavaScript and React',
  )
  assert.equal(formatSpeechTerms('use\t effect and pi agent', terms), 'useEffect and Pi Agent')
  assert.equal(formatSpeechTerms('py pi python', terms), 'py pi Python')
  const paths =
    'py/python .py main.py /tmp/python/useeffect.ts ./useeffect ../react C:\\python\\useeffect.ts python.py react-component x_useeffect myuseeffect projectFile.js'
  assert.equal(formatSpeechTerms(paths, terms), paths)
  assert.equal(
    formatSpeechTerms('happy copy pyproject pythonic reactivate', terms),
    'happy copy pyproject pythonic reactivate',
  )
  assert.equal(formatSpeechTerms('use effect and python', []), 'use effect and python')
  assert.equal(formatSpeechTerms('cargo test and npm install', terms), 'cargo test and npm install')
})

test('formatting treats a sentence-ending period as punctuation while preserving file extensions', () => {
  const terms = ['useEffect', 'TypeScript']
  assert.equal(
    formatSpeechTerms('Use effect. Then type script.', terms),
    'useEffect. Then TypeScript.',
  )
  assert.equal(
    formatSpeechTerms('useeffect.py typescript.ts ./useeffect', terms),
    'useeffect.py typescript.ts ./useeffect',
  )
})

test('formatting prefers full known phrases and never replaces ordinary lowercase terms or path syntax', () => {
  const terms = ['Agent', 'Pi Agent', 'Node.js', 'C++', 'snake_case', 'lowercase', 'useEffect']
  assert.equal(
    formatSpeechTerms('pi agent, agent, use effect', terms),
    'Pi Agent, Agent, useEffect',
  )
  assert.equal(
    formatSpeechTerms('node.js c++ snake_case LOWERCASE /useeffect/useeffect', terms),
    'node.js c++ snake_case LOWERCASE /useeffect/useeffect',
  )
})
