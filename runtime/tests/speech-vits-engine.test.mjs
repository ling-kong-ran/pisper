import assert from 'node:assert/strict'
import { fork } from 'node:child_process'
import { createHash, randomUUID } from 'node:crypto'
import { link, mkdir, mkdtemp, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import test from 'node:test'
import { pathToFileURL } from 'node:url'
import { SpeechEngineService } from '../services/speech-engine-service.mjs'
import { SpeechModelDownloadService } from '../services/speech-model-download-service.mjs'
import { createSpeechInferenceHandler } from '../workers/speech-inference-worker.mjs'

const workerUrl = new URL('../workers/speech-inference-worker.mjs', import.meta.url)
const digest = (value) => createHash('sha256').update(value).digest('hex')
const paths = [
  'model.onnx',
  'tokens.txt',
  'lexicon.txt',
  'date.fst',
  'number.fst',
  'phone.fst',
  'dict/jieba.dict.utf8',
  'dict/hmm_model.utf8',
  'dict/user.dict.utf8',
  'dict/idf.utf8',
  'dict/stop_words.utf8',
]

function modelFixture() {
  const payloads = new Map(paths.map((path) => [path, Buffer.from(`fixture:${path}`)]))
  const model = {
    id: 'vits-melo-tts-zh_en',
    name: 'MeloTTS Chinese English',
    kind: 'tts',
    engine: 'vits',
    config: {
      model: 'model.onnx',
      tokens: 'tokens.txt',
      lexicon: 'lexicon.txt',
      dictDir: 'dict',
      ruleFsts: ['date.fst', 'number.fst', 'phone.fst'],
      numThreads: 4,
      maxTextCodePoints: 16,
      noiseScale: 0.667,
      noiseScaleW: 0.8,
      lengthScale: 1,
    },
    voices: [{ id: 'female', sid: 0, sourceId: 0 }],
    files: [...payloads].map(([path, bytes]) => ({
      path,
      bytes: bytes.length,
      sha256: digest(bytes),
      urls: [`https://models.example/${path}`],
    })),
  }
  return { model, payloads }
}

async function installedFixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-vits-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const { model, payloads } = modelFixture()
  const downloads = new SpeechModelDownloadService({
    dataDir: root,
    catalog: [model],
    fetchImpl: async (url) => new Response(payloads.get(new URL(url).pathname.slice(1))),
  })
  t.after(() => downloads.dispose())
  await downloads.download(model.id)
  const modelDir = await downloads.modelDirectory(model.id)
  return { root, model, modelDir, downloads }
}

async function until(predicate, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs
  while (!predicate()) {
    assert.ok(Date.now() < deadline, 'condition timed out')
    await delay(5)
  }
}

function rejectCode(promise, code) {
  return assert.rejects(promise, (error) => {
    assert.equal(error.code, code)
    assert.doesNotMatch(error.message, /pisper-vits-|models\.example/)
    return true
  })
}

