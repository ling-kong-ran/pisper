import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { setImmediate } from 'node:timers/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
// 主 TypeScript 7 不再导出转译 API，沿用已安装 ts-morph 自带的真实编译器。
import ts from '@ts-morph/common/dist/typescript.js'
import * as abortSignal from '../../src/lib/abort-signal.ts'
import * as speechText from '../../src/features/chat/speech-text.ts'
import * as speechStreamText from '../../src/features/chat/speech-stream-text.ts'
import { createVoiceTextStream } from '../../src/features/chat/voice-response-stream.ts'

const compile = async (path) =>
  ts.transpileModule(await readFile(path, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText
const outputCode = await compile('src/features/chat/speech-output.ts')
const modelsCode = await compile('src/features/chat/speech-models.ts')
const { speechSegments } = speechText
const compact = (text) => text.replace(/\s+/g, '')
const cost = (text) => Array.from(text).length
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
function observe(promise) {
  const result = { state: 'pending' }
  result.done = promise.then(
    () => {
      result.state = 'resolved'
    },
    (error) => {
      result.state = 'rejected'
      result.error = error
    },
  )
  return result
}
function wave() {
  const bytes = new Uint8Array(48)
  const view = new DataView(bytes.buffer)
  bytes.set(new TextEncoder().encode('RIFF'), 0)
  view.setUint32(4, 40, true)
  bytes.set(new TextEncoder().encode('WAVEfmt '), 8)
  view.setUint32(16, 16, true)
  view.setUint16(20, 1, true)
  view.setUint16(22, 1, true)
  view.setUint32(24, 24000, true)
  view.setUint32(28, 48000, true)
  view.setUint16(32, 2, true)
  view.setUint16(34, 16, true)
  bytes.set(new TextEncoder().encode('data'), 36)
  view.setUint32(40, 4, true)
  return bytes
}
const response = (bytes = wave(), contentType = 'audio/wav') =>
  new Response(bytes, { headers: { 'content-type': contentType } })
const nativeAudio = (audioId = randomUUID()) => ({ audioId, sampleRate: 24000, durationMs: 20 })

// 仅替换设备和传输边界；播放器、原生桥选择及 Markdown 解析均执行生产代码。
function fixture(t, settings = {}) {
  const calls = [],
    contexts = [],
    nodes = [],
    controllers = []
  const window = {
    __PISPER_MOBILE_APP__: Boolean(settings.native),
    __TAURI__: {
      core: {
        invoke: async (command, args) => {
          calls.push({ command, args })
          if (settings.invoke) return settings.invoke(command, args)
          if (command === 'mobile_synthesize_speech') return nativeAudio()
          if (command === 'mobile_play_speech') return { completed: true }
          if (command === 'mobile_cancel_speech') return {}
          throw new Error(`Unexpected native command ${command}`)
        },
      },
    },
  }
  const apiJson = async (path, options) => {
    calls.push({ path, options })
    assert.equal(Boolean(settings.native), false, 'native speech must never use a chat-server API')
    if (settings.apiJson) return settings.apiJson(path, options)
    assert.equal(path, '/api/speech/cancel')
    return {}
  }
  class AudioContext {
    constructor(options) {
      this.options = options
      this.destination = {}
      this.closed = 0
      contexts.push(this)
      calls.push({ event: 'context' })
    }
    resume() {
      calls.push({ event: 'resume' })
      return settings.resume?.promise ?? Promise.resolve()
    }
    async decodeAudioData(bytes) {
      calls.push({ event: 'decode', bytes })
      if (settings.decodeError) throw settings.decodeError
      if (settings.decode) return settings.decode.promise
      assert.deepEqual(new Uint8Array(bytes), wave())
      return settings.buffer ?? { numberOfChannels: 1, duration: 0.02 }
    }
    createBufferSource() {
      const node = {
        buffer: null,
        started: 0,
        stopped: 0,
        disconnected: 0,
        connect: (destination) => assert.equal(destination, this.destination),
        start() {
          assert.equal(
            nodes.filter((value) => value.started && !value.ended && !value.stopped).length,
            0,
            'audio sources must not overlap',
          )
          this.started++
          calls.push({ event: 'start' })
          if (settings.startError) throw settings.startError
        },
        stop() {
          this.stopped++
          this.end()
        },
        disconnect() {
          this.disconnected++
        },
        end() {
          this.ended = true
          this.onended?.()
        },
      }
      nodes.push(node)
      return node
    }
    async close() {
      this.closed++
      calls.push({ event: 'close' })
    }
  }
  const modules = {
    '@/lib/abort-signal': abortSignal,
    '@/lib/api': { apiJson },
    './speech-text': speechText,
    './speech-stream-text': speechStreamText,
  }
  function load(code) {
    const module = { exports: {} }
    runInNewContext(code, {
      module,
      exports: module.exports,
      require: (id) => {
        if (id === './speech-text' && settings.importing) return settings.importing.promise
        assert.ok(modules[id], id)
        return modules[id]
      },
      window,
      AudioContext,
      crypto: { randomUUID },
      AbortController,
      AbortSignal: settings.legacyAbortSignal ? undefined : AbortSignal,
      DOMException,
      Error,
      RangeError,
      Uint8Array,
      ArrayBuffer,
      setTimeout,
      clearTimeout,
      fetch: async (path, options) => {
        assert.equal(Boolean(settings.native), false, 'native speech must never fetch remote WAV')
        const call = { path, options, body: JSON.parse(options.body) }
        calls.push(call)
        return settings.fetch
          ? settings.fetch(
              call,
              calls.filter((value) => value.path === '/api/speech/synthesize').length,
            )
          : response()
      },
    })
    return module.exports
  }
  modules['./speech-models'] = load(modelsCode)
  const { playLocalSpeech } = load(outputCode)
  t.after(async () => {
    controllers.forEach((controller) => controller.abort())
    await flush()
  })
  return {
    calls,
    contexts,
    nodes,
    window,
    run(text = 'First sentence. Second sentence. Third sentence.') {
      const controller = new AbortController()
      controllers.push(controller)
      return Object.assign(observe(playLocalSpeech(text, 'voice-en', controller.signal)), {
        controller,
      })
    },
    playLocalSpeech,
  }
}

test('streamed sentences synthesize and play before EOF with only one prepared successor', async (t) => {
  const playing = deferred()
  const f = fixture(t, {
    native: true,
    invoke(command) {
      if (command === 'mobile_synthesize_speech') return nativeAudio()
      if (command === 'mobile_play_speech') return playing.promise
      return {}
    },
  })
  const controller = new AbortController()
  t.after(() => controller.abort())
  const stream = createVoiceTextStream(controller.signal)
  const run = observe(f.playLocalSpeech(stream, 'voice-en', controller.signal))
  const texts = () =>
    f.calls
      .filter((call) => call.command === 'mobile_synthesize_speech')
      .map((call) => call.args.text)
  const plays = () => f.calls.filter((call) => call.command === 'mobile_play_speech')
  stream.update('第一句。')
  await flush()
  assert.deepEqual(texts(), ['第一句。'])
  assert.equal(plays().length, 1)
  assert.equal(run.state, 'pending')
  stream.update('第一句。第二句。')
  await flush()
  assert.deepEqual(texts(), ['第一句。', '第二句。'])
  assert.equal(plays().length, 1)
  stream.update('第一句。第二句。最后')
  stream.finish()
  await flush()
  assert.equal(texts().length, 2)
  playing.resolve({ completed: true })
  await run.done
  assert.equal(run.state, 'resolved')
  assert.deepEqual(texts(), ['第一句。', '第二句。', '最后'])
  assert.equal(plays().length, 3)
})

for (const native of [false, true])
  for (const deltas of [['你好，'], ['你', '好', '，'], ['Hello '], ['Hel', 'lo', ' ']])
    test(`${native ? 'native' : 'desktop'} synthesizes a closed first phrase before later deltas: ${deltas.join('|')}`, async (t) => {
      t.mock.timers.enable({ apis: ['setTimeout', 'Date'] })
      const playing = deferred()
      const f = fixture(t, {
        native,
        invoke: (command) =>
          command === 'mobile_synthesize_speech' ? nativeAudio() : playing.promise,
      })
      const controller = new AbortController()
      t.after(() => controller.abort())
      const stream = createVoiceTextStream(controller.signal)
      const run = observe(f.playLocalSpeech(stream, 'voice-zh', controller.signal))
      const texts = () =>
        f.calls
          .filter((call) =>
            native
              ? call.command === 'mobile_synthesize_speech'
              : call.path === '/api/speech/synthesize',
          )
          .map((call) => (native ? call.args.text : call.body.text))
      const phrase = deltas.join('').trim()
      let snapshot = ''
      for (const [index, delta] of deltas.entries()) {
        snapshot += delta
        stream.update(snapshot)
        await flush()
        assert.deepEqual(texts(), index < deltas.length - 1 ? [] : [phrase])
      }
      assert.equal(run.state, 'pending')
      assert.equal(
        native
          ? f.calls.filter((call) => call.command === 'mobile_play_speech').length
          : f.nodes.filter((node) => node.started).length,
        1,
      )
      // 合成和播放已经发生，随后才允许后续 SSE 文本进入生产播放器。
      for (const delta of ['今天', '很高兴']) {
        snapshot += delta
        stream.update(snapshot)
        await flush()
        assert.deepEqual(texts(), [phrase])
      }
      stream.finish()
      await flush()
      assert.deepEqual(texts(), [phrase, '今天很高兴'])
      if (native) playing.resolve({ completed: true })
      else {
        f.nodes[0].end()
        await flush()
        assert.equal(f.nodes.length, 2)
        f.nodes[1].end()
      }
      await run.done
      assert.equal(run.state, 'resolved', run.error?.stack)
      assert.equal(texts().join(''), `${phrase}今天很高兴`)
    })

for (const native of [false, true])
  for (const text of [
    '好的，',
    '我已经看到你说的问题，',
    '这是一个没有句号但需要在回复结束之前开始播报的长句',
  ])
    test(`${native ? 'native' : 'desktop'} starts a streaming fragment before punctuation or EOF: ${text}`, async (t) => {
      const playing = deferred()
      const f = fixture(t, {
        native,
        invoke: (command) =>
          command === 'mobile_synthesize_speech' ? nativeAudio() : playing.promise,
      })
      const controller = new AbortController()
      t.after(() => controller.abort())
      const stream = createVoiceTextStream(controller.signal)
      const run = observe(f.playLocalSpeech(stream, 'voice-en', controller.signal))
      stream.update(text)
      await flush()
      const synthesized = f.calls.filter((call) =>
        native
          ? call.command === 'mobile_synthesize_speech'
          : call.path === '/api/speech/synthesize',
      )
      assert.ok(synthesized.length >= 1 && synthesized.length <= 2)
      const first = native ? synthesized[0].args.text : synthesized[0].body.text
      assert.ok(text.startsWith(first))
      assert.ok(cost(first) <= 16)
      assert.equal(
        native
          ? f.calls.filter((call) => call.command === 'mobile_play_speech').length
          : f.nodes.filter((node) => node.started).length,
        1,
      )
      assert.equal(run.state, 'pending')
      controller.abort()
      await run.done
      assert.equal(run.error.name, 'AbortError')
      playing.resolve({ completed: true })
    })

for (const text of [
  '中文全文需要完整保留，第二句话不能消失。最后一句也要读完！'.repeat(12),
  'Every ordinary English word must remain intact, including the final sentence! '.repeat(12),
  '请运行 npm install，然后检查 TypeScript types. 最后运行 Cargo test！'.repeat(12),
])
  test(`real mdast preserves complete ${text.slice(0, 16)} with bounded segments`, () => {
    const segments = speechSegments(text, 32)
    assert.equal(compact(segments.join('')), compact(text))
    assert.ok(segments.every((segment) => cost(segment) <= 32))
    assert.ok(segments.length > 3)
    const defaults = speechSegments(text)
    assert.equal(compact(defaults.join('')), compact(text))
    assert.ok(defaults.every((segment) => cost(segment) <= 16))
  })

test('word and punctuation boundaries retain short words and split only oversized tokens', () => {
  const text = 'Alpha beta gamma delta! Another sentence, with words.'
  const segments = speechSegments(text, 16)
  assert.deepEqual(
    segments.flatMap((segment) => segment.match(/[A-Za-z]+/g)),
    text.match(/[A-Za-z]+/g),
  )
  assert.ok(segments.every((segment) => cost(segment) <= 16))
  assert.deepEqual(speechSegments('One. Two! Three?'), ['One.', 'Two!', 'Three?'])
  assert.equal(speechSegments('a'.repeat(200), 16).join(''), 'a'.repeat(200))
})

test('supplementary characters remain well formed at every segment boundary', () => {
  const text = '𠀀'.repeat(40) + ' music 𝄞 and English'
  const segments = speechSegments(text, 8)
  assert.equal(compact(segments.join('')), compact(text))
  assert.ok(segments.every((segment) => segment.isWellFormed() && cost(segment) <= 8))
  assert.throws(() => speechSegments('bad\ud800'), /limit|invalid/i)
})

test('real GFM extracts tables, lists, references and short inline code without speaking excluded nodes', () => {
  const markdown = [
    '# Heading',
    '',
    '- First [label](https://example.test/path)',
    '- Second `npm install`',
    '',
    '| Name | Value |',
    '| --- | --- |',
    '| alpha | beta |',
    '',
    '[reference][target]',
    '',
    '[target]: https://example.test/secret',
    '',
    '![SECRET_IMAGE](https://example.test/image.png)',
    '',
    '```js',
    'SECRET_BLOCK()',
    '```',
    '',
    '<div>SECRET_HTML</div>',
    '',
    '`x = dangerous();`',
    '',
    '`' + 'LONG_CODE'.repeat(20) + '`',
    '',
    'Tail',
  ].join('\n')
  const spoken = speechSegments(markdown).join(' ')
  for (const expected of [
    'Heading',
    'First label',
    'Second npm install',
    'Name',
    'Value',
    'alpha',
    'beta',
    'reference',
    'Tail',
  ])
    assert.ok(spoken.includes(expected), expected)
  assert.doesNotMatch(spoken, /SECRET|https:|dangerous|LONG_CODE|target/)
})

test('empty/excluded-only input and excessive text/segment count never silently truncate', () => {
  assert.deepEqual(speechSegments('```js\nignored\n```'), [])
  assert.throws(() => speechSegments('a'.repeat(32001)), /limit/i)
  assert.throws(() => speechSegments('a. '.repeat(513)), /too many|limit/i)
  for (const limit of [0, 7, 161, 8.5, NaN])
    assert.throws(() => speechSegments('test', limit), /limit/i)
})

for (const native of [false, true]) {
  test(`${native ? 'native' : 'desktop'} playback works without static AbortSignal APIs`, async (t) => {
    const f = fixture(t, { native, legacyAbortSignal: true })
    const run = f.run('Hello.')
    await flush()
    if (!native) {
      assert.equal(f.nodes.length, 1)
      f.nodes[0].end()
    }
    await run.done
    assert.equal(run.state, 'resolved', run.error?.stack)
    if (native) assert.ok(f.calls.some((call) => call.command === 'mobile_play_speech'))
  })
}

test('desktop fetches WAV, decodes it, plays serially with one next-sentence prefetch, then closes', async (t) => {
  const f = fixture(t)
  const run = f.run()
  await flush()
  assert.equal(f.calls.filter((call) => call.path === '/api/speech/synthesize').length, 2)
  assert.equal(f.nodes.length, 1)
  assert.equal(f.nodes[0].started, 1)
  assert.equal(run.state, 'pending')
  const first = f.calls.find((call) => call.path === '/api/speech/synthesize')
  assert.equal(first.options.method, 'POST')
  assert.equal(first.body.voiceId, 'voice-en')
  assert.match(first.body.requestId, /^[0-9a-f]{8}-[0-9a-f-]{27}$/)
  assert.equal(f.contexts[0].options.sampleRate, 24000)
  assert.ok(
    f.calls.findIndex((call) => call.path === '/api/speech/synthesize') <
      f.calls.findIndex((call) => call.event === 'decode'),
  )
  assert.ok(
    f.calls.findIndex((call) => call.event === 'decode') <
      f.calls.findIndex((call) => call.event === 'start'),
  )
  f.nodes[0].end()
  await flush()
  assert.equal(f.calls.filter((call) => call.path === '/api/speech/synthesize').length, 3)
  assert.equal(f.nodes.length, 2)
  f.nodes[1].end()
  await flush()
  f.nodes[2].end()
  await run.done
  assert.equal(run.state, 'resolved')
  assert.equal(f.contexts[0].closed, 1)
  assert.ok(f.nodes.every((node) => node.disconnected === 1))
  assert.equal(f.calls.filter((call) => call.path === '/api/speech/cancel').length, 0)
})

for (const stage of ['import', 'resume', 'fetch', 'decode', 'play', 'prefetch']) {
  for (const late of ['resolve', 'reject'])
    test(`abort during ${stage} ignores late ${late} without playback or unhandled rejection`, async (t) => {
      const pending = deferred()
      const settings =
        stage === 'import'
          ? { importing: pending }
          : stage === 'resume'
            ? { resume: pending }
            : stage === 'decode'
              ? { decode: pending }
              : stage === 'fetch'
                ? { fetch: () => pending.promise }
                : stage === 'prefetch'
                  ? { fetch: (_call, index) => (index === 2 ? pending.promise : response()) }
                  : {}
      const f = fixture(t, settings)
      const run = f.run()
      await flush()
      const starts = f.nodes.length
      run.controller.abort()
      if (late === 'reject' && stage !== 'play') pending.reject(new Error('late failure'))
      else
        pending.resolve(
          stage === 'import'
            ? speechText
            : stage === 'decode'
              ? { numberOfChannels: 1, duration: 0.02 }
              : stage === 'fetch' || stage === 'prefetch'
                ? response()
                : undefined,
        )
      await flush()
      assert.equal(run.state, 'rejected')
      assert.equal(run.error.name, 'AbortError')
      assert.equal(f.nodes.length, starts)
      assert.ok(f.contexts.every((context) => context.closed === 1))
      assert.ok(f.nodes.every((node) => node.stopped === 1 && node.disconnected === 1))
      const ids = f.calls
        .filter((call) => call.path === '/api/speech/synthesize')
        .map((call) => call.body.requestId)
      assert.ok(
        f.calls
          .filter((call) => call.path === '/api/speech/cancel')
          .every((call) => ids.includes(call.options.body.requestId)),
      )
    })
}

test('prefetch rejection is handled immediately and reported when the current sentence ends', async (t) => {
  const f = fixture(t, {
    fetch: (_call, index) =>
      index === 2 ? Promise.reject(new Error('prefetch failed')) : response(),
  })
  const run = f.run()
  await flush()
  assert.equal(run.state, 'pending')
  assert.equal(f.nodes.length, 1)
  f.nodes[0].end()
  await run.done
  assert.equal(run.state, 'rejected')
  assert.match(run.error.message, /prefetch failed/)
  assert.equal(f.contexts[0].closed, 1)
})

for (const [name, settings, text] of [
  ['empty input', {}, ''],
  ['excluded input', {}, '```js\ncode\n```'],
  ['overlong input', {}, 'a'.repeat(32001)],
  [
    'HTTP failure',
    { fetch: () => new Response(JSON.stringify({ error: 'synthesis failed' }), { status: 500 }) },
  ],
  ['network failure', { fetch: () => Promise.reject(new Error('network failed')) }],
  ['wrong MIME', { fetch: () => response(wave(), 'text/plain') }],
  ['empty WAV', { fetch: () => response(new Uint8Array()) }],
  ['oversized WAV', { fetch: () => response(new Uint8Array(8 * 1024 * 1024 + 1)) }],
  [
    'corrupt WAV',
    { fetch: () => response(new Uint8Array(48)), decodeError: new Error('Invalid WAV') },
  ],
  ['stereo audio', { buffer: { numberOfChannels: 2, duration: 1 } }],
  ['empty decoded audio', { buffer: { numberOfChannels: 1, duration: 0 } }],
  ['overlong decoded audio', { buffer: { numberOfChannels: 1, duration: 46 } }],
  ['non-finite decoded duration', { buffer: { numberOfChannels: 1, duration: NaN } }],
  ['source start failure', { startError: new Error('start failed') }],
])
  test(`${name} cannot resolve as successful speech`, async (t) => {
    const f = fixture(t, settings)
    const run = f.run(text ?? 'Hello.')
    await flush()
    for (const node of f.nodes) node.end()
    await flush()
    assert.equal(run.state, 'rejected')
    assert.ok(f.contexts.every((context) => context.closed === 1))
  })

test('native bridge passes opaque audio IDs and request UUIDs, with bounded serial playback and no API', async (t) => {
  const plays = []
  const f = fixture(t, {
    native: true,
    invoke(command, args) {
      if (command === 'mobile_synthesize_speech') return nativeAudio(`opaque-${args.requestId}`)
      if (command === 'mobile_play_speech') {
        const pending = deferred()
        plays.push(pending)
        return pending.promise
      }
      return {}
    },
  })
  const run = f.run()
  await flush()
  assert.equal(f.calls.filter((call) => call.command === 'mobile_synthesize_speech').length, 2)
  assert.equal(plays.length, 1)
  for (let index = 0; index < 3; index++) {
    plays[index].resolve({ completed: true })
    await flush()
  }
  assert.equal(run.state, 'resolved')
  assert.equal(f.contexts.length, 0)
  const synth = f.calls.filter((call) => call.command === 'mobile_synthesize_speech')
  assert.equal(new Set(synth.map((call) => call.args.requestId)).size, 3)
  for (const call of f.calls.filter((call) => call.command === 'mobile_play_speech')) {
    assert.match(call.args.requestId, /^[0-9a-f]{8}-[0-9a-f-]{27}$/)
    assert.equal(call.args.audioId, `opaque-${call.args.requestId}`)
    assert.deepEqual(Object.keys(call.args).sort(), ['audioId', 'requestId'])
  }
})

for (const stage of ['synthesize', 'play', 'prefetch'])
  for (const late of ['resolve', 'reject'])
    test(`native abort during ${stage} cleans its own late ${late}`, async (t) => {
      const pending = deferred()
      let synths = 0
      const f = fixture(t, {
        native: true,
        invoke(command) {
          if (command === 'mobile_synthesize_speech') {
            synths++
            return stage === 'synthesize' || (stage === 'prefetch' && synths === 2)
              ? pending.promise
              : nativeAudio()
          }
          if (command === 'mobile_play_speech')
            return stage === 'play' ? pending.promise : new Promise(() => {})
          return {}
        },
      })
      const run = f.run()
      await flush()
      run.controller.abort()
      if (late === 'resolve')
        pending.resolve(stage === 'play' ? { completed: true } : nativeAudio('late-opaque-id'))
      else pending.reject(new Error('late native failure'))
      await flush()
      assert.equal(run.state, 'rejected')
      const owned = f.calls
        .filter((call) => call.command === 'mobile_synthesize_speech')
        .map((call) => call.args.requestId)
      const cancelled = f.calls.filter((call) => call.command === 'mobile_cancel_speech')
      assert.ok(owned.every((id) => cancelled.some((call) => call.args.requestId === id)))
      assert.ok(cancelled.every((call) => owned.includes(call.args.requestId)))
      assert.ok(!f.calls.some((call) => call.args?.audioId === 'late-opaque-id'))
    })

test('cancelling one native caller never cancels a simultaneous caller', async (t) => {
  const waiting = new Map()
  const f = fixture(t, {
    native: true,
    invoke(command, args) {
      if (command === 'mobile_synthesize_speech') return nativeAudio()
      if (command === 'mobile_play_speech') {
        const pending = deferred()
        waiting.set(args.requestId, pending)
        return pending.promise
      }
      return {}
    },
  })
  const first = f.run('First.')
  await flush()
  const firstId = [...waiting.keys()][0]
  const second = f.run('Second.')
  await flush()
  const secondId = [...waiting.keys()][1]
  first.controller.abort()
  await flush()
  assert.equal(first.state, 'rejected')
  assert.equal(second.state, 'pending')
  assert.ok(
    f.calls
      .filter((call) => call.command === 'mobile_cancel_speech')
      .every((call) => call.args.requestId === firstId),
  )
  waiting.get(secondId).resolve({ completed: true })
  await second.done
  assert.equal(second.state, 'resolved')
})

for (const [name, value] of [
  ['missing ID', { sampleRate: 24000, durationMs: 1 }],
  ['empty duration', { audioId: 'opaque', durationMs: 0 }],
  ['overlong duration', { audioId: 'opaque', durationMs: 45001 }],
  ['NaN duration', { audioId: 'opaque', durationMs: NaN }],
])
  test(`native ${name} is rejected before playback`, async (t) => {
    const f = fixture(t, {
      native: true,
      invoke: (command) => (command === 'mobile_synthesize_speech' ? value : {}),
    })
    const run = f.run('Hello.')
    await run.done
    assert.equal(run.state, 'rejected')
    assert.ok(!f.calls.some((call) => call.command === 'mobile_play_speech'))
  })

test('pre-aborted playback performs no imports, synthesis or device allocation', async (t) => {
  const f = fixture(t)
  const controller = new AbortController()
  controller.abort()
  await assert.rejects(f.playLocalSpeech('Hello.', 'voice-en', controller.signal), {
    name: 'AbortError',
  })
  assert.equal(f.calls.length, 0)
  assert.equal(f.contexts.length, 0)
})

test('native bridge failure never falls back to remote synthesis', async (t) => {
  const f = fixture(t, { native: true })
  delete f.window.__TAURI__
  const run = f.run('Hello.')
  await run.done
  assert.equal(run.state, 'rejected')
  assert.match(run.error.message, /bridge.*unavailable/i)
  assert.equal(f.calls.length, 0)
  assert.equal(f.contexts.length, 0)
})

for (const [name, value] of [
  ['numeric audio ID', { audioId: 42, sampleRate: 24000, durationMs: 20 }],
  ['missing sample rate', { audioId: 'opaque', durationMs: 20 }],
  ['invalid sample rate', { audioId: 'opaque', sampleRate: -1, durationMs: 20 }],
])
  test(`native malformed ${name} is rejected rather than played`, async (t) => {
    const f = fixture(t, {
      native: true,
      invoke: (command) => (command === 'mobile_synthesize_speech' ? value : { completed: true }),
    })
    const run = f.run('Hello.')
    await run.done
    assert.equal(run.state, 'rejected')
    assert.ok(!f.calls.some((call) => call.command === 'mobile_play_speech'))
  })

test('native truthy non-boolean completion is rejected', async (t) => {
  const f = fixture(t, {
    native: true,
    invoke: (command) =>
      command === 'mobile_synthesize_speech' ? nativeAudio() : { completed: 'false' },
  })
  const run = f.run('Hello.')
  await run.done
  assert.equal(run.state, 'rejected')
})

test('additional sentence-leading whitespace does not create a punctuation-only segment', () => {
  assert.deepEqual(speechSegments('First sentence. Second sentence. Third sentence.'), [
    'First sentence.',
    'Second sentence.',
    'Third sentence.',
  ])
})

test('additional inline HTML script bodies are not spoken', () => {
  const spoken = speechSegments('Before <script>SECRET_SCRIPT</script> after.').join(' ')
  assert.match(spoken, /Before/)
  assert.match(spoken, /after/)
  assert.doesNotMatch(spoken, /SECRET_SCRIPT/)
})

for (const late of ['resolve', 'reject'])
  test(`additional abort during WAV body read ignores late ${late}`, async (t) => {
    const pending = deferred()
    let reads = 0,
      cancelled = 0,
      released = 0
    const reader = {
      read() {
        reads++
        return reads === 1 ? pending.promise : Promise.resolve({ done: true })
      },
      async cancel() {
        cancelled++
      },
      releaseLock() {
        released++
      },
    }
    const f = fixture(t, {
      fetch: () => ({
        ok: true,
        headers: new Headers({ 'content-type': 'audio/wav' }),
        body: { getReader: () => reader },
      }),
    })
    const run = f.run('Hello.')
    await flush()
    assert.equal(reads, 1)
    run.controller.abort()
    await flush()
    assert.equal(run.state, 'rejected')
    assert.equal(run.error.name, 'AbortError')
    assert.equal(f.contexts[0].closed, 1)
    if (late === 'resolve') pending.resolve({ done: false, value: wave() })
    else pending.reject(new Error('late stream failure'))
    await flush()
    assert.equal(f.nodes.length, 0)
    assert.equal(cancelled, 1)
    assert.equal(released, 1)
  })

test('additional desktop cancellation only addresses the owning request UUID', async (t) => {
  const f = fixture(t)
  const first = f.run('First.')
  await flush()
  const firstId = f.calls.find((call) => call.path === '/api/speech/synthesize').body.requestId
  const pending = deferred()
  const other = fixture(t, { fetch: () => pending.promise })
  const second = other.run('Second.')
  await flush()
  first.controller.abort()
  await flush()
  assert.equal(first.state, 'rejected')
  assert.equal(second.state, 'pending')
  assert.ok(
    f.calls
      .filter((call) => call.path === '/api/speech/cancel')
      .every((call) => call.options.body.requestId === firstId),
  )
  assert.equal(other.calls.filter((call) => call.path === '/api/speech/cancel').length, 0)
  pending.resolve(response())
  await flush()
  other.nodes[0].end()
  await second.done
  assert.equal(second.state, 'resolved')
})

for (const [name, value] of [
  ['blank ID', { ...nativeAudio(), audioId: '  ' }],
  ['oversized ID', { ...nativeAudio(), audioId: 'a'.repeat(129) }],
  ['low sample rate', { ...nativeAudio(), sampleRate: 7999 }],
  ['high sample rate', { ...nativeAudio(), sampleRate: 48001 }],
  ['fractional sample rate', { ...nativeAudio(), sampleRate: 24000.5 }],
  ['infinite duration', { ...nativeAudio(), durationMs: Infinity }],
])
  test(`additional native ${name} is rejected and its request cancelled`, async (t) => {
    const f = fixture(t, {
      native: true,
      invoke: (command) => (command === 'mobile_synthesize_speech' ? value : {}),
    })
    const run = f.run('Hello.')
    await run.done
    assert.equal(run.state, 'rejected')
    assert.equal(f.calls.filter((call) => call.command === 'mobile_play_speech').length, 0)
    const id = f.calls.find((call) => call.command === 'mobile_synthesize_speech').args.requestId
    assert.ok(
      f.calls.some((call) => call.command === 'mobile_cancel_speech' && call.args.requestId === id),
    )
  })

test('additional failed native prefetch cleans a late successful synthesis without playing it', async (t) => {
  const pending = deferred()
  const playing = deferred()
  let synths = 0
  const f = fixture(t, {
    native: true,
    invoke(command) {
      if (command === 'mobile_synthesize_speech')
        return ++synths === 1 ? nativeAudio('first') : pending.promise
      if (command === 'mobile_play_speech') return playing.promise
      return {}
    },
  })
  const run = f.run('One. Two.')
  await flush()
  playing.resolve({ completed: false })
  await flush()
  assert.equal(run.state, 'rejected')
  const nextId = f.calls.filter((call) => call.command === 'mobile_synthesize_speech')[1].args
    .requestId
  const cancellations = () =>
    f.calls.filter(
      (call) => call.command === 'mobile_cancel_speech' && call.args.requestId === nextId,
    ).length
  const before = cancellations()
  assert.ok(before > 0)
  pending.resolve(nativeAudio('late-after-failure'))
  await flush()
  assert.ok(!f.calls.some((call) => call.args?.audioId === 'late-after-failure'))
  assert.ok(
    cancellations() > before,
    'late native output must be disposed even when failure, not user abort, ended playback',
  )
})

test('native completed:false is an error, never successful speech', async (t) => {
  const f = fixture(t, {
    native: true,
    invoke: (command) =>
      command === 'mobile_synthesize_speech' ? nativeAudio() : { completed: false },
  })
  const run = f.run('Hello.')
  await run.done
  assert.equal(run.state, 'rejected')
  assert.match(run.error.message, /interrupted/)
})
