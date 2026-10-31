// 看板卡片内容（第 2 期 card 摘要）：解析按不可信数据处理、派生卡片字段、展示规则与动效闸门。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  parseBoardCard,
  parseBoardCardApproval,
  parseBoardViewPage,
} from '../../src/features/board/board-api.ts'
import { buildBoardItems, summarizeFleet } from '../../src/features/board/board-model.ts'
import {
  contextPercent,
  tailLineFailed,
  visibleChangeStats,
} from '../../src/features/board/card-presentation.ts'

const NOW = Date.parse('2026-10-29T10:00:00.000Z')
const iso = (offsetMs) => new Date(NOW + offsetMs).toISOString()
const overlay = (entries, extra = {}) => ({
  live: {},
  localRuns: {},
  runningSince: {},
  patches: {},
  pending: {},
  feed: { ready: true, entries },
  ...extra,
})
const entry = (session, card, revision = 1) => ({
  session: { cwd: '/w/pisper', messageCount: 4, ...session },
  card: parseBoardCard(card),
  revision,
})

test('card parsing treats every field as untrusted and clips by code point', () => {
  const card = parseBoardCard({
    boardStatus: 'running',
    runId: 'run_abc',
    activity: {
      type: 'tool',
      name: 'bash',
      argsSummary: 'npm   test\n--run',
      startedAt: iso(-12_000),
    },
    startedAt: iso(-60_000),
    outcome: 'exploded',
    error: { message: '\u001b[31mboom\u0007', kind: 'model' },
    tail: ['one', 42, '', 'two', 'three', '😀'.repeat(300)],
    contextPct: 140,
    changeStats: { added: 12.7, removed: -3, files: '2', status: 'known' },
    lastAssistantPreview: 'done\u0000!',
    agents: { running: 9, total: 2 },
    usage: { totalTokens: 1234 },
    approvals: 'nope',
    extra: { ignored: true },
  })
  assert.equal(card.outcome, null, 'unknown outcomes are ignored')
  assert.deepEqual(card.activity, {
    type: 'tool',
    name: 'bash',
    summary: 'npm test --run',
    startedAt: iso(-12_000),
  })
  assert.equal(card.error.message, ' [31mboom')
  assert.equal(card.error.kind, 'model')
  // 最多 3 行、去掉空行与非字符串；每行按码点截断，不切开代理对。
  assert.equal(card.tail.length, 3)
  assert.equal(card.tail.at(-1).endsWith('…'), true)
  assert.equal(Array.from(card.tail.at(-1)).length, 240)
  assert.equal(card.contextPct, 100)
  assert.deepEqual(card.changeStats, { added: 12, removed: 0, files: null, status: 'known' })
  assert.equal(card.preview, 'done !')
  assert.deepEqual(card.agents, { running: 2, total: 2 })
  assert.equal(card.totalTokens, 1234)
  assert.deepEqual(card.approvals, [])
  assert.deepEqual(parseBoardCard(null).tail, [])
  assert.equal(parseBoardCard('x').activity, null)
})

test('card approvals keep the full preview and mark truncation for /live', () => {
  const bash = parseBoardCardApproval({
    id: 'p1',
    toolName: 'bash',
    argsPreview: 'git push --force-with-lease origin fix/ci',
    severity: 'critical',
    reason: '命令守卫拦截：改写远端分支',
    createdAt: iso(-1_000),
  })
  assert.equal(bash.command, 'git push --force-with-lease origin fix/ci')
  assert.equal(bash.truncated, false)
  assert.equal(bash.severity, 'critical')
  const edit = parseBoardCardApproval({
    id: 'p2',
    toolName: 'edit',
    argsPreview: 'path: src/app.ts\noldText: a',
    argsTruncated: true,
  })
  assert.equal(edit.command, '')
  assert.equal(edit.path, 'src/app.ts')
  assert.equal(edit.argsText, 'path: src/app.ts\noldText: a')
  assert.equal(edit.truncated, true)
  // 缺 severity 的旧数据按同一张规则表降级判断；超过 512 码点的预览视为截断。
  assert.equal(
    parseBoardCardApproval({ id: 'p3', toolName: 'plugin_create', argsPreview: 'x' }).severity,
    'critical',
  )
  assert.equal(
    parseBoardCardApproval({ id: 'p4', toolName: 'bash', argsPreview: 'y'.repeat(600) }).truncated,
    true,
  )
  assert.equal(parseBoardCardApproval({ toolName: 'bash' }), null)
})

