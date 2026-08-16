import assert from 'node:assert/strict'
import test from 'node:test'
import { clipboardFiles, pathAttachments } from '../../src/features/chat/attachments.ts'

test('clipboard files include images and general files', () => {
  const image = { name: 'screenshot.png', type: 'image/png' }
  const document = { name: 'notes.pdf', type: 'application/pdf' }

  assert.deepEqual(clipboardFiles({ files: [image, document] }), [image, document])
})

test('clipboard file items are supported when the files collection is empty', () => {
  const image = { name: 'clipboard.png', type: 'image/png' }
  const document = { name: 'notes.md', type: 'text/markdown' }
  const items = [
    { kind: 'string', type: 'text/plain', getAsFile: () => null },
    { kind: 'file', type: 'image/png', getAsFile: () => image },
    { kind: 'file', type: 'text/markdown', getAsFile: () => document },
  ]

  assert.deepEqual(clipboardFiles({ files: [], items }), [image, document])
})

test('plain text clipboard data does not trigger attachment handling', () => {
  assert.deepEqual(
    clipboardFiles({ files: [], items: [{ kind: 'string', type: 'text/plain' }] }),
    [],
  )
})

test('selected files become path-only attachments without content or size fields', () => {
  assert.deepEqual(pathAttachments(['E:\\workspace\\large.bin', '/workspace/notes.md']), [
    {
      id: 'path:E:\\workspace\\large.bin',
      kind: 'path',
      name: 'large.bin',
      path: 'E:\\workspace\\large.bin',
    },
    {
      id: 'path:/workspace/notes.md',
      kind: 'path',
      name: 'notes.md',
      path: '/workspace/notes.md',
    },
  ])
})
