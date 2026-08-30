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
  'plugin_create',
  'spawn_agent',
  'list_agents',
  'send_message',
  'followup_task',
  'interrupt_agent',
]

describe('tool-activation', () => {
  it('schemaOnlyToolDefinition keeps prompt metadata only for hot tools', () => {
    const dynamic = schemaOnlyToolDefinition({
      name: 'test_tool',
      description: 'Base description.',
      promptSnippet: 'Dynamic snippet',
      promptGuidelines: ['Guideline 1', 'Guideline 2'],
    })
    assert.equal(dynamic.description, 'Base description.\nGuideline 1\nGuideline 2')
    assert.deepEqual(dynamic.promptGuidelines, [])
    assert.equal(dynamic.promptSnippet, undefined)

    const hot = schemaOnlyToolDefinition({
      name: 'spawn_agent',
      description: 'Spawn description.',
      promptSnippet: 'Delegate independent work',
      promptGuidelines: ['Use spawn_agent proactively.'],
    })
    assert.equal(hot.description, 'Spawn description.')
    assert.equal(hot.promptSnippet, 'Delegate independent work')
    assert.deepEqual(hot.promptGuidelines, ['Use spawn_agent proactively.'])
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
