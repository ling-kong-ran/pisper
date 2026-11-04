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
  SIDEBAR_RECENT_LIMIT,
  buildSidebarRecents,
} from '../../src/features/board/sidebar-recents.ts'
import { sidebarStatusGroups } from '../../src/features/board/useSidebarRecents.ts'
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

test('queued board items carry the queue position and the title of the session they wait for (A9)', () => {
  const sessions = [
    { id: 'run', name: '重构登录流程', streaming: true, modified: iso(-MIN) },
    {
      id: 'q1',
      name: '补测试',
      modified: iso(-MIN),
      queue: { position: 1, startAfter: 'run', createdAt: iso(-30_000) },
    },
    {
      id: 'q2',
      name: '更新说明',
      modified: iso(-MIN),
      queue: { position: 'x', startAfter: 'gone', createdAt: iso(-20_000) },
    },
  ]
  const empty = { live: {}, localRuns: {}, runningSince: {}, patches: {} }
  const items = buildBoardItems(sessions, empty, {
    now: NOW,
    capabilities: { sessionQueue: true },
    doneWindow: '7d',
  })
  const byId = Object.fromEntries(items.map((item) => [item.id, item]))
  assert.equal(byId.q1.lane, 'queued')
  assert.equal(byId.q1.queuePosition, 1)
  assert.deepEqual(byId.q1.queue, { startAfter: 'run', startAfterTitle: '重构登录流程' })
  // 位置不是正整数时按“未知”；前置会话不在目录里时标题为空串（界面写“等前一个任务结束”）。
  assert.equal(byId.q2.queuePosition, null)
  assert.deepEqual(byId.q2.queue, { startAfter: 'gone', startAfterTitle: '' })
  assert.equal(byId.run.queue, null)
  // 没有 sessionQueue 能力：不进排队，也不带排队字段。
  const plain = buildBoardItems(sessions, empty, { now: NOW, doneWindow: '7d' })
  assert.ok(plain.every((item) => item.lane !== 'queued' && item.queue === null))
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

test('the sidebar is the same on every page; all-agents filters live in the page over board-view state', async () => {
  const [filters, app, board] = await Promise.all([
    readFile('src/features/board/BoardFilterBar.tsx', 'utf8'),
    readFile('src/App.tsx', 'utf8'),
    readFile('src/features/board/BoardPage.tsx', 'utf8'),
  ])
  // 侧栏在全部代理页也列会话（同一块侧栏换个页面就换用法容易误会）；侧栏筛选组件已删除。
  assert.doesNotMatch(app, /SidebarBoardFilters/)
  await assert.rejects(readFile('src/features/board/SidebarFilters.tsx', 'utf8'), {
    code: 'ENOENT',
  })
  // 筛选就是 /board 的 pisper-board-view：同一个 store，不新增偏好键；窄轨计数（setLaneFilter）同样写它。
  assert.match(filters, /useBoardViewStore\(\(state\) => state\.setLaneFilter\)/)
  assert.match(filters, /useBoardViewStore\(\(state\) => state\.setProjectFilter\)/)
  assert.doesNotMatch(filters, /localStorage|pageStateStorage/)
  // 状态用分段控件、项目用 AppSelect（组件规则）；项目与管理项目同一来源；“/”聚焦状态分段。
  assert.match(filters, /<SegmentedControl[\s\S]{0,200}id=\{BOARD_FILTER_ID\}/)
  assert.match(filters, /<AppSelect[\s\S]{0,300}setProjectFilter/)
  assert.match(filters, /useCatalogProjects\(\)/)
  assert.match(
    filters,
    /lane !== 'queued' \|\| \(capabilities\.sessionQueue === true && counts\.lanes\.queued > 0\)/,
  )
  // 项目的 ⋯：添加 / 管理项目常在；选中项目时它自己的操作直接在这里（经 useProjectMenu → useProjectCommands）。
  assert.match(filters, /useProjectMenu\(/)
  assert.match(filters, /menu\.newProject\(\)/)
  assert.match(filters, /menu\.actions\.onRename\(selected\)/)
  assert.match(filters, /<ProjectMenuDialogs \{\.\.\.menu\.dialogs\} \/>/)
  assert.match(
    app,
    /\.getElementById\(BOARD_FILTER_ID\)\s*\?\.querySelector<HTMLElement>\('\[data-state="on"\]'\)/,
  )
  // 桌面页内放筛选条；手机没有筛选控件，带着筛选进来时补一行说明与“清除筛选”。
  assert.match(board, /!phone && ready && board\.total > 0 && \(\s*<BoardFilterBar/)
  assert.match(board, /ready && filtered && phone && model\.total > 0/)
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

test('narrow windows rail the sidebar without touching the saved preference', (t) => {
  // 窗口宽不到 900 时自动收成窄轨，给内容让出位置；期间展开 / 收起不落盘，变宽后回到偏好。
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
  sidebar().setCollapsed(false)
  sidebar().enterNarrowRail()
  assert.equal(sidebar().collapsed, true)
  sidebar().toggleCollapsed()
  assert.equal(sidebar().collapsed, false, 'the user can still expand it')
  assert.equal(saved().collapsed, false)
  sidebar().setCollapsed(true)
  assert.equal(saved().collapsed, false, 'collapsing while narrow is not persisted')
  // 工作室的窄轨与窄窗口叠在一起：离开其中一个时，另一个还在就保持窄轨。
  sidebar().enterRouteRail()
  sidebar().leaveNarrowRail()
  assert.equal(sidebar().collapsed, true)
  sidebar().leaveRouteRail()
  assert.equal(sidebar().collapsed, false, 'leaving both restores the preference')
  sidebar().enterNarrowRail()
  sidebar().enterRouteRail()
  sidebar().leaveRouteRail()
  assert.equal(sidebar().collapsed, true, 'still narrow after leaving the studio')
  sidebar().leaveNarrowRail()
  assert.equal(sidebar().collapsed, false)
})

test('the sidebar keeps needs-you and running in place and lists recent chats below', () => {
  // 需要你、运行中常驻（为 0 也在，只留标题）；排队只在有排队时；置顶与今天完成不单列。
  const ids = (groups) => groups.map((group) => group.id)
  const empty = buildSidebarModel([], options)
  assert.deepEqual(ids(sidebarStatusGroups(empty)), ['attention', 'running'])
  const day = 24 * 60 * MIN
  const sessions = [
    { id: 'old', name: '三天前的会话', cwd: '/w/a', modified: iso(-3 * day) },
    { id: 'new', name: '刚才的会话', cwd: '/w/b', modified: iso(-MIN), unread: true },
    { id: 'pinned', name: '置顶的旧会话', cwd: '/w/a', modified: iso(-9 * day), pinned: true },
    { id: 'archived', name: '归档的', modified: iso(0), archived: true },
    { id: 'draft', name: '空草稿', modified: iso(0), messageCount: 0 },
    { id: 'waiting', name: '等你批准', modified: iso(0) },
  ]
  const recents = buildSidebarRecents(sessions, {
    items: [],
    shownIds: new Set(['waiting']),
    currentId: '',
  })
  // 置顶在前，其余按修改时间从新到旧；归档、空草稿与上面状态组里的不列；不分项目。
  assert.deepEqual(
    recents.rows.map((row) => row.id),
    ['pinned', 'new', 'old'],
  )
  assert.equal(recents.hidden, 0)
  const fresh = recents.rows[1]
  assert.equal(fresh.group, 'recent')
  assert.equal(fresh.title, '刚才的会话')
  assert.equal(fresh.unread, true)
  // 空草稿只有正在看时才列出（侧栏里得找得到当前会话）。
  assert.ok(
    buildSidebarRecents(sessions, { items: [], shownIds: new Set(), currentId: 'draft' }).rows.some(
      (row) => row.id === 'draft',
    ),
  )
})

test('recent chats stop at the limit but always keep the open chat', () => {
  const many = Array.from({ length: SIDEBAR_RECENT_LIMIT + 3 }, (_, index) => ({
    id: `s${index}`,
    name: `会话 ${index}`,
    modified: iso(-index * MIN),
  }))
  const last = `s${SIDEBAR_RECENT_LIMIT + 2}`
  const plain = buildSidebarRecents(many, { items: [], shownIds: new Set(), currentId: '' })
  assert.equal(plain.rows.length, SIDEBAR_RECENT_LIMIT)
  assert.equal(plain.hidden, 3)
  const open = buildSidebarRecents(many, { items: [], shownIds: new Set(), currentId: last })
  assert.equal(open.rows.at(-1).id, last)
  assert.equal(open.hidden, 2)
})
