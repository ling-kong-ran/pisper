import assert from 'node:assert/strict'
import test from 'node:test'
import { assetMessageAttachment, attachGeneratedAssets } from '../services/session-assets.mjs'

test('generated media is restored on the assistant message after the tool call', () => {
  const messages = [
    { id: 'user-1', role: 'user', text: 'generate an image', timestamp: 1000 },
    { id: 'agent-1', role: 'agent', text: 'Here it is', timestamp: 3000 },
  ]
  const asset = {
    id: 'asset-1',
    name: 'image.png',
    mimeType: 'image/png',
    size: 42,
    created: new Date(2000).toISOString(),
  }
  const result = attachGeneratedAssets(messages, [asset])
  assert.equal(result[1].attachments[0].id, 'asset-1')
  assert.equal(result[1].attachments[0].kind, 'image')
})

test('video attachments expose inline and download URLs', () => {
  const attachment = assetMessageAttachment({
    id: 'video/1',
    name: 'clip.mp4',
    mimeType: 'video/mp4',
    size: 100,
  })
  assert.equal(attachment.kind, 'video')
  assert.equal(attachment.url, '/api/assets/video%2F1/download?inline=1')
  assert.equal(attachment.downloadUrl, '/api/assets/video%2F1/download')
})
