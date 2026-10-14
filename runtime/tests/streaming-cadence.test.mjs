import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import * as sessionState from '../../src/lib/session-state.ts'
import {
  createStreamingTextScheduler,
  createToolUpdateScheduler,
  createTypewriterDisplay,
} from '../../src/lib/streaming-ui.ts'

const promptCode = transformSync(
  await readFile('src/features/chat/use-prompt-commands.ts', 'utf8'),
  { loader: 'ts', format: 'cjs' },
).code
const dispatcherCode = transformSync(
  await readFile('src/features/chat/stream-event-dispatch.ts', 'utf8'),
  { loader: 'ts', format: 'cjs' },
).code

async function settleMicrotasks() {
  for (let turn = 0; turn < 8; turn += 1) await Promise.resolve()
}

// 执行真实发送、事件分发和显示调度；仅替换网络、React 与无关领域依赖。
function transportFixture(t, { onOpen, onHistory } = {}) {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  let timestamp = 0
  let nextFrame = 0
  const pending = new Map()
  const scheduled = []
  const callbacks = {}
  const responseEvents = []
  const history = []
  const changes = []
  const ref = { current: { session: { messages: [], tools: [], queuedInputs: [] } } }
  const shared = {
    sessionStatesRef: ref,
    localStreamSessionsRef: { current: new Set() },
    streamGenerationRef: { current: new Map() },
    updateSessionState(id, update) {
      ref.current[id] =
        typeof update === 'function' ? update(ref.current[id]) : { ...ref.current[id], ...update }
      changes.push({ at: timestamp, state: ref.current[id] })
    },
    updateSessions(update) {
      return typeof update === 'function' ? update([]) : update
    },
  }
  const modules = {
    react: { useCallback: (fn) => fn, useRef: (value) => ({ current: value }) },
    '@/app/brand': { APP_NAME: 'Pisper' },
    '@/app/use-i18n': { useI18n: () => ({ t: (key) => key }) },
    '@/lib/api': {},
    '@/lib/plan-protocol': {
      isPlanUpdateEvent: (event) => event === 'plan_update',
      isPlanWriteTool: () => false,
      planFromPayload: (data) => data.plan,
      planFromPayloadOr: (data, fallback) => data.plan ?? fallback,
    },
    '@/lib/session-state': sessionState,
    '@/lib/streaming-ui': {
      createStreamingTextScheduler(callback, options) {
        callbacks.thinking = callback
        const scheduler = createStreamingTextScheduler(callback, options)
        scheduled.push(scheduler)
        return scheduler
      },
      createToolUpdateScheduler(callback, options) {
        callbacks.tool = callback
        const scheduler = createToolUpdateScheduler(callback, options)
        scheduled.push(scheduler)
        return scheduler
      },
      createTypewriterDisplay(callback, options) {
        callbacks.response = callback
        const typewriter = createTypewriterDisplay(callback, {
          ...options,
          now: () => timestamp,
          requestFrame(frame) {
            const id = nextFrame++
            pending.set(id, frame)
            return id
          },
          cancelFrame(id) {
            pending.delete(id)
          },
        })
        scheduled.push(typewriter)
        return typewriter
      },
    },
    '@/lib/streaming-debug': { recordStreamingDebug() {} },
    './voice-response-stream': { publishVoiceResponse: (event) => responseEvents.push(event) },
    './mobile-operations': {},
    './run-activity': {
      planChanges: () => [],
      pushCurrentActivity: (items = [], item) => [
        ...items.filter((old) => old.id !== item.id),
        item,
      ],
      settleToolCalls: (tools = []) => tools.map((tool) => ({ ...tool, status: 'done' })),
    },
    './chat-errors': { chatErrorMessage: (error) => error.message },
    './chat-api': { chatApi: { openStream: (_input, dispatch) => onOpen?.(dispatch) } },
  }
  function load(code) {
    const module = { exports: {} }
    runInNewContext(code, {
      module,
      exports: module.exports,
      require(id) {
        assert.ok(modules[id], id)
        return modules[id]
      },
      window: { dispatchEvent() {} },
      Event,
      Error,
    })
    return module.exports
  }
  modules['./stream-event-dispatch'] = load(dispatcherCode)
  const commands = load(promptCode).usePromptCommands({
    ...shared,
    notify() {},
    setActiveId() {},
    setGlobalError() {},
    refreshSessions: async () => [],
    createSession: async () => 'session',
    syncLiveSession: async () => {},
    async loadSessionMessages(id) {
      history.push({ at: timestamp, state: ref.current[id] })
      await onHistory?.(ref.current[id])
    },
  })
  t.after(() => {
    for (const scheduler of scheduled) scheduler.cancel()
  })
  return {
    commands,
    shared,
    ref,
    history,
    changes,
    callbacks,
    responseEvents,
    tick(milliseconds = 1_000 / 120) {
      timestamp += milliseconds
      const frames = [...pending.values()]
      pending.clear()
      t.mock.timers.tick(milliseconds)
      for (const frame of frames) frame(timestamp)
    },
    get pending() {
      return pending.size
    },
  }
}

