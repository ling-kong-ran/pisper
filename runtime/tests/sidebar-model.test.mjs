// 方向 A 侧栏（规格 v2 第 2.1、2.2 节）：会话分组的纯数据模型、“需要你”原因与快捷处理、长列表截断，
// 以及侧栏收起偏好 'pisper-sidebar' 的登记与解析。分组建立在看板推导（buildBoardItems）之上，不另造规则。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { BROWSER_PREFERENCE_KEYS } from '../../shared/browser-preferences.mjs'
import { STORAGE_KEYS } from '../../src/app/storage.ts'
import { parseBoardCard } from '../../src/features/board/board-api.ts'
import { buildBoardItems } from '../../src/features/board/board-model.ts'
import {
  SIDEBAR_DONE_LIMIT,
  SIDEBAR_RUNNING_LIMIT,
  adjacentSidebarSession,
  buildSidebarModel,
  sidebarQuickAction,
  sidebarReasonKeys,
  sidebarReasons,
} from '../../src/features/board/sidebar-model.ts'
import {
  DEFAULT_SIDEBAR_PREFERENCE,
  parseSidebarPreference,
  serializeSidebarPreference,
  useSidebarStore,
} from '../../src/stores/sidebar-store.ts'
import { pageStateStorage } from '../../src/lib/page-state-storage.ts'

const NOW = Date.parse('2026-10-29T10:00:00.000Z')
const iso = (offsetMs) => new Date(NOW + offsetMs).toISOString()
const MIN = 60_000
const options = { now: NOW, timeZoneOffsetMinutes: 0 }
const overlay = (entries) => ({
  live: {},
  localRuns: {},
  runningSince: {},
  patches: {},
  pending: {},
  feed: { ready: true, entries },
})
const entry = (session, card = {}) => ({
  session: { cwd: '/w/pisper', messageCount: 4, ...session },
  card: parseBoardCard(card),
  revision: 1,
})
const approval = (id, extra = {}) => ({
  id,
  toolName: 'bash',
  argsPreview: 'npm test',
  severity: 'standard',
  createdAt: iso(-5 * MIN),
  ...extra,
})

// 一批真实形态的会话：经 buildBoardItems 推导泳道、状态和危险等级，再交给侧栏模型分组。
function fleet() {
  const entries = {
    critical: entry(
      { id: 'critical', name: '改写远端分支', needsAttention: true, attentionReason: 'approval' },
      {
        boardStatus: 'attention',
        approvals: [approval('c1', { severity: 'critical', createdAt: iso(-30 * MIN) })],
      },
    ),
    approve: entry(
      { id: 'approve', name: '修复 CI 失败', needsAttention: true, attentionReason: 'approval' },
      { boardStatus: 'attention', approvals: [approval('a1', { createdAt: iso(-18 * MIN) })] },
    ),
    fail: entry(
      {
        id: 'fail',
        name: '支付回调测试',
        needsAttention: true,
        attentionReason: 'failure',
        unread: true,
        lastCompletedAt: iso(-2 * MIN),
      },
      { boardStatus: 'attention', outcome: 'failed', error: { message: 'exit 1', kind: 'tool' } },
    ),
    runLate: entry(
      { id: 'runLate', name: '后开始的运行', streaming: true, pinned: true },
      { boardStatus: 'running', startedAt: iso(-1 * MIN) },
    ),
    runEarly: entry(
      { id: 'runEarly', name: '先开始的运行', streaming: true },
      { boardStatus: 'running', startedAt: iso(-9 * MIN) },
    ),
    doneEarly: entry(
      { id: 'doneEarly', name: '早上完成', lastCompletedAt: iso(-3 * 60 * MIN), unread: false },
      { boardStatus: 'done' },
    ),
    doneLate: entry(
      { id: 'doneLate', name: '刚完成', lastCompletedAt: iso(-10 * MIN), unread: true },
      { boardStatus: 'done' },
    ),
    handled: entry(
      {
        id: 'handled',
        name: '已查看的失败',
        attentionReason: 'failure',
        needsAttention: true,
        unread: false,
        lastCompletedAt: iso(-20 * MIN),
      },
      { boardStatus: 'done', outcome: 'failed' },
    ),
    pinnedOld: entry(
      {
        id: 'pinnedOld',
        name: '置顶的旧会话',
        pinned: true,
        lastCompletedAt: iso(-3 * 86_400_000),
      },
      { boardStatus: 'done' },
    ),
    yesterday: entry(
      { id: 'yesterday', name: '昨天完成', lastCompletedAt: iso(-26 * 60 * MIN) },
      { boardStatus: 'done' },
    ),
    archived: entry(
      { id: 'archived', name: '已归档', archived: true, streaming: true },
      { boardStatus: 'running', startedAt: iso(-1 * MIN) },
    ),
  }
  return buildBoardItems([], overlay(entries), { ...options, doneWindow: '7d' })
}

