import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createDockLayoutEnvelope,
  createSessionOpenRequest,
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

test('session open requests accept only supported dispositions', () => {
  assert.deepEqual(createSessionOpenRequest('alpha', 'left'), { sessionId: 'alpha', disposition: 'left' })
  assert.deepEqual(parseSessionOpenRequest('{"sessionId":"beta","disposition":"right"}'), { sessionId: 'beta', disposition: 'right' })
  assert.equal(createSessionOpenRequest('alpha', 'below'), null)
  assert.equal(parseSessionOpenRequest('{"sessionId":"alpha","disposition":"below"}'), null)
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
