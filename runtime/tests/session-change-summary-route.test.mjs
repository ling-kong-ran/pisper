import assert from 'node:assert/strict'
import test from 'node:test'
import { sessionRuntimeRoutes } from '../http/routes/sessions-runtime.mjs'

test('session change summary route preserves the complete bounded summary contract', async () => {
  const route = sessionRuntimeRoutes.find(
    (entry) => entry.method === 'GET' && entry.path === '/api/sessions/:sessionId/change-summary',
  )
  assert.ok(route)
  const summary = {
    status: 'known',
    changedFiles: 2,
    pendingFiles: 1,
    added: 8,
    removed: 3,
    unknownFiles: 0,
    capped: false,
  }
  let response
  await route.handler({
    runtime: {
      async getSessionChangeSummary(id) {
        assert.equal(id, 'session-1')
        return summary
      },
    },
    params: { sessionId: 'session-1' },
    json: (status, body) => {
      response = { status, body }
    },
  })
  assert.deepEqual(response, { status: 200, body: summary })
})
