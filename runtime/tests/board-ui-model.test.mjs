import assert from 'node:assert/strict'
import { readdir, readFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import test from 'node:test'
import {
  BOARD_SHORTCUT_ACTIONS,
  DEFAULT_BOARD_SHORTCUTS,
  DEFAULT_SHORTCUTS,
  boardShortcutFromEvent,
  findBoardShortcutConflict,
  matchesBoardShortcut,
  normalizeBoardShortcutBindings,
  validateBoardShortcutBindings,
  validateShortcutBindings,
} from '../../shared/shortcuts.mjs'
import {
  OVERVIEW_VIRTUALIZE_THRESHOLD,
  buildOverviewModel,
  overviewFilterCounts,
  overviewItems,
  overviewShouldVirtualize,
} from '../../src/features/board/overview-model.ts'
import {
  boardCardTone,
  boardEdgeWidth,
  boardSubAgent,
  guardReasonSummary,
  messageCount,
  modelShortLabel,
  projectLabel,
  shortPath,
} from '../../src/features/board/card-presentation.ts'

const root = resolve(import.meta.dirname, '../..')
// 全部代理（规格 v2 第 3.2 节）的分组模型：用最小的卡片数据构造，只给模型读到的字段。
const NOW = Date.parse('2026-10-31T12:00:00')
const at = (minutesAgo) => NOW - minutesAgo * 60_000
function card(id, lane, extra = {}) {
  return {
    id,
    title: id,
    lane,
    status:
      lane === 'attention'
        ? 'approval'
        : lane === 'running'
          ? 'running'
          : lane === 'queued'
            ? 'queued'
            : 'done',
    since: at(10),
    startedAt: null,
    pinned: false,
    unread: false,
    approvals: [],
    approvalsState: 'none',
    severity: null,
    workspaceKey: extra.workspaceKey ?? '/p/a',
    cwd: extra.workspaceKey ?? '/p/a',
    session: { id, archived: false },
    ...extra,
  }
}

test('overview groups by status in the fixed order and sorts each group by its own rule', () => {
  const items = [
    card('done-old', 'done', { since: at(90) }),
    card('wait-short', 'attention', { since: at(2) }),
    card('run-late', 'running', { startedAt: at(1) }),
    card('done-new', 'done', { since: at(5) }),
    card('wait-long', 'attention', { since: at(30) }),
    card('run-early', 'running', { startedAt: at(20) }),
  ]
  const model = buildOverviewModel(items, { now: NOW, groupBy: 'lane' })
  assert.deepEqual(
    model.sections.map((section) => [section.id, section.items.map((item) => item.id)]),
    [
      // 需要你：等待最久在前；运行中：按开始时间；今天完成：完成时间倒序。
      ['attention', ['wait-long', 'wait-short']],
      ['running', ['run-early', 'run-late']],
      ['done', ['done-new', 'done-old']],
    ],
  )
  assert.deepEqual(
    model.order.map((item) => item.id),
    ['wait-long', 'wait-short', 'run-early', 'run-late', 'done-new', 'done-old'],
  )
  assert.deepEqual(
    model.attention.map((item) => item.id),
    ['wait-long', 'wait-short'],
  )
  assert.equal(model.total, 6)
  assert.equal(model.running, 2)
  // 折叠的今天完成只留标题，不进键盘顺序。
  const collapsed = buildOverviewModel(items, { now: NOW, groupBy: 'lane', doneCollapsed: true })
  assert.equal(collapsed.sections.at(-1).items.length, 2)
  assert.deepEqual(
    collapsed.order.map((item) => item.id),
    ['wait-long', 'wait-short', 'run-early', 'run-late'],
  )
})

test('overview keeps the needs-you and running headings, hides queued without sessionQueue and drops archived or older done sessions', () => {
  const yesterday = NOW - 26 * 3_600_000
  const items = [
    card('queued', 'queued'),
    card('archived', 'running', { session: { id: 'archived', archived: true } }),
    // 置顶但昨天完成：在侧栏“置顶”里，不冒充“今天完成”。
    card('pinned-old', 'done', { pinned: true, since: yesterday }),
    card('done-today', 'done'),
  ]
  const model = buildOverviewModel(items, { now: NOW, groupBy: 'lane' })
  assert.deepEqual(
    model.sections.map((section) => [section.id, section.items.length]),
    [
      ['attention', 0],
      ['running', 0],
      ['done', 1],
    ],
  )
  assert.equal(model.total, 1)
  const queue = buildOverviewModel(items, {
    now: NOW,
    groupBy: 'lane',
    capabilities: { sessionQueue: true },
  })
  assert.deepEqual(
    queue.sections.map((section) => section.id),
    ['attention', 'running', 'queued', 'done'],
  )
  assert.deepEqual(
    overviewItems(items, { now: NOW }).map((item) => item.id),
    ['done-today'],
  )
})

test('overview groups by project in the remembered order with per-status counts, still sorted by status', () => {
  const items = [
    card('a-done', 'done', { workspaceKey: '/p/a' }),
    card('b-run', 'running', { workspaceKey: '/p/b', startedAt: at(3) }),
    card('a-wait', 'attention', { workspaceKey: '/p/a' }),
    card('c-new', 'running', { workspaceKey: '/p/c' }),
  ]
  const model = buildOverviewModel(items, {
    now: NOW,
    groupBy: 'project',
    projectOrder: [
      { key: '/p/b', cwd: '/p/b' },
      { key: '/p/empty', cwd: '/p/empty' },
      { key: '/p/a', cwd: '/p/a' },
    ],
  })
  // 记住顺序的项目在前（没有代理的项目不出分区），没记过的按出现顺序排在后面。
  assert.deepEqual(
    model.sections.map((section) => [section.key, section.items.map((item) => item.id)]),
    [
      ['/p/b', ['b-run']],
      ['/p/a', ['a-wait', 'a-done']],
      ['/p/c', ['c-new']],
    ],
  )
  assert.deepEqual(model.sections[1].counts, { attention: 1, running: 0, queued: 0, done: 1 })
  assert.equal(model.projectCount, 3)
  // 逐个处理只在需要你之间走，按页面上的顺序。
  assert.deepEqual(
    model.attention.map((item) => item.id),
    ['a-wait'],
  )
})

test('overview filters by status and project from pisper-board-view and reports an empty filter honestly', () => {
  const items = [
    card('a-wait', 'attention', { workspaceKey: '/p/a' }),
    card('b-run', 'running', { workspaceKey: '/p/b' }),
  ]
  const running = buildOverviewModel(items, { now: NOW, groupBy: 'lane', lanes: ['running'] })
  assert.deepEqual(
    running.sections.map((section) => [section.id, section.items.length]),
    [['running', 1]],
  )
  // 筛选不影响页头统计与逐个处理的范围（被筛掉的需要你由页面先清除筛选再开始）。
  assert.equal(running.total, 2)
  assert.deepEqual(running.attention, [])
  assert.deepEqual(
    running.attentionAll.map((item) => item.id),
    ['a-wait'],
  )
  const none = buildOverviewModel(items, {
    now: NOW,
    groupBy: 'project',
    lanes: ['done'],
  })
  assert.equal(none.filteredEmpty, true)
  assert.deepEqual(none.sections, [])
  assert.equal(buildOverviewModel([], { now: NOW, groupBy: 'lane' }).filteredEmpty, false)
})

test('the all-agents sidebar filters count the same agents as the page, before any filter', () => {
  // 规格 2.3：全部代理页侧栏的状态 / 项目筛选各带计数；与主区同一份代理（归档、更早完成、无能力的排队不算），
  // 按筛选前计——点了状态筛选，项目计数不跟着变。
  const yesterday = NOW - 26 * 3_600_000
  const items = [
    card('a1', 'attention', { workspaceKey: '/p/a' }),
    card('a2', 'attention', { workspaceKey: '/p/b' }),
    card('r1', 'running', { workspaceKey: '/p/a' }),
    card('q1', 'queued', { workspaceKey: '/p/b' }),
    card('d1', 'done', { workspaceKey: '/p/a' }),
    card('old', 'done', { workspaceKey: '/p/c', since: yesterday }),
    card('gone', 'running', { workspaceKey: '/p/c', session: { id: 'gone', archived: true } }),
  ]
  assert.deepEqual(overviewFilterCounts(items, { now: NOW }), {
    total: 4,
    lanes: { attention: 2, running: 1, queued: 0, done: 1 },
    projects: { '/p/a': 3, '/p/b': 1 },
  })
  const queue = overviewFilterCounts(items, { now: NOW, capabilities: { sessionQueue: true } })
  assert.equal(queue.total, 5)
  assert.equal(queue.lanes.queued, 1)
  assert.equal(queue.projects['/p/b'], 2)
  // 与主区的统计一致。
  const model = buildOverviewModel(items, { now: NOW, groupBy: 'lane', lanes: ['running'] })
  assert.equal(model.total, 4)
  assert.deepEqual(model.counts, overviewFilterCounts(items, { now: NOW }).lanes)
})

test('overview switches to a virtual list above 30 rows', () => {
  assert.equal(OVERVIEW_VIRTUALIZE_THRESHOLD, 30)
  assert.equal(overviewShouldVirtualize(30), false)
  assert.equal(overviewShouldVirtualize(31), true)
})

// smoke-board-ui 偶发“点 fx-standard 的允许却打开了会话页”（第 4 期查清）：按下按钮与松开之间版面移动，
// click 被派发到两者的共同祖先（卡片 / 行本身）→ 打开会话。行与手机卡片都记住按下是否落在控件上，
// 这样的点击永远不当作“打开”。（泳道冻结溢出摘要的那一层随泳道退役。）
test('a press that starts on a row or card control never opens the session', async () => {
  for (const file of ['src/features/board/AgentCard.tsx', 'src/features/board/OverviewRow.tsx']) {
    const source = await readFile(file, 'utf8')
    assert.match(
      source,
      /onPointerDownCapture=\{\(event\) => \{\s*pressOnControlRef\.current = interactiveTarget\(event\.target\)/,
      file,
    )
    assert.match(
      source,
      /if \(pressedControl \|\| interactiveTarget\(event\.target\)\) return\s*open\(\)/,
      file,
    )
    // 按钮里的图标是 SVG：判断控件时必须按 Element（A3 查清：按 HTMLElement 判断时点 ⋯ 的圆点会打开会话）。
    assert.match(source, /target instanceof Element &&\s*Boolean\(target\.closest\(/, file)
    assert.doesNotMatch(
      source,
      /target instanceof HTMLElement &&\s*Boolean\(target\.closest\(/,
      file,
    )
  }
})

test('card presentation shows only data the runtime provides', () => {
  assert.equal(guardReasonSummary('命令守卫拦截：改写远端分支'), '改写远端分支')
  assert.equal(guardReasonSummary('命令守卫拦截: 递归删除。请确认'), '递归删除')
  assert.equal(guardReasonSummary(''), '')
  assert.equal(projectLabel('/home/dev/billing-service/'), 'billing-service')
  assert.equal(projectLabel('C:\\work\\pisper'), 'pisper')
  assert.equal(shortPath('/home/dev/pisper'), '~/pisper')
  assert.equal(shortPath('/home/dev/a/b/c/d'), '~/…/c/d')
  assert.equal(modelShortLabel('openai/gpt-5.6-luna'), 'gpt-5.6-luna')
  assert.equal(modelShortLabel(undefined), '')
  assert.equal(messageCount({ session: { id: 'a', messageCount: 0 } }), 0)
  assert.equal(messageCount({ session: { id: 'a', messageCount: 7 } }), 7)
  assert.equal(boardSubAgent({ session: { id: 'a' } }), null)
  assert.deepEqual(
    boardSubAgent({
      session: { id: 'a', agents: [{ taskName: '生成发票测试夹具' }, { canonicalName: 'b' }] },
    }),
    { name: '生成发票测试夹具', extra: 1 },
  )
  assert.equal(boardCardTone({ status: 'approval' }), 'attn')
  assert.equal(boardCardTone({ status: 'failed' }), 'fail')
  assert.equal(boardEdgeWidth({ lane: 'attention', status: 'failed' }), 4)
  assert.equal(boardEdgeWidth({ lane: 'running', status: 'running' }), 3)
  assert.equal(boardEdgeWidth({ lane: 'done', status: 'failed' }), 2)
})

test('board single-letter shortcuts are a separate table and never relax app-level rules', () => {
  assert.deepEqual(validateBoardShortcutBindings(DEFAULT_BOARD_SHORTCUTS), DEFAULT_BOARD_SHORTCUTS)
  assert.ok(Object.isFrozen(BOARD_SHORTCUT_ACTIONS))
  assert.ok(Object.isFrozen(DEFAULT_BOARD_SHORTCUTS))
  // 应用级绑定依旧拒绝裸字母键。
  assert.throws(() => validateShortcutBindings({ ...DEFAULT_SHORTCUTS, voiceInput: 'KeyJ' }))
  for (const binding of ['Alt+KeyJ', 'Shift+KeyJ', 'Digit1', 'Enter', 'ArrowDown', 'Slash']) {
    assert.throws(() =>
      validateBoardShortcutBindings({ ...DEFAULT_BOARD_SHORTCUTS, boardNext: binding }),
    )
  }
  assert.throws(
    () => validateBoardShortcutBindings({ ...DEFAULT_BOARD_SHORTCUTS, boardDeny: 'KeyA' }),
    (error) => error.code === 'conflict',
  )
  assert.equal(
    findBoardShortcutConflict(DEFAULT_BOARD_SHORTCUTS, 'boardNext', 'KeyA'),
    'boardApprove',
  )
  assert.deepEqual(normalizeBoardShortcutBindings('broken'), DEFAULT_BOARD_SHORTCUTS)
  // 规格 2.8：泳道退役后 H / L（切泳道）不再是动作；旧版存下的表读进来时丢掉这两项，其余绑定原样保留，
  // H、L 空出来可以改绑给别的动作。（旧版读新表时缺的两项按默认补回，见 normalizeBoardShortcutBindings 的同一规则。）
  assert.deepEqual(
    [...BOARD_SHORTCUT_ACTIONS],
    [
      'boardNext',
      'boardPrevious',
      'boardApprove',
      'boardDeny',
      'boardRetry',
      'boardTriage',
      'boardToggleGrouping',
    ],
  )
  assert.deepEqual(
    normalizeBoardShortcutBindings({
      ...DEFAULT_BOARD_SHORTCUTS,
      boardLaneLeft: 'KeyH',
      boardLaneRight: 'KeyL',
      boardTriage: 'KeyX',
    }),
    { ...DEFAULT_BOARD_SHORTCUTS, boardTriage: 'KeyX' },
  )
  assert.equal(
    validateBoardShortcutBindings({ ...DEFAULT_BOARD_SHORTCUTS, boardTriage: 'KeyH' }).boardTriage,
    'KeyH',
  )
  // 旧数据缺项时补默认；默认键已被占用则补为未绑定。
  const migrated = normalizeBoardShortcutBindings({ boardNext: 'KeyK' })
  assert.equal(migrated.boardNext, 'KeyK')
  assert.equal(migrated.boardPrevious, null)
  assert.equal(migrated.boardApprove, 'KeyA')
  assert.equal(boardShortcutFromEvent({ code: 'KeyJ', key: 'j' }), 'KeyJ')
  assert.equal(boardShortcutFromEvent({ code: 'KeyJ', ctrlKey: true }), null)
  assert.equal(boardShortcutFromEvent({ code: 'KeyJ', isComposing: true }), null)
  assert.equal(matchesBoardShortcut({ key: 'a' }, 'KeyA'), true)
  assert.equal(matchesBoardShortcut({ code: 'KeyA', altKey: true }, 'KeyA'), false)
  assert.equal(matchesBoardShortcut({ code: 'KeyA' }, null), false)
})

async function boardSources() {
  const directory = join(root, 'src/features/board')
  const files = (await readdir(directory)).filter((file) => /\.tsx?$/.test(file))
  return Promise.all(
    files.map(async (file) => [file, await readFile(join(directory, file), 'utf8')]),
  )
}

test('only the confirmation layer may approve critical requests', async () => {
  const sources = await boardSources()
  const callers = sources
    .filter(([, source]) => /confirmCritical:\s*true/.test(source))
    .map(([file]) => file)
  assert.deepEqual(callers, ['useBoardController.ts'])
  const controller = sources.find(([file]) => file === 'useBoardController.ts')[1]
  // 允许 critical 的唯一路径是 confirmApproval（按住 0.6 秒或层内回车后调用）。
  const confirmBody = controller.slice(controller.indexOf('const confirmApproval'))
  assert.match(confirmBody.slice(0, 900), /confirmCritical: true/)
  // 侧栏的快捷处理（取代舰队计量浮层）只对 sidebar-model 判定为普通批准的行一步允许，
  // 且永远不带确认：critical、详情未到或截断的一律“打开”进会话。
  const sidebar = sources.find(([file]) => file === 'SidebarSessions.tsx')[1]
  const sidebarApprovals = [...sidebar.matchAll(/\.approve\([^)]*\)/g)].map((match) => match[0])
  assert.deepEqual(sidebarApprovals, ['.approve(row.id, approvalId, { confirmCritical: false })'])
  assert.match(sidebar, /if \(row\.quickAction\.kind !== 'decide'\) \{\s*onOpenSession\(row\.id\)/)

  // 控制器内部的 approveNow 第三个参数就是 confirmCritical：只有 confirmApproval 能传 true，
  // 卡片按钮与 A 键都传 false；critical 或详情未到时 A 只打开确认层。
  const approveNowCalls = [...controller.matchAll(/approveNow\(item, approval, (\w+)\)/g)]
  assert.deepEqual(
    approveNowCalls.map((match) => [
      match[1],
      match.index > controller.indexOf('const confirmApproval'),
    ]),
    [
      ['false', false],
      ['true', true],
    ],
  )
  // critical 不一步允许：桌面全部代理（criticalInSession）进会话处理（规格 3.5“全部代理只显示打开处理”），
  // 手机看板（A4 前）只打开确认层。Runtime 回 confirmation-required 时同样走这条路。
  assert.match(
    controller,
    /onApprove: \(item, approval\) => \{[\s\S]{0,200}if \(approval\.severity === 'critical'\) \{\s*review\(item, approval\)\s*return/,
  )
  assert.match(
    controller,
    /const review = useCallback\(\s*\(item: BoardItem, approval: BoardApproval \| null\) => \{\s*if \(criticalInSession\) openSession\(item\.id\)\s*else openConfirm\(item, approval\)/,
  )
  assert.match(controller, /result\.reason === 'confirmation-required'\) review\(item, approval\)/)
  // 键盘 A / D 与行上按钮、侧栏同一条规则：只有 sidebarQuickAction 判为 decide 的行一步允许 / 拒绝，
  // 处理的是它指定的那条审批（不是盲取 approvals[0]）；其余桌面什么也不做，手机 A 只打开确认层。
  // 拒绝在桌面同样受此限（高风险、截断、详情未到都不能在列表上被拒掉，规格 3.5）。
  assert.match(
    controller,
    /function decidableApproval\(item: BoardItem\): BoardApproval \| null \{\s*const quick = sidebarQuickAction\(item\)\s*if \(quick\.kind !== 'decide'\) return null\s*return item\.approvals\.find\(\(entry\) => entry\.id === quick\.approvalId\) \?\? null/,
  )
  assert.match(
    controller,
    /matchesBoardShortcut\(event, bindings\.boardApprove\)[\s\S]{0,600}const approval = decidableApproval\(item\)\s*if \(approval\) handlers\.onApprove\(item, approval\)\s*else if \(!criticalInSession\) openConfirm\(item, item\.approvals\[0\] \?\? null\)/,
  )
  assert.match(
    controller,
    /matchesBoardShortcut\(event, bindings\.boardDeny\)[\s\S]{0,400}const approval = criticalInSession \? decidableApproval\(item\) : \(item\.approvals\[0\] \?\? null\)\s*if \(approval\) handlers\.onDeny\(item, approval\)/,
  )
  // 一步允许 / 拒绝只有这两处调用，键盘没有绕过 decidableApproval 的第二条路径。
  assert.equal(controller.match(/handlers\.onApprove\(/g)?.length, 1)
  assert.equal(controller.match(/handlers\.onDeny\(/g)?.length, 1)
  const page = sources.find(([file]) => file === 'BoardPage.tsx')[1]
  assert.match(page, /criticalInSession: true/)
  // 按项目分组的上移 / 下移与首末判断只在画出来的项目分区之间（登记了但今天没有代理、被筛选清空的项目不参与），
  // 每次点击都看得见变化。
  assert.match(
    page,
    /const projectKeys = useMemo\(\s*\(\) =>\s*model\.sections\.flatMap\(\(section\) =>\s*section\.kind === 'project' && projectByKey\.has\(section\.key\)/,
  )
  assert.match(page, /moveWorkspace\(project\.key, offset, projectKeys\)/)
  // 全部代理行：一步“拒绝 / 允许”只给 sidebar-model 判定为普通批准（详情已到、未截断、standard）的行，
  // 其余一律“打开处理”；行本身只把动作交回控制器，不直接调用批准接口。
  const row = sources.find(([file]) => file === 'OverviewRow.tsx')[1]
  assert.match(row, /const quick = sidebarQuickAction\(item\)/)
  assert.match(row, /if \(quick\.kind === 'decide' && approval\) \{/)
  assert.match(row, /\} else actions = openButton/)
  assert.doesNotMatch(row, /\.approve\(|approveBoardApproval|useBoardActions|confirmCritical/)
  // 允许 / 拒绝 / 重试只作用于聚焦的行或逐个处理中的行（规格 2.8）；焦点在空白处
  // （body、列表空白）时按键不能落到“第一行”上。J/K、T、G 不受此限。
  assert.match(controller, /const acting = onCard \|\| Boolean\(triage\)/)
  for (const binding of ['boardApprove', 'boardDeny', 'boardRetry'])
    assert.match(
      controller,
      new RegExp(`acting &&\\s+matchesBoardShortcut\\(event, bindings\\.${binding}\\)`),
      binding,
    )
  // 动作键只在上面三处匹配，不存在绕过 acting 的第二条路径。
  assert.equal(
    controller.match(/matchesBoardShortcut\(event, bindings\.board(?:Approve|Deny|Retry)\)/g)
      ?.length,
    3,
  )
  // 确认层是 confirmApproval 唯一的调用方：只剩手机底部表单（桌面全部代理不再有确认层，高风险进会话）。
  for (const [file, source] of sources) {
    const uses = source.match(/controller\.confirmApproval\(\)/g)?.length || 0
    if (file === 'PhoneBoard.tsx') {
      assert.equal(uses, 1, file)
      assert.match(
        source,
        /<ApprovalConfirm[\s\S]*?onConfirm=\{\(\) => void controller\.confirmApproval\(\)\}/,
      )
    } else assert.equal(uses, 0, file)
  }
  // 手机看板的确认层（卡片、键盘 A、逐个处理）：截断的预览不交给确认层，
  // 确认层因此显示“正在读取审批详情”且确认键禁用，直到 /live 给出完整参数。
  const mainController = sources.find(([file]) => file === 'useBoardController.ts')[1]
  assert.match(
    mainController,
    /approval: shownApproval && !shownApproval\.truncated \? shownApproval : null/,
  )
  // 按住 0.6 秒才确认；按钮的点击（空格/回车）不能绕过按住时长，层内 ↵ 不在“取消”上时才确认。
  const confirmLayer = sources.find(([file]) => file === 'ApprovalConfirm.tsx')[1]
  assert.match(confirmLayer, /export const APPROVAL_HOLD_MS = 600\b/)
  assert.match(confirmLayer, /onClick=\{\(event\) => event\.preventDefault\(\)\}/)
  assert.match(confirmLayer, /closest\('\[data-approval-cancel\]'\)\) return/)
  // 卡片只把动作交回控制器，不直接调用批准接口。
  const card = sources.find(([file]) => file === 'AgentCard.tsx')[1]
  assert.doesNotMatch(card, /\.approve\(|approveBoardApproval|useBoardActions/)
  // 看板之外没有任何代码能调用看板的批准动作。
  const outside = []
  const scan = async (directory) => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isDirectory()) {
        if (path !== join(root, 'src/features/board')) await scan(path)
      } else if (/\.tsx?$/.test(entry.name)) {
        const source = await readFile(path, 'utf8')
        if (/useBoardActions\(|approveBoardApproval/.test(source)) outside.push(path)
      }
    }
  }
  await scan(join(root, 'src'))
  assert.deepEqual(outside, [])
})

test('board UI keeps runs alive and reuses the existing create-and-send API', async () => {
  const launch = await readFile(join(root, 'src/features/board/board-launch.ts'), 'utf8')
  assert.match(launch, /chatApi\.createSession\(/)
  assert.match(launch, /chatApi\.openStream\(/)
  assert.match(launch, /ownBoardStream\(sessionId, 'launch'/)
  // 看板页本身不直接 fetch，也不启动自己的轮询。
  for (const [file, source] of await boardSources()) {
    if (!/\.tsx$/.test(file)) continue
    assert.doesNotMatch(source, /\bfetch\(/, `${file} must not call fetch`)
    assert.doesNotMatch(source, /refetchInterval/, `${file} must not poll on its own`)
  }
})

test('full access stays in the current draft and a detached dispatch failure reaches a global toast', async () => {
  const [launchConsole, overlay, app, toast] = await Promise.all(
    [
      'src/features/board/LaunchConsole.tsx',
      'src/features/board/DispatchOverlay.tsx',
      'src/App.tsx',
      'src/components/ui/toast.tsx',
    ].map((file) => readFile(join(root, file), 'utf8')),
  )
  // 审批模式芯片：安全模式照常记住，完全访问只放进本实例的草稿状态；派发时立即回到安全模式。
  assert.match(launchConsole, /setFullAccess\(mode === 'full-access'\)/)
  assert.match(
    launchConsole,
    /if \(mode !== 'full-access'\) defaults\.rememberLaunch\(\{ executionMode: mode \}\)/,
  )
  assert.match(launchConsole, /setFullAccess\(false\)\s+defaults\.rememberLaunch\(/)
  // 失败时草稿先放回 composer-drafts（并同步给其他启动台实例），已卸载的实例把错误交给宿主。
  assert.match(
    launchConsole,
    /restoreLaunchDraft\(prompt\)[\s\S]{0,400}if \(mountedRef\.current\) setError\(\{ message, noModel: false \}\)\s+else onDetachedFailure\?\.\(message\)/,
  )
  assert.match(overlay, /onDetachedFailure=\{onDetachedFailure\}/)
  assert.match(app, /onDetachedFailure=\{dispatchFailedAfterClose\}/)
  assert.match(
    app,
    /action: \{ label: t\('board:dispatch\.reopen'\), onAction: \(\) => setDispatchOpen\(true\) \}/,
  )
  assert.match(toast, /<ToastPrimitive\.Action altText=\{action\.label\} asChild>/)
})
