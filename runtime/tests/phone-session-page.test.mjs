// 第 4 期手机会话页（规格 7.3）：代理条的条目与顺序、“上一轮”摘要胶囊的推导，
// 以及“critical 审批在手机代理条上也不能一步批准”、会话页隐藏底部标签栏、原生返回不会停止运行的源码守卫。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import test from 'node:test'
import { phoneStripItems } from '../../src/features/board/phone-strip.ts'
import { earlierTurnSummary } from '../../src/features/chat/transcript-turns.ts'

const root = resolve(import.meta.dirname, '../..')
const read = (path) => readFile(resolve(root, path), 'utf8')

test('the phone agent strip lists attention, then running, keeps the current session and drops other done agents', () => {
  const items = [
    { id: 'done-a', lane: 'done' },
    { id: 'run-a', lane: 'running' },
    { id: 'attn-a', lane: 'attention' },
    { id: 'queued-a', lane: 'queued' },
    { id: 'current', lane: 'done' },
    { id: 'attn-b', lane: 'attention' },
  ]
  assert.deepEqual(
    phoneStripItems(items, 'current', { queue: false }).map((item) => item.id),
    ['attn-a', 'attn-b', 'run-a', 'current'],
  )
  // 排队只在 Runtime 声明排队能力时出现，位置在运行中之后。
  assert.deepEqual(
    phoneStripItems(items, 'run-a', { queue: true }).map((item) => item.id),
    ['attn-a', 'attn-b', 'run-a', 'queued-a'],
  )
  assert.deepEqual(phoneStripItems([], 'missing', { queue: true }), [])
})

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

test('critical approvals cannot be approved in one step from the phone agent strip', async () => {
  const [strip, shell] = await Promise.all([
    read('src/features/board/PhoneAgentStrip.tsx'),
    read('src/app/SessionShell.tsx'),
  ])
  // 只把动作交回 useStripApprovals，不直接调批准接口，也不新开数据连接。
  assert.doesNotMatch(
    strip,
    /approveBoardApproval|useBoardActions|confirmCritical|fetch\(|EventSource/,
  )
  assert.equal(strip.match(/useBoardItems\(/g)?.length, 1)
  assert.match(strip, /const \{ items, capabilities, isLoading \} = useBoardItems\('today'\)/)
  // 截断的预览或详情未到：只能查看；critical 的“允许…”只打开确认表单（onReview）。
  assert.match(
    strip,
    /const reviewOnly = item\.status === 'approval' && \(!approval \|\| approval\.truncated\)/,
  )
  assert.match(
    strip,
    /data-agent-sheet-approve="critical"\s*onClick=\{\(\) => onReview\(item, approval\)\}/,
  )
  // 一步允许只出现在 standard 分支，且它走 useStripApprovals.approve（critical 在那里也会改走确认层）。
  assert.equal(strip.match(/data-agent-sheet-approve="standard"/g)?.length, 1)
  assert.match(
    strip,
    /onApprove=\{\(item, approval\) => \{\s*close\(\)\s*approvals\.approve\(item, approval\)/,
  )
  // 唯一的 confirmStripApproval 调用方是按住确认表单。
  assert.equal(strip.match(/confirmStripApproval\(\)/g)?.length, 1)
  assert.match(
    strip,
    /<ApprovalConfirm[\s\S]*?phone\s*pending[\s\S]*?onConfirm=\{\(\) => void approvals\.confirmStripApproval\(\)\}/,
  )
  // 应用壳把代理条交给手机会话页摆放；聊天领域不导入看板。
  assert.match(
    shell,
    /<PhoneAgentStrip currentId=\{sessionId\} notify=\{notify\} onOpenSession=\{switchSession\} \/>/,
  )
  for (const file of [
    'src/features/chat/SessionWorkspace.tsx',
    'src/features/chat/SessionPhoneHeader.tsx',
    'src/features/chat/FocusSession.tsx',
  ])
    assert.doesNotMatch(await read(file), /@\/features\/board/, file)
})

test('the phone session page hides the tab bar, uses a bottom sheet inspector and never stops a run on native back', async () => {
  const [app, tabBar, workspace, inspector, focus] = await Promise.all([
    read('src/App.tsx'),
    read('src/app/ShellTabBar.tsx'),
    read('src/features/chat/SessionWorkspace.tsx'),
    read('src/features/chat/SessionInspector.tsx'),
    read('src/features/chat/FocusSession.tsx'),
  ])
  // 标签栏在会话页不渲染，但仍挂着更新 ⌥A 的等待最久事项。
  assert.match(app, /<ShellTabBar\s+hidden=\{page === 'session'\}/)
  assert.match(
    tabBar,
    /useEffect\(\(\) => onLongestWaitingChange\?\.\(longestId\)[^\n]*\n\s*if \(hidden\) return null/,
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