test('board view pages drop malformed rows and strip cards from catalog rows', () => {
  const page = parseBoardViewPage({
    view: 'board',
    sessions: [
      { id: 'a', name: 'A', card: { boardStatus: 'done' } },
      { id: 'a', name: 'dup' },
      { name: 'no id' },
      null,
    ],
    nextCursor: 'abc',
    revision: 12,
  })
  assert.deepEqual(
    page.items.map((item) => item.session.name),
    ['A'],
  )
  assert.equal('card' in page.items[0].session, false)
  assert.equal(page.nextCursor, 'abc')
  assert.equal(page.revision, 12)
  assert.equal(parseBoardViewPage({ revision: -1 }).revision, null)
})

test('cards drive membership, tails, summaries, context and stopped runs', () => {
  const entries = {
    run: entry(
      { id: 'run', name: '重构登录流程', streaming: true },
      {
        boardStatus: 'running',
        startedAt: iso(-60_000),
        activity: { type: 'tool', name: 'bash', argsSummary: 'npm test', startedAt: iso(-5_000) },
        tail: ['✓ login accepts valid token', '✗ login rejects expired token'],
        contextPct: 41.4,
        agents: { running: 1, total: 2 },
      },
    ),
    fail: entry(
      {
        id: 'fail',
        name: '迁移支付回调',
        needsAttention: true,
        attentionReason: 'failure',
        unread: true,
        lastCompletedAt: iso(-30_000),
      },
      {
        boardStatus: 'attention',
        outcome: 'failed',
        error: { message: '3 failed, 41 passed · exit 1', kind: 'runtime' },
        tail: ['webhook › enqueues payment.succeeded'],
        lastAssistantPreview: 'should not show on failures',
      },
    ),
    done: entry(
      { id: 'done', name: '优化首屏', lastCompletedAt: iso(-60_000), unread: true },
      {
        boardStatus: 'done',
        outcome: 'completed',
        lastAssistantPreview: 'LCP 从 2.8s 降到 1.6s',
        changeStats: { added: 214, removed: 97, files: 6, status: 'known' },
        tail: ['ignored when done'],
      },
    ),
    stopped: entry(
      { id: 'stopped', name: '停掉的任务', lastCompletedAt: iso(-10_000) },
      { boardStatus: 'done', outcome: 'stopped' },
    ),
    handled: entry(
      // dismissAttention 清掉了失败注意力：靠 card.outcome 认出“失败 · 已查看”。
      { id: 'handled', name: '已查看的失败', lastCompletedAt: iso(-20_000), unread: false },
      { boardStatus: 'done', outcome: 'failed', error: { message: 'boom' } },
    ),
  }
  // 目录里还有：Runtime 判定不上看板的会话（不在条目里，目录行可能过期），以及本客户端刚派发、
  // 条目还没到的会话（带运行补丁与本地运行）。
  const sessions = [
    { id: 'stale', name: '目录里的旧行', streaming: true },
    { id: 'launch', name: '刚派发', cwd: '/w/app' },
  ]
  const items = buildBoardItems(
    sessions,
    overlay(entries, {
      localRuns: {
        launch: { token: 1, kind: 'launch', runId: '', startedAt: NOW - 1_000, activity: null },
      },
      patches: { launch: [{ token: 1, patch: { streaming: true } }] },
    }),
    { now: NOW, timeZoneOffsetMinutes: 0 },
  )
  const byId = Object.fromEntries(items.map((item) => [item.id, item]))
  assert.deepEqual(Object.keys(byId).sort(), [
    'done',
    'fail',
    'handled',
    'launch',
    'run',
    'stopped',
  ])
  assert.equal(byId.launch.lane, 'running')
  assert.equal(byId.launch.hasCard, false)

  const run = byId.run
  assert.equal(run.hasCard, true)
  assert.equal(run.startedAt, NOW - 60_000)
  assert.deepEqual(run.tail, ['✓ login accepts valid token', '✗ login rejects expired token'])
  assert.equal(run.activity.name, 'bash')
  assert.equal(contextPercent(run), 41)
  assert.deepEqual(run.agents, { running: 1, total: 2 })

  const fail = byId.fail
  assert.equal(fail.lane, 'attention')
  assert.equal(fail.error, '3 failed, 41 passed · exit 1')
  assert.deepEqual(fail.tail, ['webhook › enqueues payment.succeeded'])
  assert.equal(fail.preview, '')

  const done = byId.done
  assert.equal(done.preview, 'LCP 从 2.8s 降到 1.6s')
  assert.deepEqual(done.tail, [], 'tails only show while running or after a failure')
  assert.deepEqual(visibleChangeStats(done), { files: 6, added: 214, removed: 97 })

  assert.equal(byId.stopped.status, 'stopped')
  assert.equal(byId.stopped.lane, 'done')
  assert.deepEqual(
    [byId.handled.lane, byId.handled.status, byId.handled.handled],
    ['done', 'failed', true],
  )
  // 失败卡片的舰队摘要取 card 的错误首行。
  const fleet = summarizeFleet(items)
  assert.equal(fleet.attentionItems.find((item) => item.sessionId === 'fail').summary, fail.error)

  // 没有 card 的降级形态：不显示尾巴、摘要和上下文。
  const legacy = buildBoardItems(
    [{ id: 'old', streaming: true, messageCount: 3 }],
    overlay({}, { feed: { ready: false, entries: {} } }),
    { now: NOW },
  )
  assert.deepEqual(
    [legacy[0].hasCard, legacy[0].tail, legacy[0].preview, legacy[0].contextPct],
    [false, [], '', null],
  )
})

