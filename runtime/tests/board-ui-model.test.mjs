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
  DONE_LANE_LIMIT,
  LANE_MIN_WIDTH,
  LANE_MIN_WIDTH_FLOOR,
  LANE_RAIL_WIDTH,
  TABLET_COMPACT_LANES,
  TABLET_FULL_LANES_QUERY,
  boardLaneLayout,
  hiddenBelowCount,
  laneGridTemplate,
  laneMinWidth,
  laneShouldVirtualize,
  visibleLaneItems,
} from '../../src/features/board/lane-layout.ts'
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
const counts = (attention, running, queued, done) => ({ attention, running, queued, done })

test('empty lanes collapse to 56px rails while non-empty lanes share weighted widths', () => {
  const layout = boardLaneLayout({
    counts: counts(2, 0, 0, 3),
    lanes: ['attention', 'running', 'done'],
  })
  assert.deepEqual(
    layout.map(({ lane, mode, railReason }) => [lane, mode, railReason]),
    [
      ['attention', 'expanded', null],
      ['running', 'rail', 'empty'],
      ['done', 'expanded', null],
    ],
  )
  assert.equal(
    laneGridTemplate(layout, 1408),
    `minmax(${LANE_MIN_WIDTH}px, 1.24fr) ${LANE_RAIL_WIDTH}px minmax(${LANE_MIN_WIDTH}px, 0.9fr)`,
  )
  // 点开空泳道只是临时展开；手动折叠的已完成泳道即使有卡片也收成竖条。
  const peeked = boardLaneLayout({
    counts: counts(0, 0, 0, 4),
    lanes: ['attention', 'running', 'done'],
    peeked: ['attention'],
    collapsed: ['done'],
  })
  assert.deepEqual(
    peeked.map(({ mode, railReason }) => [mode, railReason]),
    [
      ['expanded', null],
      ['rail', 'empty'],
      ['rail', 'collapsed'],
    ],
  )
})

test('tablet compact lanes rail queued and done but still open on demand', () => {
  // 规格 7.6：手机 App 平板宽度 960–1179 时只展开需要你与运行中，排队与已完成收成竖条。
  assert.deepEqual([...TABLET_COMPACT_LANES], ['queued', 'done'])
  assert.equal(TABLET_FULL_LANES_QUERY, '(min-width: 1180px)')
  const compact = boardLaneLayout({
    counts: counts(2, 1, 1, 3),
    compact: TABLET_COMPACT_LANES,
  })
  assert.deepEqual(
    compact.map(({ lane, mode, railReason }) => [lane, mode, railReason]),
    [
      ['attention', 'expanded', null],
      ['running', 'expanded', null],
      ['queued', 'rail', 'compact'],
      ['done', 'rail', 'compact'],
    ],
  )
  // 竖条点开是临时展开；手动折叠的已完成仍按折叠处理。
  const peeked = boardLaneLayout({
    counts: counts(2, 1, 1, 3),
    lanes: ['attention', 'running', 'done'],
    compact: TABLET_COMPACT_LANES,
    peeked: ['done'],
  })
  assert.deepEqual(
    peeked.map(({ mode }) => mode),
    ['expanded', 'expanded', 'expanded'],
  )
  const collapsed = boardLaneLayout({
    counts: counts(2, 1, 0, 3),
    lanes: ['attention', 'running', 'done'],
    compact: TABLET_COMPACT_LANES,
    collapsed: ['done'],
  })
  assert.equal(collapsed[2].railReason, 'collapsed')
})

test('a lone expanded lane fills the remaining width instead of a fraction of it', () => {
  // 今天没有活动时只展开已完成泳道：.9fr 单独使用只会分走 90% 的剩余空间，右侧留出一条空白。
  const layout = boardLaneLayout({
    counts: counts(0, 0, 0, 0),
    lanes: ['attention', 'running', 'done'],
    peeked: ['done'],
  })
  assert.equal(
    laneGridTemplate(layout, 1408),
    `${LANE_RAIL_WIDTH}px ${LANE_RAIL_WIDTH}px minmax(${LANE_MIN_WIDTH}px, 1fr)`,
  )
})

