import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  createDockLayoutEnvelope,
  createSessionOpenRequest,
  dockPositionForDisposition,
  initialDockSessionIds,
  panelIdForSession,
  parseDockLayoutEnvelope,
  parseSessionOpenRequest,
  sessionIdFromPanel,
} from '../../src/features/chat/dock-layout.ts'

test('session panel ids round-trip through dock panel metadata', () => {
  assert.equal(panelIdForSession('alpha'), 'session:alpha')
  assert.equal(sessionIdFromPanel('session:alpha'), 'alpha')
  assert.equal(sessionIdFromPanel({ id: 'ignored', params: { sessionId: 'beta' } }), 'beta')
  assert.equal(sessionIdFromPanel({ id: 'other:alpha' }), '')
})

test('dock layout envelopes reject incompatible or malformed state', () => {
  const layout = { grid: { root: 'group-1' }, panels: {} }
  const envelope = createDockLayoutEnvelope(layout, 'session:alpha')
  assert.deepEqual(parseDockLayoutEnvelope(JSON.stringify(envelope)), envelope)
  assert.equal(parseDockLayoutEnvelope('{bad json'), null)
  assert.equal(parseDockLayoutEnvelope({ ...envelope, version: 2 }), null)
  assert.equal(parseDockLayoutEnvelope({ ...envelope, engine: 'other' }), null)
  assert.equal(parseDockLayoutEnvelope({ ...envelope, layout: [] }), null)
})

test('session open requests accept horizontal and vertical dispositions', () => {
  assert.equal(dockPositionForDisposition('left'), 'left')
  assert.equal(dockPositionForDisposition('right'), 'right')
  assert.equal(dockPositionForDisposition('above'), 'top')
  assert.equal(dockPositionForDisposition('below'), 'bottom')
  assert.deepEqual(createSessionOpenRequest('alpha', 'left'), {
    sessionId: 'alpha',
    disposition: 'left',
  })
  assert.deepEqual(createSessionOpenRequest('alpha', 'above'), {
    sessionId: 'alpha',
    disposition: 'above',
  })
  assert.deepEqual(parseSessionOpenRequest('{"sessionId":"beta","disposition":"right"}'), {
    sessionId: 'beta',
    disposition: 'right',
  })
  assert.deepEqual(parseSessionOpenRequest('{"sessionId":"beta","disposition":"below"}'), {
    sessionId: 'beta',
    disposition: 'below',
  })
  assert.equal(createSessionOpenRequest('alpha', 'top'), null)
  assert.equal(parseSessionOpenRequest('{"sessionId":"alpha","disposition":"bottom"}'), null)
})

test('chat split controls expose left, right, top and bottom actions', async () => {
  const [dock, focus, page, history] = await Promise.all([
    readFile('src/features/chat/ChatDock.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/chat/ChatHistoryPage.tsx', 'utf8'),
  ])
  assert.match(dock, /onSplit\(session\.id, 'above'\)/)
  assert.match(dock, /onSplit\(session\.id, 'below'\)/)
  assert.match(focus, /onSplitTop/)
  assert.match(focus, /onSplitBottom/)
  assert.match(page, /splitDockPanel\(panel\.id, 'above'\)/)
  assert.match(page, /splitDockPanel\(panel\.id, 'below'\)/)
  assert.match(history, /openSession\(session\.id, 'above'\)/)
  assert.match(history, /openSession\(session\.id, 'below'\)/)
})

test('initial dock sessions prefer the active chat and migrate valid legacy tabs once', () => {
  assert.deepEqual(initialDockSessionIds({
    activeSessionId: 'b',
    legacyTiledSessionIds: ['a', 'b', 'missing', 'c'],
    validSessionIds: ['a', 'b', 'c'],
  }), ['b', 'a', 'c'])
  assert.deepEqual(initialDockSessionIds({
    activeSessionId: 'missing',
    legacyTiledSessionIds: [],
    validSessionIds: ['first', 'second'],
  }), ['first'])
})
