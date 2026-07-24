import assert from 'node:assert/strict'
import test from 'node:test'
import { clipboardImageFiles } from '../../src/features/chat/attachments.ts'

test('clipboard image files are selected without treating other files as attachments', () => {
  const image = { name: 'screenshot.png', type: 'image/png' }
  const document = { name: 'notes.pdf', type: 'application/pdf' }

  assert.deepEqual(clipboardImageFiles({ files: [image, document] }), [image])
})

test('clipboard image items are supported when the files collection is empty', () => {
  const image = { name: 'clipboard.png', type: 'image/png' }
  const items = [
    { kind: 'string', type: 'text/plain', getAsFile: () => null },
    { kind: 'file', type: 'image/png', getAsFile: () => image },
  ]

  assert.deepEqual(clipboardImageFiles({ files: [], items }), [image])
})

test('plain text clipboard data does not trigger attachment handling', () => {
  assert.deepEqual(clipboardImageFiles({ files: [], items: [{ kind: 'string', type: 'text/plain' }] }), [])
})
