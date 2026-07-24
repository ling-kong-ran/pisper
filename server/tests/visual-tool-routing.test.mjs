import assert from 'node:assert/strict'
import test from 'node:test'
import { forceNextToolCall, forceToolChoice, isVisualContinuationRequest, isVisualGenerationRequest } from '../services/visual-tool-routing.mjs'

test('visual generation requests are detected without matching image analysis', () => {
  assert.equal(isVisualGenerationRequest('帮我生成一张图片：猫在草地上晒太阳'), true)
  assert.equal(isVisualGenerationRequest('Create a short video of a flying car'), true)
  assert.equal(isVisualGenerationRequest('分析一下这张图片里的代码'), false)
})

test('visual generation retry phrases are detected without matching unrelated regeneration', () => {
  assert.equal(isVisualContinuationRequest('再生成一下，刚刚配错 provider 了'), true)
  assert.equal(isVisualContinuationRequest('重新来一张'), true)
  assert.equal(isVisualContinuationRequest('Generate it again'), true)
  assert.equal(isVisualContinuationRequest('重新生成代码'), false)
})

test('OpenAI Responses payload forces the visual tool', () => {
  const payload = forceToolChoice({ input: [], tools: [{ type: 'function', name: 'generate_visual' }] }, 'generate_visual')
  assert.deepEqual(payload.tool_choice, { type: 'function', name: 'generate_visual' })
})

test('tool choice is forced only on the first provider request', async () => {
  const agent = { onPayload: undefined }
  const restore = forceNextToolCall(agent, 'generate_visual')
  const first = await agent.onPayload({ input: [], tools: [{ type: 'function', name: 'generate_visual' }] })
  const second = await agent.onPayload({ input: [], tools: [{ type: 'function', name: 'generate_visual' }] })
  assert.equal(first.tool_choice.name, 'generate_visual')
  assert.equal(second, undefined)
  restore()
  assert.equal(agent.onPayload, undefined)
})
