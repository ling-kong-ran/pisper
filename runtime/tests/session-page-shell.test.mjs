// 第 3 期会话页外壳（规格 4.1 / 4.2 / 4.4）：泳道条的收缩顺序与切换顺序、检查器偏好，
// 以及“critical 审批在任何入口都不能一步批准”在泳道条、代理面板与会话审批面板上的源码守卫。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import test from 'node:test'
import {
  STRIP_ATTENTION_ACTION_CHIP_WIDTH,
  STRIP_ATTENTION_ACTION_ONE_LINE_WIDTH,
  STRIP_ATTENTION_CHIP_WIDTH,
  STRIP_ATTENTION_KEEP,
  STRIP_CURRENT_CHIP_MAX_WIDTH,
  STRIP_RUNNING_TITLE_WIDTH,
  adjacentStripSession,
  effectiveStripCurrent,
  stripAttentionChipWidth,
  stripCurrentChipWidth,
  stripLayout,
  stripOrder,
  stripRequiredWidth,
} from '../../src/features/board/strip-layout.ts'

const root = resolve(import.meta.dirname, '../..')
const read = (path) => readFile(resolve(root, path), 'utf8')
const base = { attention: 2, running: 3, currentRunning: true, queued: false, done: true }

test('the lane strip shrinks in the four spec steps and never drops attention titles', () => {
  const wide = stripLayout({ ...base, width: 4000, compact: false })
  // actionChipWidth：没有带内联键的芯片时就是基础宽度。
  assert.deepEqual(wide, {
    level: 0,
    runningMode: 'full',
    attentionLines: 2,
    attentionShown: 2,
    actionChipWidth: 220,
  })
  // 逐步收窄：先运行中只留标题，再收成胶囊（当前会话芯片保留），再需要你单行，最后第 3 个起收进胶囊。
  const levels = []
  for (let width = 4000; width > 0; width -= 20) {
    const layout = stripLayout({ ...base, attention: 4, width, compact: false })
    if (levels.at(-1)?.level !== layout.level) levels.push(layout)
  }
  assert.deepEqual(
    levels.map((layout) => layout.level),
    [0, 1, 2, 3, 4],
  )
  assert.deepEqual(levels[1], {
    level: 1,
    runningMode: 'title',
    attentionLines: 2,
    attentionShown: 4,
    actionChipWidth: 220,
  })
  assert.equal(levels[2].runningMode, 'capsule')
  assert.equal(levels[3].attentionLines, 1)
  assert.equal(levels[4].attentionShown, STRIP_ATTENTION_KEEP)
  // 每一步都比上一步更窄。
  for (let index = 1; index < levels.length; index += 1)
    assert.ok(
      stripRequiredWidth({ ...base, attention: 4, width: 0, compact: false }, levels[index]) <
        stripRequiredWidth({ ...base, attention: 4, width: 0, compact: false }, levels[index - 1]),
    )
  // 再窄也停在第 4 步：需要你芯片仍然带标题（不退化成图标 + 数字）。
  const tiny = stripLayout({ ...base, attention: 4, width: 10, compact: false })
  assert.equal(tiny.level, 4)
  assert.equal(tiny.attentionShown, 2)
  // 36px 单行模式下需要你芯片本来就只有一行。
  assert.equal(stripLayout({ ...base, width: 4000, compact: true }).attentionLines, 1)
  // 只有两个需要你时第 4 步不收胶囊。
  assert.equal(stripLayout({ ...base, attention: 2, width: 10, compact: false }).attentionShown, 2)
})

