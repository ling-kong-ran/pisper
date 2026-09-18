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
  const [sidebar, appSidebar, controls, focus, app, runtime, projection] = await Promise.all([
    source('../../src/components/layout/SidebarRecentSessions.tsx'),
    source('../../src/components/layout/AppSidebar.tsx'),
    source('../../src/features/chat/FocusRuntimeControls.tsx'),
    source('../../src/features/chat/FocusSession.tsx'),
    source('../../src/App.tsx'),
    source('../../runtime/runtime/agent-runtime.mjs'),
    source('../../runtime/runtime/stream-projection.mjs'),
  ])

  // 侧边栏右键菜单：空白区域新建项目 + 按项目删除；分组行右键针对该项目。
  // 区块整体从 AppSidebar 懒加载，避免右键菜单原语进入应用壳的 eager 入口。
  assert.match(appSidebar, /SidebarRecentSessions/)
  assert.match(appSidebar, /requestConfirm=\{requestConfirm\}/)
  assert.match(sidebar, /<ContextMenu[\s\S]*?onOpenChange=/)
  assert.match(sidebar, /id="sidebar-recent-sessions"/)
  assert.match(sidebar, /navigation:appSidebar\.newProject/)
  assert.match(sidebar, /navigation:appSidebar\.deleteProject/)
  assert.match(sidebar, /<ContextMenuSub>/)
  assert.match(sidebar, /onSelect=\{\(\) => void deleteProject\(menuTargetGroup\)\}/)
  // 会话级右键菜单与项目级区分：打开/重命名/删除会话。
  assert.match(sidebar, /onContextMenu=\{\(\) => setMenuTargetSessionId\(session\.id\)\}/)
  assert.match(sidebar, /navigation:appSidebar\.openSession/)
  assert.match(sidebar, /chat:chatHistoryPage\.renameChat/)
  assert.match(sidebar, /onSelect=\{\(\) => void deleteSingleSession\(menuTargetSession\)\}/)
  assert.match(sidebar, /<WorkspacePicker[\s\S]*?onSelect=/)
  assert.match(sidebar, /announceSessionsUpdated\(\)/)
  assert.match(
    appSidebar,
    /requestText: \(options\?: PromptDialogOptions\) => Promise<string \| null>/,
  )
  assert.match(
    appSidebar,
    /requestConfirm: \(options\?: ConfirmDialogOptions\) => Promise<boolean>/,
  )
  assert.match(app, /requestText=\{appDialog\.prompt\}/)
  assert.match(app, /requestConfirm=\{appDialog\.confirm\}/)
  assert.match(app, /notify=\{notify\}/)

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
