import assert from 'node:assert/strict'
import test from 'node:test'
import { legacyHashPath, pageFromPath, pagePath, workflowPath } from '../../src/app/routes.ts'

test('page ids map to stable application paths', () => {
  assert.equal(pagePath('chat'), '/chat')
  assert.equal(pagePath('chatHistory'), '/chat/history')
  assert.equal(pagePath('workflowCreate'), '/workflows/new')
  assert.equal(pagePath('unknown'), '/chat')
})

test('router paths resolve to page ids and tolerate trailing slashes', () => {
  assert.equal(pageFromPath('/config'), 'config')
  assert.equal(pageFromPath('/chat/history/'), 'chatHistory')
  assert.equal(pageFromPath('/workflows/workflow-1'), 'workflowCreate')
  assert.equal(pageFromPath('/unknown'), null)
})

test('workflow editor paths preserve the selected workflow id', () => {
  assert.equal(workflowPath('workflow 1'), '/workflows/workflow%201')
  assert.equal(workflowPath(), '/workflows/new')
})

test('legacy hash urls migrate without changing modern router urls', () => {
  assert.equal(legacyHashPath('#chat'), '/chat')
  assert.equal(legacyHashPath('#workflowCreate'), '/workflows/new')
  assert.equal(legacyHashPath('#/chat'), null)
})
