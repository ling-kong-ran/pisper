import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { runInNewContext } from 'node:vm'
import test from 'node:test'
import { transformSync } from 'esbuild'
import { DEFAULT_SESSION_STATE } from '../../src/lib/session-state.ts'
import * as sessionState from '../../src/lib/session-state.ts'
import { createStreamEventDispatcher } from '../../src/features/chat/stream-event-dispatch.ts'
import * as sync from '../../src/features/chat/use-live-session-sync.ts'

// 使用真实 hook 与草稿存储，只替换 React 调度以确定性推进异步回调。
async function fixture(composerOptions) {
  const slots = []
  let cursor = 0
  let effects = []
  const same = (a, b) => a && a.length === b.length && a.every((item, i) => item === b[i])
  const react = {
    useRef(value) {
      const i = cursor++
      return (slots[i] ||= { current: value })
    },
    useState(value) {
      const i = cursor++
      if (!(i in slots)) slots[i] = typeof value === 'function' ? value() : value
      return [
        slots[i],
        (next) => {
          slots[i] = typeof next === 'function' ? next(slots[i]) : next
        },
      ]
    },
    useCallback(callback, deps) {
      const i = cursor++
      if (!same(slots[i]?.deps, deps)) slots[i] = { callback, deps }
      return slots[i].callback
    },
    useEffect(callback, deps) {
      const i = cursor++
      if (!same(slots[i]?.deps, deps))
        effects.push(() => {
          slots[i]?.cleanup?.()
          slots[i] = { deps, cleanup: callback() }
        })
    },
  }
  const modules = {
    react,
    '@/app/i18n.ts': { storedLanguage: () => 'en-US', translateText: (key) => key },
  }
  async function load(file) {
    const code = transformSync(await readFile(file, 'utf8'), { loader: 'ts', format: 'cjs' }).code
    const module = { exports: {} }
    runInNewContext(code, {
      module,
      exports: module.exports,
      setTimeout,
      clearTimeout,
      require: (id) => {
        assert.ok(modules[id], id)
        return modules[id]
      },
    })
    return module.exports
  }
  modules['./attachments'] = await load('src/features/chat/attachments.ts')
  const drafts = await load('src/features/chat/composer-drafts.ts')
  const focus = await load('src/features/chat/use-focus-composer.ts')
  return {
    ...drafts,
    render(id) {
      cursor = 0
      effects = []
      const result = drafts.useComposerDraft(id)
      if (composerOptions)
        result.composer = focus.useFocusComposer({
          sessionId: id,
          goalsAvailable: true,
          teamAvailable: true,
          workflowsAvailable: true,
          value: result.value,
          selection: result.selection,
          clearDraft: result.clearDraft,
          captureSubmission: result.captureSubmission,
          promptRef: { current: null },
          onSend: () => {},
          setToolsOpen: () => {},
          requestTranscriptBottom: () => {},
          ...composerOptions,
        })
      for (const effect of effects) effect()
      return result
    },
  }
}

function deferred() {
  let resolve
  const promise = new Promise((done) => {
    resolve = done
  })
  return { promise, resolve }
}

test('accepted submission clears its captured text and complete attachments', async () => {
  const f = await fixture()
  f.render('a').updateValue('original')
  f.render('a').selection.addAttachments([{ id: 'document', kind: 'document', data: 'full data' }])
  const submission = f.render('a').captureSubmission()
  submission.accept()
  assert.equal(f.readComposerDraft('a').text, '')
  assert.equal(f.readComposerDraft('a').attachments.length, 0)
})

test('late acceptance or failure never erases subsequent input, including identical text typed again', async () => {
  const f = await fixture()
  f.render('a').updateValue('original')
  const first = f.render('a').captureSubmission()
  f.render('a').updateValue('later')
  f.render('a').updateValue('original')
  first.accept()
  assert.equal(f.readComposerDraft('a').text, 'original')
  const second = f.render('a').captureSubmission()
  second.accept()
  f.render('a').updateValue('new draft')
  assert.equal(f.readComposerDraft('a').text, 'new draft')
})

test('late acceptance clears only the origin session without changing another composer', async () => {
  const f = await fixture()
  f.render('a').updateValue('origin')
  const submission = f.render('a').captureSubmission()
  f.render('b')
  f.render('b').updateValue('other')
  submission.accept()
  assert.equal(f.readComposerDraft('a').text, '')
  assert.equal(f.render('b').value, 'other')
})

