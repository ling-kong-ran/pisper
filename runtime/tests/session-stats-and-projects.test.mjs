import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test, { mock } from 'node:test'
import {
  accumulateRequestTiming,
  createRequestTiming,
  createRequestTimingTracker,
  sessionRequestTiming,
} from '../runtime/request-timing.mjs'

async function source(path) {
  return readFile(new URL(path, import.meta.url), 'utf8')
}

test('request timing accrues per-turn first-token and duration stats on sessionMeta', () => {
  // sessionRequestTiming：同一会话复用同一统计对象（live 与 meta 共享引用）。
  const meta = {}
  const first = sessionRequestTiming(meta, 's1')
  assert.equal(sessionRequestTiming(meta, 's1'), first)
  assert.deepEqual(first, {
    requests: 0,
    firstTokenSamples: 0,
    firstTokenTotalMs: 0,
    durationTotalMs: 0,
    lastFirstTokenMs: null,
    lastDurationMs: null,
  })

  // settleInto 直接把结算结果累加进统计对象（agent-runtime 的调用形态）。
  const tracker = createRequestTimingTracker()
  mock.timers.enable({ apis: ['Date'], now: 10_000 })
  try {
    // 标准路径：assistant message_start 起点 → 首个增量为首字 → message_end 结算。
    tracker.onMessageStart('user')
    tracker.onMessageStart('assistant')
    mock.timers.tick(120)
    tracker.onStreamDelta('thinking_delta')
    mock.timers.tick(880)
    tracker.settleInto(first)
    assert.equal(first.requests, 1)
    assert.equal(first.firstTokenSamples, 1)
    assert.equal(first.firstTokenTotalMs, 120)
    assert.equal(first.durationTotalMs, 1000)
    assert.equal(first.lastFirstTokenMs, 120)
    assert.equal(first.lastDurationMs, 1000)

    // 缺少 message_start 时退化为上一轮结束时刻作起点（间隔 200ms + 首字 300ms）。
    mock.timers.tick(200)
    tracker.onStreamDelta('text_delta')
    mock.timers.tick(300)
    tracker.settleInto(first)
    assert.equal(first.requests, 2)
    assert.equal(first.firstTokenSamples, 2)
    assert.equal(first.firstTokenTotalMs, 320)
    assert.equal(first.durationTotalMs, 1500)
    assert.equal(first.lastFirstTokenMs, 200)
    assert.equal(first.lastDurationMs, 500)

    // 未见首个增量（请求即失败）时首字不计入样本。
    tracker.onMessageStart('assistant')
    mock.timers.tick(50)
    tracker.settleInto(first)
    assert.equal(first.requests, 3)
    assert.equal(first.firstTokenSamples, 2)
    assert.equal(first.durationTotalMs, 1550)
  } finally {
    mock.timers.reset()
  }

  // 统计对象缺失时静默跳过（防御未来重构）；直接累加路径保持可用。
  accumulateRequestTiming(null, { durationMs: 1, firstTokenMs: 1 })
  const direct = createRequestTiming()
  accumulateRequestTiming(direct, { durationMs: 20, firstTokenMs: 5 })
  assert.equal(direct.requests, 1)
  assert.equal(direct.lastFirstTokenMs, 5)
})

test('sidebar and usage popover wire project context menu and session stats panel', async () => {
  const [sidebar, controls, focus, app, routeElements, history, runtime, projection] =
    await Promise.all([
      source('../../src/features/board/SidebarSessions.tsx'),
      source('../../src/features/chat/FocusRuntimeControls.tsx'),
      source('../../src/features/chat/FocusSession.tsx'),
      source('../../src/App.tsx'),
      source('../../src/app/route-elements.tsx'),
      source('../../src/features/chat/ChatHistoryPage.tsx'),
      source('../../runtime/runtime/agent-runtime.mjs'),
      source('../../runtime/runtime/stream-projection.mjs'),
    ])

  // 旧侧栏（AppSidebar / ProjectTaskList）已删除。会话的打开/重命名/删除由“功能 › 历史会话”承担，
  // v2 侧栏的会话行菜单（打开、置顶、重命名、归档、标记已处理、复制链接）只复用看板动作：
  // 应用壳把同一套对话框能力经路由上下文交给历史页、经属性交给侧栏，不在页面里另建弹窗。
  assert.doesNotMatch(app, /<ProjectTaskList\b|<AppSidebar\b/)
  assert.match(app, /requestText: appDialog\.prompt,/)
  assert.match(app, /requestConfirm: appDialog\.confirm,/)
  assert.match(app, /<SidebarSessions[\s\S]*?requestText=\{appDialog\.prompt\}/)
  assert.match(
    routeElements,
    /<ChatHistoryPage[\s\S]*?notify=\{context\.notify\}[\s\S]*?requestConfirm=\{context\.requestConfirm\}[\s\S]*?requestText=\{context\.requestText\}/,
  )
  assert.match(history, /onSelect=\{\(\) => void renameSession\(session\)\}/)
  assert.match(history, /onSelect=\{\(\) => void deleteSession\(session\)\}/)
  assert.match(history, /chat:chatHistoryPage\.renameChat/)
  for (const item of ['open', 'pin', 'unpin', 'rename', 'markHandled', 'copyLink', 'archive'])
    assert.match(sidebar, new RegExp(`t\\('board:menu\\.${item}'\\)`), item)
  assert.match(sidebar, /actionsRef\.current\.rename\(row\.id, trimmed\)/)
  assert.match(
    sidebar,
    /requestText: \(options\?: PromptDialogOptions\) => Promise<string \| null>/,
  )
  // 侧栏不删除会话（删除只在历史会话里，带确认）。
  assert.doesNotMatch(sidebar, /method: 'DELETE'|deleteSession/)

  // 统计弹窗：上下文用量 Popover 内嵌详细统计面板，FocusSession 传入数据源。
  assert.match(controls, /export function SessionStatsPanel/)
  assert.match(controls, /chat:focusSession\.statAvgFirstToken/)
  assert.match(controls, /chat:focusSession\.statAvgDuration/)
  assert.match(controls, /formatRunDuration\(/)
  assert.match(
    focus,
    /<ContextUsageIndicator[\s\S]*?sessionUsage=\{sessionUsage\}[\s\S]*?model=\{model\}[\s\S]*?availableModels=\{availableModels\}[\s\S]*?\/>/,
  )

  // runtime 埋点：时序统计挂 sessionMeta 并随 session_usage 结算上报。
  assert.match(runtime, /sessionRequestTiming\(this\.sessionMeta, session\.sessionId\)/)
  assert.match(runtime, /requestTiming\.settleInto\(live\.sessionUsage\.timing\)/)
  assert.match(projection, /getSessionTokenUsage\(id\)[\s\S]*\{ timing \}/)
})
