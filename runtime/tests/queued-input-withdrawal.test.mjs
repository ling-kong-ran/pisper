import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import * as sessionState from '../../src/lib/session-state.ts'
import {
  clearComposerDraft,
  mergeComposerDraft,
  readComposerDraft,
  updateComposerDraft,
} from '../../src/features/chat/composer-drafts.ts'
import { createStreamEventDispatcher } from '../../src/features/chat/stream-event-dispatch.ts'
import { reconcileLiveSnapshot } from '../../src/features/chat/use-live-session-sync.ts'
import { QueuedInputsTray } from '../../src/features/chat/focus-session-composer-bits.tsx'
import { TooltipProvider } from '../../src/components/ui/tooltip.tsx'

const promptCode = transformSync(
  await readFile('src/features/chat/use-prompt-commands.ts', 'utf8'),
  { loader: 'ts', format: 'cjs' },
).code

function deferred() {
  let resolve
  let reject
  const promise = new Promise((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}

function fixture(api = {}, initial = {}) {
  const notices = []
  const ref = {
    current: {
      session: {
        ...sessionState.DEFAULT_SESSION_STATE,
        messages: [{ id: 'durable', role: 'user', text: 'same text' }],
        ...initial,
      },
    },
  }
  const shared = {
    sessionStatesRef: ref,
    updateSessionState(id, update) {
      ref.current[id] = sessionState.applySessionUpdate(ref.current[id], update)
    },
    updateSessions(update) {
      return typeof update === 'function' ? update([]) : update
    },
  }
  const module = { exports: {} }
  const scheduler = { flush() {}, cancel() {}, setTarget() {}, drain: async () => true }
  const modules = {
    react: { useCallback: (callback) => callback, useRef: (value) => ({ current: value }) },
    '@/app/brand': { APP_NAME: 'Pisper' },
    '@/app/use-i18n': { useI18n: () => ({ t: (key) => key }) },
    '@/lib/session-state': sessionState,
    '@/lib/streaming-ui': {
      createStreamingTextScheduler: () => scheduler,
      createToolUpdateScheduler: () => scheduler,
      createTypewriterDisplay: () => scheduler,
    },
    './chat-api': { chatApi: api },
    './chat-errors': {
      chatErrorMessage: (error) => error.message,
      isEndedSessionQueueError: () => false,
    },
    './run-activity': { settleToolCalls: (tools) => tools, pushCurrentActivity: (items) => items },
    './stream-event-dispatch': { createStreamEventDispatcher },
  }
  runInNewContext(promptCode, {
    module,
    exports: module.exports,
    window: { dispatchEvent() {} },
    Event,
    require(id) {
      assert.ok(modules[id], id)
      return modules[id]
    },
  })
  const commands = module.exports.usePromptCommands({
    ...shared,
    notify: (...notice) => notices.push(notice),
    localStreamSessionsRef: { current: new Set() },
    streamGenerationRef: { current: new Map() },
    loadSessionMessages: async () => {},
    syncLiveSession: async () => {},
    setActiveId() {},
    setGlobalError() {},
    refreshSessions: async () => [],
  })
  const dispatcher = createStreamEventDispatcher({
    ...shared,
    sessionId: 'session',
    agentId: 'agent',
    t: (key) => key,
    typewriter: scheduler,
    thinkingScheduler: scheduler,
    toolScheduler: scheduler,
  })
  return {
    commands,
    notices,
    dispatch: dispatcher.dispatch,
    updateState: (update) => shared.updateSessionState('session', update),
    get state() {
      return ref.current.session
    },
  }
}

test('versioned snapshots reject stale HTTP, SSE, meta and live queues', () => {
  const newest = [{ id: 'new', text: 'newest' }]
  const f = fixture({}, { queuedInputs: newest, queueRevision: 8 })
  for (const event of ['queue_update', 'meta']) {
    f.dispatch(event, { queueRevision: 7, queuedInputs: [{ id: 'old', text: 'old' }] })
    assert.equal(f.state.queuedInputs, newest)
    assert.equal(f.state.queueRevision, 8)
  }
  const snapshot = reconcileLiveSnapshot(f.state, {
    queueRevision: 6,
    queuedInputs: [{ id: 'old' }],
    messages: f.state.messages,
    streaming: true,
  })
  assert.equal(snapshot.queuedInputs, newest)
  assert.equal(snapshot.queueRevision, 8)
  f.dispatch('queue_update', { queuedInputs: [], queueRevision: 9 })
  assert.deepEqual(f.state.queuedInputs, [])
  assert.equal(f.state.queueRevision, 9)
})

test('unversioned runtimes retain omitted queues and accept explicit clearing', () => {
  const queuedInputs = [{ text: 'legacy' }]
  const current = { ...sessionState.DEFAULT_SESSION_STATE, queuedInputs }
  assert.equal(sessionState.reconcileQueuedInputSnapshot(current, {}).queuedInputs, queuedInputs)
  assert.deepEqual(
    sessionState.reconcileQueuedInputSnapshot(current, { queuedInputs: [] }).queuedInputs,
    [],
  )
  assert.equal(sessionState.reconcileQueuedInputSnapshot(current, {}).queueRevision, undefined)
})

test('queued bubbles bind stable identities without matching identical text', async () => {
  let count = 0
  const queuedInputs = []
  const f = fixture({
    queueInput: async () => {
      const inputId = `queued-${++count}`
      queuedInputs.push({ id: inputId, text: 'same text' })
      return { inputId, queuedInputs: [...queuedInputs], queueRevision: count }
    },
  })
  assert.equal(await f.commands.queuePrompt('same text', 'session'), true)
  assert.equal(await f.commands.queuePrompt('same text', 'session'), true)
  const second = f.state.messages.find((message) => message.queuedInputId === 'queued-2')
  assert.ok(second)
  f.dispatch('queue_update', {
    queuedInputs: [queuedInputs[1]],
    queueRevision: 3,
    removedInputId: 'queued-1',
  })
  assert.deepEqual(
    f.state.messages.map((message) => message.id),
    ['durable', second.id],
  )
  assert.equal(f.state.queuedInputs[0].id, 'queued-2')
})

for (const removed of [false, true]) {
  test(`late POST ${removed ? 'cannot resurrect a withdrawn' : 'keeps the bubble of a consumed'} input`, async () => {
    const pending = deferred()
    const f = fixture({ queueInput: () => pending.promise })
    const sending = f.commands.queuePrompt('same text', 'session')
    f.dispatch('queue_update', {
      queuedInputs: [],
      queueRevision: 2,
      ...(removed ? { removedInputId: 'queued-1' } : {}),
    })
    pending.resolve({
      inputId: 'queued-1',
      queueRevision: 1,
      queuedInputs: [{ id: 'queued-1', text: 'same text' }],
    })
    assert.equal(await sending, true)
    assert.deepEqual(f.state.queuedInputs, [])
    assert.equal(f.state.queueRevision, 2)
    if (removed) {
      assert.deepEqual(
        f.state.messages.map((message) => message.id),
        ['durable'],
      )
    } else {
      assert.equal(f.state.messages.length, 2)
      assert.equal(f.state.messages[1].queuedInputId, 'queued-1')
      const history = reconcileLiveSnapshot(f.state, {
        messages: [
          f.state.messages[0],
          { id: 'durable-consumed', role: 'user', text: 'same text' },
        ],
        queueRevision: 2,
        queuedInputs: [],
        streaming: false,
      })
      assert.deepEqual(
        history.messages.map((message) => message.id),
        ['durable', 'durable-consumed'],
      )
    }
  })
}

test('SSE-first withdrawal restores once through HTTP and prevents concurrent requests', async () => {
  const pending = deferred()
  let calls = 0
  const withdrawnInput = {
    text: 'same text',
    attachments: [{ id: 'document', kind: 'document', data: 'full-binary', extension: 'pdf' }],
  }
  const f = fixture(
    {
      withdrawQueuedInput: () => {
        calls += 1
        return pending.promise
      },
    },
    {
      queueRevision: 1,
      queuedInputs: [{ id: 'one', text: 'same text' }],
      messages: [
        { id: 'durable', role: 'user', text: 'same text' },
        { id: 'temporary', role: 'user', text: 'same text', queuedInputId: 'one' },
      ],
    },
  )
  updateComposerDraft('other-panel', { text: 'leave untouched', attachments: [] })
  const removing = f.commands.withdrawQueuedInput('session', 'one')
  assert.deepEqual([...f.state.withdrawingInputIds], ['one'])
  assert.equal(await f.commands.withdrawQueuedInput('session', 'one'), null)
  assert.equal(calls, 1)
  f.dispatch('queue_update', { queueRevision: 2, queuedInputs: [], removedInputId: 'one' })
  assert.deepEqual(
    f.state.messages.map((message) => message.id),
    ['durable'],
  )
  assert.equal(readComposerDraft('other-panel').text, 'leave untouched')
  pending.resolve({
    removed: true,
    inputId: 'one',
    queueRevision: 2,
    queuedInputs: [],
    withdrawnInput,
  })
  assert.equal(await removing, withdrawnInput)
  assert.deepEqual([...f.state.withdrawingInputIds], [])
  assert.equal(f.notices.length, 0)
  clearComposerDraft('other-panel')
})

test('a consumed input returns no draft, preserves its sent bubble and notifies clearly', async () => {
  const messages = [{ id: 'sent', role: 'user', text: 'same text', queuedInputId: 'one' }]
  const f = fixture(
    {
      withdrawQueuedInput: async () => ({
        removed: false,
        inputId: 'one',
        queueRevision: 2,
        queuedInputs: [],
      }),
    },
    { messages, queueRevision: 1, queuedInputs: [{ id: 'one', text: 'same text' }] },
  )
  assert.equal(await f.commands.withdrawQueuedInput('session', 'one'), null)
  assert.equal(f.state.messages, messages)
  assert.deepEqual(f.state.queuedInputs, [])
  assert.deepEqual(f.notices, [['chat:focusSession.queuedInputAlreadyConsumed', 'info']])
})

test('withdrawal failures retain the queue and bubble and allow retry', async () => {
  let calls = 0
  const withdrawnInput = { text: 'retry', attachments: [] }
  const queuedInputs = [{ id: 'one', text: 'retry' }]
  const messages = [{ id: 'temporary', role: 'user', queuedInputId: 'one' }]
  const f = fixture(
    {
      withdrawQueuedInput: async () => {
        if (++calls === 1) throw new Error('offline')
        return { removed: true, inputId: 'one', queuedInputs: [], queueRevision: 2, withdrawnInput }
      },
    },
    { messages, queuedInputs, queueRevision: 1 },
  )
  assert.equal(await f.commands.withdrawQueuedInput('session', 'one'), null)
  assert.equal(f.state.messages, messages)
  assert.equal(f.state.queuedInputs, queuedInputs)
  assert.deepEqual([...f.state.withdrawingInputIds], [])
  assert.equal(f.notices[0][0], 'offline')
  assert.equal(await f.commands.withdrawQueuedInput('session', 'one'), withdrawnInput)
  assert.equal(calls, 2)
  assert.deepEqual(f.state.messages, [])
})

test('stale successful DELETE preserves a newer queue while removing only its own bubble', async () => {
  const withdrawnInput = { text: 'removed', attachments: [] }
  const newest = [{ id: 'two', text: 'new' }]
  const f = fixture(
    {
      withdrawQueuedInput: async () => ({
        removed: true,
        inputId: 'one',
        queuedInputs: [],
        queueRevision: 2,
        withdrawnInput,
      }),
    },
    {
      queuedInputs: newest,
      queueRevision: 3,
      messages: [
        { id: 'one-bubble', role: 'user', queuedInputId: 'one' },
        { id: 'two-bubble', role: 'user', queuedInputId: 'two' },
      ],
    },
  )
  assert.equal(await f.commands.withdrawQueuedInput('session', 'one'), withdrawnInput)
  assert.equal(f.state.queuedInputs, newest)
  assert.equal(f.state.queueRevision, 3)
  assert.deepEqual(
    f.state.messages.map((message) => message.id),
    ['two-bubble'],
  )
})

test('draft restoration preserves current text and every complete attachment beyond eight items', () => {
  const existing = Array.from({ length: 8 }, (_, index) => ({
    id: `existing-${index}`,
    kind: 'text',
    text: 'kept',
  }))
  const restored = [
    { id: 'existing-0', kind: 'text', text: 'different complete content' },
    { id: 'image', kind: 'image', data: 'image-data', mimeType: 'image/png' },
    { id: 'document', kind: 'document', data: 'document-data', extension: 'pdf' },
    { id: 'path', kind: 'path', path: '/workspace/file.txt' },
  ]
  const merged = mergeComposerDraft(
    { text: 'new draft  ', attachments: existing },
    { text: ' original\nmessage ', attachments: restored },
  )
  assert.equal(merged.text, 'new draft  \n original\nmessage ')
  assert.equal(merged.attachments.length, 12)
  assert.equal(new Set(merged.attachments.map((item) => item.id)).size, 12)
  assert.deepEqual(merged.attachments.slice(0, 8), existing)
  assert.equal(merged.attachments[8].text, restored[0].text)
  assert.deepEqual(merged.attachments.slice(9), restored.slice(1))
  assert.equal(restored[0].id, 'existing-0')
  assert.equal(
    mergeComposerDraft({ text: '', attachments: [] }, { text: 'exact', attachments: [] }).text,
    'exact',
  )
})

test('old live message pages cannot reintroduce a withdrawn local bubble', () => {
  const messages = [
    { id: 'durable', role: 'user', text: 'same text' },
    { id: 'withdrawn', role: 'user', text: 'same text', queuedInputId: 'one' },
    { id: 'consumed', role: 'user', text: 'same text', queuedInputId: 'two' },
  ]
  const f = fixture({}, { messages, queueRevision: 2 })
  f.dispatch('queue_update', { queueRevision: 3, queuedInputs: [], removedInputId: 'one' })
  const live = reconcileLiveSnapshot(f.state, {
    messages,
    pageInfo: { start: 0 },
    streaming: true,
    queueRevision: 2,
    queuedInputs: [{ id: 'one' }],
  })
  assert.deepEqual(
    live.messages.map((message) => message.id),
    ['durable', 'consumed'],
  )
  assert.deepEqual(live.queuedInputs, [])
  assert.equal(live.queueRevision, 3)
})

for (const terminal of ['done', 'error']) {
  test(`${terminal} adopts the terminal queue revision and rejects prior POST snapshots`, () => {
    const f = fixture({}, { queueRevision: 1, queuedInputs: [{ id: 'one' }] })
    const complete = () =>
      f.dispatch(terminal, {
        queueRevision: 2,
        queuedInputs: [],
        message: 'failed',
      })
    if (terminal === 'error') assert.throws(complete, /failed/)
    else complete()
    assert.equal(f.state.queueRevision, 2)
    f.dispatch('queue_update', { queueRevision: 1, queuedInputs: [{ id: 'one' }] })
    assert.deepEqual(f.state.queuedInputs, [])
  })
}

test('a new local run clears its withdrawal records but suppresses late prior-run POST bubbles', async () => {
  const pending = deferred()
  const f = fixture(
    {
      queueInput: () => pending.promise,
      openStream: async (_input, dispatch) => {
        dispatch('done', { queueRevision: 3, queuedInputs: [] })
      },
    },
    { queueRevision: 2, withdrawnInputIds: ['one'], queuedInputRunId: 'previous' },
  )
  const queueing = f.commands.queuePrompt('old text', 'session')
  await f.commands.sendPrompt('new run', 'session')
  assert.deepEqual([...f.state.withdrawnInputIds], [])
  assert.notEqual(f.state.queuedInputRunId, 'previous')
  pending.resolve({ inputId: 'one', queueRevision: 1, queuedInputs: [{ id: 'one' }] })
  assert.equal(await queueing, true)
  assert.ok(!f.state.messages.some((message) => message.queuedInputId === 'one'))
  assert.deepEqual(f.state.queuedInputs, [])
})

const composerCode = transformSync(await readFile('src/features/chat/composer-drafts.ts', 'utf8'), {
  loader: 'ts',
  format: 'cjs',
}).code
const attachmentsCode = transformSync(await readFile('src/features/chat/attachments.ts', 'utf8'), {
  loader: 'ts',
  format: 'cjs',
}).code

// 保留真实草稿和附件 hook，仅用确定性的 React hook 调度模拟切换、卸载与迟到回调。
function composerFixture() {
  const slots = []
  let cursor = 0
  let effects = []
  const sameDependencies = (left, right) =>
    left && left.length === right.length && left.every((item, index) => item === right[index])
  const react = {
    useRef(value) {
      const index = cursor++
      return (slots[index] ||= { current: value })
    },
    useState(initial) {
      const index = cursor++
      if (!(index in slots)) slots[index] = typeof initial === 'function' ? initial() : initial
      return [
        slots[index],
        (value) => {
          slots[index] = typeof value === 'function' ? value(slots[index]) : value
        },
      ]
    },
    useCallback(callback, dependencies) {
      const index = cursor++
      if (!sameDependencies(slots[index]?.dependencies, dependencies))
        slots[index] = { callback, dependencies }
      return slots[index].callback
    },
    useEffect(callback, dependencies) {
      const index = cursor++
      if (!sameDependencies(slots[index]?.dependencies, dependencies))
        effects.push(() => {
          slots[index]?.cleanup?.()
          slots[index] = { dependencies, cleanup: callback() }
        })
    },
  }
  const modules = {
    react,
    '@/app/i18n.ts': { storedLanguage: () => 'en-US', translateText: (key) => key },
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
    })
    return module.exports
  }
  modules['./attachments'] = load(attachmentsCode)
  const composer = load(composerCode)
  return {
    ...composer,
    render(sessionId) {
      cursor = 0
      effects = []
      const result = composer.useComposerDraft(sessionId)
      for (const effect of effects) effect()
      return result
    },
    unmount() {
      for (const slot of slots) slot?.cleanup?.()
    },
  }
}