test('reading a file has cancellable identity and cannot leak into another session', async () => {
  const f = await fixture()
  const content = deferred()
  let composer = f.render('a')
  const reading = composer.selection.addFiles([
    { name: 'slow.txt', type: 'text/plain', size: 5, lastModified: 1, text: () => content.promise },
  ])
  composer = f.render('a')
  assert.equal(composer.selection.preparingAttachments.length, 1)
  composer.selection.removeAttachment(composer.selection.preparingAttachments[0].id)
  content.resolve('hello')
  assert.equal(await reading, false)
  assert.equal(f.render('a').selection.attachments.length, 0)
  const next = deferred()
  const pending = f
    .render('a')
    .selection.addFiles([
      { name: 'next.txt', type: 'text/plain', size: 5, lastModified: 2, text: () => next.promise },
    ])
  f.render('b')
  f.render('b').updateValue('other')
  next.resolve('later')
  await pending
  assert.equal(f.render('b').selection.attachments.length, 0)
  assert.equal(f.readComposerDraft('a').attachments.length, 0)
})

test('older pages remain reachable with bounded retained messages', () => {
  let current = {
    ...DEFAULT_SESSION_STATE,
    messages: Array.from({ length: 200 }, (_, i) => ({
      id: `m${i + 300}`,
      role: 'user',
      text: String(i),
    })),
    messageStart: 300,
  }
  for (let start = 250; start >= 0; start -= 50) {
    const page = sync.reconcileOlderMessagePage(current, {
      messages: Array.from({ length: 50 }, (_, i) => ({
        id: `m${i + start}`,
        role: 'user',
        text: String(i),
      })),
      pageInfo: { start, hasMore: start > 0, nextCursor: start > 0 ? String(start) : null },
    })
    assert.equal(page.messages[0].id, `m${start}`)
    assert.ok(page.messages.length <= sync.MAX_FOCUS_MESSAGES)
    assert.equal(page.messageStart, start)
    current = { ...current, ...page }
  }
  assert.equal(current.hasOlder, false)
})

const submitEvent = { preventDefault() {} }

test('synchronous rejection retains draft and attachments', async () => {
  const f = await fixture({
    onSend() {
      throw new Error('rejected')
    },
  })
  f.render('a').updateValue('retry this')
  f.render('a').selection.addAttachments([{ id: 'file', kind: 'text', text: 'full file' }])
  await f.render('a').composer.submit(submitEvent)
  assert.equal(f.readComposerDraft('a').text, 'retry this')
  assert.equal(f.readComposerDraft('a').attachments[0].text, 'full file')
})

test('async rejection retains draft until explicit acceptance and accepted run failures do not resend', async () => {
  const pending = deferred()
  let accept
  const f = await fixture({
    onSend(...args) {
      accept = args[6]
      return pending.promise
    },
  })
  f.render('a').updateValue('pending')
  const sending = f.render('a').composer.submit(submitEvent)
  assert.equal(f.readComposerDraft('a').text, 'pending')
  accept()
  assert.equal(f.readComposerDraft('a').text, '')
  pending.resolve(false)
  await sending
  assert.equal(f.readComposerDraft('a').text, '')
})

test('queue rejection releases busy state and retains the current draft', async () => {
  const f = await fixture({ streaming: true, onQueue: () => Promise.reject(new Error('rejected')) })
  f.render('a').updateValue('guidance')
  await f.render('a').composer.submit(submitEvent)
  assert.equal(f.readComposerDraft('a').text, 'guidance')
  assert.equal(f.render('a').composer.queueing, false)
})

test('starting another file while sending protects the new draft from late acceptance', async () => {
  const f = await fixture()
  f.render('a').updateValue('submitted')
  const snapshot = f.render('a').captureSubmission()
  const content = deferred()
  const reading = f.render('a').selection.addFiles([
    {
      name: 'new.txt',
      type: 'text/plain',
      size: 5,
      lastModified: 4,
      text: () => content.promise,
    },
  ])
  snapshot.accept()
  content.resolve('later')
  assert.equal(await reading, true)
  assert.equal(f.readComposerDraft('a').attachments[0].text, 'later')
  assert.equal(f.readComposerDraft('a').text, '')
})

const promptCode = transformSync(
  await readFile('src/features/chat/use-prompt-commands.ts', 'utf8'),
  { loader: 'ts', format: 'cjs' },
).code

