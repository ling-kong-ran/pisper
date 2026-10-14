import assert from 'node:assert/strict'
import test from 'node:test'
import { loadAttachmentPreview } from '../../src/features/chat/attachment-preview.ts'

test('file preview uses the bounded asset endpoint, not attachment paths or URLs', async (t) => {
  const calls = []
  const controller = new AbortController()
  t.mock.method(globalThis, 'fetch', async (url, options) => {
    calls.push({ url, options })
    return Response.json({ kind: 'text', text: 'export const answer = 42', truncated: true })
  })
  const result = await loadAttachmentPreview(
    { id: 'asset / 1', path: '/private/file', url: 'https://untrusted.example/file' },
    controller.signal,
  )
  assert.equal(result.text, 'export const answer = 42')
  assert.equal(result.truncated, true)
  assert.equal(calls.length, 1)
  assert.equal(calls[0].url, '/api/assets/asset%20%2F%201/content?preview=1')
  assert.ok(calls[0].options.signal instanceof AbortSignal)
})

test('path-only attachments do not start arbitrary file reads and inline text stays bounded', async (t) => {
  t.mock.method(globalThis, 'fetch', () => assert.fail('No request expected'))
  assert.deepEqual(
    await loadAttachmentPreview({ id: 'path:/private/file', path: '/private/file' }),
    { kind: 'file' },
  )
  assert.deepEqual(await loadAttachmentPreview({ text: '' }), {
    kind: 'text',
    text: '',
    mimeType: undefined,
    truncated: false,
  })
  const content = await loadAttachmentPreview({ text: 'x'.repeat(400_001) })
  assert.equal(content.text.length, 400_000)
  assert.equal(content.truncated, true)
})

test('file preview surfaces missing assets and supports cancellation', async (t) => {
  t.mock.method(globalThis, 'fetch', async () =>
    Response.json({ error: 'Asset not found' }, { status: 404 }),
  )
  await assert.rejects(loadAttachmentPreview({ id: 'missing' }), /Asset not found/)
  const controller = new AbortController()
  controller.abort()
  await assert.rejects(
    loadAttachmentPreview({ id: 'cancelled' }, controller.signal),
    (error) => error.kind === 'cancelled',
  )
})
