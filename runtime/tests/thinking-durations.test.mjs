import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  MAX_THINKING_DURATIONS,
  attachThinkingDurations,
  recordThinkingDuration,
} from '../services/thinking-durations.mjs'

test('thinking durations are recorded per final reply and capped', () => {
  const meta = { s: { name: 'kept' } }
  recordThinkingDuration(meta, 's', 1700000000000, 6234.4)
  assert.deepEqual(meta.s, { name: 'kept', thinkingDurations: { 1700000000000: 6234 } })
  for (const [timestamp, ms] of [
    [null, 100],
    ['', 100],
    [1, 0],
    [2, -5],
    [3, Number.NaN],
  ])
    recordThinkingDuration(meta, 's', timestamp, ms)
  assert.equal(Object.keys(meta.s.thinkingDurations).length, 1)
  recordThinkingDuration(meta, '', 4, 100)
  assert.equal(meta[''], undefined)
  for (let index = 0; index < MAX_THINKING_DURATIONS + 5; index += 1)
    recordThinkingDuration(meta, 's', `t-${index}`, index + 1)
  const keys = Object.keys(meta.s.thinkingDurations)
  assert.equal(keys.length, MAX_THINKING_DURATIONS)
  assert.equal(keys.at(-1), `t-${MAX_THINKING_DURATIONS + 4}`)
  assert.equal(meta.s.thinkingDurations[1700000000000], undefined)
})

test('durations are attached to matching agent replies only', () => {
  const messages = [
    { id: 'u', role: 'user', timestamp: 1 },
    { id: 'a', role: 'agent', timestamp: 2 },
    { id: 'b', role: 'agent', timestamp: null },
  ]
  assert.equal(attachThinkingDurations(messages, undefined), messages)
  assert.equal(attachThinkingDurations(messages, { 9: 100 }), messages)
  const attached = attachThinkingDurations(messages, { 1: 50, 2: 6200 })
  assert.notEqual(attached, messages)
  assert.equal(attached[0].thinkingMs, undefined)
  assert.equal(attached[1].thinkingMs, 6200)
  assert.equal(messages[1].thinkingMs, undefined)
})

test('the run loop stops the thinking clock on text, tool calls and message end', async () => {
  // 源码守卫：运行循环里思考计时的起止点（真实运行需要模型，这里锁住接线）。
  const source = await readFile(new URL('../runtime/agent-runtime.mjs', import.meta.url), 'utf8')
  assert.match(
    source,
    /update\.type === 'thinking_start' \|\| update\.type === 'thinking_delta'\)\s*\n\s*thinkingStartedAt \|\|= Date\.now\(\)/,
  )
  assert.match(
    source,
    /update\.type === 'text_start' \|\| update\.type === 'toolcall_start'\) endThinking\(\)/,
  )
  assert.match(
    source,
    /recordThinkingDuration\(this\.sessionMeta, session\.sessionId, last\.timestamp, live\.thinkingMs\)/,
  )
  assert.match(
    source,
    /thinkingMs: live\.thinkingMs > 0 \? Math\.round\(live\.thinkingMs\) : undefined/,
  )
})
