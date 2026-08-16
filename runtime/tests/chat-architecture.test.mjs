import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'
import { DEFAULT_SESSION_STATE, applySessionUpdate } from '../../src/lib/session-state.ts'
import {
  reconcileLiveSnapshot,
  reconcileMessagePage,
} from '../../src/features/chat/use-live-session-sync.ts'
import {
  createStreamEventDispatcher,
  reconcileTerminalStreamState,
} from '../../src/features/chat/stream-event-dispatch.ts'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..')

function sessionState(update = {}) {
  return {
    ...DEFAULT_SESSION_STATE,
    messages: [],
    tools: [],
    approvals: [],
    queuedInputs: [],
    activityFeed: [],
    agents: [],
    ...update,
  }
}

test('message page reconciliation preserves an already loaded prefix', () => {
  const current = sessionState({
    messages: [
      { id: 'm0', role: 'user', text: 'zero' },
      { id: 'm1', role: 'agent', text: 'one' },
      { id: 'm2', role: 'user', text: 'stale' },
    ],
    messageStart: 0,
  })
  const page = reconcileMessagePage(current, {
    messages: [{ id: 'm2', role: 'user', text: 'fresh' }],
    pageInfo: { start: 2 },
  })
  assert.deepEqual(
    page.messages.map((message) => [message.id, message.text]),
    [
      ['m0', 'zero'],
      ['m1', 'one'],
      ['m2', 'fresh'],
    ],
  )
  assert.equal(page.messageStart, 0)
  assert.equal(page.hasOlder, false)
})

test('live and terminal reconciliation preserve explicit Plan clears', () => {
  const current = sessionState({
    streaming: true,
    plan: {
      items: [{ id: 'old', title: 'Old step', status: 'in_progress' }],
      updatedAt: '2026-08-02T00:00:00.000Z',
    },
    messages: [{ id: 'agent-1', role: 'agent', text: 'partial', streaming: true }],
    currentActivity: { type: 'tool', id: 'tool-1' },
  })
  const live = reconcileLiveSnapshot(
    current,
    {
      messages: current.messages,
      pageInfo: { start: 0 },
      streaming: false,
      tools: [],
      activityFeed: [
        { type: 'tool', id: 'settled' },
        { type: 'agent', id: 'child' },
      ],
      agents: [],
      plan: null,
      lifecycle: { phase: 'completed', event: 'runtime_done', turn: 2 },
      sessionTreeRevision: 4,
    },
    '2026-08-02T01:00:00.000Z',
  )
  assert.equal(live.plan, null)
  assert.equal(live.streaming, false)
  assert.equal(live.lifecycle.event, 'runtime_done')
  assert.equal(live.sessionTreeRevision, 4)
  assert.deepEqual(live.activityFeed, [{ type: 'agent', id: 'child' }])

  const terminal = reconcileTerminalStreamState(current, {
    agentId: 'agent-1',
    responseText: 'fallback',
    data: {
      text: 'durable',
      tools: [],
      activityFeed: [],
      agents: [],
      plan: null,
    },
    finishedAt: '2026-08-02T01:01:00.000Z',
  })
  assert.equal(terminal.plan, null)
  assert.equal(terminal.messages[0].text, 'durable')
  assert.equal(terminal.messages[0].streaming, false)
})

