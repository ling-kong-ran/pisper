import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { setImmediate } from 'node:timers/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import * as abortSignal from '../../src/lib/abort-signal.ts'
// 主 TypeScript 7 不再导出转译 API，沿用已安装 ts-morph 自带的真实编译器。
import ts from '@ts-morph/common/dist/typescript.js'

const compile = async (path) =>
  ts.transpileModule(await readFile(path, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText
const modelCode = await compile('src/features/chat/speech-models.ts')
const hookCode = await compile('src/features/chat/use-speech-models.ts')
const storageCode = await compile('src/app/storage.ts')
function deferred() {
  let resolve, reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
function observe(promise) {
  const result = { state: 'pending', resolutions: 0, rejections: 0 }
  result.done = promise.then(
    () => {
      result.state = 'resolved'
      result.resolutions++
    },
    (error) => {
      result.state = 'rejected'
      result.error = error
      result.rejections++
    },
  )
  return result
}
function catalog(status = 'not-installed') {
  return {
    defaults: { asr: 'asr-model', tts: 'tts-model', voice: 'voice-en' },
    models: [
      {
        id: 'asr-model',
        kind: 'asr',
        name: 'Recognition',
        languages: ['zh', 'en'],
        status,
        downloadedBytes: status === 'installed' ? 100 : 0,
        totalBytes: 100,
        filesBytes: 200,
      },
      {
        id: 'tts-model',
        kind: 'tts',
        name: 'Synthesis',
        languages: ['zh', 'en'],
        status,
        downloadedBytes: status === 'installed' ? 100 : 0,
        totalBytes: 100,
        filesBytes: 200,
        voices: [
          { id: 'voice-en', name: 'English', language: 'en' },
          { id: 'voice-zh', name: 'Chinese', language: 'zh' },
        ],
        license: { name: 'Apache-2.0', url: 'https://example.test/license' },
      },
    ],
  }
}

// 采用现有 voice-conversation 的 Hook 调度方式，目录校验和操作函数不做替身。
function fixture(t, settings = {}) {
  const slots = [],
    calls = [],
    timers = new Map(),
    stored = new Map(settings.stored ?? [])
  let cursor = 0,
    effects = [],
    dirty = false,
    mounted = true,
    state,
    nextTimer = 0,
    now = 0,
    lateWrites = 0
  let current = settings.catalog ?? catalog()
  let handler = settings.handler
  const react = {
    useRef(value) {
      const index = cursor++
      slots[index] ??= { current: value }
      return slots[index]
    },
    useState(initial) {
      const index = cursor++
      slots[index] ??= { value: typeof initial === 'function' ? initial() : initial }
      return [
        slots[index].value,
        (next) => {
          if (!mounted) lateWrites++
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
        slots[index] = { callback, deps }
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
  const get = (request) => {
    calls.push(request)
    return handler ? handler(request) : structuredClone(current)
  }
  const window = {
    __PISPER_MOBILE_APP__: Boolean(settings.android),
    location: { origin: 'https://chat-server.example.test' },
    __TAURI_INTERNALS__: { invoke: async (command, args) => get({ command, args }) },
  }
  const modules = {
    '@/lib/abort-signal': abortSignal,
    react,
    '@/lib/api': {
      apiJson: async (path, options) => {
        assert.equal(
          Boolean(settings.android),
          false,
          'Android model storage must not depend on chat-server routing',
        )
        return get({ path, options })
      },
    },
  }
  function load(code) {
    const module = { exports: {} }
    runInNewContext(code, {
      module,
      exports: module.exports,
      require: (id) => {
        assert.ok(modules[id], id)
        return modules[id]
      },
      window,
      localStorage: {
        getItem: (key) => stored.get(key) ?? null,
        setItem: (key, value) => stored.set(key, value),
      },
      AbortController,
      AbortSignal,
      DOMException,
      URL,
      Error,
      RangeError,
      setTimeout: (callback, delay) => {
        const id = ++nextTimer
        timers.set(id, { callback, at: now + delay })
        return id
      },
      clearTimeout: (id) => timers.delete(id),
    })
    return module.exports
  }
  modules['@/app/storage'] = load(storageCode)
  const api = load(modelCode)
  modules['./speech-models'] = api
  const hook = load(hookCode).useSpeechModels
  function render() {
    if (!mounted) return
    do {
      dirty = false
      cursor = 0
      effects = []
      state = hook(settings.kinds ?? ['asr', 'tts'])
      for (const effect of effects) effect()
    } while (dirty)
  }
  async function flush() {
    for (let i = 0; i < 4; i++) {
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
      timers.delete(id)
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
    api,
    calls,
    stored,
    window,
    flush,
    tick,
    render,
    unmount,
    setCatalog(value) {
      current = value
    },
    setHandler(value) {
      handler = value
    },
    get state() {
      render()
      return state
    },
    get timerCount() {
      return timers.size
    },
    get lateWrites() {
      return lateWrites
    },
    ensure() {
      const controller = new AbortController()
      return Object.assign(observe(state.ensureReady(controller.signal)), { controller })
    },
  }
}
const isDownload = (request) =>
  request.path === '/api/speech/models/download' ||
  request.command === 'mobile_download_speech_model'
const isCancel = (request) =>
  request.path === '/api/speech/models/cancel' ||
  request.command === 'mobile_cancel_speech_model_download'

for (const android of [false, true])
  test(`${android ? 'Android' : 'desktop'} missing models only open the gate until the user starts downloads`, async (t) => {
    const f = fixture(t, { android })
    await f.flush()
    assert.equal(f.state.open, false)
    const run = f.ensure()
    await f.flush()
    assert.equal(run.state, 'pending')
    assert.equal(f.state.open, true)
    assert.equal(f.calls.filter(isDownload).length, 0)
    await f.tick(1500)
    assert.equal(f.calls.filter(isDownload).length, 0)
    assert.equal(f.state.models.length, 2)
    f.setHandler((request) => {
      if (isDownload(request)) {
        const next = structuredClone(f.state.catalog)
        const id = request.args?.modelId ?? request.options.body.modelId
        next.models.find((model) => model.id === id).status = 'downloading'
        f.setCatalog(next)
        return next.models.find((model) => model.id === id)
      }
      return catalog('downloading')
    })
    const downloading = f.state.downloadAll()
    f.render()
    assert.equal(f.state.loading, true)
    await downloading
    await f.flush()
    assert.equal(f.calls.filter(isDownload).length, 2)
    assert.equal(f.state.loading, false)
    assert.ok(f.state.models.every((model) => model.status === 'downloading'))
    const progress = catalog('downloading')
    progress.models[0].downloadedBytes = 50
    progress.models[1].downloadedBytes = 25
    f.setHandler(() => structuredClone(progress))
    await f.tick(750)
    assert.equal(f.state.models[0].downloadedBytes, 50)
    assert.equal(run.state, 'pending')
    progress.models[0].status = 'installed'
    await f.tick(750)
    assert.equal(run.state, 'pending')
    progress.models[1].status = 'installed'
    await f.tick(750)
    assert.equal(run.state, 'resolved')
    assert.equal(run.resolutions, 1)
    assert.equal(f.state.open, false)
    assert.equal(f.timerCount, 0)
    const count = f.calls.length
    await f.tick(3000)
    assert.equal(f.calls.length, count)
    assert.equal(run.resolutions, 1)
  })

test('installed required models resolve without a gate and ASR-only use does not require TTS', async (t) => {
  const current = catalog()
  current.models[0].status = 'installed'
  const f = fixture(t, { catalog: current, kinds: ['asr'] })
  await f.flush()
  const run = f.ensure()
  await run.done
  await f.flush()
  assert.equal(run.state, 'resolved')
  assert.equal(f.state.open, false)
  assert.equal(f.calls.filter(isDownload).length, 0)
})

for (const reason of ['close', 'abort', 'unmount'])
  test(`${reason} cancels an open gate and ignores a late installed poll`, async (t) => {
    const f = fixture(t)
    await f.flush()
    const run = f.ensure()
    await f.flush()
    const pending = deferred()
    f.setHandler(() => pending.promise)
    await f.tick(750)
    if (reason === 'close') f.state.close()
    if (reason === 'abort') run.controller.abort()
    if (reason === 'unmount') f.unmount()
    await f.flush()
    pending.resolve(catalog('installed'))
    await f.flush()
    assert.equal(run.state, 'rejected')
    assert.equal(run.error.name, 'AbortError')
    assert.equal(run.resolutions, 0)
    assert.equal(f.timerCount, 0)
    assert.equal(f.lateWrites, 0)
    if (reason !== 'unmount') assert.equal(f.state.open, false)
  })

for (const reason of ['close', 'abort', 'unmount'])
  test(`${reason} while ensureReady fetch is pending prevents a late gate reopening`, async (t) => {
    const f = fixture(t)
    await f.flush()
    const pending = deferred()
    f.setHandler(() => pending.promise)
    const run = f.ensure()
    if (reason === 'close') f.state.close()
    if (reason === 'abort') run.controller.abort()
    if (reason === 'unmount') f.unmount()
    pending.resolve(catalog())
    await f.flush()
    assert.equal(run.state, 'rejected')
    assert.equal(run.resolutions, 0)
    assert.equal(f.lateWrites, 0)
    if (reason !== 'unmount') assert.equal(f.state.open, false)
  })

test('a stale initial setup result cannot cancel or replace a newer setup request', async (t) => {
  const f = fixture(t)
  await f.flush()
  const oldFetch = deferred(),
    newFetch = deferred()
  let requests = 0
  f.setHandler(() =>
    ++requests === 1 ? oldFetch.promise : requests === 2 ? newFetch.promise : catalog(),
  )
  const old = f.ensure()
  const current = f.ensure()
  newFetch.resolve(catalog())
  await f.flush()
  oldFetch.resolve(catalog())
  await f.flush()
  assert.equal(current.state, 'pending')
  assert.equal(old.state, 'rejected')
  f.setHandler(() => catalog('installed'))
  await f.tick(750)
  assert.equal(current.state, 'resolved')
  assert.equal(current.resolutions, 1)
})

for (const android of [false, true])
  test(`${android ? 'native' : 'desktop'} failed/cancelled downloads remain retryable`, async (t) => {
    const f = fixture(t, { android })
    await f.flush()
    const run = f.ensure()
    await f.flush()
    f.setHandler((request) => {
      if (isDownload(request)) throw new Error('download failed')
      return catalog('error')
    })
    await f.state.download('asr-model')
    await f.flush()
    assert.match(f.state.error, /download failed/)
    assert.equal(f.state.loading, false)
    assert.equal(run.state, 'pending')
    f.setHandler((request) => (isDownload(request) ? {} : catalog('downloading')))
    await f.state.download('asr-model')
    await f.flush()
    assert.equal(f.state.error, '')
    assert.equal(f.state.models[0].status, 'downloading')
    f.setHandler((request) => (isCancel(request) ? {} : catalog('cancelled')))
    await f.state.cancelDownload('asr-model')
    await f.flush()
    assert.equal(f.state.models[0].status, 'cancelled')
    assert.equal(f.calls.filter(isCancel).length, 1)
    f.setHandler((request) => (isDownload(request) ? {} : catalog('installed')))
    await f.state.downloadAll()
    await f.flush()
    assert.equal(run.state, 'resolved')
    assert.equal(f.state.loading, false)
    assert.equal(f.calls.filter(isDownload).length, 4)
  })

test('unmount during user download ignores its late refresh and never writes React state', async (t) => {
  const pending = deferred()
  const f = fixture(t)
  await f.flush()
  f.setHandler((request) => (isDownload(request) ? pending.promise : catalog('installed')))
  const run = f.state.download('asr-model')
  f.unmount()
  pending.resolve({})
  await run
  await f.flush()
  assert.equal(f.lateWrites, 0)
  assert.equal(f.timerCount, 0)
})

test('the catalog default is the only voice choice and obsolete preferences are neither read nor rewritten', async (t) => {
  const f = fixture(t, { stored: [['pisper-speech-voice', 'obsolete-voice']] })
  await f.flush()
  assert.equal(f.state.selectedVoice, 'voice-en')
  assert.equal(f.state.selectVoice, undefined)
  assert.equal(f.stored.get('pisper-speech-voice'), 'obsolete-voice')
  const restored = fixture(t, { stored: [['pisper-speech-voice', 'voice-zh']] })
  await restored.flush()
  assert.equal(restored.state.selectedVoice, 'voice-en')
})

const malformed = [
  ['null catalog', () => null],
  ['empty models', (value) => ({ ...value, models: [] })],
  [
    'null model',
    (value) => {
      value.models[0] = null
      return value
    },
  ],
  [
    'unknown status',
    (value) => {
      value.models[0].status = 'ready'
      return value
    },
  ],
  [
    'negative progress',
    (value) => {
      value.models[0].downloadedBytes = -1
      return value
    },
  ],
  [
    'progress beyond total',
    (value) => {
      value.models[0].downloadedBytes = 101
      return value
    },
  ],
  [
    'fractional total',
    (value) => {
      value.models[0].totalBytes = 0.5
      return value
    },
  ],
  [
    'duplicate models',
    (value) => {
      value.models.push(value.models[0])
      return value
    },
  ],
  [
    'missing ASR default',
    (value) => {
      value.defaults.asr = 'absent'
      return value
    },
  ],
  [
    'wrong-kind default',
    (value) => {
      value.defaults.asr = value.defaults.tts
      return value
    },
  ],
  [
    'missing voice default',
    (value) => {
      delete value.defaults.voice
      return value
    },
  ],
  [
    'unknown voice default',
    (value) => {
      value.defaults.voice = 'absent'
      return value
    },
  ],
  [
    'default voice belongs to a non-default model',
    (value) => {
      value.models.push({
        ...value.models[1],
        id: 'other-tts',
        voices: [{ id: 'other-voice', name: 'Other', language: 'en' }],
      })
      value.defaults.voice = 'other-voice'
      return value
    },
  ],
  [
    'null voice',
    (value) => {
      value.models[1].voices.push(null)
      return value
    },
  ],
  [
    'duplicate voice IDs',
    (value) => {
      value.models[1].voices.push(value.models[1].voices[0])
      return value
    },
  ],
  [
    'malformed license URL',
    (value) => {
      value.models[1].license.url = 42
      return value
    },
  ],
  [
    'malformed language list',
    (value) => {
      value.models[0].languages = null
      return value
    },
  ],
  [
    'invalid files size',
    (value) => {
      value.models[0].filesBytes = -1
      return value
    },
  ],
]
for (const [name, mutate] of malformed)
  test(`model API rejects ${name}`, async (t) => {
    const f = fixture(t)
    await f.flush()
    f.setHandler(() => mutate(catalog()))
    await assert.rejects(f.api.getLocalSpeechModels(), /invalid.*catalog/i)
  })

test('pre-aborted model setup performs no extra request or download', async (t) => {
  const f = fixture(t)
  await f.flush()
  const count = f.calls.length
  const controller = new AbortController()
  controller.abort()
  await assert.rejects(f.state.ensureReady(controller.signal), { name: 'AbortError' })
  assert.equal(f.calls.length, count)
  assert.equal(f.state.open, false)
})

test('additional stale mount refresh cannot resolve a newer missing-model gate', async (t) => {
  const initial = deferred()
  const f = fixture(t, { handler: () => initial.promise })
  f.setHandler(() => catalog())
  const run = f.ensure()
  await f.flush()
  assert.equal(run.state, 'pending')
  assert.equal(f.state.open, true)
  initial.resolve(catalog('installed'))
  await f.flush()
  assert.equal(
    run.state,
    'pending',
    'a catalog snapshot requested before ensureReady cannot make its newer missing-model snapshot ready',
  )
  assert.equal(f.state.open, true)
  assert.ok(f.state.models.every((model) => model.status === 'not-installed'))
})

for (const reason of ['close', 'abort', 'unmount'])
  test(`additional ${reason} rejects pending setup before its network response settles`, async (t) => {
    const f = fixture(t)
    await f.flush()
    const pending = deferred()
    f.setHandler(() => pending.promise)
    const run = f.ensure()
    if (reason === 'close') f.state.close()
    if (reason === 'abort') run.controller.abort()
    if (reason === 'unmount') f.unmount()
    await f.flush()
    assert.equal(run.state, 'rejected')
    assert.equal(run.error.name, 'AbortError')
    pending.reject(new Error('late catalog failure'))
    await f.flush()
    assert.equal(run.rejections, 1)
    assert.equal(f.lateWrites, 0)
    assert.equal(f.timerCount, 0)
  })

test('additional old download rejection cannot overwrite a replacement gate error state', async (t) => {
  const f = fixture(t)
  await f.flush()
  const old = f.ensure()
  await f.flush()
  const downloading = deferred()
  f.setHandler((request) => (isDownload(request) ? downloading.promise : catalog()))
  const action = f.state.download('asr-model')
  f.state.close()
  const current = f.ensure()
  await f.flush()
  assert.equal(old.state, 'rejected')
  assert.equal(current.state, 'pending')
  downloading.reject(new Error('obsolete download error'))
  await action
  await f.flush()
  assert.equal(current.state, 'pending')
  assert.equal(f.state.open, true)
  assert.equal(f.state.error, '', 'an action from the closed gate cannot contaminate the new gate')
})

test('additional downloadAll skips installed, downloading and verifying models', async (t) => {
  for (const status of ['installed', 'downloading', 'verifying']) {
    const value = catalog()
    value.models[0].status = status
    const f = fixture(t, { catalog: value })
    await f.flush()
    await f.state.downloadAll()
    const calls = f.calls.filter(isDownload)
    assert.equal(calls.length, 1)
    assert.equal(calls[0].options.body.modelId, 'tts-model')
  }
})

for (const android of [false, true])
  test(`additional ${android ? 'Android' : 'desktop'} late catalog result respects API cancellation`, async (t) => {
    const f = fixture(t, { android })
    await f.flush()
    const pending = deferred()
    const controller = new AbortController()
    f.setHandler(() => pending.promise)
    const run = observe(f.api.getLocalSpeechModels(controller.signal))
    controller.abort()
    pending.resolve(catalog('installed'))
    await run.done
    assert.equal(run.state, 'rejected')
    assert.equal(run.error.name, 'AbortError')
    assert.equal(f.calls.filter(isDownload).length, 0)
  })

for (const [name, mutate] of [
  [
    'HTTP license',
    (value) => {
      value.models[1].license.url = 'http://example.test/license'
    },
  ],
  [
    'credentialed license',
    (value) => {
      value.models[1].license.url = 'https://user:password@example.test/license'
    },
  ],
  [
    'unpaired surrogate model name',
    (value) => {
      value.models[0].name = 'bad' + String.fromCharCode(0xd800)
    },
  ],
  [
    'duplicate voices across models',
    (value) => {
      value.models.push({ ...value.models[1], id: 'tts-other' })
    },
  ],
  [
    'non-finite total',
    (value) => {
      value.models[0].totalBytes = Infinity
    },
  ],
  [
    'non-finite files bytes',
    (value) => {
      value.models[0].filesBytes = NaN
    },
  ],
])
  test(`additional model API rejects ${name} on native and desktop`, async (t) => {
    for (const android of [false, true]) {
      const f = fixture(t, { android })
      await f.flush()
      f.setHandler(() => {
        const value = catalog()
        mutate(value)
        return value
      })
      await assert.rejects(f.api.getLocalSpeechModels(), /invalid.*catalog/i)
    }
  })

test('Android model API stays local after chat-server switches and fails without the native bridge', async (t) => {
  const f = fixture(t, { android: true })
  await f.flush()
  await f.api.getLocalSpeechModels()
  f.window.location.origin = 'https://other-chat-server.example.test'
  await f.api.downloadLocalSpeechModel('asr-model')
  await f.api.cancelLocalSpeechModelDownload('asr-model')
  assert.ok(f.calls.every((call) => call.command && !call.path))
  assert.ok(f.calls.some((call) => call.command === 'mobile_speech_models'))
  assert.ok(
    f.calls.some(
      (call) =>
        call.command === 'mobile_download_speech_model' && call.args.modelId === 'asr-model',
    ),
  )
  assert.ok(
    f.calls.some(
      (call) =>
        call.command === 'mobile_cancel_speech_model_download' && call.args.modelId === 'asr-model',
    ),
  )
  delete f.window.__TAURI_INTERNALS__
  await assert.rejects(f.api.getLocalSpeechModels(), /bridge.*unavailable/i)
})
