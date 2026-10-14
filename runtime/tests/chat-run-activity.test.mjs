import assert from 'node:assert/strict'
import test from 'node:test'
import {
  activityDurationMs,
  activityRenderKey,
  activityScrollVersion,
  agentActivityState,
  deriveRunActivity,
  formatRunDuration,
  groupToolCalls,
  latestUnrecoveredToolError,
  primaryRunActivity,
  pushCurrentActivity,
  RUN_INACTIVITY_THRESHOLD_MS,
  settleToolCalls,
  planChanges,
} from '../../src/features/chat/run-activity.ts'

test('chat activity derives meaningful stages and inactivity states', () => {
  const now = Date.parse('2026-07-20T10:00:20.000Z')
  assert.equal(
    deriveRunActivity({ streaming: true, lastActivityAt: '2026-07-20T10:00:19.000Z', now }).stage,
    'thinking',
  )
  assert.equal(
    deriveRunActivity({
      streaming: true,
      tools: [{ name: 'read', status: 'running' }],
      lastActivityAt: '2026-07-20T10:00:19.000Z',
      now,
    }).stage,
    'researching',
  )
  assert.equal(
    deriveRunActivity({
      streaming: true,
      tools: [{ name: 'bash', status: 'running' }],
      lastActivityAt: '2026-07-20T10:00:19.000Z',
      now,
    }).stage,
    'validating',
  )
  assert.equal(
    deriveRunActivity({
      streaming: true,
      compaction: { active: true },
      lastActivityAt: '2026-07-20T10:00:00.000Z',
      now,
    }).stage,
    'compacting',
  )
  const waiting = deriveRunActivity({
    streaming: true,
    tools: [{ name: 'bash', status: 'running' }],
    lastActivityAt: new Date(now - RUN_INACTIVITY_THRESHOLD_MS).toISOString(),
    now,
  })
  assert.equal(waiting.stage, 'waiting_tool')
  assert.equal(waiting.inactiveMs, RUN_INACTIVITY_THRESHOLD_MS)
  assert.equal(deriveRunActivity({ streaming: false, stopped: true }).stage, 'stopped')
})

test('unknown Subagent states render neutrally instead of as failures', () => {
  assert.deepEqual(agentActivityState('failed'), { titleKey: '{name} 执行失败', tone: 'failed' })
  assert.deepEqual(agentActivityState('queued'), { titleKey: '{name} 等待调度', tone: 'waiting' })
  assert.deepEqual(agentActivityState(undefined), {
    titleKey: '{name} 状态已更新',
    tone: 'waiting',
  })
  assert.deepEqual(agentActivityState('future-status'), {
    titleKey: '{name} 状态已更新',
    tone: 'waiting',
  })
})

test('primary Agent activity never duplicates the latest tool row', () => {
  const tool = {
    type: 'tool',
    id: 'tool-1',
    name: 'read',
    status: 'running',
    updatedAt: '2026-07-20T10:00:01.000Z',
  }
  assert.deepEqual(
    primaryRunActivity({
      currentActivity: tool,
      thinkingText: 'Inspect the implementation.',
      lastActivityAt: tool.updatedAt,
    }),
    {
      type: 'model',
      stage: 'working',
      updatedAt: tool.updatedAt,
    },
  )
  const completed = primaryRunActivity({
    currentActivity: { ...tool, status: 'done' },
    thinkingText: 'Review the result.',
  })
  assert.equal(completed.type, 'model')
  assert.equal(completed.stage, 'processing_result')
  const retry = { type: 'retry', message: 'Retrying' }
  assert.equal(primaryRunActivity({ currentActivity: retry }), retry)
})