const ids = (group) => group.rows.map((row) => row.id)

test('sessions group into needs-you, running, pinned and today, archived ones never appear', () => {
  const model = buildSidebarModel(fleet(), options)
  assert.deepEqual(
    model.groups.map((group) => group.id),
    ['attention', 'running', 'pinned', 'done'],
    'queued stays hidden without the sessionQueue capability',
  )
  const [attention, running, pinned, done] = model.groups
  // 需要你：等待最久在前（不受置顶影响），永不截断。
  assert.deepEqual(ids(attention), ['critical', 'approve', 'fail'])
  // 运行中：按开始时间先后，置顶不改变组内顺序；已归档的运行不出现。
  assert.deepEqual(ids(running), ['runEarly', 'runLate'])
  // 置顶只收不在上面几组里的置顶会话；今天完成按完成时间倒序，不含置顶、昨天与 7 天窗口里更早的。
  assert.deepEqual(ids(pinned), ['pinnedOld'])
  assert.deepEqual(ids(done), ['doneLate', 'handled', 'doneEarly'])
  assert.equal(
    model.groups.flatMap((group) => group.rows).some((row) => row.id === 'archived'),
    false,
  )
  assert.deepEqual(model.counts, { attention: 3, running: 2, queued: 0, pinned: 1, done: 3 })
  assert.deepEqual(model.order, [
    'critical',
    'approve',
    'fail',
    'runEarly',
    'runLate',
    'pinnedOld',
    'doneLate',
    'handled',
    'doneEarly',
  ])
  assert.equal(model.longestWaitingId, 'critical')

  // 行内容：需要你带等待起点与原因；运行中带计时起点和错开的相位；今天完成带完成时刻、未读与形状。
  assert.equal(attention.rows[0].waitingSince, NOW - 30 * MIN)
  assert.deepEqual(
    attention.rows.map((row) => row.glyph),
    ['attention', 'attention', 'attention'],
  )
  assert.deepEqual(
    running.rows.map((row) => [row.startedAt, row.glyph]),
    [
      [NOW - 9 * MIN, 'run'],
      [NOW - 1 * MIN, 'run'],
    ],
  )
  assert.notEqual(running.rows[0].phase, running.rows[1].phase)
  assert.deepEqual(
    done.rows.map((row) => [row.id, row.glyph, row.unread, row.finishedAt]),
    [
      ['doneLate', 'done', true, NOW - 10 * MIN],
      ['handled', 'failed', false, NOW - 20 * MIN],
      ['doneEarly', 'done', false, NOW - 180 * MIN],
    ],
  )
})

test('needs-you rows carry a Chinese reason without commands and one-step actions only for standard approvals', () => {
  const rows = Object.fromEntries(
    buildSidebarModel(fleet(), options).groups[0].rows.map((row) => [row.id, row]),
  )
  assert.deepEqual(rows.critical.reasons, [{ kind: 'approval', critical: true }])
  assert.deepEqual(rows.approve.reasons, [{ kind: 'approval', critical: false }])
  assert.deepEqual(rows.fail.reasons, [{ kind: 'failed' }])
  // 普通批准一步“拒绝 / 允许”（带第一条审批 id）；高风险与失败一律“打开”。
  assert.deepEqual(rows.approve.quickAction, { kind: 'decide', approvalId: 'a1', remaining: 0 })
  assert.deepEqual(rows.critical.quickAction, { kind: 'open' })
  assert.deepEqual(rows.fail.quickAction, { kind: 'open' })
  // 第二行只写原因：任何字段里都没有命令或输出。
  for (const row of Object.values(rows))
    assert.doesNotMatch(JSON.stringify(row), /npm test|exit 1/, row.id)

  assert.deepEqual(sidebarReasonKeys(rows.critical.reasons), [
    { key: 'board:sidebar.reason.criticalApproval' },
  ])
  assert.deepEqual(sidebarReasonKeys(rows.fail.reasons), [{ key: 'board:sidebar.reason.failed' }])
})

