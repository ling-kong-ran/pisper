import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { setImmediate } from 'node:timers/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import * as sessionState from '../../src/lib/session-state.ts'
import { applyTextPatch, consumeEventStream } from '../../src/lib/api.ts'
import * as responseStream from '../../src/features/chat/voice-response-stream.ts'

const compiled = transformSync(
  await readFile('src/features/chat/stream-event-dispatch.ts', 'utf8'),
  { loader: 'ts', format: 'cjs' },
).code

test('actual SSE byte frames publish reconstructed text and prompt ownership before UI animation', async (t) => {
  const updates = []
  t.after(responseStream.subscribeVoiceResponse('session', (event) => updates.push(event)))
  const modules = {
    '@/lib/api': { applyTextPatch },
    './voice-response-stream': responseStream,
    '@/lib/plan-protocol': {
      isPlanUpdateEvent: () => false,
      planFromPayloadOr: (data, fallback) => data.plan ?? fallback,
    },
    '@/lib/session-state': sessionState,
    './mobile-operations': {},
    './run-activity': { settleToolCalls: (tools) => tools || [] },
  }
  const module = { exports: {} }
  runInNewContext(compiled, {
    module,
    exports: module.exports,
    require: (id) => {
      assert.ok(modules[id], id)
      return modules[id]
    },
  })
  const ref = {
    current: {
      session: {
        messages: [
          { id: 'u', role: 'user', text: 'spoken prompt' },
          { id: 'a', role: 'agent', text: '', streaming: true },
        ],
      },
    },
  }
  const dispatcher = module.exports.createStreamEventDispatcher({
    sessionId: 'session',
    agentId: 'a',
    sessionStatesRef: ref,
    updateSessionState(id, update) {
      ref.current[id] =
        typeof update === 'function' ? update(ref.current[id]) : { ...ref.current[id], ...update }
    },
    updateSessions(update) {
      return typeof update === 'function' ? update([]) : update
    },
    typewriter: { setTarget() {}, flush() {} },
    thinkingScheduler: { flush() {} },
    toolScheduler: { cancel() {} },
    t: (key) => key,
  })
  let producer
  const wire = new ReadableStream({
    start(controller) {
      producer = controller
    },
  })
  const reading = consumeEventStream(new Response(wire), dispatcher.dispatch)
  const frame = (event, data) =>
    new TextEncoder().encode(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`)
  const first = frame('text_delta', { delta: '第一句。' })
  for (const byte of first) producer.enqueue(Uint8Array.of(byte))
  await setImmediate()
  assert.equal(updates.length, 1)
  assert.equal(updates[0].text, '第一句。')
  assert.equal(updates[0].prompt, 'spoken prompt')
  assert.equal(updates[0].messageId, 'a')
  assert.equal(updates[0].status, 'streaming')
  assert.equal(ref.current.session.messages[1].text, '')
  producer.enqueue(frame('text_patch', { start: 4, text: '尾句' }))
  producer.enqueue(frame('text_end', { text: '第一句。尾句' }))
  producer.enqueue(frame('done', { text: '第一句。尾句' }))
  producer.close()
  await reading
  assert.equal(updates.at(-1).text, '第一句。尾句')
  assert.equal(updates.at(-1).status, 'completed')
  // 语音终态立即收到完整文本，视觉正文仍由独立打字机继续显示。
  assert.equal(ref.current.session.messages[1].text, '')
})

const promptCode = transformSync(
  await readFile('src/features/chat/use-prompt-commands.ts', 'utf8'),
  { loader: 'ts', format: 'cjs' },
).code
const syncCode = transformSync(
  await readFile('src/features/chat/use-live-session-sync.ts', 'utf8'),
  { loader: 'ts', format: 'cjs' },
).code

// 执行真实发送、事件分发和快照同步，仅替换网络、React 调度及无关领域依赖。
function transportFixture(t, { openStream, live, loadError } = {}) {
  const updates = []
  t.after(responseStream.subscribeVoiceResponse('session', (event) => updates.push(event)))
  const ref = { current: { session: { messages: [], tools: [], queuedInputs: [] } } }
  const shared = {
    sessionStates: ref.current,
    sessionStatesRef: ref,
    localStreamSessionsRef: { current: new Set() },
    streamGenerationRef: { current: new Map() },
    updateSessionState(id, update) {
      ref.current[id] =
        typeof update === 'function' ? update(ref.current[id]) : { ...ref.current[id], ...update }
    },
    updateSessions(update) {
      return typeof update === 'function' ? update([]) : update
    },
  }
  const scheduler = () => ({
    flush() {},
    cancel() {},
    setTarget() {},
    push() {},
    drain: async () => true,
  })
  const modules = {
    react: { useCallback: (fn) => fn, useRef: (value) => ({ current: value }), useEffect() {} },
    '@/app/brand': { APP_NAME: 'Pisper' },
    '@/app/use-i18n': { useI18n: () => ({ t: (key) => key }) },
    '@/lib/api': { applyTextPatch },
    '@/lib/plan-protocol': {
      isPlanUpdateEvent: () => false,
      planFromPayload: (data) => data.plan,
      planFromPayloadOr: (data, fallback) => data.plan ?? fallback,
    },
    '@/lib/session-state': sessionState,
    '@/lib/streaming-ui': {
      createStreamingTextScheduler: scheduler,
      createToolUpdateScheduler: scheduler,
      createTypewriterDisplay: scheduler,
    },
    './voice-response-stream': responseStream,
    './mobile-operations': {},
    './run-activity': { settleToolCalls: (tools) => tools || [] },
    './chat-errors': { chatErrorMessage: (error) => error.message },
    './live-session-sync': {},
    './use-session-catalog': { FOCUS_MESSAGE_PAGE_SIZE: 40 },
    './chat-api': {
      chatApi: {
        openStream,
        getLiveSession: async () => live,
        getMessages: async () => {
          if (loadError) throw new Error(loadError)
          return { messages: ref.current.session.messages }
        },
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
      window: { dispatchEvent() {} },
      Event,
      Error,
    })
    return module.exports
  }
  modules['./stream-event-dispatch'] = load(compiled)
  const syncing = load(syncCode).useLiveSessionSync(shared)
  const commands = load(promptCode).usePromptCommands({
    ...shared,
    ...syncing,
    notify() {},
    setActiveId() {},
    setGlobalError() {},
    refreshSessions: async () => [],
    createSession: async () => 'session',
  })
  return { commands, syncing, ref, updates, shared }
}

for (const failure of ['resync_required', 'transport-error']) {
  test(`${failure} preserves run metadata and hands real prompt transport to live snapshot events`, async (t) => {
    const startedAt = '2026-07-17T01:02:03.456Z'
    const f = transportFixture(t, {
      openStream: async (_input, dispatch) => {
        dispatch('run', { runId: 'run-one' })
        dispatch('meta', { startedAt })
        dispatch('text_delta', { delta: '第一句。后' })
        if (failure === 'transport-error') throw new Error('connection lost')
        assert.equal(dispatch('resync_required', {}), false)
      },
      live: {
        startedAt,
        streaming: true,
        messages: [
          { id: 'message-0', role: 'user', text: 'spoken prompt' },
          { id: 'live-session', role: 'agent', text: '第一句。后续', streaming: true },
        ],
      },
    })
    await f.commands.sendPrompt('spoken prompt', 'session')
    await setImmediate()
    assert.equal(f.ref.current.session.streaming, true)
    assert.equal(f.ref.current.session.error, '')
    assert.equal(f.shared.localStreamSessionsRef.current.size, 0)
    assert.deepEqual(
      f.updates.map((event) => event.status),
      ['started', 'started', 'streaming', 'recovering', 'streaming'],
    )
    const recovery = f.updates.find((event) => event.status === 'recovering')
    assert.equal(recovery.runId, 'run-one')
    assert.equal(recovery.startedAt, startedAt)
    const snapshot = f.updates.at(-1)
    assert.equal(snapshot.source, 'snapshot')
    assert.equal(snapshot.startedAt, startedAt)
    assert.equal(snapshot.messageId, 'live-session')
    assert.equal(snapshot.text, '第一句。后续')
    assert.equal(snapshot.prompt, 'spoken prompt')
  })
}

test('completed prompt settles from the done frame without a durable transcript reload', async (t) => {
  const f = transportFixture(t, {
    openStream: async (_input, dispatch) => {
      dispatch('run', { runId: 'run-one' })
      dispatch('meta', { startedAt: '2026-07-17T01:02:03.456Z' })
      dispatch('done', { text: '完整回答。', turnBoundaryEntryId: 'entry-final' })
    },
    // 即使历史加载会失败，成功路径也不再触发它。
    loadError: 'metadata unavailable',
  })
  await f.commands.sendPrompt('spoken prompt', 'session')
  assert.equal(f.ref.current.session.error, '')
  assert.equal(f.ref.current.session.messages.at(-1).error, undefined)
  assert.equal(f.ref.current.session.messages.at(-1).text, '完整回答。')
  assert.equal(f.ref.current.session.messages.at(-1).turnBoundaryEntryId, 'entry-final')
  assert.equal(f.updates.at(-1).status, 'completed')
  assert.ok(!f.updates.some((event) => event.status === 'failed'))
})

test('speech feed cancellation works without native throwIfAborted or reason', async () => {
  const controller = new AbortController()
  Object.defineProperties(controller.signal, {
    throwIfAborted: { value: undefined },
    reason: { get: () => undefined },
  })
  const stream = responseStream.createVoiceTextStream(controller.signal)
  const iterator = stream[Symbol.asyncIterator]()
  stream.update('first')
  assert.equal((await iterator.next()).value, 'first')
  const pending = iterator.next()
  controller.abort()
  await assert.rejects(pending, { name: 'AbortError' })
})

test('the speech feed coalesces snapshots without duplicate deltas and rejects rewrites or oversized input', async () => {
  const controller = new AbortController()
  const stream = responseStream.createVoiceTextStream(controller.signal)
  const iterator = stream[Symbol.asyncIterator]()
  stream.update('first')
  stream.update('first second')
  assert.equal((await iterator.next()).value, 'first second')
  stream.update('first second')
  stream.update('first second tail')
  assert.equal((await iterator.next()).value, ' tail')
  assert.throws(() => stream.update('revised'), /rewritten/)
  assert.throws(() => stream.update('x'.repeat(32001)), /limit/)
  const pending = iterator.next()
  controller.abort()
  await assert.rejects(pending, { name: 'AbortError' })
})