test('chat activity groups completed tools while preserving running and failed calls', () => {
  const grouped = groupToolCalls([
    { id: 'read-1', name: 'read', status: 'done' },
    { id: 'read-2', name: 'read', status: 'done', message: 'done' },
    { id: 'bash-1', name: 'bash', status: 'running' },
    { id: 'edit-1', name: 'edit', status: 'error', message: 'failed' },
  ])
  assert.equal(grouped.completed.length, 1)
  assert.equal(grouped.completed[0].count, 2)
  assert.equal(grouped.completed[0].message, 'done')
  assert.deepEqual(
    grouped.running.map((tool) => tool.id),
    ['bash-1'],
  )
  assert.deepEqual(
    grouped.errors.map((tool) => tool.id),
    ['edit-1'],
  )
})

test('recoverable tool failures stop being prominent after later progress', () => {
  const failedAt = '2026-07-20T10:00:10.000Z'
  const failure = {
    id: 'bash-1',
    name: 'bash',
    status: 'error',
    message: 'failed',
    updatedAt: failedAt,
  }
  assert.equal(
    latestUnrecoveredToolError([failure], { streaming: true, lastActivityAt: failedAt }),
    failure,
  )
  assert.equal(
    latestUnrecoveredToolError([failure], {
      streaming: true,
      lastActivityAt: '2026-07-20T10:00:11.000Z',
    }),
    null,
  )
  assert.equal(
    latestUnrecoveredToolError(
      [failure, { id: 'bash-2', name: 'bash', status: 'running', startedAt: failedAt }],
      { streaming: true, lastActivityAt: failedAt },
    ),
    null,
  )
  assert.equal(
    latestUnrecoveredToolError([failure], { streaming: false, lastActivityAt: failedAt }),
    null,
  )
})

test('only the latest unresolved failure remains prominent while streaming', () => {
  const first = {
    id: 'read-1',
    name: 'read',
    status: 'error',
    updatedAt: '2026-07-20T10:00:09.000Z',
  }
  const latest = {
    id: 'read-2',
    name: 'read',
    status: 'error',
    updatedAt: '2026-07-20T10:00:10.000Z',
  }
  assert.equal(
    latestUnrecoveredToolError([first, latest], {
      streaming: true,
      lastActivityAt: '2026-07-20T10:00:10.000Z',
    }),
    latest,
  )
})

test('chat activity formats millisecond, short, and long elapsed time', () => {
  assert.equal(formatRunDuration(248, 'zh-CN'), '248 毫秒')
  assert.equal(formatRunDuration(999, 'en-US'), '999ms')
  assert.equal(formatRunDuration(9_900, 'zh-CN'), '9 秒')
  assert.equal(formatRunDuration(65_000, 'zh-CN'), '1:05')
  assert.equal(formatRunDuration(3_665_000, 'en-US'), '1:01:05')
})

test('completed activity durations freeze while running activities keep advancing', () => {
  const startedAt = '2026-07-20T10:00:00.000Z'
  const finishedAt = '2026-07-20T10:00:05.000Z'
  const now = Date.parse('2026-07-20T10:01:00.000Z')
  assert.equal(
    activityDurationMs({ type: 'tool', status: 'done', startedAt, finishedAt }, startedAt, now),
    5_000,
  )
  assert.equal(
    activityDurationMs(
      { type: 'tool', status: 'done', startedAt, updatedAt: finishedAt },
      startedAt,
      now,
    ),
    5_000,
  )
  assert.equal(
    activityDurationMs({ type: 'tool', status: 'running', startedAt }, startedAt, now),
    60_000,
  )
  assert.equal(
    activityDurationMs(
      { type: 'agent', agent: { status: 'completed', startedAt, completedAt: finishedAt } },
      startedAt,
      now,
    ),
    5_000,
  )
})

test('activity scroll versions ignore timestamp-only progress updates', () => {
  const before = [
    { type: 'tool', id: 'tool-1', status: 'running', updatedAt: '2026-07-20T10:00:01.000Z' },
  ]
  const afterProgress = [{ ...before[0], updatedAt: '2026-07-20T10:00:02.000Z' }]
  const afterStatus = [{ ...afterProgress[0], status: 'done' }]
  assert.equal(activityScrollVersion(before), activityScrollVersion(afterProgress))
  assert.notEqual(activityScrollVersion(before), activityScrollVersion(afterStatus))
})

