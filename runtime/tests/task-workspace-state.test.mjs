import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createContextPages,
  updateContextPages as update,
  MAX_CONTEXT_PAGES,
} from '../../src/features/chat/session-context-pages.ts'
import { useContextPagesStore } from '../../src/features/chat/session-context-pages-store.ts'
import {
  anchorScrollDelta,
  captureReadingPosition,
  LATEST_SLACK_PX,
} from '../../src/features/chat/transcript-reading-position.ts'
import {
  clearComposerDraft,
  readComposerDraft,
  updateComposerDraft,
} from '../../src/features/chat/composer-drafts.ts'

test('first launcher opens exactly one blank tab without manufacturing a files tab', () => {
  useContextPagesStore.setState({ sessions: {} })
  useContextPagesStore.getState().update('fresh', 'files', { type: 'add', kind: 'new' })
  assert.deepEqual(
    useContextPagesStore.getState().sessions.fresh.pages.map((page) => page.kind),
    ['new'],
  )
})

test('blank launcher reuses existing resource and closes without leaving a phantom tab', () => {
  let state = update(createContextPages('changes'), { type: 'add', kind: 'new' })
  const blank = state.activeId
  state = update(state, { type: 'open', kind: 'changes', replaceId: blank })
  assert.equal(state.pages.length, 1)
  assert.equal(state.pages[0].kind, 'changes')
  state = update(state, { type: 'close', id: state.activeId })
  assert.deepEqual(state.pages, [])
  assert.equal(state.activeId, '')
  assert.equal(state.contentOpen, false)
})

test('closing an active tab chooses the right neighbor then the left', () => {
  let state = createContextPages('new')
  state = update(state, { type: 'add', kind: 'browser' })
  state = update(state, { type: 'add', kind: 'terminal', resourceId: 'pty-1' })
  state = update(state, { type: 'select', id: 'page-2' })
  state = update(state, { type: 'close', id: 'page-2' })
  assert.equal(state.activeId, 'page-3')
  state = update(state, { type: 'close', id: 'page-3' })
  assert.equal(state.activeId, 'page-1')
})

test('per-task workspaces retain scroll, summary, tabs and delayed resource state beyond old cache limits', () => {
  useContextPagesStore.setState({ sessions: {} })
  const store = () => useContextPagesStore.getState()
  store().update('a', 'changes', { type: 'open', kind: 'changes' })
  store().update('a', 'changes', {
    type: 'workspace',
    summaryOpen: true,
    conversationScrollTop: 412,
  })
  for (let i = 0; i < 40; i++) store().update(`b-${i}`, 'files', { type: 'add', kind: 'new' })
  store().update('a', 'changes', {
    type: 'view',
    id: 'page-1',
    patch: { expandedFiles: ['src/a.ts'] },
  })
  assert.equal(store().sessions.a.summaryOpen, true)
  assert.equal(store().sessions.a.conversationScrollTop, 412)
  assert.deepEqual(store().sessions.a.pages[0].view.expandedFiles, ['src/a.ts'])
  assert.deepEqual(store().sessions['b-39'].pages[0].view.expandedFiles, [])
})

test('page limit returns an explicit outcome and still allows resource reuse', () => {
  useContextPagesStore.setState({ sessions: {} })
  const store = useContextPagesStore.getState()
  store.update('limit', 'changes', { type: 'open', kind: 'changes' })
  for (let i = 1; i < MAX_CONTEXT_PAGES; i++)
    store.update('limit', 'changes', { type: 'add', kind: 'new' })
  assert.equal(store.update('limit', 'changes', { type: 'add', kind: 'new' }), 'limit')
  assert.equal(store.update('limit', 'changes', { type: 'open', kind: 'changes' }), 'updated')
  assert.equal(useContextPagesStore.getState().sessions.limit.pages.length, MAX_CONTEXT_PAGES)
})

test('unsent drafts are never evicted merely because another task is opened', () => {
  try {
    for (let i = 0; i < 80; i++) updateComposerDraft(`retain-${i}`, { text: `draft ${i}` })
    assert.equal(readComposerDraft('retain-0').text, 'draft 0')
  } finally {
    for (let i = 0; i < 80; i++) clearComposerDraft(`retain-${i}`)
  }
})

test('file identity and change comparison scope keep distinct resources separate', () => {
  let state = createContextPages('files')
  state = update(state, {
    type: 'open',
    kind: 'files',
    resourceId: 'connection-a/project-a/src/index.ts',
  })
  const first = state.activeId
  state = update(state, {
    type: 'open',
    kind: 'files',
    resourceId: 'connection-a/project-b/src/index.ts',
  })
  assert.notEqual(state.activeId, first)
  state = update(state, {
    type: 'open',
    kind: 'files',
    resourceId: 'connection-a/project-a/src/index.ts',
  })
  assert.equal(state.activeId, first)
  state = update(state, { type: 'open', kind: 'changes', resourceId: 'project-a/worktree' })
  const worktree = state.activeId
  state = update(state, { type: 'open', kind: 'changes', resourceId: 'project-a/branch/main' })
  assert.notEqual(state.activeId, worktree)
})

test('late terminal creation binds only its original page and never reopens a closed page', () => {
  let state = createContextPages('terminal')
  const id = state.activeId
  state = update(state, { type: 'resource', id, resourceId: 'pty-a' })
  assert.equal(state.pages[0].resourceId, 'pty-a')
  state = update(state, { type: 'close', id })
  assert.equal(update(state, { type: 'resource', id, resourceId: 'pty-b' }), state)
})

