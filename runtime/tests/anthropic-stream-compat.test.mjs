import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { withAnthropicStreamCompatibility } from '../services/anthropic-stream-compat.mjs'
import { streamSideQuestion } from '../runtime/side-question.mjs'

function frames(usage, repeats = 1) {
  const events = [
    {
      type: 'message_start',
      message: {
        id: 'msg_fixture',
        type: 'message',
        role: 'assistant',
        model: 'test-model',
        content: [],
        stop_reason: null,
        ...(usage === undefined ? {} : { usage }),
      },
    },
    { type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } },
    { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'hi' } },
    { type: 'content_block_stop', index: 0 },
  ]
  for (let index = 0; index < repeats; index++) {
    events.push(
      {
        type: 'message_delta',
        delta: { stop_reason: 'end_turn' },
        usage: { input_tokens: 2, output_tokens: 27 },
      },
      { type: 'message_stop' },
    )
  }
  return events.map((event) => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join('')
}

function response(body) {
  return new Response(body, { headers: { 'content-type': 'text/event-stream' } })
}

test('Anthropic gateways tolerate missing start usage through every ModelRuntime entry', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-anthropic-compat-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  await runtime.createProvider({
    id: 'compat-test',
    name: 'Compatibility Test',
    api: 'anthropic-messages',
    baseUrl: 'https://relay.example/anthropic',
    apiKey: 'fixture-key',
    model: 'test-model',
  })
  const model = runtime.modelRuntime.getModel('compat-test', 'test-model')
  const context = { messages: [{ role: 'user', content: '你好', timestamp: 0 }] }
  for (const method of ['stream', 'streamSimple', 'complete', 'completeSimple']) {
    for (const usage of [
      undefined,
      null,
      {},
      { input_tokens: 9, output_tokens: 0, cache_read_input_tokens: 4 },
    ]) {
      for (const repeats of [1, 3]) {
        let observed = false
        const result = runtime.modelRuntime[method](model, context, {
          maxTokens: 64,
          fetch: async () => response(frames(usage, repeats)),
          onProviderStreamEvent: async (event, receivedModel) => {
            assert.equal(receivedModel.id, model.id)
            await Promise.resolve()
            if (event.type === 'message_start') observed = true
          },
        })
        const events = []
        if (method.startsWith('stream')) for await (const event of result) events.push(event)
        const message = await (method.startsWith('stream') ? result.result() : result)
        assert.equal(message.stopReason, 'stop', `${method}: ${message.errorMessage}`)
        assert.deepEqual(message.content, [{ type: 'text', text: 'hi' }])
        assert.equal(message.usage.input, 2)
        assert.equal(message.usage.output, 27)
        assert.equal(message.usage.cacheRead, usage?.cache_read_input_tokens || 0)
        assert.equal(message.usage.totalTokens, 29 + (usage?.cache_read_input_tokens || 0))
        assert.equal(observed, true)
        if (events.length) {
          assert.equal(events.filter((event) => event.type === 'done').length, 1)
          assert.equal(
            events
              .filter((event) => event.type === 'text_delta')
              .map((event) => event.delta)
              .join(''),
            'hi',
          )
        }
      }
    }
  }
  // 旁问使用同一装饰过的运行时，不在每个调用方各补一遍。
  const streamSimple = runtime.modelRuntime.streamSimple.bind(runtime.modelRuntime)
  const side = await streamSideQuestion({
    modelRuntime: {
      streamSimple: (model, context, options) =>
        streamSimple(model, context, {
          ...options,
          fetch: async () => response(frames(undefined, 3)),
        }),
    },
    session: { model, messages: [], sessionId: 'fixture-session' },
    question: '你好',
  })
  assert.equal(side.text, 'hi')
  assert.equal(side.usage.output, 27)
})

test('compatibility preserves real usage, caller hooks and unrelated events', async () => {
  const options = withAnthropicStreamCompatibility(undefined)
  for (const event of [
    null,
    [],
    'invalid',
    {},
    { type: 'message_start', message: null },
    { type: 'message_delta', usage: null },
  ]) {
    const original = structuredClone(event)
    await options.onProviderStreamEvent(event, {})
    assert.deepEqual(event, original)
  }
  const usage = { input_tokens: 7, cache_creation: { ephemeral_1h_input_tokens: 5 } }
  const event = { type: 'message_start', message: { usage } }
  await options.onProviderStreamEvent(event, {})
  assert.equal(event.message.usage, usage)
  const hooked = withAnthropicStreamCompatibility({
    onProviderStreamEvent: async (event) => {
      event.message.usage = usage
    },
  })
  const missing = { type: 'message_start', message: {} }
  await hooked.onProviderStreamEvent(missing, {})
  assert.equal(missing.message.usage, usage)
  const failure = new Error('caller failure')
  const throwing = withAnthropicStreamCompatibility({
    onProviderStreamEvent: async () => {
      throw failure
    },
  })
  await assert.rejects(throwing.onProviderStreamEvent({}, {}), (error) => error === failure)
})
