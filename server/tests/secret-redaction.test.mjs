import assert from 'node:assert/strict'
import test from 'node:test'
import {
  REDACTED_SECRET,
  redactSecretText,
  redactSecretValue,
} from '../security/secret-redaction.mjs'

test('credential redaction removes common secrets without hiding token usage fields', () => {
  const text = redactSecretText(
    [
      'apiKey: sk-example-secret-token-1234567890',
      'Authorization: Bearer abcdefghijklmnopqrstuvwxyz',
      'https://example.test/mcp?access_token=private-token-value',
    ].join('\n'),
  )
  assert.doesNotMatch(text, /sk-example|abcdefghijklmnopqrstuvwxyz|private-token-value/)
  assert.match(text, new RegExp(REDACTED_SECRET.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')))

  const value = redactSecretValue({
    maxTokens: 128_000,
    totalTokens: 42,
    token: 'completion-budget',
    nextToken: 'page-2',
    accessToken: 'pencil-private-token',
    nested: { client_secret: 'client-private-secret' },
  })
  assert.equal(value.maxTokens, 128_000)
  assert.equal(value.totalTokens, 42)
  assert.equal(value.token, 'completion-budget')
  assert.equal(value.nextToken, 'page-2')
  assert.equal(value.accessToken, REDACTED_SECRET)
  assert.equal(value.nested.client_secret, REDACTED_SECRET)
  assert.match(
    redactSecretText('token: completion-budget\nnextToken: page-2'),
    /token: completion-budget\nnextToken: page-2/,
  )
  assert.doesNotMatch(
    redactSecretText('token: secret-value-that-is-long-123456'),
    /secret-value-that-is-long/,
  )
  assert.equal(redactSecretText(text), text)
})

test('credential redaction preserves repeated object references while still stopping real cycles', () => {
  const agent = { id: 'agent-1', status: 'completed' }
  const repeated = redactSecretValue({ agent, currentActivity: { type: 'agent', agent } })
  assert.deepEqual(repeated, {
    agent: { id: 'agent-1', status: 'completed' },
    currentActivity: { type: 'agent', agent: { id: 'agent-1', status: 'completed' } },
  })

  const circular = { id: 'cycle' }
  circular.self = circular
  assert.equal(redactSecretValue(circular).self, '[CIRCULAR]')
})