test('stream dispatcher applies Plan updates in place and clears them on done', () => {
  let state = sessionState({
    streaming: true,
    messages: [{ id: 'agent-1', role: 'agent', text: '', streaming: true }],
  })
  let sessions = [{ id: 'session-1', name: 'Session', plan: null }]
  const sessionStatesRef = { current: { 'session-1': state } }
  const updateSessionState = (_id, update) => {
    state = applySessionUpdate(state, update)
    sessionStatesRef.current['session-1'] = state
  }
  const updateSessions = (update) => {
    sessions = typeof update === 'function' ? update(sessions) : update
    return sessions
  }
  const scheduler = { push() {}, flush() {}, cancel() {} }
  const typewriter = { ...scheduler, setTarget() {} }
  const dispatcher = createStreamEventDispatcher({
    sessionId: 'session-1',
    agentId: 'agent-1',
    sessionStatesRef,
    updateSessionState,
    updateSessions,
    typewriter,
    thinkingScheduler: scheduler,
    toolScheduler: scheduler,
    t: (message) => message,
  })
  const plan = {
    items: [{ id: 'step-1', title: 'Ship it', status: 'in_progress' }],
    updatedAt: '2026-08-02T02:00:00.000Z',
  }
  dispatcher.dispatch('plan_update', { plan })
  assert.equal(state.plan, plan)
  assert.equal(sessions[0].plan, plan)
  assert.equal(state.currentActivity.type, 'plan')

  dispatcher.dispatch('retry', { attempt: 1, maxAttempts: 3, message: 'Temporary error' })
  assert.ok(state.runNotice)
  dispatcher.dispatch('agent_lifecycle', {
    lifecycle: {
      phase: 'thinking',
      event: 'turn_start',
      turn: 2,
      updatedAt: '2026-08-02T02:00:30.000Z',
    },
    currentActivity: { type: 'model', stage: 'thinking' },
  })
  assert.equal(state.lifecycle.event, 'turn_start')
  assert.equal(state.runNotice, '')
  dispatcher.dispatch('thinking_level_changed', { level: 'high' })
  assert.equal(state.thinkingLevel, 'high')
  assert.equal(sessions[0].thinkingLevel, 'high')
  dispatcher.dispatch('session_tree_changed', { revision: 3 })
  assert.equal(state.sessionTreeRevision, 3)

  const keepStreaming = dispatcher.dispatch('done', {
    text: 'complete',
    plan: null,
    tools: [],
    activityFeed: [],
    agents: [],
    finishedAt: '2026-08-02T02:01:00.000Z',
  })
  assert.equal(keepStreaming, false)
  assert.equal(state.plan, null)
  assert.equal(sessions[0].plan, null)
  assert.equal(state.messages[0].text, 'complete')
  assert.equal(state.lifecycle.phase, 'completed')
  assert.equal(state.lifecycle.event, 'runtime_done')
})

test('dock split handles stay contained below global overlays', async () => {
  const dock = await readFile(resolve(root, 'src/features/chat/ChatPage.tsx'), 'utf8')
  assert.match(dock, /chat-dock-workspace[^"\n]*isolate/)
})

test('chat facade and focus layout stay below their architecture budgets', async () => {
  const [chatPage, focusSession, transcript, dock, liveSync, promptCommands] = await Promise.all(
    [
      'src/features/chat/ChatPage.tsx',
      'src/features/chat/FocusSession.tsx',
      'src/features/chat/FocusTranscript.tsx',
      'src/features/chat/use-chat-dock.ts',
      'src/features/chat/use-live-session-sync.ts',
      'src/features/chat/use-prompt-commands.ts',
    ].map((path) => readFile(resolve(root, path), 'utf8')),
  )
  assert.ok(chatPage.split(/\r?\n/).length < 800)
  assert.ok(focusSession.split(/\r?\n/).length < 750)
  assert.match(chatPage, /useSessionCatalog/)
  assert.match(chatPage, /useChatDock/)
  assert.match(chatPage, /useLiveSessionSync/)
  assert.match(chatPage, /usePromptCommands/)
  assert.doesNotMatch(chatPage, /chatPage\.deriveChatDescription/)
  assert.match(focusSession, /<FocusTranscript/)
  assert.match(transcript, /scrollRequest/)
  assert.doesNotMatch(dock, /from ['"].*ChatPage/)
  assert.doesNotMatch(liveSync, /from ['"].*ChatPage/)
  assert.doesNotMatch(promptCommands, /from ['"].*ChatPage/)
})
