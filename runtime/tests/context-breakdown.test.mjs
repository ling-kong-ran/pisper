import assert from 'node:assert/strict'
import test from 'node:test'
import { buildContextBreakdown, estimateTextTokens } from '../runtime/context-breakdown.mjs'
import {
  CONTEXT_CATEGORY_IDS,
  contextShareLabel,
  parseContextBreakdown,
} from '../../src/features/chat/context-breakdown.ts'

const category = (result, id) => result.categories.find((item) => item.id === id)
const sum = (result) => result.categories.reduce((total, item) => total + item.tokens, 0)

const SYSTEM = {
  role: 'system',
  content: '',
  sections: {
    preamble: 'x'.repeat(400),
    rules: 'x'.repeat(400),
    cwd: '<cwd>/repo</cwd>',
    project_context: 'x'.repeat(800),
    skills: 'x'.repeat(200),
  },
  toolsAdded: [
    { name: 'bash', description: 'Run a command', parameters: { type: 'object' } },
    { name: 'read', description: 'Read a file', parameters: { type: 'object' } },
    {
      name: 'gh_issue',
      description: 'Open an issue\nRemote MCP server: github. Remote tools can act.',
      parameters: { type: 'object', properties: { title: { type: 'string' } } },
    },
    {
      name: 'gh_pr',
      description: 'Open a PR\nRemote MCP server: github. Remote tools can act.',
      parameters: { type: 'object' },
    },
    {
      name: 'linear_task',
      description: 'Open a task\nRemote MCP server: linear. Remote tools can act.',
      parameters: { type: 'object' },
    },
  ],
}

test('Chinese text counts more per character than English', () => {
  assert.equal(estimateTextTokens('abcd'), 1)
  assert.equal(estimateTextTokens('你好'), 3)
  assert.equal(estimateTextTokens(''), 0)
  assert.equal(estimateTextTokens(null), 0)
})

test('each part of the context lands in its own category', () => {
  const result = buildContextBreakdown({
    messages: [
      SYSTEM,
      { role: 'user', content: [{ type: 'text', text: 'y'.repeat(40) }] },
      {
        role: 'assistant',
        content: [
          { type: 'text', text: 'z'.repeat(40) },
          { type: 'toolCall', name: 'read', arguments: { path: '/a' } },
        ],
      },
      { role: 'toolResult', content: [{ type: 'text', text: 'r'.repeat(400) }] },
      { role: 'bashExecution', command: 'ls', output: 'o'.repeat(40) },
      { role: 'compactionSummary', summary: 's'.repeat(40) },
    ],
    usage: { contextWindow: 1000 },
  })
  assert.ok(category(result, 'project').tokens >= 200)
  assert.ok(category(result, 'skills').tokens >= 50)
  assert.ok(category(result, 'system').tokens > 200)
  assert.equal(category(result, 'tools').count, 2)
  const mcp = category(result, 'mcp')
  assert.equal(mcp.count, 3)
  assert.deepEqual(
    mcp.servers.map((server) => [server.name, server.tools]),
    [
      ['github', 2],
      ['linear', 1],
    ],
  )
  assert.equal(
    mcp.tokens,
    mcp.servers.reduce((total, server) => total + server.tokens, 0),
  )
  assert.equal(category(result, 'messages').count, 2)
  assert.equal(category(result, 'toolResults').count, 2)
  assert.ok(category(result, 'toolResults').tokens > 100)
  assert.equal(result.totalKnown, false)
  assert.equal(result.usedTokens, sum(result))
  assert.equal(result.freeTokens, 1000 - result.usedTokens)
})

test('parts are calibrated to the total the model reported', () => {
  const result = buildContextBreakdown({
    messages: [SYSTEM, { role: 'user', content: 'hello' }],
    usage: { tokens: 12_345, contextWindow: 200_000 },
  })
  assert.equal(result.totalKnown, true)
  assert.equal(result.usedTokens, 12_345)
  assert.equal(sum(result), 12_345, 'rounding never makes the parts disagree with the total')
  assert.equal(result.freeTokens, 200_000 - 12_345)
  assert.ok(category(result, 'mcp').servers[0].tokens > 0)
})

test('system messages apply as patches: later sections and tool changes win', () => {
  const result = buildContextBreakdown({
    messages: [
      SYSTEM,
      {
        role: 'system',
        content: '',
        sections: { skills: null, project_context: 'x'.repeat(40) },
        toolsRemoved: ['read', 'gh_pr'],
        toolsAdded: [{ name: 'write', description: 'Write', parameters: {} }],
      },
    ],
    usage: { contextWindow: 1000 },
  })
  assert.equal(category(result, 'skills').tokens, 0)
  assert.equal(category(result, 'project').tokens, 10)
  assert.equal(category(result, 'tools').count, 2)
  assert.equal(category(result, 'mcp').count, 2)
})

test('a session with no system message falls back to the live prompt and tools', () => {
  const result = buildContextBreakdown({
    messages: [],
    systemPrompt: 'x'.repeat(400),
    activeTools: [{ name: 'bash', description: 'Run', parameters: {} }, null],
    usage: { contextWindow: 1000 },
  })
  assert.equal(category(result, 'system').tokens, 100)
  assert.equal(category(result, 'tools').count, 1)
  assert.equal(result.prefixKnown, true)
})

test('a cold session that never ran says the fixed parts are unknown, not zero', () => {
  const result = buildContextBreakdown({
    messages: [{ role: 'user', content: 'hi' }],
    usage: { contextWindow: 1000 },
  })
  assert.equal(result.prefixKnown, false)
  assert.equal(category(result, 'system').tokens, 0)
})

