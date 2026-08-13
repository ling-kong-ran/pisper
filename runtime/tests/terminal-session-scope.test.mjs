import assert from 'node:assert/strict'
import test from 'node:test'
import {
  activeSessionTerminalId,
  markOrphanedSessionTerminals,
  visibleSessionTerminals,
} from '../../src/features/terminal/terminal-session-scope.ts'

const terminals = [
  { id: 'a-1', sessionId: 'session-a', orphaned: false },
  { id: 'b-1', sessionId: 'session-b', orphaned: false },
  { id: 'a-2', sessionId: 'session-a', orphaned: false },
]

test('session terminals are isolated and restore the last active terminal per session', () => {
  assert.deepEqual(
    visibleSessionTerminals(terminals, 'session-a').map((terminal) => terminal.id),
    ['a-1', 'a-2'],
  )
  assert.deepEqual(
    visibleSessionTerminals(terminals, 'session-b').map((terminal) => terminal.id),
    ['b-1'],
  )
  assert.equal(
    activeSessionTerminalId(terminals, { 'session-a': 'a-2', 'session-b': 'b-1' }, 'session-a'),
    'a-2',
  )
  assert.equal(activeSessionTerminalId(terminals, {}, 'session-a'), 'a-1')
  assert.equal(activeSessionTerminalId(terminals, {}, 'session-c'), '')
})

test('terminals from a deleted session stay alive as orphaned terminals in the active group', () => {
  const next = markOrphanedSessionTerminals(terminals, ['session-b'], 'session-b')

  assert.deepEqual(next, [
    { id: 'a-1', sessionId: 'session-b', orphaned: true },
    { id: 'b-1', sessionId: 'session-b', orphaned: false },
    { id: 'a-2', sessionId: 'session-b', orphaned: true },
  ])
  assert.strictEqual(markOrphanedSessionTerminals(next, ['session-b'], 'session-b'), next)
})

test('deleted-session terminals remain available without an active session', () => {
  const orphaned = markOrphanedSessionTerminals(terminals, [], '')
  assert.deepEqual(orphaned, [
    { id: 'a-1', sessionId: '', orphaned: true },
    { id: 'b-1', sessionId: '', orphaned: true },
    { id: 'a-2', sessionId: '', orphaned: true },
  ])
  assert.deepEqual(markOrphanedSessionTerminals(orphaned, ['session-c'], 'session-c'), [
    { id: 'a-1', sessionId: 'session-c', orphaned: true },
    { id: 'b-1', sessionId: 'session-c', orphaned: true },
    { id: 'a-2', sessionId: 'session-c', orphaned: true },
  ])
})
