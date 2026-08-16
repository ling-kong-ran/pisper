import assert from 'node:assert/strict'
import test from 'node:test'
import {
  clearComposerDraft,
  readComposerDraft,
  updateComposerDraft,
} from '../../src/features/chat/composer-drafts.ts'

test('composer drafts retain text and attachments independently per session', () => {
  const attachment = { id: 'attachment-1', kind: 'path', path: '/workspace/notes.md' }
  updateComposerDraft('session-a', { text: 'first draft', attachments: [attachment] })
  updateComposerDraft('session-b', { text: 'second draft' })

  assert.deepEqual(readComposerDraft('session-a'), {
    text: 'first draft',
    attachments: [attachment],
  })
  assert.deepEqual(readComposerDraft('session-b'), { text: 'second draft', attachments: [] })

  clearComposerDraft('session-a')
  assert.deepEqual(readComposerDraft('session-a'), { text: '', attachments: [] })
  assert.deepEqual(readComposerDraft('session-b'), { text: 'second draft', attachments: [] })
  clearComposerDraft('session-b')
})

test('empty composer drafts are removed after successful submission', () => {
  updateComposerDraft('session-submit', { text: 'send this' })
  updateComposerDraft('session-submit', { text: '' })

  assert.deepEqual(readComposerDraft('session-submit'), { text: '', attachments: [] })
})