function burstAndDone(dispatch, { finalText = 'x'.repeat(900), boundaries = false } = {}) {
  dispatch('run', { runId: 'run-one' })
  dispatch('meta', { startedAt: '2026-01-01T00:00:00.000Z' })
  for (let delta = 0; delta < 10; delta += 1) dispatch('text_delta', { delta: 'x'.repeat(90) })
  dispatch('text_end', { text: 'x'.repeat(900) })
  if (boundaries) {
    dispatch('tool_start', { id: 'tool-one', name: 'read' })
    dispatch('tool_end', { id: 'tool-one' })
    dispatch('plan_update', { plan: { items: [] } })
    dispatch('compaction_start', { startedAt: 'compacting' })
    dispatch('compaction_end', { finishedAt: 'compacted' })
    dispatch('permission_request', { id: 'permission' })
    dispatch('permission_resolved', { id: 'permission' })
  }
  dispatch('done', {
    text: finalText,
    finishedAt: '2026-01-01T00:00:01.000Z',
    turnBoundaryEntryId: 'entry-final',
  })
}

for (const boundaries of [false, true]) {
  test(`synchronous burst plus done drains and settles from the done frame (boundaries=${boundaries})`, async (t) => {
    const finalText = 'x'.repeat(870) + ' revised \u{1f4a1} ending'
    const f = transportFixture(t, {
      onOpen: (dispatch) => burstAndDone(dispatch, { finalText, boundaries }),
    })
    let settled = false
    const sending = f.commands.sendPrompt('prompt', 'session').then(() => {
      settled = true
    })
    await settleMicrotasks()
    assert.equal(f.ref.current.session.streaming, false)
    assert.equal(f.ref.current.session.lifecycle.phase, 'completed')
    assert.equal(f.responseEvents.at(-1).status, 'completed')
    assert.equal(f.responseEvents.at(-1).text, finalText)
    assert.equal(f.ref.current.session.messages.at(-1).text, '')
    assert.equal(f.history.length, 0)
    assert.equal(settled, false)
    f.tick(50)
    const firstFrame = f.ref.current.session.messages.at(-1)
    assert.equal(firstFrame.text, finalText)
    assert.equal(firstFrame.streaming, false)
    assert.equal(f.history.length, 0)
    for (let frame = 0; f.pending && frame < 1_000; frame += 1) f.tick()
    assert.equal(f.pending, 0)
    await sending
    assert.equal(settled, true)
    // 成功路径不再重拉历史：边界元数据随 done 帧就地补齐。
    assert.equal(f.history.length, 0)
    assert.equal(f.ref.current.session.messages.at(-1).turnBoundaryEntryId, 'entry-final')
    assert.equal(f.ref.current.session.messages.at(-1).text, finalText)
    assert.equal(f.ref.current.session.messages.at(-1).streaming, false)
    const completedStates = f.changes.filter(({ state }) => state.lifecycle?.phase === 'completed')
    assert.ok(completedStates.every(({ state }) => state.currentActivity === null))
    assert.ok(
      completedStates.every(({ state }) => state.lastActivityAt === '2026-01-01T00:00:01.000Z'),
    )
    const rendered = completedStates.map(({ state }) => state.messages.at(-1).text)
    assert.ok(rendered.every((text) => text.isWellFormed() && finalText.startsWith(text)))
    assert.equal(f.shared.localStreamSessionsRef.current.size, 0)
  })
}