// 替身只替换原生模块，入口、IPC、构造配置、编码和进程生命周期均执行生产 worker。
async function workerHarness(t, { idleUnloadMs = 0 } = {}) {
  const fixture = await installedFixture(t)
  const nativePath = join(fixture.root, 'native.cjs')
  const loaderPath = join(fixture.root, 'loader.mjs')
  await writeFile(
    nativePath,
    `class OfflineTts {
  constructor(config) {
    this.sampleRate = 8000
    this.numSpeakers = 0
    process.send({ event: 'native-config', config })
  }
  static async createAsync(config) { return new OfflineTts(config) }
  async generateAsync(options) {
    process.send({ event: 'native-generate', text: options.text, sid: options.sid, speed: options.speed })
    if (options.text === 'hang') return new Promise(() => {})
    const samples = options.text.startsWith('long') ? new Float32Array(8000 * 45 + 1)
      : options.text === 'boundary' ? new Float32Array(8000 * 45)
      : options.text === 'nan' ? new Float32Array([NaN])
      : options.text === 'infinity' ? new Float32Array([Infinity])
      : new Float32Array([0, -1, 0.5, 1])
    if (!options.text.includes('no-progress')) options.onProgress({ samples })
    return { samples, sampleRate: this.sampleRate }
  }
}
module.exports = { OfflineTts }
`,
  )
  await writeFile(
    loaderPath,
    `import { registerHooks } from 'node:module'
registerHooks({ resolve(specifier, context, nextResolve) {
  return specifier === 'sherpa-onnx-node'
    ? { url: ${JSON.stringify(pathToFileURL(nativePath).href)}, shortCircuit: true }
    : nextResolve(specifier, context)
} })
`,
  )
  const children = []
  const events = []
  const lifecycle = []
  const catalog = {
    defaults: { tts: fixture.model.id, voice: 'female' },
    models: [fixture.model],
  }
  const service = new SpeechEngineService({
    catalog,
    modelDownloads: fixture.downloads,
    idleUnloadMs,
    sessionSweepMs: 0,
    startupTimeoutMs: 5000,
    inferenceTimeoutMs: 5000,
    forkProcess(url, args, options) {
      assert.equal(url.href, workerUrl.href)
      assert.deepEqual(args, ['--pisper-speech-worker'])
      assert.deepEqual(options.execArgv, [])
      assert.equal(options.serialization, 'advanced')
      assert.equal(options.env.NODE_OPTIONS, undefined)
      const child = fork(url, args, {
        ...options,
        execArgv: ['--import', pathToFileURL(loaderPath).href],
        stdio: ['ignore', 'ignore', 'pipe', 'ipc'],
      })
      let stderr = ''
      child.stderr.on('data', (chunk) => {
        stderr += String(chunk)
      })
      child.on('exit', (code) => {
        if (code) t.diagnostic(stderr)
      })
      children.push(child)
      lifecycle.push(['spawn', child.pid])
      child.on('message', (message) => {
        if (message.event) events.push(message)
      })
      child.on('exit', () => lifecycle.push(['exit', child.pid]))
      return child
    },
  })
  t.after(async () => {
    await service.dispose()
    for (const child of children) {
      assert.ok(child.signalCode || child.exitCode !== null)
      assert.throws(() => process.kill(child.pid, 0), { code: 'ESRCH' })
    }
  })
  const speak = (text = 'Hello.', requestId = randomUUID(), voiceId = 'female') =>
    service.synthesize({ text, requestId, voiceId })
  return { ...fixture, service, speak, children, events, lifecycle }
}

test('real worker uses CJS default OfflineTts VITS config, canonical scales and single sid 0', async (t) => {
  const h = await workerHarness(t)
  const audio = await h.speak()
  assert.equal(audio.wav.toString('ascii', 0, 4), 'RIFF')
  assert.equal(audio.wav.length, 52)
  assert.equal(audio.durationMs, 0.5)
  const config = h.events.find((event) => event.event === 'native-config').config
  assert.deepEqual(config, {
    model: {
      vits: {
        model: join(h.modelDir, 'model.onnx'),
        tokens: join(h.modelDir, 'tokens.txt'),
        lexicon: join(h.modelDir, 'lexicon.txt'),
        dictDir: join(h.modelDir, 'dict'),
        noiseScale: 0.667,
        noiseScaleW: 0.8,
        lengthScale: 1,
      },
      numThreads: 4,
      provider: 'cpu',
    },
    ruleFsts: ['date.fst', 'number.fst', 'phone.fst']
      .map((path) => join(h.modelDir, path))
      .join(','),
    maxNumSentences: 1,
  })
  assert.deepEqual(
    h.events.find((event) => event.event === 'native-generate'),
    {
      event: 'native-generate',
      text: 'Hello.',
      sid: 0,
      speed: 1,
    },
  )
  await h.speak('Next.')
  assert.equal(h.children.length, 1)
})

