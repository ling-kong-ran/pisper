// 规格 v2 第 4.5 节会话树老问题的前端回归：
// 1 标记写回共享摘要后转录能读到；2 另一条分支上的标记先确认、取消时什么都不改；
// 3 “从此处继续”等导航的提示带“撤销”，撤销把 restoreEntryId 传回导航接口并广播分支变化；
// 6 导航响应里的新树立刻更新分支数。Runtime 一侧的撤销语义在 session-tree.test.mjs。
import assert from 'node:assert/strict'
import test from 'node:test'

// events.ts 用 window 广播；Node 里给一个最小的 EventTarget。
globalThis.window ??= Object.assign(new EventTarget(), { location: { hash: '' } })
globalThis.CustomEvent ??= class CustomEvent extends Event {
  constructor(type, init = {}) {
    super(type)
    this.detail = init.detail
  }
}

const { chatApi } = await import('../../src/features/chat/chat-api.ts')
const { queryClient } = await import('../../src/lib/startup-queries.ts')
const summary = await import('../../src/features/chat/session-tree-summary.ts')
const actions = await import('../../src/features/chat/session-tree-actions.ts')
const events = await import('../../src/features/chat/events.ts')

const t = (key, options) => (options ? `${key} ${JSON.stringify(options)}` : key)
const tree = (branchCount, labels = {}) => ({
  sessionId: 's1',
  leafId: 'leaf',
  nodeCount: 3,
  branchCount,
  streaming: false,
  nodes: [
    { id: 'a1', label: labels.a1 || '' },
    { id: 'a2', label: labels.a2 || '' },
    { id: 'x', label: 7 },
  ],
  lineage: null,
})

test('session tree summary keeps only branch count and labels from untrusted trees', () => {
  assert.deepEqual(summary.summarizeSessionTree(tree(2, { a1: '  checkpoint  ' })), {
    branchCount: 2,
    labels: { a1: 'checkpoint' },
  })
  assert.deepEqual(summary.summarizeSessionTree(null), { branchCount: 0, labels: {} })
  assert.deepEqual(summary.summarizeSessionTree({ branchCount: -3, nodes: 'nope' }), {
    branchCount: 0,
    labels: {},
  })
  assert.equal(
    summary.summarizeSessionTree({ nodes: [{ id: 'a', label: 'x'.repeat(200) }] }).labels.a.length,
    80,
  )
})

test('navigation and label responses update the shared summary at once (old issues 1 and 6)', () => {
  summary.applySessionTreeResponse('s1', tree(0))
  const key = summary.sessionTreeSummaryKey('s1')
  assert.equal(queryClient.getQueryData(key).branchCount, 0)
  summary.applySessionTreeResponse('s1', tree(1, { a2: 'Resume here' }))
  assert.deepEqual(queryClient.getQueryData(key), { branchCount: 1, labels: { a2: 'Resume here' } })
  queryClient.removeQueries({ queryKey: key })
})

test('restore targets are untrusted strings: empty, non-string or oversized ids disable undo', () => {
  assert.equal(actions.normalizeRestoreEntryId(' e1 '), 'e1')
  assert.equal(actions.normalizeRestoreEntryId(''), '')
  assert.equal(actions.normalizeRestoreEntryId(42), '')
  assert.equal(actions.normalizeRestoreEntryId('x'.repeat(201)), '')
})

test('a tree navigation toast offers undo that navigates back and announces the branch change (old issue 3)', async (context) => {
  const notices = []
  const notify = (message, tone, options) => notices.push({ message, tone, options })
  const calls = []
  const original = chatApi.navigateSessionTree
  context.after(() => {
    chatApi.navigateSessionTree = original
    queryClient.removeQueries({ queryKey: summary.sessionTreeSummaryKey('s1') })
  })
  chatApi.navigateSessionTree = async (...args) => {
    calls.push(args)
    return { ...tree(0), cancelled: false, editorText: null, restoreEntryId: 'a1' }
  }
  const announced = []
  const unsubscribe = events.subscribeSessionBranchChanges((id) => announced.push(id))
  context.after(unsubscribe)

  actions.notifyTreeNavigation('s1', { restoreEntryId: 'leaf-before' }, 'continued', { notify, t })
  assert.equal(notices.length, 1)
  assert.equal(notices[0].message, 'continued')
  assert.equal(notices[0].options.action.label, 'chat:sessionTree.undo')
  notices[0].options.action.onAction()
  await new Promise((resolve) => setTimeout(resolve, 0))
  assert.deepEqual(calls, [['s1', 'leaf-before', false]])
  assert.deepEqual(announced, ['s1'])
  assert.equal(notices[1].message, 'chat:sessionTree.undone')
  // 撤销的响应带新树：分支数立刻回写。
  assert.equal(queryClient.getQueryData(summary.sessionTreeSummaryKey('s1')).branchCount, 0)

  // 没有撤销目标（旧 Runtime、叶子是用户消息）时只提示，不给撤销按钮。
  actions.notifyTreeNavigation('s1', {}, 'continued', { notify, t })
  assert.equal(notices[2].options, undefined)
})

test('a failed undo reports the error instead of claiming success', async (context) => {
  const notices = []
  const original = chatApi.navigateSessionTree
  context.after(() => {
    chatApi.navigateSessionTree = original
  })
  chatApi.navigateSessionTree = async () => {
    throw new Error('当前会话正在运行')
  }
  const ok = await actions.undoTreeNavigation('s1', 'a1', {
    notify: (message, tone) => notices.push({ message, tone }),
    t,
  })
  assert.equal(ok, false)
  assert.equal(notices.length, 1)
  assert.equal(notices[0].tone, 'error')
})

test('opening a mark on another branch asks first; cancelling changes nothing (old issue 2)', async (context) => {
  const original = chatApi.navigateSessionTreeTarget
  context.after(() => {
    chatApi.navigateSessionTreeTarget = original
  })
  const navigations = []
  chatApi.navigateSessionTreeTarget = async (...args) => {
    navigations.push(args)
    return { cancelled: false, editorText: null, restoreEntryId: 'old-leaf' }
  }
  const confirms = []
  const opened = []
  const notices = []
  const deps = {
    sessionId: 's2',
    entryId: 'marked',
    open: (...args) => opened.push(args),
    notify: (message, tone, options) => notices.push({ message, options }),
    t,
  }
  const cancelled = await actions.switchToTurnOnOtherBranch({
    ...deps,
    confirm: async (options) => {
      confirms.push(options)
      return false
    },
  })
  assert.equal(cancelled, false)
  assert.deepEqual(navigations, [])
  assert.deepEqual(opened, [])
  assert.equal(confirms[0].title, 'chat:sessionTree.otherBranchTitle')
  assert.equal(confirms[0].confirmLabel, 'chat:sessionTree.switchBranch')

  const switched = await actions.switchToTurnOnOtherBranch({ ...deps, confirm: async () => true })
  assert.equal(switched, true)
  assert.deepEqual(navigations, [['s2', 'marked']])
  assert.deepEqual(opened, [['s2', 'marked']])
  assert.equal(notices[0].message, 'chat:sessionTree.switchedBranch')
  assert.equal(notices[0].options.action.label, 'chat:sessionTree.undo')
})