test('reasons combine and anything without full standard approval details opens the session', () => {
  const base = {
    id: 's',
    title: 't',
    lane: 'attention',
    since: NOW,
    startedAt: null,
    pinned: false,
    unread: true,
    approvals: [{ id: 'p1', toolName: 'bash' }],
    approvalsState: 'ready',
    severity: 'standard',
  }
  // 提问（A5 的 askUser）：单独写“等你回答 · N 个问题”，跟在批准后只写“N 个问题”。
  const question = { ...base, status: 'question', approvals: [], approvalsState: 'none' }
  assert.deepEqual(sidebarReasons({ ...question, questionCount: 2 }), [
    { kind: 'question', count: 2 },
  ])
  assert.deepEqual(sidebarReasonKeys(sidebarReasons({ ...question, questionCount: 2 })), [
    { key: 'board:sidebar.reason.questions', params: { count: 2 } },
  ])
  assert.deepEqual(sidebarReasonKeys(sidebarReasons(question)), [
    { key: 'board:sidebar.reason.question' },
  ])
  const combined = { ...base, status: 'approval', severity: 'critical', questionCount: 2 }
  assert.deepEqual(sidebarReasonKeys(sidebarReasons(combined)), [
    { key: 'board:sidebar.reason.criticalApproval' },
    { key: 'board:sidebar.reason.questionCount', params: { count: 2 } },
  ])
  assert.deepEqual(sidebarQuickAction(question), { kind: 'open' })
  // 普通批准但同时在等问题、详情还在加载 / 加载失败、只有截断的预览、危险等级未知：都只能打开。
  for (const item of [
    { ...base, status: 'approval', questionCount: 1 },
    { ...base, status: 'approval', approvalsState: 'loading' },
    { ...base, status: 'approval', approvalsState: 'error' },
    { ...base, status: 'approval', approvals: [{ id: 'p1', truncated: true }] },
    { ...base, status: 'approval', severity: null },
    { ...base, status: 'approval', approvals: [] },
    // 第一条是普通批准，但后面还排着高风险（行的 severity 取最高）：整行只能打开处理。
    {
      ...base,
      status: 'approval',
      severity: 'critical',
      approvals: [
        { id: 'p1', toolName: 'bash', severity: 'standard' },
        { id: 'p2', toolName: 'bash', severity: 'critical' },
      ],
    },
  ])
    assert.deepEqual(sidebarQuickAction(item), { kind: 'open' }, JSON.stringify(item))
  assert.deepEqual(sidebarQuickAction({ ...base, status: 'approval' }), {
    kind: 'decide',
    approvalId: 'p1',
    remaining: 0,
  })
  // 同一会话挂着多条普通批准：一步处理第一条，并告诉侧栏还剩几条（行不收起，下一条顶上来）。
  assert.deepEqual(
    sidebarQuickAction({
      ...base,
      status: 'approval',
      approvals: [
        { id: 'p1', toolName: 'bash' },
        { id: 'p2', toolName: 'bash' },
      ],
    }),
    { kind: 'decide', approvalId: 'p1', remaining: 1 },
  )
})

test('long running lists show eight rows plus a remainder and today keeps at most ten rows', () => {
  const items = []
  for (let index = 0; index < 11; index++)
    items.push({
      id: `run${String(index).padStart(2, '0')}`,
      title: `运行 ${index}`,
      lane: 'running',
      status: 'running',
      since: NOW - (20 - index) * MIN,
      startedAt: null,
      pinned: false,
      unread: false,
      approvals: [],
      approvalsState: 'none',
      severity: null,
    })
  for (let index = 0; index < 13; index++)
    items.push({
      id: `done${String(index).padStart(2, '0')}`,
      title: `完成 ${index}`,
      lane: 'done',
      status: 'done',
      since: NOW - (index + 1) * MIN,
      startedAt: null,
      pinned: false,
      unread: false,
      approvals: [],
      approvalsState: 'none',
      severity: null,
    })
  const model = buildSidebarModel(items, options)
  const running = model.groups.find((group) => group.id === 'running')
  const done = model.groups.find((group) => group.id === 'done')
  assert.equal(SIDEBAR_RUNNING_LIMIT, 8)
  assert.equal(SIDEBAR_DONE_LIMIT, 10)
  assert.deepEqual([running.count, running.rows.length, running.hidden], [11, 8, 3])
  assert.equal(running.rows[0].id, 'run00', 'earliest start first')
  assert.deepEqual([done.count, done.rows.length, done.hidden], [13, 10, 3])
  assert.equal(done.rows[0].id, 'done00', 'most recently finished first')
  // 可见顺序只含显示出来的行：⌥↓ 不会跳进被截掉的部分。
  assert.equal(model.order.length, 18)
  assert.equal(new Set(running.rows.map((row) => row.phase)).size, 8, 'every arc has its own phase')
})

