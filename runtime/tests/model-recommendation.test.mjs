import assert from 'node:assert/strict'
import test from 'node:test'
import { recommendedChatModel } from '../../src/features/config/model-recommendation.ts'

function provider(overrides = {}) {
  return {
    id: 'openai',
    name: 'OpenAI',
    type: 'chat',
    api: 'openai-responses',
    models: [
      { id: 'gpt-image', name: 'Image', kind: 'image' },
      { id: 'gpt-first', name: 'First', kind: 'chat' },
      { id: 'gpt-default', name: 'Default', kind: 'chat' },
    ],
    defaultModel: 'gpt-default',
    enabled: true,
    configured: true,
    ...overrides,
  }
}

test('recommended chat model prefers the provider default model', () => {
  assert.equal(recommendedChatModel(provider())?.id, 'gpt-default')
  // 无默认模型时回退到目录排序后的第一个 chat 模型（后端已按旗舰能力排序）
  assert.equal(recommendedChatModel(provider({ defaultModel: '' }))?.id, 'gpt-first')
  // 默认模型不在目录中时同样回退
  assert.equal(recommendedChatModel(provider({ defaultModel: 'gpt-gone' }))?.id, 'gpt-first')
  assert.equal(recommendedChatModel(null), null)
  assert.equal(recommendedChatModel(provider({ models: [] })), null)
  // 纯视觉 Provider 没有可推荐的对话模型
  assert.equal(
    recommendedChatModel(provider({ models: [{ id: 'gpt-image', name: 'Image', kind: 'image' }] })),
    null,
  )
})
