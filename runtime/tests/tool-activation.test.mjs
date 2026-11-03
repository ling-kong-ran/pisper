import { describe, it } from 'node:test'
import assert from 'node:assert'
import {
  hotToolNames,
  mergePromotedToolNames,
  schemaOnlyToolDefinition,
  selectedToolNames,
} from '../tools/tool-activation.mjs'
import { ToolActivation, runToolBlocker } from '../runtime/tool-activation.mjs'

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

describe('run tool scope', () => {
  it('blocks isolated, unattended and out-of-scope tools but keeps the gateway infrastructure', () => {
    const open = runToolBlocker({})
    assert.equal(open('bash'), false)

    const isolated = runToolBlocker({ blockedToolNames: ['memory_search'] })
    assert.equal(isolated('memory_search'), true)
    assert.equal(isolated('bash'), false)

    const unattended = runToolBlocker({ unattendedRun: true })
    assert.equal(unattended('ask_user'), true)
    assert.equal(unattended('schedule_manage'), true)

    const scoped = runToolBlocker({
      allowedToolNames: ['read', 'grep'],
      blockedToolNames: ['memory_search'],
    })
    assert.equal(scoped('read'), false)
    assert.equal(scoped('bash'), true)
    assert.equal(scoped('memory_search'), true)
    // 发现与网关本身、计划与提问始终可用；网关里真正调用的工具仍逐个检查。
    for (const name of ['discover_tools', 'call_tool', 'update_plan', 'ask_user'])
      assert.equal(scoped(name), false, name)
    // 范围内的工具仍受隔离屏蔽约束。
    assert.equal(
      runToolBlocker({ allowedToolNames: ['memory_search'], blockedToolNames: ['memory_search'] })(
        'memory_search',
      ),
      true,
    )
    // 空列表表示只留基础设施。
    assert.equal(runToolBlocker({ allowedToolNames: [] })('read'), true)
  })

  it('scoped runs hide out-of-scope tools from activation and discovery', () => {
    const activation = new ToolActivation({
      getSessionMeta: () => ({}),
      saveSessionMeta: async () => {},
      getGoal: () => null,
      promoteTools: async () => ({ routedToolNames: [] }),
    })
    let active = []
    const value = {
      baseToolNames: ['read', 'bash', 'grep'],
      allowedToolNames: ['read'],
      session: {
        getAllTools: () => [{ name: 'browser_automation' }],
        setActiveToolsByName: (names) => {
          active = names
        },
      },
    }
    const optional = activation.optionalToolNames(value)
    assert.ok(optional.includes('read'))
    for (const name of ['bash', 'grep', 'browser_automation', 'spawn_agent'])
      assert.ok(!optional.includes(name), name)
    activation.syncGoalTools(value, null)
    assert.ok(active.includes('read'))
    assert.ok(!active.includes('bash'))
    assert.ok(active.includes('call_tool'))
  })
})
