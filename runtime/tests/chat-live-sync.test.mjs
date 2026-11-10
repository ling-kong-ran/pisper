import assert from 'node:assert/strict'
import test from 'node:test'
import {
  IDLE_LIVE_POLL_EVERY,
  LOCAL_STREAM_STALE_MS,
  hasActiveSessionAgents,
  liveSessionPollEvery,
  shouldPollLiveSession,
  shouldReclaimLocalStream,
} from '../../src/features/chat/live-session-sync.ts'
import { reconcileMessagePage } from '../../src/features/chat/use-live-session-sync.ts'

test('a locally owned SSE stream blocks live snapshot polling', () => {
  const active = {
    recovering: true,
    approvals: [{ id: 'approval-1' }],
    agents: [{ id: 'agent-1', status: 'running' }],
  }
  assert.equal(shouldPollLiveSession(active, { localStreamOwned: true }), false)
})

test('sessions without a local SSE owner poll only when live recovery data is needed', () => {
  assert.equal(shouldPollLiveSession({ recovering: true }), true)
  assert.equal(shouldPollLiveSession({ approvals: [{ id: 'approval-1' }] }), true)
  assert.equal(shouldPollLiveSession({ agents: [{ id: 'agent-1', status: 'queued' }] }), true)
  assert.equal(shouldPollLiveSession({ agents: [{ id: 'agent-1', status: 'completed' }] }), false)
  assert.equal(shouldPollLiveSession({}), false)
  assert.equal(hasActiveSessionAgents({ agents: [{ status: 'starting' }] }), true)
})

test('a desktop tab switch keeps a local stream that is still receiving events', () => {
  const now = 1_000_000
  const fresh = { mobile: false, reason: 'visible', lastEventAt: now - 2_000, now }
  assert.equal(shouldReclaimLocalStream(fresh), false)
  // 很久没出数据的流多半已经断了：交给服务端快照。
  assert.equal(
    shouldReclaimLocalStream({ ...fresh, lastEventAt: now - LOCAL_STREAM_STALE_MS }),
    true,
  )
  assert.equal(shouldReclaimLocalStream({ ...fresh, lastEventAt: 0 }), true)
  // 手机挂起、断网恢复、从往返缓存恢复：连接不可信，一律作废。
  assert.equal(shouldReclaimLocalStream({ ...fresh, mobile: true }), true)
  assert.equal(shouldReclaimLocalStream({ ...fresh, reason: 'online' }), true)
  assert.equal(shouldReclaimLocalStream({ ...fresh, reason: 'restored' }), true)
})

test('an idle conversation that only waits on approvals or background work polls less often', () => {
  assert.equal(liveSessionPollEvery({ streaming: true }), 1)
  assert.equal(liveSessionPollEvery({ recovering: true, approvals: [{ id: 'a' }] }), 1)
  assert.equal(liveSessionPollEvery({ approvals: [{ id: 'a' }] }), IDLE_LIVE_POLL_EVERY)
  assert.equal(
    liveSessionPollEvery({ agents: [{ id: 'agent-1', status: 'running' }] }),
    IDLE_LIVE_POLL_EVERY,
  )
})

test('a polled page keeps the objects of unchanged messages and replaces changed ones', () => {
  const kept = { id: 'm1', role: 'user', text: 'hello' }
  const changed = { id: 'm2', role: 'agent', text: 'par' }
  const current = { messages: [kept, changed], messageStart: 0 }
  const page = reconcileMessagePage(current, {
    pageInfo: { start: 0 },
    messages: [
      { id: 'm1', role: 'user', text: 'hello' },
      { id: 'm2', role: 'agent', text: 'partial' },
    ],
  })
  assert.equal(page.messages[0], kept)
  assert.notEqual(page.messages[1], changed)
  assert.equal(page.messages[1].text, 'partial')
  // 再来一页同样的内容：两条都沿用上一次的对象。
  const again = reconcileMessagePage(
    { messages: page.messages, messageStart: 0 },
    {
      pageInfo: { start: 0 },
      messages: [
        { id: 'm1', role: 'user', text: 'hello' },
        { id: 'm2', role: 'agent', text: 'partial' },
      ],
    },
  )
  assert.deepEqual(
    [again.messages[0] === kept, again.messages[1] === page.messages[1]],
    [true, true],
  )
})