test('chips with inline actions and the current chip get room for their titles', () => {
  // 收缩判断按基础宽度（两行 220、单行 180）；带 [拒绝][允许] 的标准审批芯片只用剩余宽度放宽，
  // 最多到规格上限 280（单行 220），不会因为放宽而多收一步。
  assert.equal(stripAttentionChipWidth(2), STRIP_ATTENTION_CHIP_WIDTH)
  const roomy = stripLayout({
    ...base,
    attentionActions: [true, false],
    width: 4000,
    compact: false,
  })
  assert.equal(roomy.level, 0)
  assert.equal(roomy.actionChipWidth, STRIP_ATTENTION_ACTION_CHIP_WIDTH)
  const plainLevels = []
  const actionLevels = []
  for (let width = 2000; width > 0; width -= 10) {
    plainLevels.push(stripLayout({ ...base, attention: 3, width, compact: false }).level)
    const layout = stripLayout({
      ...base,
      attention: 3,
      attentionActions: [false, true, true],
      width,
      compact: false,
    })
    actionLevels.push(layout.level)
    const ceiling =
      layout.attentionLines === 2
        ? STRIP_ATTENTION_ACTION_CHIP_WIDTH
        : STRIP_ATTENTION_ACTION_ONE_LINE_WIDTH
    assert.ok(layout.actionChipWidth >= stripAttentionChipWidth(layout.attentionLines))
    assert.ok(layout.actionChipWidth <= ceiling)
  }
  assert.deepEqual(actionLevels, plainLevels)
  // 当前会话的芯片按标题估算：短标题不被压到 140 以下的截断，长标题最多 220。
  assert.equal(stripCurrentChipWidth('改'), STRIP_RUNNING_TITLE_WIDTH)
  assert.ok(stripCurrentChipWidth('重构登录流程') > STRIP_RUNNING_TITLE_WIDTH)
  assert.equal(stripCurrentChipWidth('重构登录流程'.repeat(4)), STRIP_CURRENT_CHIP_MAX_WIDTH)
  const titleOnly = { runningMode: 'title', attentionLines: 2, attentionShown: 2 }
  const withCurrent = stripRequiredWidth(
    { ...base, currentWidth: 180, width: 0, compact: false },
    titleOnly,
  )
  const withoutCurrent = stripRequiredWidth(
    { ...base, currentRunning: false, width: 0, compact: false },
    titleOnly,
  )
  assert.equal(withCurrent - withoutCurrent, 180 - STRIP_RUNNING_TITLE_WIDTH)
})

test('option-arrow switching follows the strip order and wraps', () => {
  const items = [
    { id: 'd1', lane: 'done' },
    { id: 'r1', lane: 'running' },
    { id: 'a1', lane: 'attention' },
    { id: 'q1', lane: 'queued' },
    { id: 'a2', lane: 'attention' },
  ]
  const order = stripOrder(items)
  assert.deepEqual(
    order.map((item) => item.id),
    ['a1', 'a2', 'r1', 'q1', 'd1'],
  )
  assert.equal(adjacentStripSession(order, 'a2', 1), 'r1')
  assert.equal(adjacentStripSession(order, 'a1', -1), 'd1')
  assert.equal(adjacentStripSession(order, 'd1', 1), 'a1')
  // 当前会话不在条上（例如更早的空闲会话）：↓ 去第一个，↑ 去最后一个。
  assert.equal(adjacentStripSession(order, 'elsewhere', 1), 'a1')
  assert.equal(adjacentStripSession(order, 'elsewhere', -1), 'd1')
  assert.equal(adjacentStripSession([{ id: 'only', lane: 'running' }], 'only', 1), '')
  assert.equal(adjacentStripSession([], 'x', 1), '')
})

