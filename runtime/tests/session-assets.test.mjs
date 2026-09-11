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

// 资产在工具执行时产生，时间戳晚于该轮消息起点：必须归属到「创建它的那轮」，
// 而不是错挂到下一轮的助手消息上。
test('workspace file assets attach to the turn that created them, not the next turn', () => {
  const messages = [
    { id: 'user-1', role: 'user', text: '第一轮', timestamp: 1000 },
    { id: 'agent-1', role: 'agent', text: '第一轮回复', timestamp: 2000 },
    { id: 'user-2', role: 'user', text: '第二轮', timestamp: 4000 },
    { id: 'agent-2', role: 'agent', text: '第二轮回复', timestamp: 5000 },
  ]
  const asset = {
    id: 'asset-file-1',
    name: 'app.ts',
    mimeType: 'text/typescript',
    size: 42,
    filePath: '/workspace/app.ts',
    created: new Date(3000).toISOString(),
  }
  const result = attachGeneratedAssets(messages, [asset])
  assert.equal(result[1].attachments[0]?.id, 'asset-file-1')
  assert.equal(result[3].attachments.length, 0)
})

// 会话最后一轮之后产生的资产（当前轮）仍然挂到最后一条助手消息。
test('assets created after the final turn fall back to the last assistant message', () => {
  const messages = [
    { id: 'user-1', role: 'user', text: 'hi', timestamp: 1000 },
    { id: 'agent-1', role: 'agent', text: 'done', timestamp: 2000 },
  ]
  const asset = {
    id: 'asset-late',
    name: 'late.txt',
    mimeType: 'text/plain',
    size: 3,
    filePath: '/workspace/late.txt',
    created: new Date(9000).toISOString(),
  }
  const result = attachGeneratedAssets(messages, [asset])
  assert.equal(result[1].attachments[0]?.id, 'asset-late')
})