test('text_end settles an already displayed block without flushing its text again', async (t) => {
  let dispatch
  let close
  const f = transportFixture(t, {
    onOpen(nextDispatch) {
      dispatch = nextDispatch
      return new Promise((resolve) => {
        close = resolve
      })
    },
  })
  const sending = f.commands.sendPrompt('prompt', 'session')
  dispatch('text_delta', { delta: 'a' })
  f.tick(50)
  assert.equal(f.ref.current.session.messages.at(-1).text, 'a')
  assert.equal(f.ref.current.session.messages.at(-1).streaming, true)
  dispatch('text_end', { text: 'a' })
  assert.equal(f.ref.current.session.messages.at(-1).text, 'a')
  assert.equal(f.ref.current.session.messages.at(-1).streaming, false)
  assert.equal(f.pending, 0)
  dispatch('done', { text: 'a' })
  close()
  await sending
  assert.equal(f.history.length, 0)
})

test('a late final rewrite replaces the displayed suffix gradually and preserves the final text', async (t) => {
  let dispatch
  let close
  const f = transportFixture(t, {
    onOpen(nextDispatch) {
      dispatch = nextDispatch
      return new Promise((resolve) => {
        close = resolve
      })
    },
  })
  const sending = f.commands.sendPrompt('prompt', 'session')
  dispatch('text_delta', { delta: 'prefix ' + '\u{1f4a1}'.repeat(200) })
  f.tick(50)
  const previous = f.ref.current.session.messages.at(-1).text
  assert.ok(previous.startsWith('prefix \u{1f4a1}'))
  const finalText = 'prefix ' + '\u{1f4a2}'.repeat(220)
  dispatch('text_end', { text: finalText })
  dispatch('done', { text: finalText })
  close()
  await settleMicrotasks()
  assert.equal(f.ref.current.session.messages.at(-1).text, previous)
  f.tick(50)
  const revised = f.ref.current.session.messages.at(-1).text
  assert.ok(revised.startsWith('prefix \u{1f4a2}'))
  assert.ok(revised.length < finalText.length)
  assert.equal(revised.isWellFormed(), true)
  for (let frame = 0; f.pending && frame < 1_000; frame += 1) f.tick()
  await sending
  assert.equal(f.ref.current.session.messages.at(-1).text, finalText)
  assert.equal(f.history.length, 0)
})

for (const nextGeneration of ['forced-snapshot', 'new-run']) {
  test(`${nextGeneration} invalidates a pending drain without stale writes or history loading`, async (t) => {
    const f = transportFixture(t, { onOpen: (dispatch) => burstAndDone(dispatch) })
    const sending = f.commands.sendPrompt('prompt', 'session')
    await settleMicrotasks()
    assert.equal(f.history.length, 0)
    f.shared.streamGenerationRef.current.set('session', 2)
    if (nextGeneration === 'forced-snapshot')
      f.shared.localStreamSessionsRef.current.delete('session')
    const replacement = {
      ...f.ref.current.session,
      messages: [
        { id: 'replacement', role: 'agent', text: 'authoritative snapshot', streaming: true },
      ],
      streaming: true,
      thinkingText: 'new thinking',
      currentActivity: { type: 'tool', id: 'new-tool' },
    }
    f.ref.current.session = replacement
    const count = f.changes.length
    f.tick()
    await sending
    assert.equal(f.ref.current.session, replacement)
    assert.equal(f.changes.length, count)
    assert.equal(f.history.length, 0)
    assert.equal(f.pending, 0)
    assert.equal(f.shared.streamGenerationRef.current.get('session'), 2)
  })
}

