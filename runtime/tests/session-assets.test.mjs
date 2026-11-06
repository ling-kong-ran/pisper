import assert from 'node:assert/strict'
import test from 'node:test'
import {
  assetMessageAttachment,
  attachGeneratedAssets,
  dedupeMessageAttachments,
} from '../services/session-assets.mjs'

test('message attachments deduplicate the same file path across different asset ids', () => {
  const attachments = dedupeMessageAttachments([
    { id: 'asset-1', kind: 'file', path: 'C:/workspace/app.ts' },
    { id: 'asset-2', kind: 'file', path: 'c:\\workspace\\app.ts' },
    { id: 'asset-3', kind: 'file', path: 'C:/workspace/other.ts' },
  ])
  assert.deepEqual(
    attachments.map((attachment) => attachment.id),
    ['asset-1', 'asset-3'],
  )
})
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

test('assets hidden from replies stay off messages after a reload', () => {
  const messages = [{ id: 'agent-1', role: 'agent', text: 'done', timestamp: 1000 }]
  const asset = (id, extra = {}) => ({
    id,
    name: `${id}.md`,
    mimeType: 'text/markdown',
    filePath: `/workspace/${id}.md`,
    created: new Date(2000).toISOString(),
    ...extra,
  })
  const result = attachGeneratedAssets(messages, [
    asset('updated', { hiddenInReply: true }),
    asset('added'),
  ])
  assert.deepEqual(
    result[0].attachments.map((attachment) => attachment.id),
    ['added'],
  )
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

// 真实时序：工具先产出资产，工具跑完后才开始的最终消息时间戳更晚；不能错挂到上一轮的回复上。
test('assets from a tool attach to the reply that follows it, not the previous turn', () => {
  const messages = [
    { id: 'user-1', role: 'user', text: '第一轮', timestamp: 1000 },
    { id: 'agent-1', role: 'agent', text: '第一轮回复', timestamp: 2000 },
    { id: 'user-2', role: 'user', text: '第二轮', timestamp: 3000 },
    { id: 'agent-2', role: 'agent', text: '第二轮回复', timestamp: 5000 },
  ]
  const asset = {
    id: 'asset-tool',
    name: 'notes.md',
    mimeType: 'text/markdown',
    filePath: '/workspace/notes.md',
    created: new Date(4000).toISOString(),
  }
  const result = attachGeneratedAssets(messages, [asset])
  assert.equal(result[1].attachments.length, 0)
  assert.equal(result[3].attachments[0]?.id, 'asset-tool')
})

test('one turn with several replies attaches assets to the reply after them', () => {
  const messages = [
    { id: 'user-1', role: 'user', text: '目标', timestamp: 1000 },
    { id: 'agent-1', role: 'agent', text: '第一段', timestamp: 2000 },
    { id: 'agent-2', role: 'agent', text: '第二段', timestamp: 4000 },
  ]
  const asset = (id, created) => ({
    id,
    name: `${id}.txt`,
    mimeType: 'text/plain',
    filePath: `/workspace/${id}.txt`,
    created: new Date(created).toISOString(),
  })
  const result = attachGeneratedAssets(messages, [asset('between', 3000), asset('after', 9000)])
  assert.equal(result[1].attachments.length, 0)
  assert.deepEqual(
    result[2].attachments.map((attachment) => attachment.id),
    ['between', 'after'],
  )
})

test('a turn without a reply yet keeps assets on the latest earlier reply', () => {
  const messages = [
    { id: 'user-1', role: 'user', text: '第一轮', timestamp: 1000 },
    { id: 'agent-1', role: 'agent', text: '第一轮回复', timestamp: 2000 },
    { id: 'user-2', role: 'user', text: '第二轮', timestamp: 3000 },
  ]
  const asset = {
    id: 'asset-running',
    name: 'running.txt',
    mimeType: 'text/plain',
    filePath: '/workspace/running.txt',
    created: new Date(4000).toISOString(),
  }
  assert.equal(attachGeneratedAssets(messages, [asset])[1].attachments[0]?.id, 'asset-running')
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

test('workspace file attachments carry the absolute path for reveal and per-file diff', () => {
  const messages = [{ id: 'agent-1', role: 'agent', text: 'done', timestamp: 2000 }]
  const asset = {
    id: 'asset-path',
    name: 'notes.md',
    mimeType: 'text/markdown',
    size: 5,
    filePath: '/workspace/notes.md',
    created: new Date(2500).toISOString(),
  }
  const result = attachGeneratedAssets(messages, [asset])
  assert.equal(result[0].attachments[0].path, '/workspace/notes.md')
  // 媒体资产不带 path：chip 仍走预览/下载，不出现文件操作面板。
  const media = attachGeneratedAssets(messages, [
    { ...asset, id: 'asset-media', mimeType: 'image/png', filePath: undefined },
  ])
  assert.equal('path' in media[0].attachments[0], false)
})
