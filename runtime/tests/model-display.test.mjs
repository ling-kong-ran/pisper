// 模型显示名：带路径前缀的模型 ID 在窄处只显示能区分的最后几段。
import assert from 'node:assert/strict'
import test from 'node:test'

const { compactModelName } = await import('../../src/lib/model-display.ts')

test('model names without a path prefix are shown as they are', () => {
  assert.equal(compactModelName('GPT-5.6 Luna'), 'GPT-5.6 Luna')
  assert.equal(compactModelName('qwen3.8-27b'), 'qwen3.8-27b')
})

test('path prefixes are dropped so the model itself stays visible', () => {
  assert.equal(compactModelName('accounts/fireworks/models/qwen3-235b-a22b'), 'qwen3-235b-a22b')
  assert.equal(compactModelName('Qwen/Qwen3-Coder-480B'), 'Qwen3-Coder-480B')
  assert.equal(compactModelName('openrouter/anthropic/claude-opus-5/'), 'claude-opus-5')
  assert.equal(compactModelName('models/gemini-3.5-flash'), 'gemini-3.5-flash')
})

test('a clashing last segment keeps one more segment to tell siblings apart', () => {
  const siblings = ['fireworks/models/qwen3-8b', 'together/qwen3-8b', 'deepseek/deepseek-v4']
  assert.equal(compactModelName('fireworks/models/qwen3-8b', siblings), 'fireworks/qwen3-8b')
  assert.equal(compactModelName('together/qwen3-8b', siblings), 'together/qwen3-8b')
  assert.equal(compactModelName('deepseek/deepseek-v4', siblings), 'deepseek-v4')
  assert.equal(
    compactModelName('accounts/fireworks/models/qwen3-8b', [
      'accounts/fireworks/models/qwen3-8b',
      'together/qwen3-8b',
    ]),
    'fireworks/qwen3-8b',
  )
})