test('real worker rejects nonfinite PCM, kills failed native state and recovers', async (t) => {
  const h = await workerHarness(t)
  await rejectCode(h.speak('nan'), 'inference')
  assert.ok(h.children[0].signalCode)
  await rejectCode(h.speak('infinity'), 'inference')
  assert.ok(h.children[1].signalCode)
  await h.speak()
  assert.equal(h.children.length, 3)
})

test('real worker counts final audio once and enforces the 45-second output bound', async (t) => {
  const h = await workerHarness(t)
  const audio = await h.speak('boundary')
  assert.equal(audio.durationMs, 45000)
  assert.equal(audio.wav.length, 44 + 8000 * 45 * 2)
  await rejectCode(h.speak('long'), 'limit')
  assert.ok(h.children[0].signalCode)
  await rejectCode(h.speak('long-no-progress'), 'limit')
  assert.ok(h.children[1].signalCode)
  await h.speak('no-progress')
})

test('public text, UUID and single-voice contract rejects before native spawn', async (t) => {
  const h = await workerHarness(t)
  await rejectCode(h.speak('x'.repeat(17)), 'limit')
  await rejectCode(h.speak('\ud800'), 'invalid')
  await rejectCode(h.speak('', randomUUID()), 'invalid')
  await rejectCode(h.speak('Hello.', 'not-a-uuid'), 'invalid')
  await rejectCode(h.speak('Hello.', randomUUID(), 'old-voice'), 'invalid')
  assert.equal(h.children.length, 0)
  await h.speak('\u{20000}'.repeat(16))
})

test('real worker UUID cancellation waits for exit and allows a fresh native process', async (t) => {
  const h = await workerHarness(t)
  const requestId = randomUUID()
  const pending = rejectCode(h.speak('hang', requestId), 'cancelled')
  await until(() => h.events.some((event) => event.event === 'native-generate'))
  assert.deepEqual(await h.service.cancelSpeech(randomUUID()), { cancelled: false })
  await rejectCode(h.speak(), 'busy')
  await rejectCode(h.service.startSession(), 'busy')
  assert.deepEqual(await h.service.cancelSpeech(requestId), { cancelled: true })
  await pending
  assert.equal(h.service.pending.size, 0)
  assert.equal(h.service.activeSpeech, null)
  assert.ok(h.children[0].signalCode)
  await h.speak()
  assert.deepEqual(h.lifecycle.slice(0, 3), [
    ['spawn', h.children[0].pid],
    ['exit', h.children[0].pid],
    ['spawn', h.children[1].pid],
  ])
})

test('real worker idle recycle and dispose release actual native child processes', async (t) => {
  const h = await workerHarness(t, { idleUnloadMs: 20 })
  await h.speak()
  await until(() => h.children[0].exitCode !== null)
  const pending = rejectCode(h.speak('hang'), 'disposed')
  await until(() => h.events.some((event) => event.text === 'hang'))
  await h.service.dispose()
  await pending
  assert.equal(h.children.length, 2)
  assert.equal(h.service.worker, null)
  assert.equal(h.service.pending.size, 0)
  assert.equal(h.service.controllers.size, 0)
})

function nativeCapture() {
  let constructions = 0
  const handler = createSpeechInferenceHandler({
    nativeModule: {
      OfflineTts: class {
        constructor() {
          constructions += 1
          this.sampleRate = 8000
          this.numSpeakers = 1
        }
        async generateAsync() {
          return { samples: new Float32Array([0, 1]), sampleRate: 8000 }
        }
      },
    },
  })
  return { handler, constructions: () => constructions }
}

async function initialize(h, fixture, model = fixture.model, extra = {}) {
  return h.handler('init', { kind: 'tts', model, modelDir: fixture.modelDir, ...extra })
}

