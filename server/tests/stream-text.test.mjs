import assert from 'node:assert/strict'
import test from 'node:test'
import { splitAssistantStreamText } from '../../src/features/chat/stream-text.ts'

test('without tools the full stream stays in one body block', () => {
  assert.deepEqual(
    splitAssistantStreamText('hello world', 'hello', { streaming: true, hasTools: false }),
    { lead: '', body: 'hello world', mode: 'single' },
  )
})

test('with tools the preamble stays above and only new text streams below', () => {
  assert.deepEqual(
    splitAssistantStreamText('hello world and more', 'hello world', { streaming: true, hasTools: true }),
    { lead: 'hello world', body: 'and more', mode: 'split' },
  )
})

test('exact restated preamble after tools is stripped from the body', () => {
  assert.deepEqual(
    splitAssistantStreamText('hello world\nhello world\nfinal', 'hello world', { streaming: true, hasTools: true }),
    { lead: 'hello world', body: 'final', mode: 'split' },
  )
})

test('restated middle sentence is stripped so body only keeps the true continuation', () => {
  const lead = '查看消息区里工具活动与文本的渲染顺序，定位重复文本来源。问题在于工具面板在上、全文在下，且工具前后文本叠在同一段里。改为： 工具前'
  const full = `${lead}\n问题在于工具面板在上、全文在下，且工具前后文本叠在同一段里。改为： 工具前文本 / 工具 / 工具后文本分段显示，并去掉重复前缀。`
  const result = splitAssistantStreamText(full, lead, { streaming: true, hasTools: true })
  assert.equal(result.mode, 'split')
  assert.equal(result.lead, lead)
  assert.equal(result.body, '文本 / 工具 / 工具后文本分段显示，并去掉重复前缀。')
})

test('rewritten post-tool answer collapses to a single body under tools', () => {
  const lead = '查看消息区里工具活动与文本的渲染顺序，定位重复文本来源。'
  const full = '问题在于工具面板在上、全文在下，且工具前后文本叠在同一段里。改为： 工具前文本 / 工具 / 工具后文本分段显示，并去掉重复前缀。'
  const result = splitAssistantStreamText(full, lead, { streaming: true, hasTools: true })
  assert.equal(result.mode, 'single')
  assert.equal(result.lead, '')
  assert.equal(result.body, full)
})

test('finished messages render as a single full body', () => {
  assert.deepEqual(
    splitAssistantStreamText('hello world\nfinal', 'hello world', { streaming: false, hasTools: true }),
    { lead: '', body: 'hello world\nfinal', mode: 'single' },
  )
})
