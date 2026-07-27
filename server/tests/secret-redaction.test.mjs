import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  installSessionPersistenceRedaction,
  REDACTED_SECRET,
  createStreamingSecretRedactor,
  redactPersistedSessionFiles,
  redactSecretText,
  redactSecretValue,
} from '../security/secret-redaction.mjs'
import { applyTextPatch } from '../../src/lib/api.ts'

test('credential redaction removes common secrets without hiding token usage fields', () => {
  const text = redactSecretText([
    'apiKey: sk-example-secret-token-1234567890',
    'Authorization: Bearer abcdefghijklmnopqrstuvwxyz',
    'https://example.test/mcp?access_token=private-token-value',
  ].join('\n'))
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
  assert.match(redactSecretText('token: completion-budget\nnextToken: page-2'), /token: completion-budget\nnextToken: page-2/)
  assert.doesNotMatch(redactSecretText('token: secret-value-that-is-long-123456'), /secret-value-that-is-long/)
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

test('session persistence redacts a copy without changing the Agent message', () => {
  const persisted = []
  const manager = {
    appendMessage(message) {
      persisted.push(message)
      return 'message-1'
    },
  }
  installSessionPersistenceRedaction(manager)
  const message = {
    role: 'toolResult',
    content: [{ type: 'text', text: '{"accessToken":"pencil-private-token"}' }],
    details: { Authorization: 'Bearer another-private-token' },
  }

  assert.equal(manager.appendMessage(message), 'message-1')
  assert.match(JSON.stringify(message), /pencil-private-token|another-private-token/)
  assert.doesNotMatch(JSON.stringify(persisted[0]), /pencil-private-token|another-private-token/)
})

test('existing JSONL sessions are scrubbed while preserving ordinary usage data', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-redaction-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'session.jsonl')
  await writeFile(path, `${JSON.stringify({
    type: 'message',
    message: {
      role: 'toolResult',
      content: [{ type: 'text', text: '{"accessToken":"persisted-private-token"}' }],
      usage: { maxTokens: 128_000, totalTokens: 123 },
    },
  })}\n`, 'utf8')

  assert.equal(await redactPersistedSessionFiles(directory), 1)
  const entry = JSON.parse((await readFile(path, 'utf8')).trim())
  assert.doesNotMatch(JSON.stringify(entry), /persisted-private-token/)
  assert.equal(entry.message.usage.maxTokens, 128_000)
  assert.equal(entry.message.usage.totalTokens, 123)
  assert.equal(await redactPersistedSessionFiles(directory), 0)
})

test('streaming redaction preserves text order and hides secrets split across chunks', () => {
  const redactor = createStreamingSecretRedactor()
  const source = '调用 `rgPath`，然后设置 apiKey: sk-example-secret-token-1234567890。\n\n## 最终建议\n保留 `--json`。'
  const chunks = ['调用 `rg', 'Path`，然后设置 api', 'Key: sk-example-', 'secret-token-1234567890。\n', '\n## 最终', '建议\n保留 `--json`。']
  let visible = ''
  for (const chunk of chunks) {
    const patch = redactor.push(chunk)
    if (patch) visible = applyTextPatch(visible, patch)
    assert.doesNotMatch(visible, /sk-example|secret-token/)
  }
  visible = applyTextPatch(visible, redactor.flush())
  assert.equal(visible, redactSecretText(source))
})

test('streaming redaction keeps flushed content visible when a later block starts', () => {
  const redactor = createStreamingSecretRedactor()
  let visible = applyTextPatch('', redactor.push('First completed response block.'))
  visible = applyTextPatch(visible, redactor.flush())
  assert.equal(visible, 'First completed response block.')

  visible = applyTextPatch(visible, redactor.push(' Next'))
  assert.equal(visible, 'First completed response block.')
  visible = applyTextPatch(visible, redactor.flush())
  assert.equal(visible, 'First completed response block. Next')
})

test('streaming redaction withholds an incomplete private key', () => {
  const redactor = createStreamingSecretRedactor({ guardLength: 0 })
  let visible = applyTextPatch('', redactor.push('before\n-----BEGIN PRIVATE KEY-----\nprivate-material'))
  assert.equal(visible, 'before\n')
  visible = applyTextPatch(visible, redactor.push('\n-----END PRIVATE KEY-----\nafter'))
  visible = applyTextPatch(visible, redactor.flush())
  assert.equal(visible, `before\n${REDACTED_SECRET}\nafter`)
})