test('queued shows only with the sessionQueue capability and in queue order', () => {
  const queued = (id, queuePosition, since) => ({
    id,
    title: id,
    lane: 'queued',
    status: 'queued',
    since,
    startedAt: null,
    pinned: false,
    unread: false,
    approvals: [],
    approvalsState: 'none',
    severity: null,
    queuePosition,
  })
  const items = [queued('b', 2, NOW - MIN), queued('a', 1, NOW), queued('c', null, NOW - 9 * MIN)]
  assert.equal(
    buildSidebarModel(items, options).groups.some((group) => group.id === 'queued'),
    false,
  )
  const model = buildSidebarModel(items, { ...options, capabilities: { sessionQueue: true } })
  const group = model.groups.find((entry) => entry.id === 'queued')
  assert.deepEqual(
    group.rows.map((row) => [row.id, row.position, row.glyph]),
    [
      ['a', 1, 'queued'],
      ['b', 2, 'queued'],
      ['c', 3, 'queued'],
    ],
  )
  // 能力在但没人排队：分组不出现；需要你与运行中即使为 0 也保留（窄轨与标题计数不消失）。
  const empty = buildSidebarModel([], { ...options, capabilities: { sessionQueue: true } })
  assert.deepEqual(
    empty.groups.map((entry) => [entry.id, entry.count]),
    [
      ['attention', 0],
      ['running', 0],
    ],
  )
  assert.equal(empty.longestWaitingId, null)
})

test('junk rows, duplicates and missing times never throw or reorder unpredictably', () => {
  const row = (id, since) => ({
    id,
    title: id,
    lane: 'attention',
    status: 'approval',
    since,
    startedAt: null,
    pinned: false,
    unread: true,
    approvals: [],
    approvalsState: 'loading',
    severity: null,
  })
  const model = buildSidebarModel(
    [
      null,
      { id: '' },
      row('late', NOW),
      row('unknown', null),
      row('late', NOW - MIN),
      row('x', NaN),
    ],
    options,
  )
  assert.deepEqual(ids(model.groups[0]), ['late', 'unknown', 'x'])
})

test('alt arrows walk the visible sidebar order without wrapping', () => {
  const order = ['a', 'b', 'c']
  assert.equal(adjacentSidebarSession(order, 'b', 1), 'c')
  assert.equal(adjacentSidebarSession(order, 'b', -1), 'a')
  assert.equal(adjacentSidebarSession(order, 'c', 1), null)
  assert.equal(adjacentSidebarSession(order, 'a', -1), null)
  assert.equal(adjacentSidebarSession(order, 'missing', 1), 'a')
  assert.equal(adjacentSidebarSession(order, null, -1), 'c')
  assert.equal(adjacentSidebarSession([], 'a', 1), null)
})