test('odd input does not throw', () => {
  for (const messages of [
    undefined,
    null,
    'x',
    [null, {}, { role: 'assistant' }, { role: 'system' }],
  ]) {
    const result = buildContextBreakdown({ messages, usage: null })
    assert.equal(result.contextWindow, 0)
    assert.equal(result.freeTokens, 0)
  }
  assert.equal(buildContextBreakdown().totalKnown, false)
  // 用量是 null（刚压缩过）：总量也是估算，不当成 0。
  const unknown = buildContextBreakdown({
    messages: [SYSTEM],
    usage: { tokens: null, contextWindow: 1000 },
  })
  assert.equal(unknown.totalKnown, false)
  assert.ok(unknown.usedTokens > 0)
})

test('connected MCP tools that are not in use count as waiting, not as context', () => {
  const result = buildContextBreakdown({
    messages: [SYSTEM],
    usage: { contextWindow: 1000 },
    mcpAvailable: [
      { name: 'github', toolNames: ['gh_issue', 'gh_pr', 'gh_a', 'gh_b', 'gh_c'] },
      { name: 'slack', toolNames: ['s1', 's2', 's3'] },
    ],
  })
  const mcp = category(result, 'mcp')
  assert.equal(mcp.count, 3, 'github 2 + linear 1 are loaded')
  assert.equal(mcp.deferred, 6, 'github 3 + slack 3 are still waiting')
  assert.equal(mcp.calls, 0)
  assert.deepEqual(
    mcp.servers.map((server) => server.name),
    ['github', 'linear'],
  )
  const none = buildContextBreakdown({ usage: { contextWindow: 1000 } })
  assert.equal(category(none, 'mcp').deferred, 0)
})

test('MCP calls count as MCP, whether direct or through call_tool', () => {
  const big = 'r'.repeat(4000)
  const result = buildContextBreakdown({
    messages: [
      { role: 'system', content: '', sections: { preamble: 'p' }, toolsAdded: [] },
      {
        role: 'assistant',
        content: [
          {
            type: 'toolCall',
            id: 'a',
            name: 'call_tool',
            arguments: { name: 'gh_issue', arguments: {} },
          },
          { type: 'toolCall', id: 'b', name: 'gh_pr', arguments: {} },
          {
            type: 'toolCall',
            id: 'c',
            name: 'call_tool',
            arguments: { name: 'not_mcp', arguments: {} },
          },
          { type: 'toolCall', id: 'd', name: 'read', arguments: { path: '/a' } },
        ],
      },
      { role: 'toolResult', toolCallId: 'a', content: [{ type: 'text', text: big }] },
      { role: 'toolResult', toolCallId: 'b', content: [{ type: 'text', text: big }] },
      { role: 'toolResult', toolCallId: 'c', content: [{ type: 'text', text: 'x' }] },
      { role: 'toolResult', toolCallId: 'd', content: [{ type: 'text', text: 'x' }] },
    ],
    usage: { contextWindow: 100000 },
    mcpAvailable: [{ name: 'github', toolNames: ['gh_issue', 'gh_pr', 'gh_c'] }],
  })
  const mcp = category(result, 'mcp')
  assert.equal(mcp.calls, 2)
  assert.ok(mcp.tokens >= 2000, `${mcp.tokens}`)
  assert.equal(mcp.count, 0, 'calling through the gateway does not load the definition')
  assert.equal(mcp.deferred, 3)
  assert.deepEqual(mcp.servers[0].name, 'github')
  assert.equal(mcp.servers[0].calls, 2)
  assert.equal(category(result, 'toolResults').count, 2, 'only the two non-MCP results')
  assert.ok(category(result, 'toolResults').tokens < 100)
})

test('the compaction line only shows when auto compaction is on', () => {
  const on = buildContextBreakdown({
    usage: { contextWindow: 1000, autoCompactEnabled: true, compactAtPercent: 80 },
  })
  assert.equal(on.compactAtPercent, 80)
  const off = buildContextBreakdown({
    usage: { contextWindow: 1000, autoCompactEnabled: false, compactAtPercent: 80 },
  })
  assert.equal(off.compactAtPercent, null)
})

test('the client keeps category order fixed and tolerates a broken response', () => {
  const real = parseContextBreakdown(
    buildContextBreakdown({
      messages: [SYSTEM],
      usage: { tokens: 500, contextWindow: 1000, autoCompactEnabled: true, compactAtPercent: 80 },
    }),
  )
  assert.deepEqual(
    real.categories.map((item) => item.id),
    [...CONTEXT_CATEGORY_IDS],
  )
  assert.equal(real.usedTokens, 500)
  assert.equal(real.compactAtPercent, 80)
  for (const broken of [null, 'x', { categories: 'x', compactAtPercent: 900, usedTokens: -1 }]) {
    const parsed = parseContextBreakdown(broken)
    assert.equal(parsed.usedTokens, 0)
    assert.equal(parsed.compactAtPercent, null)
    assert.equal(parsed.categories.length, CONTEXT_CATEGORY_IDS.length)
  }
})

test('shares never read as nothing when something is there', () => {
  assert.equal(contextShareLabel(1, 1000), '<1%')
  assert.equal(contextShareLabel(250, 1000), '25%')
  assert.equal(contextShareLabel(0, 1000), '0%')
  assert.equal(contextShareLabel(5, 0), '0%')
})