test('all three delayed scheduler callbacks ignore an old generation', async (t) => {
  let dispatch
  let close
  const f = transportFixture(t, {
    onOpen(nextDispatch) {
      dispatch = nextDispatch
      return new Promise((resolve) => {
        close = resolve
      })
    },
  })
  const sending = f.commands.sendPrompt('prompt', 'session')
  dispatch('text_delta', { delta: 'pending reply' })
  f.callbacks.thinking('current thinking', 'current')
  assert.equal(f.ref.current.session.thinkingText, 'current thinking')
  f.shared.streamGenerationRef.current.set('session', 2)
  const before = f.ref.current.session
  const count = f.changes.length
  f.callbacks.thinking('stale thinking', 'stale')
  f.callbacks.response('stale reply', 'stale')
  f.callbacks.tool(new Map([['tool-one', { output: 'stale output' }]]), 'stale')
  assert.equal(f.ref.current.session, before)
  assert.equal(f.changes.length, count)
  close()
  await sending
})

test('a settled stream stays closed when a later run reuses its generation number', async (t) => {
  const f = transportFixture(t, { onOpen: (dispatch) => burstAndDone(dispatch) })
  const sending = f.commands.sendPrompt('prompt', 'session')
  await settleMicrotasks()
  for (let frame = 0; f.pending && frame < 1_000; frame += 1) f.tick()
  await sending
  assert.equal(f.shared.streamGenerationRef.current.size, 0)
  f.shared.streamGenerationRef.current.set('session', 1)
  const before = f.ref.current.session
  const count = f.changes.length
  f.callbacks.thinking('stale thinking', 'stale')
  f.callbacks.response('stale reply', 'stale')
  f.callbacks.tool(new Map([['tool-one', { output: 'stale output' }]]), 'stale')
  assert.equal(f.ref.current.session, before)
  assert.equal(f.changes.length, count)
})

for (const initiallyHidden of [false, true]) {
  test(`hidden completion releases the actual send chain (initiallyHidden=${initiallyHidden})`, async (t) => {
    const original = Object.getOwnPropertyDescriptor(globalThis, 'document')
    const page = new EventTarget()
    page.visibilityState = initiallyHidden ? 'hidden' : 'visible'
    Object.defineProperty(globalThis, 'document', { configurable: true, value: page })
    const f = transportFixture(t, { onOpen: (dispatch) => burstAndDone(dispatch) })
    t.after(() => {
      if (original) Object.defineProperty(globalThis, 'document', original)
      else delete globalThis.document
    })
    const sending = f.commands.sendPrompt('prompt', 'session')
    await settleMicrotasks()
    if (!initiallyHidden) {
      assert.equal(f.history.length, 0)
      f.tick(50)
      page.visibilityState = 'hidden'
      page.dispatchEvent(new Event('visibilitychange'))
    }
    await sending
    assert.equal(f.ref.current.session.messages.at(-1).text, 'x'.repeat(900))
    assert.equal(f.history.length, 0)
    assert.equal(f.pending, 0)
    assert.equal(f.shared.localStreamSessionsRef.current.size, 0)
  })
}

test('error termination still flushes immediately without starting a drain', async (t) => {
  const f = transportFixture(t, {
    onOpen(dispatch) {
      dispatch('text_delta', { delta: 'partial' })
      dispatch('error', { text: 'final error context', message: 'failed' })
    },
  })
  await f.commands.sendPrompt('prompt', 'session')
  assert.equal(f.ref.current.session.messages.at(-1).text, 'final error context')
  assert.equal(f.ref.current.session.error, 'failed')
  assert.equal(f.ref.current.session.streaming, false)
  assert.equal(f.pending, 0)
  assert.equal(f.responseEvents.at(-1).status, 'failed')
})

test('settled runs apply the done-frame boundary id without loading history', async (t) => {
  const f = transportFixture(t, { onOpen: (dispatch) => burstAndDone(dispatch) })
  const sending = f.commands.sendPrompt('prompt', 'session')
  await settleMicrotasks()
  for (let frame = 0; f.pending && frame < 1_000; frame += 1) f.tick()
  await sending
  const last = f.ref.current.session.messages.at(-1)
  assert.equal(last.text, 'x'.repeat(900))
  assert.equal(last.turnBoundaryEntryId, 'entry-final')
  assert.equal(last.streaming, false)
  assert.equal(last.error, undefined)
  assert.equal(f.ref.current.session.lifecycle.phase, 'completed')
  assert.equal(f.responseEvents.at(-1).status, 'completed')
  assert.equal(f.history.length, 0)
})
