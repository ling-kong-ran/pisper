// 手机会话页（规格 v2 第 7.2 节）：“上一轮”摘要胶囊的推导，以及顶栏 ☰ / 标题 ▾ / ✎ 的接线、
// 检查器底部表单、原生返回不会停止运行的源码守卫。第 4 期的代理条与底部标签栏在 A4 退役（并行的代理在抽屉里）。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import test from 'node:test'
import { earlierTurnSummary } from '../../src/features/chat/transcript-turns.ts'

const root = resolve(import.meta.dirname, '../..')
const read = (path) => readFile(resolve(root, path), 'utf8')

test('earlier turns collapse into one capsule that summarises the previous turn', () => {
  const user = (id, text) => ({ id, role: 'user', text })
  const agent = (id, extra = {}) => ({ id, role: 'agent', text: 'ok', ...extra })
  // 只有一轮时不收起。
  assert.equal(earlierTurnSummary([user('u1', 'hi'), agent('a1')], { turnsKnown: true }), null)
  assert.equal(earlierTurnSummary([agent('a0'), agent('a1')], { turnsKnown: true }), null)
  const messages = [
    user('u1', '先确认问题'),
    agent('a1', { runActivity: { tools: [{ id: 't1' }] } }),
    user('u2', '\n  复现过期 token 问题\n第二行'),
    agent('a2', { runActivity: { activityFeed: [{ id: 'f1' }, { id: 'f2' }], tools: [{}] } }),
    agent('a3', { runActivity: { tools: [] } }),
    user('u3', '重构校验'),
    agent('a4'),
  ]
  assert.deepEqual(earlierTurnSummary(messages, { turnsKnown: true }), {
    latestIndex: 5,
    turnBase: 2,
    previous: { turn: 2, text: '复现过期 token 问题', operations: 2, failed: false },
  })
  // 历史没有从开头加载时不写轮次；上一轮以失败结束时标红。
  const failed = [...messages.slice(0, 4), agent('a3', { error: 'boom' }), ...messages.slice(5)]
  assert.deepEqual(earlierTurnSummary(failed, { turnsKnown: false }).previous, {
    turn: null,
    text: '复现过期 token 问题',
    operations: 2,
    failed: true,
  })
})

test('the transcript only collapses on the phone page and keeps turn numbers continuous', async () => {
  const [transcript, virtual, focus] = await Promise.all([
    read('src/features/chat/FocusTranscript.tsx'),
    read('src/features/chat/VirtualMessageTranscript.tsx'),
    read('src/features/chat/FocusSession.tsx'),
  ])
  assert.match(focus, /collapseEarlierTurns=\{phonePage\}/)
  // 收起时不自动翻页，跳转到某一轮时不收起。
  assert.match(transcript, /const collapsed = Boolean\(earlier\) && !earlierOpen && !targetEntryId/)
  assert.match(
    transcript,
    /if \(!collapsed && event\.currentTarget\.scrollTop <= 96\) void loadOlder\(\)/,
  )
  assert.match(transcript, /turnBase=\{collapsed && earlier \? earlier\.turnBase : 0\}/)
  assert.match(virtual, /let turns = turnBase/)
})

test('the phone session header takes the drawer button from the shell and never imports the board', async () => {
  const [shell, home, header, workspace] = await Promise.all([
    read('src/app/SessionShell.tsx'),
    read('src/app/SessionHome.tsx'),
    read('src/features/chat/SessionPhoneHeader.tsx'),
    read('src/features/chat/SessionWorkspace.tsx'),
  ])
  // 应用壳把 ☰（带需要你计数）交给手机会话页与主页摆放，没有改动时的 ✎ 走应用壳的新任务。
  assert.match(shell, /routeLeading=\{pageLayout \? undefined : <PhoneMenuButton \/>\}/)
  assert.match(shell, /\(\{ \.\.\.pageChrome, phone: \{ onNewTask: startNewTask \} \}\)/)
  assert.match(home, /const entry = <PhoneMenuButton \/>/)
  assert.doesNotMatch(shell + home, /PhoneAgentStrip|BoardEntry|switchSession/)
  // 顶栏右侧：有改动是改动芯片（打开检查器表单），没有改动是 ✎；会话页下方不再有代理条。
  assert.match(header, /changedFiles > 0 && inspector \?[\s\S]{0,200}data-session-changes=/)
  assert.match(header, /onClick=\{chrome\.phone\.onNewTask\}/)
  assert.doesNotMatch(workspace, /phone\?\.strip/)
  for (const file of [
    'src/features/chat/SessionWorkspace.tsx',
    'src/features/chat/SessionPhoneHeader.tsx',
    'src/features/chat/FocusSession.tsx',
  ])
    assert.doesNotMatch(await read(file), /@\/features\/board/, file)
})

test('the phone session page has no tab bar, uses a bottom sheet inspector and never stops a run on native back', async () => {
  const [app, workspace, inspector, focus] = await Promise.all([
    read('src/App.tsx'),
    read('src/features/chat/SessionWorkspace.tsx'),
    read('src/features/chat/SessionInspector.tsx'),
    read('src/features/chat/FocusSession.tsx'),
  ])
  // 底部标签栏退役；手机外壳的 ⌥A（外接键盘）交给侧栏命令层打开抽屉，不再由标签栏上报等待最久事项。
  assert.doesNotMatch(app, /ShellTabBar|onLongestWaitingChange|longestWaitingId/)
  assert.match(
    app,
    /onJumpToAttention=\{\(\) => requestSidebarCommand\(\{ kind: 'focus-attention' \}\)\}/,
  )
  // 底部表单与覆盖层同属临时层：不写持久化偏好，不在运行结束时自动弹出，关闭后卸载。
  // 表单关闭时先以关闭态（Sheet open={open} → data-state=closed）播完退场动画，只在退场期间多挂载
  //（sheetLeaving 只在手机表单时成立），退场结束、兜底超时或减少动效时卸载；覆盖层仍是关闭即卸载。
  assert.match(
    workspace,
    /const overlayMode = sheetMode \|\| Boolean\(chrome\?\.compact \|\| chrome\?\.overlayOnly\)/,
  )
  assert.match(workspace, /enabled: !overlayMode,/)
  assert.match(workspace, /const sheetLeaving = sheetMode && sheetPresence\.leaving/)
  assert.match(
    workspace,
    /\{overlayMode && \(open \|\| sheetLeaving\) && \([\s\S]{0,400}\{sheetMode \? \(\s*<Sheet open=\{open\} onOpenChange=\{\(next\) => !next && close\(\)\}>/,
  )
  assert.match(workspace, /<SheetDragHandle onDismiss=\{close\} \/>/)
  assert.match(inspector, /const sheet = mode === 'sheet'/)
  // 原生返回键合成的 Esc 不是用户按键：不能被当成“停止运行”。
  const bubble = focus.slice(focus.indexOf('const bubble = (event: KeyboardEvent) => {'))
  assert.match(bubble.slice(0, 700), /!event\.isTrusted \|\|/)
})