test('lane minimum width shrinks at 1080 so four expanded lanes never scroll sideways', () => {
  const four = boardLaneLayout({ counts: counts(1, 1, 1, 1) })
  // 1080 宽窗口减去 16px 外边距：四条泳道各自让到等分宽度。
  assert.equal(laneMinWidth(four, 1048), Math.floor((1048 - 3 * 12) / 4))
  assert.equal(laneMinWidth(four, 1408), LANE_MIN_WIDTH)
  assert.equal(laneMinWidth(four, 600), LANE_MIN_WIDTH_FLOOR)
  assert.equal(laneMinWidth([], 0), LANE_MIN_WIDTH)
})

test('done lane caps at 20 cards and long lanes switch to virtual lists', () => {
  const items = Array.from({ length: 31 }, (_, index) => index)
  assert.equal(visibleLaneItems('done', items).length, DONE_LANE_LIMIT)
  assert.equal(visibleLaneItems('running', items).length, 31)
  assert.equal(laneShouldVirtualize(30), false)
  assert.equal(laneShouldVirtualize(31), true)
  assert.equal(hiddenBelowCount([100, 380, 420, 900], 400), 2)
})

// smoke-board-ui 偶发“点 fx-standard 的允许却打开了会话页”（第 4 期查清）：按下按钮时卡片获得焦点，
// 泳道重新渲染时把早已排好的溢出摘要收起，滚到底的滚动区变高 36px，按钮在按下与松开之间下移，
// click 被派发到两者的共同祖先（卡片正文）→ 打开会话。两层保护：
// 1) 泳道在按住期间冻结屏幕上的溢出摘要，松开或取消后再显示最新测量；
// 2) 卡片记住按下是否落在控件上，这样的点击永远不当作“打开卡片”。
test('a press that starts on a card control never opens the session, and lanes hold still while pressed', async () => {
  const lane = await readFile('src/features/board/LaneColumn.tsx', 'utf8')
  assert.match(lane, /const overflowCount = frozenHidden \?\? hidden/)
  assert.match(lane, /\{overflowCount > 0 && \(/)
  assert.doesNotMatch(lane, /\{hidden > 0 && \(/, 'the footer must render the frozen count')
  assert.match(
    lane,
    /onPointerDownCapture=\{\(\) => \{\s*pressedRef\.current = true\s*setFrozenHidden\(shownOverflowRef\.current\)/,
  )
  for (const type of ['pointerup', 'pointercancel'])
    assert.match(lane, new RegExp(`window\\.addEventListener\\('${type}', release, true\\)`))
  const card = await readFile('src/features/board/AgentCard.tsx', 'utf8')
  assert.match(
    card,
    /onPointerDownCapture=\{\(event\) => \{\s*pressOnControlRef\.current = interactiveTarget\(event\.target\)/,
  )
  assert.match(
    card,
    /if \(pressedControl \|\| interactiveTarget\(event\.target\)\) return\s*open\(\)/,
  )
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
  // 舰队计量只负责跳转，永远不批准。
  const fleet = await readFile(join(root, 'src/app/FleetMeter.tsx'), 'utf8')
  assert.doesNotMatch(fleet, /\.approve\(|approveBoardApproval/)

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
  assert.match(
    controller,
    /onApprove: \(item, approval\) => \{[\s\S]{0,160}if \(approval\.severity === 'critical'\) \{\s*openConfirm\(item, approval\)\s*return/,
  )
  assert.match(
    controller,
    /matchesBoardShortcut\(event, bindings\.boardApprove\)[\s\S]{0,240}if \(!approval \|\| approval\.severity === 'critical'\) openConfirm\(item, approval \?\? null\)/,
  )
  // 允许 / 拒绝 / 重试只作用于聚焦的卡片或逐个处理中的卡片（规格 2.8）；焦点在空白处
  // （body、泳道空白）时按键不能落到“第一张卡片”上。J/K/H/L、T、G 不受此限。
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
  // 确认层是 confirmApproval 唯一的调用方（桌面对话框与手机底部表单共用）。
  for (const [file, source] of sources) {
    const uses = source.match(/controller\.confirmApproval\(\)/g)?.length || 0
    if (file === 'BoardPage.tsx' || file === 'PhoneBoard.tsx') {
      assert.equal(uses, 1, file)
      assert.match(
        source,
        /<ApprovalConfirm[\s\S]*?onConfirm=\{\(\) => void controller\.confirmApproval\(\)\}/,
      )
    } else assert.equal(uses, 0, file)
  }
  // 看板主确认层（卡片、键盘 A、逐个处理）与泳道条一致：截断的预览不交给确认层，
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
