import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createContextPages,
  updateContextPages as update,
  MAX_CONTEXT_PAGES,
} from '../../src/features/chat/session-context-pages.ts'

test('auxiliary pages maintain independent browser state through select, change and close', () => {
  let state = createContextPages('browser')
  const original = structuredClone(state)
  state = update(state, {
    type: 'browser',
    id: 'page-1',
    draft: 'https://a.example',
    url: 'https://a.example/',
  })
  state = update(state, { type: 'add', kind: 'browser' })
  state = update(state, {
    type: 'browser',
    id: 'page-2',
    draft: 'https://b.example',
    url: 'https://b.example/',
  })
  state = update(state, { type: 'select', id: 'page-1' })
  assert.equal(state.pages[0].url, 'https://a.example/')
  state = update(state, { type: 'kind', id: 'page-1', kind: 'plan' })
  assert.equal(state.pages[1].url, 'https://b.example/')
  state = update(state, { type: 'close', id: 'page-1' })
  assert.equal(state.activeId, 'page-2')
  assert.equal(state.pages[0].url, 'https://b.example/')
  state = update(state, { type: 'add', kind: 'files' })
  assert.equal(state.activeId, 'page-3')
  assert.deepEqual(original, createContextPages('browser'))
})
test('page limit and stale actions cannot damage existing pages', () => {
  let state = createContextPages()
  for (let i = 1; i < MAX_CONTEXT_PAGES; i++) state = update(state, { type: 'add', kind: 'files' })
  assert.equal(update(state, { type: 'add', kind: 'browser' }), state)
  for (const type of ['select', 'close', 'kind', 'browser'])
    assert.equal(update(state, { type, id: 'missing', kind: 'plan', url: 'bad' }), state)
  assert.equal(new Set(state.pages.map((page) => page.id)).size, MAX_CONTEXT_PAGES)
})
test('closing the last page resets reopen state and closing an inactive page retains selection', () => {
  let state = update(createContextPages('browser'), { type: 'add', kind: 'plan' })
  state = update(state, { type: 'close', id: 'page-1' })
  assert.equal(state.activeId, 'page-2')
  state = update(state, { type: 'close', id: 'page-2' })
  assert.deepEqual(state, createContextPages())
})
