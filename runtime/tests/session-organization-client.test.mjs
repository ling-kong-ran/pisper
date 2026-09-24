import assert from 'node:assert/strict'
import test from 'node:test'
import { parseSessionOrganization } from '../../src/features/chat/session-organization-api.ts'
import { applySessionOrganizationUpdate } from '../../src/features/chat/session-list.ts'
import { parseSessionChangeSummary } from '../../src/features/chat/session-change-summary-api.ts'

test('client applies only a confirmed organization response to its existing session', () => {
  const response = parseSessionOrganization({
    id: 'one',
    pinned: true,
    archived: true,
    unread: false,
    needsAttention: true,
    attentionReason: 'failure',
  })
  const current = [
    { id: 'one', name: 'Keep title', modified: '2026-01-01' },
    { id: 'two', name: 'Other title', modified: '2026-01-02' },
  ]
  const updated = applySessionOrganizationUpdate(current, response)
  assert.deepEqual(updated[0], { ...current[0], ...response })
  assert.strictEqual(updated[1], current[1])
  assert.throws(() => parseSessionOrganization({ id: 'one', pinned: 'true' }))
  assert.throws(() => parseSessionOrganization({ ...response, attentionReason: 'unknown' }))
})

test('client never displays ambiguous session change counts as exact', () => {
  assert.deepEqual(
    parseSessionChangeSummary({
      status: 'known',
      changedFiles: 1,
      pendingFiles: 0,
      added: 2,
      removed: 1,
      unknownFiles: 0,
      capped: false,
    }).changedFiles,
    1,
  )
  assert.throws(() =>
    parseSessionChangeSummary({
      status: 'partial',
      changedFiles: 1,
      pendingFiles: null,
      added: null,
      removed: null,
      unknownFiles: 1,
      capped: false,
    }),
  )
})
