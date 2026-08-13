import { describe, it } from 'node:test'
import assert from 'node:assert'
import {
  hotToolNames,
  mergePromotedToolNames,
  schemaOnlyToolDefinition,
  selectedToolNames,
} from '../tools/tool-activation.mjs'

const available = [
  'read',
  'edit',
  'web_search',
  'browser_automation',
  'memory_search',
  'memory_remember',
  'mcp_list',
  'mcp_manage',
  'skill_create',
  'spawn_agent',
  'list_agents',
  'send_message',
  'followup_task',
  'interrupt_agent',
]

describe('tool-activation', () => {
  it('schemaOnlyToolDefinition flattens promptGuidelines into description', () => {
    const tool = {
      name: 'test_tool',
      description: 'Base description.',
      promptGuidelines: ['Guideline 1', 'Guideline 2'],
    }
    const result = schemaOnlyToolDefinition(tool)
    assert.equal(result.description, 'Base description.\nGuideline 1\nGuideline 2')
    assert.equal(result.promptGuidelines.length, 0)
    assert.equal(result.promptSnippet, undefined)
  })

  it('hotToolNames returns only hot tools', () => {
    const result = hotToolNames(available)
    assert.deepEqual(result, [
      'read',
      'edit',
      'spawn_agent',
      'list_agents',
      'send_message',
      'followup_task',
      'interrupt_agent',
    ])
  })

  it('mergePromotedToolNames merges and deduplicates', () => {
    const result = mergePromotedToolNames({
      availableToolNames: available,
      promotedToolNames: ['web_search'],
      requestedToolNames: ['web_search', 'memory_search'],
    })
    assert.deepEqual(result, ['web_search', 'memory_search'])
  })

  it('selectedToolNames includes hot, promoted, and requested tools', () => {
    const result = selectedToolNames({
      availableToolNames: available,
      promotedToolNames: ['web_search'],
      requestedToolNames: ['memory_search'],
      goalToolNames: [],
      goalActive: false,
    })
    assert.deepEqual(
      result.sort(),
      [
        'edit',
        'memory_search',
        'read',
        'web_search',
        'spawn_agent',
        'list_agents',
        'send_message',
        'followup_task',
        'interrupt_agent',
      ].sort(),
    )
  })

  it('selectedToolNames includes goal tools when goal is active', () => {
    const result = selectedToolNames({
      availableToolNames: [...available, 'update_goal'],
      promotedToolNames: [],
      requestedToolNames: [],
      goalToolNames: ['update_goal'],
      goalActive: true,
    })
    assert.ok(result.includes('update_goal'))
  })
})