test('tool activity render keys stay stable when progress or status changes', () => {
  const running = {
    type: 'tool',
    id: 'tool-1',
    status: 'running',
    updatedAt: '2026-07-20T10:00:01.000Z',
  }
  const completed = { ...running, status: 'done', updatedAt: '2026-07-20T10:00:02.000Z' }
  assert.equal(activityRenderKey(running), activityRenderKey(completed))
  assert.notEqual(
    activityRenderKey({ type: 'agent', agent: { id: 'agent-1', status: 'running' } }),
    activityRenderKey({ type: 'agent', agent: { id: 'agent-1', status: 'completed' } }),
  )
})

test('current activity feed updates tools in place and evicts its oldest entries', () => {
  let feed = []
  for (let index = 1; index <= 7; index += 1) {
    feed = pushCurrentActivity(feed, {
      type: 'tool',
      id: `tool-${index}`,
      name: 'read',
      status: 'running',
      updatedAt: `2026-07-20T10:00:0${index}.000Z`,
    })
  }
  assert.equal(feed.length, 6)
  assert.equal(feed[0].id, 'tool-2')
  feed = pushCurrentActivity(feed, { ...feed.at(-1), status: 'done' })
  assert.equal(feed.length, 6)
  assert.equal(feed.at(-1).status, 'done')
  const beforeModelUpdate = feed
  feed = pushCurrentActivity(feed, {
    type: 'model',
    stage: 'responding',
    updatedAt: '2026-07-20T10:00:08.000Z',
  })
  assert.equal(feed, beforeModelUpdate)
  assert.equal(feed.filter((item) => item.type === 'model').length, 0)
})

test('Subagent status updates replace the same activity card in place', () => {
  const running = {
    type: 'agent',
    agent: { id: 'agent-1', canonicalName: '/root/review_1', status: 'running' },
  }
  const completed = {
    type: 'agent',
    agent: {
      id: 'agent-1',
      canonicalName: '/root/review_1',
      status: 'completed',
      completedAt: '2026-07-20T10:00:05.000Z',
    },
  }
  const feed = pushCurrentActivity(pushCurrentActivity([], running), completed)
  assert.equal(feed.length, 1)
  assert.equal(feed[0].agent.status, 'completed')
})

test('plan activity reports the concrete items whose status changed', () => {
  const changes = planChanges(
    {
      items: [
        { id: 'one', title: 'Inspect', status: 'in_progress' },
        { id: 'removed', title: 'Old step', status: 'pending' },
      ],
    },
    {
      items: [
        { id: 'one', title: 'Inspect', status: 'completed' },
        { id: 'two', title: 'Implement', status: 'in_progress' },
      ],
    },
  )
  assert.deepEqual(changes, [
    {
      id: 'one',
      title: 'Inspect',
      status: 'completed',
      previousStatus: 'in_progress',
      kind: 'updated',
    },
    { id: 'two', title: 'Implement', status: 'in_progress', kind: 'added' },
    { id: 'removed', title: 'Old step', status: 'pending', kind: 'removed' },
  ])
})

test('terminal run state settles any tool missing its final event', () => {
  const finishedAt = '2026-07-20T10:00:30.000Z'
  const tools = settleToolCalls(
    [
      { id: 'read-1', name: 'read', status: 'done', finishedAt: '2026-07-20T10:00:10.000Z' },
      { id: 'bash-1', name: 'bash', status: 'running' },
    ],
    { finishedAt },
  )
  assert.equal(tools[0].finishedAt, '2026-07-20T10:00:10.000Z')
  assert.deepEqual(tools[1], {
    id: 'bash-1',
    name: 'bash',
    status: 'done',
    message: '',
    updatedAt: finishedAt,
    finishedAt,
  })
})
