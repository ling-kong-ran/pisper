import assert from 'node:assert/strict'
import test from 'node:test'
import { explicitlyRequestedToolNames, hotToolNames } from '../tools/tool-activation.mjs'

const available = [
  'read', 'grep', 'find', 'ls', 'edit', 'write', 'bash',
  'web_search', 'browser_automation', 'generate_visual',
  'memory_search', 'memory_remember', 'mcp_list', 'mcp_manage',
  'get_task_list', 'update_task_list', 'discover_tools',
]

test('generate_visual is a static hot tool and does not use phrase routing', () => {
  assert.ok(hotToolNames(available).includes('generate_visual'))
  assert.deepEqual(explicitlyRequestedToolNames('先给我来个设计图，我看看样式是什么样的', {
    availableToolNames: available,
  }), [])
  assert.deepEqual(explicitlyRequestedToolNames('Design a logo for the app', {
    availableToolNames: available,
  }), [])
  assert.deepEqual(explicitlyRequestedToolNames('不要生成图片，只分析需求', {
    availableToolNames: available,
  }), [])
  assert.deepEqual(explicitlyRequestedToolNames('generate_visual', {
    availableToolNames: available,
  }), [])
})
