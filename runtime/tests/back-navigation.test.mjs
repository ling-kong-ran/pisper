import assert from 'node:assert/strict'
import test from 'node:test'
import { createBackStack } from '../../src/lib/back-navigation.ts'
test('back closes one top layer then sidebar then task content, without changing history', () => {
  const stack = createBackStack()
  const calls = []
  const removeContent = stack.register(100, () => {
    calls.push('content')
    removeContent()
  })
  const removeSidebar = stack.register(200, () => {
    calls.push('sidebar')
    removeSidebar()
  })
  const removeModal = stack.register(300, () => {
    calls.push('modal')
    removeModal()
  })
  assert.equal(stack.dismiss(), true)
  assert.deepEqual(calls, ['modal'])
  assert.equal(stack.dismiss(), true)
  assert.deepEqual(calls, ['modal', 'sidebar'])
  assert.equal(stack.dismiss(), true)
  assert.deepEqual(calls, ['modal', 'sidebar', 'content'])
  assert.equal(stack.dismiss(), false)
})
test('most recently mounted sibling wins and disposed layers cannot intercept back', () => {
  const stack = createBackStack()
  const calls = []
  stack.register(100, () => calls.push('older'))
  const dispose = stack.register(100, () => calls.push('newer'))
  stack.dismiss()
  assert.deepEqual(calls, ['newer'])
  dispose()
  dispose()
  stack.dismiss()
  assert.deepEqual(calls, ['newer', 'older'])
})
