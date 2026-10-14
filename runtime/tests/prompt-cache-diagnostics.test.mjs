import assert from 'node:assert/strict'
import test from 'node:test'
import {
  capturePromptCacheShape,
  comparePromptCacheShapes,
} from '../runtime/prompt-cache-diagnostics.mjs'

test('prompt cache shape is independent of tool order and object key order', () => {
  const first = capturePromptCacheShape({
    systemPrompt: 'stable prompt',
    tools: [
      { name: 'write', description: 'write', parameters: { path: 'string', content: 'string' } },
      { name: 'read', description: 'read', parameters: { path: 'string' } },
    ],
  })
  const second = capturePromptCacheShape({
    systemPrompt: 'stable prompt',
    tools: [
      { name: 'read', description: 'read', parameters: { path: 'string' } },
      { name: 'write', description: 'write', parameters: { content: 'string', path: 'string' } },
    ],
  })
  assert.equal(first.prefixHash, second.prefixHash)
  assert.equal(first.toolsHash, second.toolsHash)
})

test('prompt cache diagnostics identify system and tool changes', () => {
  const previous = capturePromptCacheShape({
    systemPrompt: 'before',
    tools: [{ name: 'read', parameters: {} }],
  })
  const current = capturePromptCacheShape({
    systemPrompt: 'after',
    tools: [{ name: 'write', parameters: {} }],
    runtime: { model: 'next-model', thinkingLevel: 'high' },
  })
  const compared = comparePromptCacheShapes(previous, current)
  assert.equal(compared.changed, true)
  assert.deepEqual(compared.changeReasons, ['system', 'tools', 'runtime'])
})