test('the sidebar preference is registered, read through page-state storage and tolerant of junk', async () => {
  assert.equal(STORAGE_KEYS.sidebar, 'pisper-sidebar')
  assert.ok(BROWSER_PREFERENCE_KEYS.includes('pisper-sidebar'))
  assert.deepEqual(DEFAULT_SIDEBAR_PREFERENCE, { collapsed: false, doneCollapsed: false })
  for (const raw of [null, '', 'nope', '[]', '1', 'true', '{"collapsed":"yes"}', 42, {}])
    assert.deepEqual(parseSidebarPreference(raw), { collapsed: false, doneCollapsed: false })
  const saved = serializeSidebarPreference({ collapsed: true, doneCollapsed: false })
  assert.deepEqual(parseSidebarPreference(saved), { collapsed: true, doneCollapsed: false })
  assert.deepEqual(parseSidebarPreference('{"collapsed":true,"doneCollapsed":true,"width":999}'), {
    collapsed: true,
    doneCollapsed: true,
  })
  assert.equal(parseSidebarPreference(saved).width, undefined)

  // 唯一所有者：侧栏状态只经 page-state-storage 写 'pisper-sidebar'；ui-store 不再持有旧侧栏字段，
  // 也不再读旧的仅 localStorage 键 pisper-sidebar-collapsed。
  const [store, ui] = await Promise.all([
    readFile('src/stores/sidebar-store.ts', 'utf8'),
    readFile('src/stores/ui-store.ts', 'utf8'),
  ])
  assert.match(
    store,
    /pageStateStorage\.setItem\(STORAGE_KEYS\.sidebar, serializeSidebarPreference/,
  )
  assert.doesNotMatch(store, /\blocalStorage\b/)
  // 状态里不再有旧侧栏字段（读旧快照时只做剔除）。
  assert.doesNotMatch(
    ui,
    /sidebarCollapsed: (?:false|boolean|stored)|sidebarWidth: (?:DEFAULT|number|clamp)|toggleSidebar|STORAGE_KEYS\.sidebarCollapsed/,
  )
  assert.equal(Object.hasOwn(STORAGE_KEYS, 'sidebarCollapsed'), false)
})

test('the all-agents sidebar is a status / project filter over the same board-view state', async () => {
  const [filters, app, board] = await Promise.all([
    readFile('src/features/board/SidebarFilters.tsx', 'utf8'),
    readFile('src/App.tsx', 'utf8'),
    readFile('src/features/board/BoardPage.tsx', 'utf8'),
  ])
  // 规格 2.3：全部代理页侧栏不列会话行（代理只在主区列一次），只有状态与项目两组筛选。
  assert.match(app, /\{page === 'board' \? \(\s*<SidebarBoardFilters/)
  assert.doesNotMatch(filters, /data-sidebar-row|SidebarRowItem/)
  // 筛选就是 /board 的 pisper-board-view：同一个 store，不新增偏好键；窄轨计数（setLaneFilter）同样写它。
  assert.match(filters, /useBoardViewStore\(\(state\) => state\.setLaneFilter\)/)
  assert.match(filters, /useBoardViewStore\(\(state\) => state\.setProjectFilter\)/)
  assert.doesNotMatch(filters, /localStorage|pageStateStorage/)
  // 按钮语义：aria-pressed；“/ 聚焦筛选”落在第一项“全部”；排队只在 sessionQueue 能力且确有排队时出现。
  assert.match(filters, /aria-pressed=\{pressed\}/)
  assert.match(filters, /id=\{BOARD_FILTER_ID\}\s+filter="all"/)
  assert.match(
    filters,
    /lane !== 'queued' \|\| \(capabilities\.sessionQueue === true && counts\.lanes\.queued > 0\)/,
  )
  // 项目组 ⋯：添加项目（选目录 → 派发）与管理项目，沿用 useProjectCommands / ManageProjectsDialog。
  assert.match(filters, /commands\.newProject\(\)/)
  assert.match(filters, /<ManageProjectsDialog\s+open=\{manageOpen\}/)
  // 页内不再有筛选控件：“/”的目标从分组切换挪到侧栏；侧栏收成窄轨（或手机没有侧栏）时页内补一行筛选说明。
  assert.doesNotMatch(board, /BOARD_FILTER_ID/)
  assert.match(board, /ready && filtered && \(sidebarCollapsed \|\| phone\) && model\.total > 0/)
})

test('the component studio rails the sidebar for its visit without touching the saved preference', (t) => {
  // 规格 2.3 / 5.1：进入工作室时侧栏默认收成窄轨；用户在工作室里展开 / 收起只改本次停留；
  // 离开后回到 'pisper-sidebar' 里的偏好，偏好本身从头到尾没被写过。
  // Node 里没有 window：给 page-state-storage 一个内存版的 localStorage / sessionStorage。
  const memory = () => {
    const map = new Map()
    return {
      getItem: (key) => (map.has(key) ? map.get(key) : null),
      setItem: (key, value) => map.set(key, String(value)),
      removeItem: (key) => map.delete(key),
    }
  }
  const hadWindow = 'window' in globalThis
  const previousWindow = globalThis.window
  globalThis.window = { localStorage: memory(), sessionStorage: memory() }
  t.after(() => {
    if (hadWindow) globalThis.window = previousWindow
    else delete globalThis.window
  })
  const sidebar = () => useSidebarStore.getState()
  const saved = () => parseSidebarPreference(pageStateStorage.getItem(STORAGE_KEYS.sidebar))
  for (const preferred of [false, true]) {
    sidebar().setCollapsed(preferred)
    assert.equal(saved().collapsed, preferred)
    sidebar().enterRouteRail()
    assert.equal(sidebar().collapsed, true)
    assert.equal(sidebar().routeRail, true)
    sidebar().toggleCollapsed()
    assert.equal(sidebar().collapsed, false, 'the user can expand it inside the studio')
    sidebar().setDoneCollapsed(true)
    assert.equal(saved().collapsed, preferred, 'expanding inside the studio is not persisted')
    assert.equal(saved().doneCollapsed, true, 'other preferences still save normally')
    sidebar().setCollapsed(true)
    assert.equal(saved().collapsed, preferred)
    sidebar().leaveRouteRail()
    assert.equal(sidebar().routeRail, false)
    assert.equal(sidebar().collapsed, preferred, 'leaving restores the preference')
    sidebar().leaveRouteRail()
    assert.equal(sidebar().collapsed, preferred, 'leaving twice is harmless')
    sidebar().setDoneCollapsed(false)
  }
  sidebar().setCollapsed(false)
})