// 路由在 transition 里提交：⌥↓ 之后 URL 已换、界面还停在原会话时立刻再按 ⌥↑，
// 必须以刚发出的目标为起点，回到原会话，而不是从原会话再算一次邻居（smoke-board-ui 7a 曾偶发失败）。
test('rapid option-arrow presses start from the switch that has not committed yet', async () => {
  const order = stripOrder([
    { id: 'a1', lane: 'attention' },
    { id: 'r1', lane: 'running' },
    { id: 'r2', lane: 'running' },
    { id: 'r3', lane: 'running' },
  ])
  const pending = { from: 'r2', to: 'r3' }
  // 界面仍是 r2：起点是 r3，⌥↑ 回到 r2；再按 ⌥↓ 去 r3 之后的 a1。
  assert.equal(effectiveStripCurrent('r2', pending), 'r3')
  assert.equal(adjacentStripSession(order, effectiveStripCurrent('r2', pending), -1), 'r2')
  assert.equal(adjacentStripSession(order, effectiveStripCurrent('r2', pending), 1), 'a1')
  // 切换提交之后，或者界面换到了别处，都以界面上的会话为准。
  assert.equal(effectiveStripCurrent('r3', pending), 'r3')
  assert.equal(effectiveStripCurrent('a1', pending), 'a1')
  assert.equal(effectiveStripCurrent('r2', null), 'r2')

  const [shell, strip] = await Promise.all([
    read('src/app/SessionShell.tsx'),
    read('src/features/board/SessionLaneStrip.tsx'),
  ])
  // 外壳的切换入口记下待提交的目标并据此去重；泳道条的键盘与点击都从同一个起点计算。
  assert.match(shell, /pendingSwitchRef\.current = \{ from, to: id \}/)
  assert.match(shell, /id === effectiveStripCurrent\(from, pendingSwitchRef\.current\)/)
  assert.match(shell, /resolveCurrentId=\{resolveCurrentId\}/)
  assert.match(strip, /adjacentStripSession\(ordered, switchBase\(\)/)
  assert.match(strip, /sessionId === switchBase\(\)/)
})

test('inspector preferences persist open, tab and width under a registered key', async () => {
  const values = new Map([
    [
      'pisper-session-inspector',
      JSON.stringify({
        state: { open: true, tab: 'side-chat', width: 9999, terminalView: 'processes' },
        version: 1,
      }),
    ],
  ])
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: (key) => values.delete(key),
  }
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: { localStorage: storage },
  })
  try {
    const {
      SESSION_INSPECTOR_MAX_WIDTH,
      SESSION_INSPECTOR_MIN_WIDTH,
      inspectorDisplayWidth,
      normalizeInspectorPreferences,
      useSessionInspectorStore: store,
    } = await import('../../src/features/chat/session-inspector-store.ts')
    const { BROWSER_PREFERENCE_KEYS } = await import('../../shared/browser-preferences.mjs')
    assert.ok(BROWSER_PREFERENCE_KEYS.includes('pisper-session-inspector'))
    // 侧聊是临时标签，恢复时回到“改动”；宽度夹在允许范围内。
    assert.equal(store.getState().open, true)
    assert.equal(store.getState().tab, 'changes')
    assert.equal(store.getState().width, SESSION_INSPECTOR_MAX_WIDTH)
    assert.equal(store.getState().terminalView, 'processes')
    store.getState().setTab('preview')
    store.getState().setWidth(100)
    assert.equal(store.getState().width, SESSION_INSPECTOR_MIN_WIDTH)
    assert.equal(JSON.parse(values.get('pisper-session-inspector')).state.tab, 'preview')
    assert.equal(normalizeInspectorPreferences({ open: false }).open, false)
    assert.deepEqual(normalizeInspectorPreferences('broken'), {
      open: true,
      tab: 'changes',
      width: 456,
      terminalView: 'terminal',
      changesView: 'session',
      diffMode: 'unified',
    })
    // 差异视图只认 unified / split，其余值回到统一视图。
    assert.equal(normalizeInspectorPreferences({ diffMode: 'split' }).diffMode, 'split')
    assert.equal(normalizeInspectorPreferences({ diffMode: 'side' }).diffMode, 'unified')
    // 窗口变窄只收窄显示宽度，对话列至少保留 480px，不改写偏好。
    assert.equal(inspectorDisplayWidth(600, 1400), 600)
    assert.equal(inspectorDisplayWidth(600, 1000), 512)
    assert.equal(inspectorDisplayWidth(600, 700), SESSION_INSPECTOR_MIN_WIDTH)
  } finally {
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow)
    else delete globalThis.window
  }
})