test('late withdrawal reads the latest draft and remains isolated after a session switch', () => {
  const composer = composerFixture()
  const first = composer.render('first')
  first.updateValue('before request')
  const restoreFirst = first.restoreDraft
  first.updateValue('typed while withdrawing')
  first.selection.replaceAttachments([{ id: 'new', kind: 'text', text: 'new attachment' }])
  composer.render('second')
  const second = composer.render('second')
  second.updateValue('other session draft')
  second.selection.replaceAttachments([{ id: 'other', kind: 'path', path: '/other/file' }])
  const restored = {
    text: 'withdrawn original',
    attachments: [{ id: 'old', kind: 'document', data: 'complete data' }],
  }
  assert.equal(restoreFirst(restored), false)
  assert.equal(
    composer.readComposerDraft('first').text,
    'typed while withdrawing\nwithdrawn original',
  )
  assert.equal(composer.readComposerDraft('first').attachments.length, 2)
  assert.equal(composer.readComposerDraft('first').attachments[1].data, 'complete data')
  assert.equal(composer.render('second').value, 'other session draft')
  assert.equal(composer.readComposerDraft('second').attachments[0].path, '/other/file')
  composer.render('first')
  const reopened = composer.render('first')
  assert.equal(reopened.value, 'typed while withdrawing\nwithdrawn original')
  assert.equal(reopened.selection.attachments.length, 2)
})