function promptFixture(api = {}, initial = {}, options = {}) {
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
      isEndedSessionQueueError: (error) => error.message === 'ended',
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
    ...options,
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

test('transport failure before acceptance returns false and never confirms the draft', async () => {
  let accepted = 0
  const f = promptFixture({ openStream: () => Promise.reject(new Error('rejected')) })
  assert.equal(
    await f.commands.sendPrompt('send', 'session', [], false, false, null, null, {
      onAccepted: () => accepted++,
    }),
    false,
  )
  assert.equal(accepted, 0)
  assert.equal(f.state.error, 'rejected')
})

test('accepted run transport failure retains acceptance while handing recovery to the session owner', async () => {
  let accepted = 0
  const f = promptFixture({
    openStream: (_request, emit) => {
      emit('run', { runId: 'accepted' })
      return Promise.reject(new Error('transport lost'))
    },
  })
  assert.equal(
    await f.commands.sendPrompt('send', 'session', [], false, false, null, null, {
      onAccepted: () => accepted++,
    }),
    true,
  )
  assert.equal(accepted, 1)
})

test('ended queue fallback reports send rejection instead of losing the queued draft', async () => {
  const f = promptFixture({
    queueInput: () => Promise.reject(new Error('ended')),
    openStream: () => Promise.reject(new Error('rejected')),
  })
  assert.equal(await f.commands.queuePrompt('send', 'session'), false)
})

test('acceptance removes submitted fields while retaining attachments added during the request', async () => {
  const f = await fixture()
  f.render('a').updateValue('submitted')
  f.render('a').selection.addAttachments([{ id: 'old', kind: 'text', text: 'submitted file' }])
  const snapshot = f.render('a').captureSubmission()
  f.render('a').selection.addAttachments([{ id: 'new', kind: 'text', text: 'next file' }])
  snapshot.accept()
  assert.equal(f.readComposerDraft('a').text, '')
  assert.deepEqual(
    Array.from(f.readComposerDraft('a').attachments, (item) => item.id),
    ['new'],
  )
})

test('bounded history projection retains the locally owned streaming message identity through return to latest', () => {
  const messages = Array.from({ length: 200 }, (_, index) => ({
    id: index === 199 ? 'agent-local' : `m${300 + index}`,
    role: index === 199 ? 'agent' : 'user',
    text: '',
    streaming: index === 199,
  }))
  const ref = {
    current: {
      session: {
        ...DEFAULT_SESSION_STATE,
        messages,
        messageStart: 300,
        hasOlder: true,
        olderCursor: '300',
        streaming: true,
      },
    },
  }
  ref.current.session = sync.reconcileOlderMessageWindow(ref.current.session, {
    messages: Array.from({ length: 50 }, (_, index) => ({
      id: `m${250 + index}`,
      role: 'user',
      text: 'old',
    })),
    pageInfo: { start: 250, hasMore: true, nextCursor: '250' },
  })
  assert.equal(ref.current.session.messages, messages)
  assert.equal(ref.current.session.historyWindow.messages.length, 200)
  assert.equal(ref.current.session.historyWindow.messages[0].id, 'm250')
  const update = (_id, patch) => {
    ref.current.session = sessionState.applySessionUpdate(ref.current.session, patch)
  }
  const scheduler = {
    flush() {},
    cancel() {},
    setTarget(text) {
      update('session', (current) => ({
        ...current,
        messages: current.messages.map((message) =>
          message.id === 'agent-local' ? { ...message, text } : message,
        ),
      }))
    },
  }
  const dispatcher = createStreamEventDispatcher({
    sessionId: 'session',
    agentId: 'agent-local',
    sessionStatesRef: ref,
    updateSessionState: update,
    updateSessions: () => [],
    typewriter: scheduler,
    thinkingScheduler: scheduler,
    toolScheduler: scheduler,
    t: (key) => key,
  })
  dispatcher.dispatch('text_delta', { delta: 'still updating while reading history' })
  ref.current.session = sync.restoreLatestMessageWindow(ref.current.session)
  assert.equal(ref.current.session.historyWindow, null)
  assert.equal(ref.current.session.messages.at(-1).id, 'agent-local')
  assert.equal(ref.current.session.messages.at(-1).text, 'still updating while reading history')
  dispatcher.dispatch('text_delta', { delta: ' and after returning' })
  assert.ok(ref.current.session.messages.at(-1).text.endsWith(' and after returning'))
  dispatcher.dispatch('done', { text: 'COMPLETE RESPONSE' })
  assert.equal(ref.current.session.messages.at(-1).text, 'COMPLETE RESPONSE')
  assert.equal(ref.current.session.messages.at(-1).streaming, false)
  assert.equal(ref.current.session.streaming, false)
})

test('retry resolves the latest source after leaving a historical window', async () => {
  let f
  let observed
  f = promptFixture(
    {
      retryLastTurn: () => {
        observed = f.state.messages.at(-2).text
        return Promise.resolve()
      },
    },
    {
      messages: [
        { id: 'old-u', role: 'user', text: 'OLD QUESTION' },
        { id: 'old-a', role: 'agent', text: 'OLD ANSWER' },
      ],
      historyWindow: { messages: [], messageStart: 0, hasOlder: false, olderCursor: null },
    },
    {
      loadSessionMessages: () => {
        f.updateState({
          historyWindow: null,
          messages: [
            { id: 'latest-u', role: 'user', text: 'LATEST QUESTION' },
            { id: 'latest-a', role: 'agent', text: 'LATEST ANSWER' },
          ],
        })
        return Promise.resolve()
      },
    },
  )
  await f.commands.retryLastTurn('session')
  assert.equal(observed, 'LATEST QUESTION')
})

test('failed history refresh leaves retry unexecuted', async () => {
  let requests = 0
  const f = promptFixture(
    {
      retryLastTurn: () => {
        requests++
        return Promise.resolve()
      },
    },
    { historyWindow: { messages: [], messageStart: 0, hasOlder: false, olderCursor: null } },
    { loadSessionMessages: () => Promise.reject(new Error('refresh failed')) },
  )
  await f.commands.retryLastTurn('session')
  assert.equal(requests, 0)
})

async function liveSyncFixture(api, initial, locallyOwned = false) {
  const code = transformSync(await readFile('src/features/chat/use-live-session-sync.ts', 'utf8'), {
    loader: 'ts',
    format: 'cjs',
  }).code
  const ref = { current: { session: { ...DEFAULT_SESSION_STATE, ...initial } } }
  const owners = new Set(locallyOwned ? ['session'] : [])
  const modules = {
    react: {
      useCallback: (callback) => callback,
      useRef: (value) => ({ current: value }),
      useEffect() {},
    },
    '@/lib/plan-protocol': { planFromPayloadOr: (data, fallback) => data.plan ?? fallback },
    '@/lib/session-state': sessionState,
    './chat-api': { chatApi: api },
    './chat-errors': { chatErrorMessage: (error) => error.message },
    './live-session-sync': { shouldPollLiveSession: () => false },
    './run-activity': { settleToolCalls: (tools) => tools },
    './use-session-catalog': { FOCUS_MESSAGE_PAGE_SIZE: 50 },
    './voice-response-stream': { publishVoiceSnapshot() {} },
  }
  const module = { exports: {} }
  runInNewContext(code, {
    module,
    exports: module.exports,
    require(id) {
      assert.ok(modules[id], id)
      return modules[id]
    },
  })
  const commands = module.exports.useLiveSessionSync({
    sessionStates: ref.current,
    sessionStatesRef: ref,
    localStreamSessionsRef: { current: owners },
    streamGenerationRef: { current: new Map() },
    updateSessionState(id, update) {
      ref.current[id] = sessionState.applySessionUpdate(ref.current[id], update)
    },
    updateSessions: () => [],
  })
  return {
    commands,
    owners,
    get state() {
      return ref.current.session
    },
  }
}

const historicalWindow = () => ({
  messages: [{ id: 'history', role: 'user', text: 'old' }],
  messageStart: 20,
  hasOlder: true,
  olderCursor: '20',
})

test('returning from history while locally streaming preserves stream ownership without requesting a snapshot', async () => {
  let reads = 0
  const messages = [{ id: 'agent-local', role: 'agent', text: 'partial', streaming: true }]
  const f = await liveSyncFixture(
    {
      getLiveSession: () => {
        reads++
        throw new Error('must not replace local identity')
      },
      getMessages: () => {
        reads++
        throw new Error('must not replace local identity')
      },
    },
    { messages, streaming: true, historyWindow: historicalWindow() },
    true,
  )
  await f.commands.loadSessionMessages('session', { force: true })
  assert.equal(reads, 0)
  assert.equal(f.state.historyWindow, null)
  assert.equal(f.state.messages, messages)
  assert.ok(f.owners.has('session'))
})

test('explicit latest during recovery clears the history projection and updates the latest snapshot', async () => {
  const window = historicalWindow()
  const f = await liveSyncFixture(
    {
      getLiveSession: () =>
        Promise.resolve({
          messages: [{ id: 'live-session', role: 'agent', text: 'LATEST' }],
          pageInfo: { start: 100 },
          streaming: true,
        }),
    },
    { messages: [], recovering: true, streaming: true, historyWindow: window },
  )
  await f.commands.syncLiveSession('session')
  assert.equal(f.state.historyWindow, window)
  await f.commands.loadSessionMessages('session', { force: true })
  assert.equal(f.state.historyWindow, null)
  assert.equal(f.state.messages.at(-1).text, 'LATEST')
})

test('retry aborts when an intervening latest user identity invalidates the captured turn', async () => {
  let calls = 0
  const f = promptFixture(
    {
      retryLastTurn: () => {
        calls++
        return Promise.resolve()
      },
    },
    { messages: [{ id: 'latest', role: 'user', text: 'latest' }] },
  )
  const result = await f.commands.sendPrompt('old', 'session', [], false, false, null, null, {
    retryFromIndex: 0,
    retryUserMessageId: 'old',
  })
  assert.equal(result, false)
  assert.equal(calls, 0)
  assert.equal(f.state.messages[0].text, 'latest')
})