test('session page layout: lane strip above the chat, inspector overlay under 1280 or 760 tall', async () => {
  const [routes, workspace, header, focus, dock, page] = await Promise.all([
    read('src/app/SessionShell.tsx'),
    read('src/features/chat/SessionWorkspace.tsx'),
    read('src/features/chat/SessionPageHeader.tsx'),
    read('src/features/chat/FocusSession.tsx'),
    read('src/features/chat/ChatDock.tsx'),
    read('src/features/chat/ChatPage.tsx'),
  ])
  const session = routes
  assert.match(routes, /SESSION_COMPACT_QUERY = '\(max-width: 1279px\), \(max-height: 760px\)'/)
  // 手机外壳保持原来的单会话视图；手机 App 平板走桌面布局但检查器始终是覆盖层。
  assert.match(session, /const pageLayout = !phone \|\| tablet/)
  assert.match(session, /overlayOnly: mobileApp/)
  assert.match(session, /\{pageLayout && \(\s*<SessionLaneStrip/)
  assert.match(session, /pageChrome=\{pageLayout \? pageChrome : undefined\}/)
  // 切换代理替换历史记录：浏览器后退直接回看板。
  assert.match(session, /navigate\(sessionPath\(id\), \{ replace: true \}\)/)
  const loaders = await read('src/app/route-elements.tsx')
  const loader = loaders.slice(
    loaders.indexOf('export async function sessionRoute'),
    loaders.indexOf('export async function moreRoute'),
  )
  // 外壳编排按需加载，聊天页仍是独立的懒加载入口；看板文案随会话路由注册。
  assert.match(loader, /import\('@\/app\/SessionShell'\)/)
  assert.match(loader, /import\('@\/features\/chat\/ChatPage'\)/)
  assert.match(loader, /ensureBoardMessages\(\)/)
  // 页头 44px，⌘I 开关检查器；覆盖层 420px、无遮罩、--sh-float。
  assert.match(header, /className="flex h-11 min-w-0 shrink-0/)
  assert.match(header, /data-inspector-toggle/)
  assert.match(workspace, /export const INSPECTOR_SHORTCUT = 'Mod\+KeyI'/)
  assert.match(workspace, /data-inspector-overlay[\s\S]{0,200}shadow-float/)
  assert.doesNotMatch(workspace, /overlay-scrim|backdrop/)
  assert.match(workspace, /useBackLayer\(active && overlayMode && overlayOpen, 100, close\)/)
  // 页头只在会话页布局里替换旧顶栏；旧顶栏（手机外壳）保持不变。
  assert.match(focus, /const headerBlock = pageHeaderBlock \|\| \(/)
  assert.match(dock, /pageChrome \? \(\s*\/\/[^\n]*\n\s*<SessionWorkspace/)
  assert.match(
    page,
    /SessionPageChromeContext\.Provider value=\{routeMode \? \(pageChrome \?\? null\) : null\}/,
  )
})

test('critical approvals cannot be approved in one step from the strip, the panel or the session', async () => {
  const [strip, mini, panel, controller, toolApproval] = await Promise.all([
    read('src/features/board/SessionLaneStrip.tsx'),
    read('src/features/board/AgentMiniCard.tsx'),
    read('src/features/board/AgentPanel.tsx'),
    read('src/features/board/useBoardController.ts'),
    read('src/features/chat/ToolApproval.tsx'),
  ])
  // 泳道条、迷你卡片与代理面板只把动作交回 useStripApprovals，不直接调批准接口。
  for (const source of [strip, mini, panel])
    assert.doesNotMatch(source, /approveBoardApproval|useBoardActions|confirmCritical/)
  // critical、截断预览或详情未到时只有“查看”。
  assert.match(
    mini,
    /if \(!approval \|\| approval\.severity === 'critical' \|\| approval\.truncated\)\s*return \(\s*<Button[\s\S]{0,200}data-strip-review/,
  )
  const hook = controller.slice(controller.indexOf('export function useStripApprovals'))
  assert.match(
    hook,
    /if \(approval\.severity === 'critical' \|\| approval\.truncated\) \{\s*review\(item, approval\)\s*return/,
  )
  assert.equal(hook.match(/confirmCritical: true/g)?.length, 1)
  assert.match(
    hook.slice(hook.indexOf('const confirmStripApproval')),
    /^const confirmStripApproval = useCallback\(async \(\) => \{[\s\S]{0,400}confirmCritical: true/,
  )
  // 泳道条里唯一的 confirmStripApproval 调用方是确认层。
  assert.equal(strip.match(/confirmStripApproval\(\)/g)?.length, 1)
  assert.match(
    strip,
    /<ApprovalConfirm[\s\S]*?onConfirm=\{\(\) => void approvals\.confirmStripApproval\(\)\}/,
  )
  // 会话审批面板：critical 的“允许…”只打开确认层，确认层回调才真正批准。
  assert.match(toolApproval, /const critical = approvalSeverity\(approval\) === 'critical'/)
  assert.match(
    toolApproval,
    /onClick=\{\(\) => \(critical \? setConfirming\(true\) : resolve\(true\)\)\}/,
  )
  assert.match(
    toolApproval,
    /<ApprovalConfirm[\s\S]*?onConfirm=\{\(\) => \{\s*(?:\/\/[^\n]*\n\s*)?void resolve\(true\)/,
  )
})

test('the session strip reads the board store and keeps escape for overlays only', async () => {
  const [strip, panel] = await Promise.all([
    read('src/features/board/SessionLaneStrip.tsx'),
    read('src/features/board/AgentPanel.tsx'),
  ])
  // 数据只来自看板 store（同一个所有者）：不新开轮询、事件流或 fetch。
  assert.match(strip, /useBoardItems\('today'\)/)
  assert.doesNotMatch(strip, /fetch\(|setInterval|openBoardEvents|EventSource/)
  assert.match(strip, /export const AGENT_PANEL_SHORTCUT = 'Mod\+KeyJ'/)
  // ⌥↑ / ⌥↓ 只换路由，不批准也不返回；Esc 只关预览和面板。
  assert.match(strip, /event\.key !== 'ArrowUp' && event\.key !== 'ArrowDown'/)
  assert.doesNotMatch(strip, /Escape[\s\S]{0,120}(onOpenSession|navigate)/)
  assert.match(panel, /side="left"/)
  assert.match(panel, /width: 320/)
})

// —— 第 3 期守卫迁移：会话页的新布局守卫 ——

function stripItem(approval, overrides = {}) {
  return {
    id: 'fx-approval',
    session: { id: 'fx-approval', name: '部署修复' },
    title: '部署修复',
    cwd: '/work/app',
    workspaceKey: '/work/app',
    lane: 'attention',
    status: 'approval',
    handled: false,
    since: Date.now() - 30_000,
    startedAt: null,
    pinned: false,
    unread: true,
    approvals: approval ? [approval] : [],
    approvalsState: approval ? 'ready' : 'loading',
    severity: approval?.severity ?? null,
    activity: null,
    error: '',
    localRun: false,
    hasCard: true,
    tail: [],
    preview: '',
    changeStats: null,
    contextPct: null,
    agents: null,
    ...overrides,
  }
}

function boardApproval(overrides = {}) {
  return {
    id: 'approval-1',
    toolName: 'bash',
    toolCallId: 'call-1',
    args: { command: 'npm test' },
    command: 'npm test',
    path: '',
    reason: '',
    risk: 'low',
    severity: 'standard',
    createdAt: null,
    ...overrides,
  }
}

test('strip chips, agent panel rows and previews never offer a one-step critical approval', async () => {
  const [{ default: React }, { renderToStaticMarkup }, { i18n }, { boardMessages }, mini] =
    await Promise.all([
      import('react'),
      import('react-dom/server'),
      import('../../src/app/i18n.ts'),
      import('../../src/app/board-messages.ts'),
      import('../../src/features/board/AgentMiniCard.tsx'),
    ])
  // 会话路由随页注册看板命名空间；测试里同样注册，确认渲染的是真实按钮而不是键名。
  i18n.addResourceBundle('zh-CN', 'board', boardMessages.zh)
  i18n.addResourceBundle('en-US', 'board', boardMessages.en)
  const handlers = {
    onOpen() {},
    onApprove() {},
    onDeny() {},
    onReview() {},
    onRetry() {},
    isPending: () => false,
  }
  // 三个入口：泳道条芯片（StripItemActions）、⌘J 面板与胶囊列表的行（AgentMiniRow）、悬停预览（AgentMiniPreview）。
  const surfaces = (item) => [
    renderToStaticMarkup(React.createElement(mini.StripItemActions, { item, handlers })),
    renderToStaticMarkup(
      React.createElement(mini.AgentMiniRow, {
        item,
        handlers,
        now: Date.now(),
        current: false,
        onOpen() {},
        word: '待批准',
      }),
    ),
    renderToStaticMarkup(
      React.createElement(mini.AgentMiniPreview, {
        item,
        handlers,
        now: Date.now(),
        word: '待批准',
      }),
    ),
  ]
  for (const html of surfaces(stripItem(boardApproval()))) {
    assert.match(html, /data-strip-approve/)
    assert.match(html, /data-strip-deny/)
  }
  // critical、预览被截断或详情未到：每个入口都只有“查看”（打开确认层），没有允许 / 拒绝键。
  for (const item of [
    stripItem(boardApproval({ severity: 'critical', command: 'git push --force origin main' })),
    stripItem(boardApproval({ truncated: true, argsText: 'command: rm -r build …' })),
    stripItem(null),
  ])
    for (const html of surfaces(item)) {
      assert.doesNotMatch(html, /data-strip-approve|data-strip-deny/)
      assert.match(html, /data-strip-review/)
    }
})

test('strip rows and previews never set Chinese text in monospace', async () => {
  const [{ default: React }, { renderToStaticMarkup }, { i18n }, { boardMessages }, mini] =
    await Promise.all([
      import('react'),
      import('react-dom/server'),
      import('../../src/app/i18n.ts'),
      import('../../src/app/board-messages.ts'),
      import('../../src/features/board/AgentMiniCard.tsx'),
    ])
  i18n.addResourceBundle('zh-CN', 'board', boardMessages.zh)
  i18n.addResourceBundle('en-US', 'board', boardMessages.en)
  await i18n.changeLanguage('zh-CN')
  const handlers = {
    onOpen() {},
    onApprove() {},
    onDeny() {},
    onReview() {},
    onRetry() {},
    isPending: () => false,
  }
  const now = Date.now()
  const items = [
    // 已完成：结果摘要是中文正文，时间是相对时间（“N 分钟前”）。
    stripItem(null, {
      lane: 'done',
      status: 'done',
      approvalsState: 'ready',
      since: now - 40 * 60_000,
      preview: '已生成 THIRD_PARTY.md，列出 128 个依赖',
    }),
    // 需要你：“已等”是中文前缀，数值才等宽。
    stripItem(boardApproval({ command: 'npm test -- --watch' }), { since: now - 18 * 60_000 }),
    // 失败：错误行等宽，“可重试”无衬线。
    stripItem(null, {
      status: 'failed',
      approvalsState: 'ready',
      error: 'Error: exit 1',
      since: now - 60_000,
    }),
    // 运行中：尾巴等宽，“已运行”无衬线；当前会话的标记 12px。
    stripItem(null, {
      lane: 'running',
      status: 'running',
      approvalsState: 'ready',
      startedAt: now - 195_000,
      tail: ['✓ 42 passed'],
    }),
  ]
  const cjk = /[\u3400-\u9fff\uff00-\uffef]/
  for (const item of items) {
    const html = [
      renderToStaticMarkup(
        React.createElement(mini.AgentMiniRow, {
          item,
          handlers,
          now,
          current: true,
          onOpen() {},
          word: '状态',
        }),
      ),
      renderToStaticMarkup(
        React.createElement(mini.AgentMiniPreview, { item, handlers, now, word: '状态' }),
      ),
    ].join('')
    // 等宽节点（叶子 span / p / pre）的文字里不能有中文。
    for (const match of html.matchAll(
      /<(span|p|pre)[^>]*class="[^"]*font-mono[^"]*"[^>]*>([^<]*)</g,
    ))
      assert.doesNotMatch(match[2], cjk, `${item.status}: ${match[2]}`)
    assert.doesNotMatch(html, /text-\[11px\][^>]*>[^<]*[\u3400-\u9fff]/, item.status)
  }
  const done = renderToStaticMarkup(
    React.createElement(mini.AgentMiniRow, {
      item: items[0],
      handlers,
      now,
      current: false,
      onOpen() {},
      word: '已完成',
    }),
  )
  assert.match(done, /40 分钟前/)
})

test('the lane strip and the agent panel render board store data and open no connection of their own', async () => {
  const [strip, panel, mini, presentation, shell, items] = await Promise.all([
    read('src/features/board/SessionLaneStrip.tsx'),
    read('src/features/board/AgentPanel.tsx'),
    read('src/features/board/AgentMiniCard.tsx'),
    read('src/features/board/strip-presentation.ts'),
    read('src/app/SessionShell.tsx'),
    read('src/features/board/useBoardItems.ts'),
  ])
  const connection =
    /\bfetch\(|setInterval\(|EventSource|openBoardEvents|consumeEventStream|apiJson|useQuery\(|refetchInterval/
  for (const [file, source] of Object.entries({ strip, panel, mini, presentation, shell }))
    assert.doesNotMatch(source, connection, file)
  // 代理面板、迷你卡片与文案推导只拿数据类型，不在运行时导入看板接口、store 或会话数据层。
  for (const [file, source] of Object.entries({ panel, mini, presentation })) {
    assert.doesNotMatch(
      source,
      /^import (?!type\b)[^\n]*from '\.\/(?:board-api|board-store|board-model|useBoardItems|useSessionListQuery)'/m,
      file,
    )
    assert.doesNotMatch(source, /from '@\/features\/chat\//, file)
  }
  // 唯一的数据源：泳道条读 useBoardItems('today')（看板 store 的同一份目录与覆盖层），
  // 分组后整组交给 ⌘J 面板，面板不再自己读取。
  assert.equal(strip.match(/useBoardItems\(/g)?.length, 1)
  assert.match(strip, /const \{ items, capabilities, isLoading \} = useBoardItems\('today'\)/)
  assert.match(strip, /<AgentPanel[\s\S]*?lanes=\{lanes\}[\s\S]*?handlers=\{handlers\}/)
  assert.match(panel, /lanes: Record<BoardLane, BoardItem\[\]>/)
  assert.match(items, /from '\.\/board-store'/)
  assert.match(items, /const query = useSessionListQuery\(\)/)
  assert.doesNotMatch(items, connection)
})

test('escape never leaves the session page; returning to the board needs an explicit action', async () => {
  const sources = Object.fromEntries(
    await Promise.all(
      [
        'src/app/SessionShell.tsx',
        'src/app/SessionReturnBar.tsx',
        'src/features/board/SessionLaneStrip.tsx',
        'src/features/board/AgentPanel.tsx',
        'src/features/board/AgentMiniCard.tsx',
        'src/features/chat/SessionWorkspace.tsx',
        'src/features/chat/SessionInspector.tsx',
        'src/features/chat/SessionPageHeader.tsx',
        'src/features/chat/InspectorChanges.tsx',
        'src/features/chat/FocusSession.tsx',
      ].map(async (file) => [file, await read(file)]),
    ),
  )
  // 会话页外壳不监听键盘：返回只认返回键、⌘[（应用级快捷键）、看板标签与浏览器 / 原生返回。
  assert.doesNotMatch(sources['src/app/SessionShell.tsx'], /keydown|Escape|onKeyDown/)
  assert.doesNotMatch(sources['src/app/SessionReturnBar.tsx'], /keydown|Escape|onKeyDown/)
  // 任何 Esc 处理附近都不出现路由跳转或返回动作。
  for (const [file, source] of Object.entries(sources)) {
    for (const match of source.matchAll(/'Escape'/g)) {
      const handler = source.slice(Math.max(0, match.index - 200), match.index + 400)
      assert.doesNotMatch(
        handler,
        /navigate\(|history\.back|backToBoard|onBack\b|onOpenSession|requestBack|routeBack/,
        `${file} binds Escape near a navigation call`,
      )
    }
  }
  // 检查器覆盖层只登记为返回层（Esc / 原生返回先关它），不登记路由返回。
  assert.match(
    sources['src/features/chat/SessionWorkspace.tsx'],
    /useBackLayer\(active && overlayMode && overlayOpen, 100, close\)/,
  )
  // ⌘[ 是返回看板的默认键；Esc 不能被设成返回看板（校验拒绝，归一化退回默认值）。
  const shortcuts = await import('../../shared/shortcuts.mjs')
  assert.equal(shortcuts.DEFAULT_SHORTCUTS.backToBoard, 'Mod+BracketLeft')
  for (const binding of ['Escape', 'Mod+Escape'])
    assert.throws(
      () =>
        shortcuts.validateShortcutBindings({
          ...shortcuts.DEFAULT_SHORTCUTS,
          backToBoard: binding,
        }),
      (error) => error instanceof shortcuts.ShortcutValidationError,
    )
  assert.equal(
    shortcuts.normalizeShortcutBindings({ backToBoard: 'Escape' }).backToBoard,
    'Mod+BracketLeft',
  )
  // 键盘 Esc 走 BackNavigation 的返回层，路由层返回只挂在手机原生返回上（行为见 back-navigation.test.mjs）。
  const app = await read('src/App.tsx')
  assert.match(app, /<BackNavigation onRouteBack=\{mobileLayout \? routeBack : undefined\} \/>/)
})

test('external open requests and the end-of-run reveal land on inspector tabs', async () => {
  const workspace = await read('src/features/chat/SessionWorkspace.tsx')
  // 旧入口（网页预览、任务内容事件、自动展示）仍写会话上下文页 store；这里映射到检查器标签与子视图。
  for (const [kind, target] of [
    ['changes', "{ tab: 'changes', changesView: 'workspace' }"],
    ['files', "{ tab: 'changes', changesView: 'session' }"],
    ['sources', "{ tab: 'files' }"],
    ['browser', "{ tab: 'preview' }"],
    ['plan', "{ tab: 'plan' }"],
    ['side-chat', "{ tab: 'side-chat' }"],
  ])
    assert.ok(
      new RegExp(`case '${kind}':\\s*return ${target.replace(/[{}]/g, '\\$&')}`).test(workspace),
      `${kind} -> ${target}`,
    )
  assert.match(
    workspace,
    /case 'terminal':\s*case 'processes':\s*return \{ tab: 'terminal', terminalView: kind \}/,
  )
  assert.match(workspace, /addEventListener\(TASK_CONTENT_REQUESTED, request\)/)
  // 运行结束且留下改动：宽屏自动展开“改动 · 本会话”；覆盖层不自动弹出，避免盖住刚出来的回复。
  assert.match(
    workspace,
    /useSessionContextAutoReveal\(\{[\s\S]*?enabled: !overlayMode,[\s\S]*?onReveal: \(\) => reveal\('files'\),?\s*\}\)/,
  )
})

test('the inspector sits after the conversation with a keyboard separator only when docked', async () => {
  // 取代旧上下文面板的布局守卫（session-context-regions 仍覆盖手机外壳的 SessionContextLayout）：
  // 对话列在前、检查器在后；并排时有可聚焦、可用方向键调宽的分隔条，覆盖层没有分隔条也不抢对话列的位置。
  const workspace = await read('src/features/chat/SessionWorkspace.tsx')
  const conversation = workspace.indexOf('data-session-conversation')
  const aside = workspace.indexOf('data-inspector-aside')
  const overlay = workspace.indexOf('data-inspector-overlay')
  assert.ok(conversation > 0 && conversation < aside && aside < overlay)
  const docked = workspace.slice(workspace.indexOf('{!overlayMode && open && ('), overlay)
  assert.match(docked, /role="separator"\s+tabIndex=\{0\}\s+aria-orientation="vertical"/)
  assert.match(docked, /aria-controls=\{panelId\}/)
  assert.match(docked, /\['ArrowLeft', 'ArrowRight', 'Enter'\]\.includes\(event\.key\)/)
  const floating = workspace.slice(
    overlay,
    workspace.indexOf('</SessionInspectorContext.Provider>'),
  )
  assert.doesNotMatch(floating, /role="separator"/)
  // 覆盖层贴右、从会话页头下方开始（页头的“⋯”、侧聊与开关始终可点），没有遮罩。
  assert.match(floating, /absolute right-0 bottom-0/)
  assert.match(floating, /top: headerHeight/)
  // 宽度够时对话内容让出覆盖层的宽度（停止键、发送键不被盖住）；不够时保持纯覆盖。
  assert.match(
    workspace,
    /width - SESSION_INSPECTOR_OVERLAY_WIDTH >= SESSION_CONVERSATION_MIN_WIDTH\s*\?\s*SESSION_INSPECTOR_OVERLAY_WIDTH\s*:\s*0/,
  )
  assert.match(workspace, /style=\{overlayInset \? \{ paddingRight: overlayInset \} : undefined\}/)
  // 收起时对话列独占：检查器只在 open 时渲染。
  assert.match(workspace, /\{overlayMode && open && \(/)
})