test('late queue completion cannot clear restored content or new edits after submission', () => {
  const composer = composerFixture()
  composer.render('pending').updateValue('submitted')
  const submitted = composer.render('pending')
  submitted.restoreDraft({
    text: 'withdrawn',
    attachments: [{ id: 'restored', kind: 'text', text: 'full content' }],
  })
  submitted.clearDraft()
  assert.equal(composer.readComposerDraft('pending').text, 'submitted\nwithdrawn')
  assert.equal(composer.readComposerDraft('pending').attachments[0].text, 'full content')
  const edited = composer.render('pending')
  edited.updateValue('new draft')
  edited.clearDraft()
  assert.equal(composer.readComposerDraft('pending').text, 'new draft')
  const current = composer.render('pending')
  current.clearDraft()
  assert.equal(composer.readComposerDraft('pending').text, '')
  assert.equal(composer.readComposerDraft('pending').attachments.length, 0)
})

test('late successful submission only clears the submitted session after switching', () => {
  const composer = composerFixture()
  composer.render('first').updateValue('submitted')
  const submitted = composer.render('first')
  composer.render('second')
  composer.render('second').updateValue('second draft')
  submitted.clearDraft()
  assert.equal(composer.readComposerDraft('first').text, '')
  assert.equal(composer.render('second').value, 'second draft')
  assert.equal(composer.readComposerDraft('second').text, 'second draft')
})

