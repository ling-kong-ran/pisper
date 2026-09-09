import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { DEFAULT_SESSION_STATE } from '../../src/lib/session-state.ts'
import { reconcileMessagePage } from '../../src/features/chat/use-live-session-sync.ts'
import { reconcileTerminalStreamState } from '../../src/features/chat/stream-event-dispatch.ts'

function sessionState(update = {}) {
  return {
    ...DEFAULT_SESSION_STATE,
    messages: [],
    tools: [],
    approvals: [],
    queuedInputs: [],
    activityFeed: [],
    agents: [],
    ...update,
  }
}

// 回归基线：SSE 的平滑逐帧打字 + 自然滚动是核心体验，以下测试锁定整条链路，
// 任何改动让正文瞬切全文、提前结算或整页重渲染都必须在这里失败。

test('done keeps the partially displayed text so the typewriter finishes naturally', () => {
  const current = sessionState({
    streaming: true,
    messages: [{ id: 'agent-1', role: 'agent', text: '已显示的一半', streaming: true }],
  })
  const next = reconcileTerminalStreamState(current, {
    agentId: 'agent-1',
    responseText: '已显示的一半，完整的回复',
    data: { text: '已显示的一半，完整的回复', tools: [], activityFeed: [], agents: [] },
    finishedAt: '2026-09-10T00:00:00.000Z',
    preserveDisplayedText: true,
  })
  // 终态帧不得把正文瞬切为全文：保留已显示部分，由打字机逐帧排空。
  assert.equal(next.messages[0].text, '已显示的一半')
  assert.equal(next.messages[0].streaming, true)
  assert.equal(next.streaming, false)
})

test('message page reconciliation reuses identical message objects to avoid transcript flicker', () => {
  const current = sessionState({
    messages: [
      { id: 'm1', role: 'user', text: 'hello' },
      { id: 'm2', role: 'agent', text: 'world' },
    ],
    messageStart: 0,
  })
  const page = reconcileMessagePage(current, {
    messages: [
      { id: 'm1', role: 'user', text: 'hello' },
      { id: 'm2', role: 'agent', text: 'world' },
      { id: 'm3', role: 'user', text: 'next' },
    ],
    pageInfo: { start: 0 },
  })
  // 内容一致的消息保持对象引用：虚拟行的 memo 命中，结束后不会整页重渲染。
  assert.equal(page.messages[0], current.messages[0])
  assert.equal(page.messages[1], current.messages[1])
  assert.equal(page.messages[2].text, 'next')
})

test('changed messages are replaced while unchanged rows keep their identity', () => {
  const current = sessionState({
    messages: [
      { id: 'm1', role: 'user', text: 'hello' },
      { id: 'm2', role: 'agent', text: 'partial', streaming: true },
    ],
    messageStart: 0,
  })
  const page = reconcileMessagePage(current, {
    messages: [
      { id: 'm1', role: 'user', text: 'hello' },
      { id: 'm2', role: 'agent', text: 'complete' },
    ],
    pageInfo: { start: 0 },
  })
  assert.equal(page.messages[0], current.messages[0])
  assert.notEqual(page.messages[1], current.messages[1])
  assert.equal(page.messages[1].text, 'complete')
})

test('SSE text deltas flow only through the typewriter, never direct state writes', async () => {
  const [dispatch, prompt] = await Promise.all([
    readFile('src/features/chat/stream-event-dispatch.ts', 'utf8'),
    readFile('src/features/chat/use-prompt-commands.ts', 'utf8'),
  ])
  // text_patch / text_delta 分支只更新打字机目标，不直接写会话正文。
  const patchBranch = dispatch.slice(
    dispatch.indexOf("event === 'text_patch'"),
    dispatch.indexOf("event === 'text_end'"),
  )
  assert.match(patchBranch, /typewriter\.setTarget/)
  assert.doesNotMatch(patchBranch, /updateSessionState/)
  // done 必须保留屏幕上已显示的正文，交给打字机排空。
  assert.match(dispatch, /preserveDisplayedText: event === 'done'/)
  // 终态先等打字机排空，再整体替换为持久化消息。
  const drainIndex = prompt.indexOf('await typewriter.drain()')
  const reloadIndex = prompt.indexOf('loadSessionMessages(sessionId, { force: true })')
  assert.ok(drainIndex >= 0, 'typewriter drain must run before reloading history')
  assert.ok(reloadIndex > drainIndex, 'durable reload must run after the drain completes')
})

test('transcript keeps natural scroll following and live row remeasure wiring', async () => {
  const [transcript, virtualList, liveSync] = await Promise.all([
    readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
    readFile('src/features/chat/VirtualMessageTranscript.tsx', 'utf8'),
    readFile('src/features/chat/use-live-session-sync.ts', 'utf8'),
  ])
  assert.match(transcript, /useAutoScroll\(transcriptVersion/)
  assert.match(transcript, /onContentSizeChange=\{maintainBottom\}/)
  assert.match(virtualList, /measureElement: measuredElementHeight/)
  assert.match(virtualList, /useAnimationFrameWithResizeObserver: true/)
  // 历史合并必须复用未变化消息的对象引用。
  assert.match(liveSync, /reuseStableMessages/)
})