test('pending approvals never flash back from a late card frame', () => {
  const approval = { id: 'p1', toolName: 'bash', argsPreview: 'ls', severity: 'standard' }
  const entries = {
    s: entry(
      { id: 's', needsAttention: true, attentionReason: 'approval' },
      { boardStatus: 'attention', approvals: [approval, { ...approval, id: 'p2' }] },
    ),
  }
  const items = buildBoardItems([], overlay(entries, { pending: { 'approval:s:p1': 'approve' } }), {
    now: NOW,
  })
  assert.deepEqual(
    items[0].approvals.map((item) => item.id),
    ['p2'],
  )
})

test('tail lines are marked failed only on real failure signals', () => {
  for (const line of [
    '✗ login rejects expired token',
    'FAIL src/auth.test.ts',
    'Tests: 3 failed, 41 passed',
    'Error: connect ECONNREFUSED',
    'npm ERR! code 1',
    'Process exited with exit code 2',
    'thread main panicked: panic at src/lib.rs',
  ])
    assert.equal(tailLineFailed(line), true, line)
  for (const line of [
    '✓ login accepts valid token',
    'Tests: 0 failed, 44 passed',
    '0 errors, 2 warnings',
    'no errors found',
    'exit code 0',
    '',
  ])
    assert.equal(tailLineFailed(line), false, line)
  assert.equal(contextPercent({ contextPct: null }), null)
  assert.equal(visibleChangeStats({ changeStats: { status: 'unavailable', files: 3 } }), null)
  assert.equal(visibleChangeStats({ changeStats: { status: 'known', files: 0 } }), null)
})

test('rows and phone cards carry no looping card motion beyond the status glyph', async () => {
  // A4 起手机卡片也是 OverviewRow：第 4 期卡片墙的沿边扫光与尾巴光标（AgentCard / useRunningMotion）随之删除，
  // 运行中的动效只剩状态图形自己的转动（它在减少动效时停转，见 status-primitives.test）。
  const [row, styles] = await Promise.all([
    readFile('src/features/board/OverviewRow.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])
  assert.doesNotMatch(row, /sweep|caret|IntersectionObserver|animate-/)
  assert.doesNotMatch(styles, /@keyframes (?:edge-sweep|caret-blink)\b/)
})