test('unmounted composers retain a late successful withdrawal for reopening', () => {
  const composer = composerFixture()
  const first = composer.render('closed')
  first.updateValue('retained draft')
  composer.unmount()
  assert.equal(first.restoreDraft({ text: 'returned message', attachments: [] }), false)
  assert.equal(composer.readComposerDraft('closed').text, 'retained draft\nreturned message')
})

test('queue SSR exposes all rows with labelled Undo controls and disables only pending or legacy items', () => {
  const queuedInputs = Array.from({ length: 7 }, (_, index) => ({
    id: `item-${index}`,
    text: `queued ${index} ${'x'.repeat(150)}`,
  }))
  queuedInputs.push({ text: 'legacy message' })
  const html = renderToStaticMarkup(
    createElement(
      TooltipProvider,
      null,
      createElement(QueuedInputsTray, {
        queuedInputs,
        withdrawingInputIds: ['item-1'],
        onWithdraw() {},
      }),
    ),
  )
  assert.equal((html.match(/<li\b/g) || []).length, 8)
  assert.equal((html.match(/<button\b/g) || []).length, 8)
  assert.equal((html.match(/lucide-undo-2/g) || []).length, 8)
  assert.equal((html.match(/disabled=""/g) || []).length, 2)
  assert.equal((html.match(/aria-busy="true"/g) || []).length, 1)
  assert.equal((html.match(/aria-label=/g) || []).length, 9)
  assert.match(html, /overflow-y-auto/)
  assert.match(html, /overflow-wrap:anywhere/)
  assert.match(html, /grid-cols-\[minmax\(0,1fr\)_auto\]/)
  assert.match(html, /queued 0/)
  assert.match(html, /queued 6/)
})
