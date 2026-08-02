import assert from 'node:assert/strict'
import test from 'node:test'

import {
  isPlanReadTool,
  isPlanTool,
  isPlanUpdateEvent,
  isPlanWriteTool,
  planFromPayload,
  planFromPayloadOr,
} from '../../src/lib/plan-protocol.ts'

test('plan protocol prefers canonical fields and preserves explicit clears', () => {
  const canonical = { items: [{ id: 'new' }] }
  const legacy = { items: [{ id: 'old' }] }

  assert.equal(planFromPayload({ plan: canonical, taskList: legacy }), canonical)
  assert.equal(planFromPayload({ taskList: legacy }), legacy)
  assert.equal(planFromPayload({ plan: null }), null)
  assert.equal(planFromPayload({ taskList: null }), null)
  assert.equal(planFromPayload({}), undefined)
  assert.equal(planFromPayloadOr({}, canonical), canonical)
  assert.equal(planFromPayloadOr({ plan: null }, canonical), null)
})

test('plan protocol accepts one release of event and tool aliases', () => {
  assert.equal(isPlanUpdateEvent('plan_update'), true)
  assert.equal(isPlanUpdateEvent('task_list_update'), true)
  assert.equal(isPlanReadTool('get_plan'), true)
  assert.equal(isPlanReadTool('get_task_list'), true)
  assert.equal(isPlanWriteTool('update_plan'), true)
  assert.equal(isPlanWriteTool('update_task_list'), true)
  assert.equal(isPlanTool('read'), false)
})
