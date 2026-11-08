// 会话协作提示变化通知（看板事件流的 session_workspace 帧）：看板 feed 只推进修订号，
// 被接受的帧转给会话页；重连快照通知 '*'；非法帧丢弃；会话页对空闲、已加载的会话重读 /live。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

const changes = await import('../../src/features/board/session-workspace-changes.ts')
const feed = await import('../../src/features/board/board-feed.ts')

test('session_workspace frames advance the feed revision and reach listeners', () => {
  const state = {
    mode: 'events',
    ready: true,
    revision: 5,
    awaitingSnapshot: false,
    entries: {},
    updatedAt: 0,
  }
  const result = feed.applyBoardFeedEvent(state, 'session_workspace', { id: 's1', revision: 6 })
  assert.equal(result.feed.revision, 6)
  assert.equal(result.effects.changed, true)
  assert.equal(result.effects.catalog, false)

  const seen = []
  const stop = changes.subscribeSessionWorkspaceChanges((id) => seen.push(id))
  changes.notifySessionWorkspaceFeedEvent('session_workspace', { id: 's1', revision: 6 }, true)
  changes.notifySessionWorkspaceFeedEvent('session_workspace', { id: 's2' }, false)
  changes.notifySessionWorkspaceFeedEvent('session_workspace', { id: 42 }, true)
  changes.notifySessionWorkspaceFeedEvent('heartbeat', { revision: 7 }, true)
  changes.notifySessionWorkspaceFeedEvent('snapshot', { revision: 8, items: [] }, true)
  stop()
  changes.notifySessionWorkspaceFeedEvent('session_workspace', { id: 's3' }, true)
  assert.deepEqual(seen, ['s1', '*'])
})

test('the session page rereads idle loaded sessions when their workspace changes', async () => {
  const page = await readFile('src/features/chat/ChatPage.tsx', 'utf8')
  assert.match(page, /subscribeSessionWorkspaceChanges\(/)
  assert.match(
    page,
    /if \(!state\?\.loaded \|\| state\.streaming \|\| localStreamSessionsRef\.current\.has\(id\)\) continue\s+void syncLiveSession\(id\)/,
  )
  const store = await readFile('src/features/board/board-store.ts', 'utf8')
  assert.match(store, /notifySessionWorkspaceFeedEvent\(event, data, result\.effects\.changed\)/)
})