test('synchronous native constructor fallback allows only sid 0', async (t) => {
  const fixture = await installedFixture(t)
  const h = nativeCapture()
  await initialize(h, fixture)
  assert.equal(h.constructions(), 1)
  await h.handler('synthesize', { text: 'Hello.', sid: 0 })
  for (const sid of [-1, 1, 0.5, NaN, Infinity, '0', undefined]) {
    await rejectCode(h.handler('synthesize', { text: 'Hello.', sid }), 'invalid')
  }
})

for (const [label, change] of [
  [
    'removed engine',
    (model) => {
      model.engine = 'unsupported-tts'
    },
  ],
  [
    'model escape',
    (model) => {
      model.config.model = '../model.onnx'
    },
  ],
  [
    'absolute model',
    (model) => {
      model.config.model = resolve('model.onnx')
    },
  ],
  [
    'lexicon list',
    (model) => {
      model.config.lexicon = ['lexicon.txt']
    },
  ],
  [
    'lexicon separator',
    (model) => {
      model.config.lexicon = 'lexicon.txt,other.txt'
    },
  ],
  [
    'dictionary escape',
    (model) => {
      model.config.dictDir = '../dict'
    },
  ],
  [
    'dictionary alias path',
    (model) => {
      model.config.dictDir = 'dict/../dict'
    },
  ],
  [
    'dictionary file',
    (model) => {
      model.config.dictDir = 'tokens.txt'
    },
  ],
  [
    'unlisted dictionary',
    (model) => {
      model.config.dictDir = 'other-dict'
    },
  ],
  [
    'unlisted lexicon',
    (model) => {
      model.config.lexicon = 'other.txt'
    },
  ],
  [
    'nonfinite scale',
    (model) => {
      model.config.noiseScale = Infinity
    },
  ],
  [
    'invalid length scale',
    (model) => {
      model.config.lengthScale = 0
    },
  ],
  [
    'derived voice config',
    (model) => {
      model.config.legacyVoices = {}
    },
  ],
  [
    'unlisted rule fst',
    (model) => {
      model.config.ruleFsts = ['missing.fst']
    },
  ],
  [
    'rule fst escape',
    (model) => {
      model.config.ruleFsts = ['../date.fst']
    },
  ],
]) {
  test(`download schema and worker reject ${label} before native construction`, async (t) => {
    const fixture = await installedFixture(t)
    const model = structuredClone(fixture.model)
    change(model)
    assert.throws(
      () => new SpeechModelDownloadService({ dataDir: fixture.root, catalog: [model] }),
      { code: 'catalog' },
    )
    const h = nativeCapture()
    await rejectCode(initialize(h, fixture, model), 'config')
    assert.equal(h.constructions(), 0)
  })
}

for (const [label, mutate] of [
  ['missing dictionary file', async (f) => rm(join(f.modelDir, 'dict/idf.utf8'))],
  ['unlisted dictionary file', async (f) => writeFile(join(f.modelDir, 'dict/extra'), 'extra')],
  [
    'dictionary file hardlink',
    async (f) => link(join(f.modelDir, 'dict/idf.utf8'), join(f.root, 'alias')),
  ],
  [
    'dictionary directory link',
    async (f) => {
      await rm(join(f.modelDir, 'dict'), { recursive: true })
      const outside = join(f.root, 'outside-dict')
      await mkdir(outside)
      await symlink(
        outside,
        join(f.modelDir, 'dict'),
        process.platform === 'win32' ? 'junction' : 'dir',
      )
    },
  ],
  [
    'model ancestor link',
    async (f) => {
      const alias = join(f.root, 'model-alias')
      await symlink(f.modelDir, alias, process.platform === 'win32' ? 'junction' : 'dir')
      f.modelDir = alias
    },
  ],
]) {
  test(`worker rejects ${label} even after installation verification`, async (t) => {
    const fixture = await installedFixture(t)
    await mutate(fixture)
    const h = nativeCapture()
    await rejectCode(initialize(h, fixture), 'config')
    assert.equal(h.constructions(), 0)
    if (label !== 'model ancestor link')
      await rejectCode(fixture.downloads.modelDirectory(fixture.model.id), 'missing')
  })
}
