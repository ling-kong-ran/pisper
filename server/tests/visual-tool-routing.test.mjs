import assert from 'node:assert/strict'
import test from 'node:test'
import {
  enforceRequiredVisualToolCall,
  enforceVisualClaimEvidence,
  forceNextToolCall,
  forceToolChoice,
  isVisualContinuationRequest,
  isVisualGenerationRequest,
  isVisualSuccessClaim,
  visualClaimValidationError,
} from '../services/visual-tool-routing.mjs'

test('visual generation requests are detected without matching image analysis', () => {
  assert.equal(isVisualGenerationRequest('帮我生成一张图片：猫在草地上晒太阳'), true)
  assert.equal(isVisualGenerationRequest('先给我来个设计图，我看看样式是什么样的'), true)
  assert.equal(isVisualGenerationRequest('帮我出一张产品效果图'), true)
  assert.equal(isVisualGenerationRequest('Design a logo for the app'), true)
  assert.equal(isVisualGenerationRequest('Create a short video of a flying car'), true)
  assert.equal(isVisualGenerationRequest('请直接调用 generate_visual'), true)
  assert.equal(isVisualGenerationRequest('分析一下这张设计图里的布局'), false)
  assert.equal(isVisualGenerationRequest('不要生成图片，只分析需求'), false)
})

test('visual generation retry phrases are detected without matching unrelated regeneration', () => {
  assert.equal(isVisualContinuationRequest('再生成一下，刚刚配错 provider 了'), true)
  assert.equal(isVisualContinuationRequest('重新来一张'), true)
  assert.equal(isVisualContinuationRequest('Generate it again'), true)
  assert.equal(isVisualContinuationRequest('重新生成代码'), false)
})

test('visual success claims require a successful visual tool result in the same run', () => {
  assert.equal(isVisualSuccessClaim('已重新生成设计图。'), true)
  assert.equal(isVisualSuccessClaim('并没有重新生成设计图。'), false)
  assert.match(visualClaimValidationError({ assistantText: '已重新生成设计图。' }), /没有成功执行 generate_visual/)
  assert.equal(visualClaimValidationError({
    assistantText: '已重新生成设计图。',
    tools: [{ name: 'generate_visual', status: 'done' }],
  }), '')
  assert.equal(visualClaimValidationError({
    assistantText: '我现在重新生成。',
    messageCallsVisualTool: true,
  }), '')

  const skippedTool = {
    role: 'assistant',
    content: [{ type: 'text', text: '' }],
    stopReason: 'stop',
  }
  const skippedError = enforceRequiredVisualToolCall(skippedTool, [], true)
  assert.match(skippedError, /未执行 generate_visual/)
  assert.equal(skippedTool.stopReason, 'error')

  const requestedTool = {
    role: 'assistant',
    content: [
      { type: 'text', text: '我现在生成。' },
      { type: 'toolCall', name: 'generate_visual', arguments: {} },
    ],
    stopReason: 'toolUse',
  }
  assert.equal(enforceRequiredVisualToolCall(requestedTool, [], true), '')

  const hallucinated = {
    role: 'assistant',
    content: [{ type: 'text', text: '已重新生成设计图。' }],
    stopReason: 'stop',
  }
  const error = enforceVisualClaimEvidence(hallucinated, [])
  assert.match(error, /没有生成或重新生成任何视觉文件/)
  assert.equal(hallucinated.stopReason, 'error')
  assert.equal(hallucinated.errorMessage, error)
  assert.equal(hallucinated.content[0].text, error)

  const realResult = {
    role: 'assistant',
    content: [{ type: 'text', text: '已重新生成设计图。' }],
    stopReason: 'stop',
  }
  assert.equal(enforceVisualClaimEvidence(realResult, [{ name: 'generate_visual', status: 'done' }]), '')
  assert.equal(realResult.stopReason, 'stop')
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
