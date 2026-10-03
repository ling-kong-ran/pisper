import assert from 'node:assert/strict'
import test from 'node:test'
import { decodeSideChatResponse } from '../../src/features/chat/model/side-chat-api.ts'
import { ApiError } from '../../src/lib/http/api-error.ts'

const response = {
  session: { id: 'side-fixture', cwd: '/workspace', model: 'provider/model', streaming: false },
  expiresAt: '2026-09-28T00:00:00.000Z',
  created: true,
}

test('side chat response accepts empty, created and restored sessions without leaking extensions', () => {
  assert.deepEqual(decodeSideChatResponse({ session: null, expiresAt: null, created: false }), {
    session: null,
    expiresAt: null,
    created: false,
  })
  for (const created of [true, false]) {
    assert.deepEqual(
      decodeSideChatResponse({
        ...response,
        created,
        extension: 'ignored',
        session: { ...response.session, extension: 'ignored' },
      }),
      { ...response, created },
    )
  }
})

test('side chat response rejects malformed metadata with a stable protocol error', () => {
  for (const input of [
    null,
    [],
    {},
    { ...response, created: 'true' },
    { ...response, expiresAt: 'invalid' },
    { ...response, expiresAt: null },
    { ...response, session: null },
    { session: null, expiresAt: null, created: true },
    { ...response, session: {} },
    { ...response, session: { id: '' } },
    { ...response, session: { id: 'side-fixture', cwd: 42 } },
    { ...response, session: { id: 'side-fixture', streaming: 'false' } },
  ]) {
    assert.throws(
      () => decodeSideChatResponse(input),
      (error) => {
        assert.ok(error instanceof ApiError)
        assert.equal(error.kind, 'protocol')
        assert.equal(error.data.code, 'INVALID_RESPONSE')
        return true
      },
    )
  }
})