test('the transcript saves its reading position before it unmounts on a plain route switch', async () => {
  // 侧栏 / ⌥↑⌥↓ / 后退直接换路由，没有“导航前”通知；最后一次滚动的 scroll 事件可能晚于卸载。
  // 布局 effect 的清理在 DOM 移除前运行，必须在那里补存阅读位置（test:workbench 的“切换会话保存阅读位置”曾因此偶发失败）。
  const { readFile } = await import('node:fs/promises')
  const source = await readFile('src/features/chat/FocusTranscript.tsx', 'utf8')
  // 滚动停下后才写的那次可能还没到点：清理里先取消它，再同步写入，不靠定时器活过卸载。
  assert.match(
    source,
    /useLayoutEffect\(\(\) => \{\s*const element = transcriptElement\s*if \(!element \|\| transcriptLoadState !== 'ready'\) return\s*return \(\) => \{\s*window\.clearTimeout\(scrollSaveTimer\.current\)\s*if \(element\.clientHeight <= 0\) return\s*saveReadingPosition\(sessionId, element\)/,
  )
  // 离开时贴着底部记 null（回来落到最新消息），往上翻着读的记视口顶上那条消息，判断交给纯函数。
  assert.match(source, /return captureReadingPosition\(/)
  assert.match(source, /conversationAnchor: position\?\.anchor \?\? null/)
  // 回来时按那条消息校正（像素只作后备）。
  assert.match(source, /restoreAnchor=\{/)
})

const view = (scrollTop, extra = {}) => ({
  scrollTop,
  scrollHeight: 5000,
  clientHeight: 800,
  viewportTop: 100,
  ...extra,
})
const rows = [
  { messageId: 'm1', top: -300, bottom: 50 },
  { messageId: 'm2', top: 50, bottom: 700 },
  { messageId: 'm3', top: 700, bottom: 1400 },
]

test('leaving at the latest message records null so the reader comes back to the latest message', () => {
  assert.equal(captureReadingPosition(view(4200), rows), null)
  assert.equal(captureReadingPosition(view(4200 - LATEST_SLACK_PX), rows), null)
  assert.notEqual(captureReadingPosition(view(4200 - LATEST_SLACK_PX - 1), rows), null)
})

test('leaving scrolled up records the message at the top of the view and how far it is scrolled past', () => {
  // m1 的底边在视口顶（100）之上，视口顶上那条是 m2：它的顶边已经滚过视口顶 50px。
  assert.deepEqual(captureReadingPosition(view(1200), rows), {
    top: 1200,
    anchor: { messageId: 'm2', offset: 50 },
  })
  // 行的顺序不影响结果；顶边还在视口顶下方时 offset 为负。
  const shifted = rows.map((row) => ({ ...row, top: row.top + 420, bottom: row.bottom + 420 }))
  assert.deepEqual(captureReadingPosition(view(1200), shifted.toReversed()).anchor, {
    messageId: 'm1',
    offset: -20,
  })
})

test('a collapsed or torn-down transcript never overwrites the saved reading position', () => {
  // 容器高度为 0（被收起或已从布局里拿掉）。
  assert.equal(captureReadingPosition(view(1200, { clientHeight: 0 }), rows), undefined)
  // 消息行已卸载：scrollHeight 只剩外壳，算出来总像“在底部”，不能把往上读的位置改成 null。
  assert.equal(captureReadingPosition(view(0, { scrollHeight: 800 }), []), undefined)
})

test('restoring an anchor moves the remembered message back to the same place in the view', () => {
  const anchor = { messageId: 'm2', offset: 50 }
  // 回来时按估算高度排版，m2 的顶边落在视口顶下方 400px，离开时它滚过视口顶 50px：要往下滚 450px。
  assert.equal(anchorScrollDelta(500, 100, anchor), 450)
  assert.equal(anchorScrollDelta(50, 100, anchor), 0)
})

test('a reader who left at the bottom clears the saved reading position', () => {
  useContextPagesStore.setState({ sessions: {} })
  const store = () => useContextPagesStore.getState()
  store().update('a', 'files', { type: 'workspace', conversationScrollTop: 412 })
  assert.equal(store().sessions.a.conversationScrollTop, 412)
  store().update('a', 'files', { type: 'workspace', conversationScrollTop: null })
  assert.equal(store().sessions.a.conversationScrollTop, null)
  // 只改别的工作区字段时不动阅读位置。
  store().update('a', 'files', { type: 'workspace', conversationScrollTop: 300 })
  store().update('a', 'files', { type: 'workspace', summaryOpen: true })
  assert.equal(store().sessions.a.conversationScrollTop, 300)
})

test('the reading anchor is validated, cleared at the latest message and unchanged writes keep the state object', () => {
  useContextPagesStore.setState({ sessions: {} })
  const store = () => useContextPagesStore.getState()
  const anchor = { messageId: 'msg-1', offset: 40.4 }
  store().update('a', 'files', {
    type: 'workspace',
    conversationScrollTop: 900,
    conversationAnchor: anchor,
  })
  assert.deepEqual(store().sessions.a.conversationAnchor, { messageId: 'msg-1', offset: 40 })
  const before = store().sessions.a
  store().update('a', 'files', {
    type: 'workspace',
    conversationScrollTop: 900,
    conversationAnchor: { messageId: 'msg-1', offset: 40 },
  })
  assert.equal(store().sessions.a, before)
  // 非法值不进状态。
  for (const bad of [{ messageId: '', offset: 1 }, { messageId: 'x', offset: Number.NaN }, 'x'])
    store().update('a', 'files', { type: 'workspace', conversationAnchor: bad })
  assert.deepEqual(store().sessions.a.conversationAnchor, { messageId: 'msg-1', offset: 40 })
  store().update('a', 'files', {
    type: 'workspace',
    conversationScrollTop: null,
    conversationAnchor: null,
  })
  assert.equal(store().sessions.a.conversationAnchor, null)
  assert.equal(store().sessions.a.conversationScrollTop, null)
})
