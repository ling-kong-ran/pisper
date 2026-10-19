import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createContextPages,
  updateContextPages as update,
  MAX_CONTEXT_PAGES,
} from '../../src/features/chat/session-context-pages.ts'
import { useContextPagesStore } from '../../src/features/chat/session-context-pages-store.ts'
import {
  clearComposerDraft,
  readComposerDraft,
  updateComposerDraft,
} from '../../src/features/chat/composer-drafts.ts'

test('first launcher opens exactly one blank tab without manufacturing a files tab', () => {
  useContextPagesStore.setState({ sessions: {} })
  useContextPagesStore.getState().update('fresh', 'files', { type: 'add', kind: 'new' })
  assert.deepEqual(
    useContextPagesStore.getState().sessions.fresh.pages.map((page) => page.kind),
    ['new'],
  )
})

test('blank launcher reuses existing resource and closes without leaving a phantom tab', () => {
  let state = update(createContextPages('changes'), { type: 'add', kind: 'new' })
  const blank = state.activeId
  state = update(state, { type: 'open', kind: 'changes', replaceId: blank })
  assert.equal(state.pages.length, 1)
  assert.equal(state.pages[0].kind, 'changes')
  state = update(state, { type: 'close', id: state.activeId })
  assert.deepEqual(state.pages, [])
  assert.equal(state.activeId, '')
  assert.equal(state.contentOpen, false)
})

test('closing an active tab chooses the right neighbor then the left', () => {
  let state = createContextPages('new')
  state = update(state, { type: 'add', kind: 'browser' })
  state = update(state, { type: 'add', kind: 'terminal', resourceId: 'pty-1' })
  state = update(state, { type: 'select', id: 'page-2' })
  state = update(state, { type: 'close', id: 'page-2' })
  assert.equal(state.activeId, 'page-3')
  state = update(state, { type: 'close', id: 'page-3' })
  assert.equal(state.activeId, 'page-1')
})

test('per-task workspaces retain scroll, summary, tabs and delayed resource state beyond old cache limits', () => {
  useContextPagesStore.setState({ sessions: {} })
  const store = () => useContextPagesStore.getState()
  store().update('a', 'changes', { type: 'open', kind: 'changes' })
  store().update('a', 'changes', {
    type: 'workspace',
    summaryOpen: true,
    conversationScrollTop: 412,
  })
  for (let i = 0; i < 40; i++) store().update(`b-${i}`, 'files', { type: 'add', kind: 'new' })
  store().update('a', 'changes', {
    type: 'view',
    id: 'page-1',
    patch: { expandedFiles: ['src/a.ts'] },
  })
  assert.equal(store().sessions.a.summaryOpen, true)
  assert.equal(store().sessions.a.conversationScrollTop, 412)
  assert.deepEqual(store().sessions.a.pages[0].view.expandedFiles, ['src/a.ts'])
  assert.deepEqual(store().sessions['b-39'].pages[0].view.expandedFiles, [])
})

test('page limit returns an explicit outcome and still allows resource reuse', () => {
  useContextPagesStore.setState({ sessions: {} })
  const store = useContextPagesStore.getState()
  store.update('limit', 'changes', { type: 'open', kind: 'changes' })
  for (let i = 1; i < MAX_CONTEXT_PAGES; i++)
    store.update('limit', 'changes', { type: 'add', kind: 'new' })
  assert.equal(store.update('limit', 'changes', { type: 'add', kind: 'new' }), 'limit')
  assert.equal(store.update('limit', 'changes', { type: 'open', kind: 'changes' }), 'updated')
  assert.equal(useContextPagesStore.getState().sessions.limit.pages.length, MAX_CONTEXT_PAGES)
})

test('unsent drafts are never evicted merely because another task is opened', () => {
  try {
    for (let i = 0; i < 80; i++) updateComposerDraft(`retain-${i}`, { text: `draft ${i}` })
    assert.equal(readComposerDraft('retain-0').text, 'draft 0')
  } finally {
    for (let i = 0; i < 80; i++) clearComposerDraft(`retain-${i}`)
  }
})

test('file identity and change comparison scope keep distinct resources separate', () => {
  let state = createContextPages('files')
  state = update(state, {
    type: 'open',
    kind: 'files',
    resourceId: 'connection-a/project-a/src/index.ts',
  })
  const first = state.activeId
  state = update(state, {
    type: 'open',
    kind: 'files',
    resourceId: 'connection-a/project-b/src/index.ts',
  })
  assert.notEqual(state.activeId, first)
  state = update(state, {
    type: 'open',
    kind: 'files',
    resourceId: 'connection-a/project-a/src/index.ts',
  })
  assert.equal(state.activeId, first)
  state = update(state, { type: 'open', kind: 'changes', resourceId: 'project-a/worktree' })
  const worktree = state.activeId
  state = update(state, { type: 'open', kind: 'changes', resourceId: 'project-a/branch/main' })
  assert.notEqual(state.activeId, worktree)
})

test('late terminal creation binds only its original page and never reopens a closed page', () => {
  let state = createContextPages('terminal')
  const id = state.activeId
  state = update(state, { type: 'resource', id, resourceId: 'pty-a' })
  assert.equal(state.pages[0].resourceId, 'pty-a')
  state = update(state, { type: 'close', id })
  assert.equal(update(state, { type: 'resource', id, resourceId: 'pty-b' }), state)
})
