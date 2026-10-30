import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import { parseBoardViewQuery } from '../http/routes/board.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import {
  BOARD_VIEW_MAX_LIMIT,
  BoardViewQueryError,
  decodeBoardCursor,
  encodeBoardCursor,
} from '../services/session-board-view.mjs'
import {
  CARD_LIMITS,
  clipCodePoints,
  firstLine,
  normalizePersistedCard,
  SessionCardSummaries,
  tailLines,
  toolArgsSummary,
} from '../services/session-card-summary.mjs'

const DAY = 86_400_000

function freshRuntime(directory) {
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.settingsManager = { getGlobalSettings: () => ({}) }
  return runtime
}

async function startServer(runtime) {
  const handleApi = createApiHandler(runtime)
  const server = createServer((req, res) => {
    void handleApi(req, res, new URL(req.url, 'http://127.0.0.1'))
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const { port } = server.address()
  return {
    base: `http://127.0.0.1:${port}`,
    close: () => new Promise((resolve) => server.close(resolve)),
  }
}

// 严格 JSON / UTF-8：字节必须能被严格解码，且不出现 serde_json 拒绝的孤立代理项转义。
async function strictJson(response) {
  const bytes = new Uint8Array(await response.arrayBuffer())
  const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes)
  assert.doesNotMatch(text, /\\u[dD][89abAB][0-9a-fA-F]{2}(?!\\u[dD][c-fC-F])/)
  assert.doesNotMatch(text, /\\u[dD][c-fC-F][0-9a-fA-F]{2}/)
  return JSON.parse(text)
}

function setOrganization(runtime, id, organization) {
  runtime.sessionMeta[id] = {
    ...runtime.sessionMeta[id],
    organization: {
      pinned: false,
      archived: false,
      unread: false,
      failed: false,
      lastCompletedAt: null,
      ...organization,
    },
  }
}

async function boardFixture(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-board-view-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const projectA = join(directory, 'project-a')
  const projectB = join(directory, 'project-b')
  await mkdir(projectA)
  await mkdir(projectB)
  const runtime = freshRuntime(directory)
  const now = Date.now()
  const iso = (offset) => new Date(now - offset).toISOString()
  const ids = {}
  for (const [key, name, cwd] of [
    ['doneA', '已完成 A', projectA],
    ['failedA', '失败 A', projectA],
    ['approvalB', '待批准 B', projectB],
    ['doneB', '已完成 B', projectB],
    ['archived', '已归档', projectA],
    ['old', '很久以前', projectB],
    ['pinnedOld', '置顶旧会话', projectB],
    ['empty', '空会话', projectA],
  ]) {
    ids[key] = (await runtime.createSession(name, cwd)).id
  }
  // 旧会话的修改时间也在一个月前（存储会话缓存是列表的数据源）。
  for (const session of await runtime.listStoredSessions()) {
    if (session.id === ids.old || session.id === ids.pinnedOld)
      session.modified = new Date(now - 30 * DAY)
  }
  setOrganization(runtime, ids.doneA, { unread: true, lastCompletedAt: iso(60_000) })
  setOrganization(runtime, ids.failedA, { unread: true, failed: true, lastCompletedAt: iso(5_000) })
  setOrganization(runtime, ids.doneB, { lastCompletedAt: iso(120_000) })
  setOrganization(runtime, ids.archived, { archived: true, lastCompletedAt: iso(1_000) })
  setOrganization(runtime, ids.old, { lastCompletedAt: iso(30 * DAY) })
  setOrganization(runtime, ids.pinnedOld, { pinned: true, lastCompletedAt: iso(30 * DAY) })
  runtime.sessionMeta[ids.doneA].boardCard = {
    v: 1,
    outcome: 'completed',
    finishedAt: iso(60_000),
    // 孤立代理项必须在响应里变成 U+FFFD，而不是 serde_json 拒绝的 \ud800 转义。
    lastAssistantPreview: `结果\ud800摘要\n第二行`,
    changeStats: { added: 12, removed: 3, files: 2, status: 'known' },
    usage: { totalTokens: 4200 },
  }
  runtime.sessionMeta[ids.failedA].boardCard = {
    v: 1,
    outcome: 'failed',
    finishedAt: iso(5_000),
    error: { message: `模型错误：${'x'.repeat(400)}`, kind: 'model' },
    tail: ['\u001b[31mFAIL\u001b[0m tests/a.test.ts', '3 failed, 41 passed'],
  }
  // 一条真实的待批准请求：attentionReason 由权限服务的待处理数量推出。
  const approval = runtime.permissions.requestApproval({
    sessionId: ids.approvalB,
    toolName: 'bash',
    toolCallId: 'call-approval',
    args: { command: 'rm -r build' },
    mode: 'auto',
    risk: 'high',
    reason: '命令守卫拦截：递归删除（rm -r）。请确认后执行。',
  })
  t.after(async () => {
    runtime.permissions.resolveSession(ids.approvalB, false)
    await approval
    await runtime.dispose().catch(() => {})
  })
  return { directory, projectA, projectB, runtime, ids }
}

// 第 2 期之前（bfd689b1）默认列表每个会话的字段：TUI（src-tui/src/model.rs SessionSummary）、Web 目录缓存、
// 历史页与 MCP 工具都读它。协议只做加法：这些字段一个都不能少、不能改名；看板专属字段不能混进默认列表。
const DEFAULT_SESSION_FIELDS = [
  'agents',
  'archived',
  'attentionReason',
  'created',
  'cwd',
  'executionMode',
  'firstMessage',
  'goal',
  'id',
  'lastCompletedAt',
  'lineage',
  'messageCount',
  'model',
  'modified',
  'name',
  'needsAttention',
  'permissionMode',
  'pinned',
  'plan',
  'runMode',
  'streaming',
  'team',
  'thinkingLevel',
  'unread',
]

test('default session list keeps its shape and still ignores ?limit', async (t) => {
  const { runtime, ids } = await boardFixture(t)
  const server = await startServer(runtime)
  t.after(server.close)

  const expected = JSON.parse(JSON.stringify({ sessions: await runtime.listSessions() }))
  for (const query of ['', '?limit=1', '?view=unknown', '?cursor=abc&lane=done']) {
    const response = await fetch(`${server.base}/api/sessions${query}`)
    assert.equal(response.status, 200)
    const body = await strictJson(response)
    assert.deepEqual(Object.keys(body), ['sessions'], query)
    assert.equal(body.sessions.length, expected.sessions.length, query)
    assert.ok(
      body.sessions.every((session) => !Object.hasOwn(session, 'card')),
      query,
    )
    assert.deepEqual(
      body.sessions.map((session) => session.id),
      expected.sessions.map((session) => session.id),
    )
    // 默认列表不按看板资格过滤：已归档、时间窗外的空闲会话都在。
    for (const id of [ids.archived, ids.old, ids.empty])
      assert.ok(
        body.sessions.some((session) => session.id === id),
        `${query} keeps ${id}`,
      )
    for (const session of body.sessions) {
      for (const field of DEFAULT_SESSION_FIELDS)
        assert.ok(Object.hasOwn(session, field), `${query} ${session.id} keeps ${field}`)
      // 落盘的卡片摘要、卡片与运行 id 只出现在 view=board、事件流与 /live。
      for (const field of ['card', 'boardCard', 'runId', 'nextCursor', 'revision', 'view'])
        assert.equal(Object.hasOwn(session, field), false, `${query} ${session.id} leaks ${field}`)
      // TUI 反序列化依赖的类型：id 必填字符串，其余缺省也能解析，但出现时类型必须一致。
      assert.equal(typeof session.id, 'string')
      for (const field of ['name', 'model', 'cwd', 'executionMode', 'thinkingLevel', 'modified'])
        assert.equal(typeof session[field], 'string', `${session.id}.${field}`)
      assert.equal(typeof session.streaming, 'boolean')
      assert.ok(session.plan === null || typeof session.plan === 'object')
    }
  }
})

test('board view returns eligible sessions with cards, filters, and strict JSON', async (t) => {
  const { runtime, ids, projectA, projectB } = await boardFixture(t)
  const server = await startServer(runtime)
  t.after(server.close)

  const response = await fetch(`${server.base}/api/sessions?view=board`)
  assert.equal(response.status, 200)
  const body = await strictJson(response)
  assert.equal(body.view, 'board')
  assert.equal(body.nextCursor, null)
  assert.equal(typeof body.revision, 'number')
  const byId = new Map(body.sessions.map((session) => [session.id, session]))
  // 默认排除：已归档、时间窗外未置顶的空闲会话、从未发过消息的空会话。
  assert.equal(byId.has(ids.archived), false)
  assert.equal(byId.has(ids.old), false)
  assert.equal(byId.has(ids.empty), false)
  assert.equal(body.total, 5)
  // 需要你的卡片排在第一页最前面。
  assert.deepEqual(
    body.sessions.slice(0, 2).map((session) => session.card.boardStatus),
    ['attention', 'attention'],
  )

  const approval = byId.get(ids.approvalB)
  // 现有列表字段原样保留，card 只是附加。
  assert.equal(approval.attentionReason, 'approval')
  assert.equal(approval.name, '待批准 B')
  assert.equal(approval.card.boardStatus, 'attention')
  assert.equal(approval.card.approvals.length, 1)
  assert.equal(approval.card.approvals[0].severity, 'critical')
  assert.equal(approval.card.approvals[0].argsPreview, 'rm -r build')

  const done = byId.get(ids.doneA)
  assert.equal(done.card.boardStatus, 'done')
  assert.equal(done.card.outcome, 'completed')
  assert.equal(done.card.lastAssistantPreview, '结果�摘要')
  assert.deepEqual(done.card.changeStats, { added: 12, removed: 3, files: 2, status: 'known' })
  assert.deepEqual(done.card.usage, { totalTokens: 4200 })
  assert.equal(Object.hasOwn(done.card, 'tail'), false)

  const failed = byId.get(ids.failedA)
  assert.equal(failed.card.boardStatus, 'attention')
  assert.equal(failed.card.error.kind, 'model')
  assert.equal([...failed.card.error.message].length, CARD_LIMITS.errorMessage)
  assert.deepEqual(failed.card.tail, ['FAIL tests/a.test.ts', '3 failed, 41 passed'])

  assert.equal(byId.get(ids.pinnedOld).card.boardStatus, 'done')

  // cwd 可多值，写法不同（尾斜杠）也按项目键命中。
  const onlyA = await strictJson(
    await fetch(`${server.base}/api/sessions?view=board&cwd=${encodeURIComponent(`${projectA}/`)}`),
  )
  assert.deepEqual(
    new Set(onlyA.sessions.map((session) => session.id)),
    new Set([ids.doneA, ids.failedA]),
  )
  const both = await strictJson(
    await fetch(
      `${server.base}/api/sessions?view=board&cwd=${encodeURIComponent(projectA)}&cwd=${encodeURIComponent(projectB)}`,
    ),
  )
  assert.equal(both.total, 5)

  const attention = await strictJson(
    await fetch(`${server.base}/api/sessions?view=board&lane=attention`),
  )
  assert.deepEqual(
    new Set(attention.sessions.map((session) => session.id)),
    new Set([ids.approvalB, ids.failedA]),
  )
  const doneOnly = await strictJson(
    await fetch(
      `${server.base}/api/sessions?view=board&lane=done&cwd=${encodeURIComponent(projectB)}`,
    ),
  )
  assert.deepEqual(
    new Set(doneOnly.sessions.map((session) => session.id)),
    new Set([ids.doneB, ids.pinnedOld]),
  )
})

test('board view pages with an opaque cursor and rejects malformed queries', async (t) => {
  const { runtime } = await boardFixture(t)
  const server = await startServer(runtime)
  t.after(server.close)

  const seen = []
  let cursor = ''
  let pages = 0
  do {
    const body = await strictJson(
      await fetch(
        `${server.base}/api/sessions?view=board&limit=2${cursor ? `&cursor=${cursor}` : ''}`,
      ),
    )
    assert.ok(body.sessions.length <= 2)
    assert.equal(body.total, 5)
    seen.push(...body.sessions.map((session) => session.id))
    cursor = body.nextCursor || ''
    pages += 1
  } while (cursor && pages < 10)
  assert.equal(pages, 3)
  assert.equal(seen.length, 5)
  assert.equal(new Set(seen).size, 5)
  const full = await strictJson(await fetch(`${server.base}/api/sessions?view=board`))
  assert.deepEqual(
    seen,
    full.sessions.map((session) => session.id),
  )

  for (const [query, code] of [
    ['lane=bogus', 'invalid_board_query'],
    ['limit=0', 'invalid_board_query'],
    ['limit=abc', 'invalid_board_query'],
    ['cursor=%21%21', 'invalid_board_cursor'],
    [`cursor=${Buffer.from('[1,0]').toString('base64url')}`, 'invalid_board_cursor'],
    ['window=month', 'invalid_board_query'],
    ['tzOffset=9999', 'invalid_board_query'],
    ['cwd=', 'invalid_board_query'],
  ]) {
    const response = await fetch(`${server.base}/api/sessions?view=board&${query}`)
    assert.equal(response.status, 400, query)
    assert.equal((await strictJson(response)).code, code, query)
  }
})

test('board query parsing caps limits and keeps cursors opaque but validated', () => {
  assert.equal(parseBoardViewQuery(new URLSearchParams('limit=100000')).limit, BOARD_VIEW_MAX_LIMIT)
  assert.equal(parseBoardViewQuery(new URLSearchParams('')).limit, 100)
  const parsed = parseBoardViewQuery(
    new URLSearchParams('lane=done,attention&lane=done&tzOffset=-300&window=today'),
  )
  assert.deepEqual(parsed.lanes, ['done', 'attention'])
  assert.equal(parsed.timeZoneOffsetMinutes, -300)
  assert.equal(parsed.doneWindow, 'today')
  assert.throws(
    () => parseBoardViewQuery(new URLSearchParams(Array(65).fill('cwd=/a').join('&'))),
    BoardViewQueryError,
  )
  const key = { rank: 3, modified: 1_700_000_000_000, id: 'session-1' }
  assert.deepEqual(decodeBoardCursor(encodeBoardCursor(key)), key)
  assert.throws(() => decodeBoardCursor('not-json'), BoardViewQueryError)
  assert.throws(
    () => decodeBoardCursor(Buffer.from(JSON.stringify([1, 9, 0, 'x'])).toString('base64url')),
    BoardViewQueryError,
  )
})

test('card text is truncated by code point, cleaned of ANSI and bounded in bytes', () => {
  const emoji = '🙂'.repeat(300)
  const clipped = clipCodePoints(emoji, 200)
  assert.equal([...clipped].length, 200)
  assert.ok(clipped.endsWith('…'))
  assert.doesNotMatch(clipped, /[\ud800-\udbff](?![\udc00-\udfff])/)
  assert.equal(clipCodePoints('短文本', 200), '短文本')

  assert.equal(firstLine('\n\n  # 标题\u0007 \n正文', CARD_LIMITS.preview), '# 标题')
  assert.equal([...firstLine('长'.repeat(500), CARD_LIMITS.preview)].length, CARD_LIMITS.preview)

  const tail = tailLines(
    [
      'line 1',
      '\u001b[32mline 2\u001b[0m',
      'progress 10%\rprogress 100%',
      '',
      'API_KEY=sk-test-abcdefghijklmnopqrstuvwxyz',
      '\u001b]0;title\u0007line 5',
    ].join('\n'),
  )
  assert.deepEqual(tail, ['progress 100%', 'API_KEY=[REDACTED SECRET]', 'line 5'])
  const wide = tailLines(['宽'.repeat(190), '宽'.repeat(190), '宽'.repeat(190)].join('\n'))
  assert.ok(Buffer.byteLength(wide.join('\n')) <= CARD_LIMITS.tailBytes)
  assert.ok(wide.length >= 1 && wide.length <= CARD_LIMITS.tailLines)
  const huge = tailLines('字'.repeat(5000))
  assert.equal(huge.length, 1)
  assert.ok(Buffer.byteLength(huge[0]) <= CARD_LIMITS.tailBytes)

  assert.equal(toolArgsSummary('bash', { command: 'npm\ttest\n--watch' }), 'npm test --watch')
  assert.equal(
    [...toolArgsSummary('bash', { command: 'x'.repeat(400) })].length,
    CARD_LIMITS.argsSummary,
  )
  assert.equal(toolArgsSummary('custom', { apiKey: 'secret', mode: 'fast', n: 2 }), 'mode=fast n=2')
  assert.equal(toolArgsSummary('custom', {}), 'custom')
})

test('persisted card summaries are validated field by field', () => {
  assert.equal(normalizePersistedCard(null), null)
  assert.equal(
    normalizePersistedCard({ outcome: 'exploded', finishedAt: '2026-10-29T00:00:00Z' }),
    null,
  )
  assert.equal(normalizePersistedCard({ outcome: 'completed', finishedAt: 'yesterday' }), null)
  const card = normalizePersistedCard({
    outcome: 'completed',
    finishedAt: '2026-10-29T00:00:00Z',
    error: { message: 'ignored for completed runs' },
    contextPct: 180,
    usage: { totalTokens: -4 },
    changeStats: { added: 'many', removed: 2.6, changedFiles: 1, status: 'known' },
    tail: ['only kept for failures'],
  })
  assert.deepEqual(card, {
    v: 1,
    outcome: 'completed',
    finishedAt: '2026-10-29T00:00:00.000Z',
    changeStats: { added: null, removed: 3, files: 1, status: 'known' },
    contextPct: 100,
  })
  // 错误行与尾巴同样脱敏：Provider 回显的密钥不会经事件流广播或留在落盘摘要里。
  const failed = normalizePersistedCard({
    outcome: 'failed',
    finishedAt: '2026-10-29T00:00:00Z',
    error: { message: '401 Unauthorized: api_key=sk-live-1234567890abcdefABCDEF', kind: 'model' },
  })
  assert.equal(failed.error.kind, 'model')
  assert.doesNotMatch(failed.error.message, /sk-live-1234567890abcdefABCDEF/)
  assert.match(failed.error.message, /^401 Unauthorized: api_key=/)
})

test('a session deleted mid-run does not get its card summary or boardCard rebuilt', () => {
  const persisted = []
  const changed = []
  const cards = new SessionCardSummaries({
    persist: (id, card) => persisted.push([id, card]),
    onChange: (id) => changed.push(id),
  })
  cards.beginRun('gone', { startedAt: '2026-10-29T00:00:00Z' })
  assert.ok(cards.entry('gone'))
  cards.forget('gone', { deleted: true })
  assert.equal(cards.isDeleted('gone'), true)
  // 本轮收尾仍会送来结束事件：不重建条目，也不交出要落盘的摘要。
  assert.equal(
    cards.finishRun('gone', {
      outcome: 'completed',
      finishedAt: '2026-10-29T00:01:00Z',
      text: 'ok',
    }),
    null,
  )
  cards.beginRun('gone')
  assert.equal(cards.entry('gone'), null)
  assert.deepEqual(persisted, [])
  // 普通的 forget（LRU 淘汰、重启模拟）不留墓碑。
  cards.beginRun('kept')
  cards.forget('kept')
  assert.equal(cards.isDeleted('kept'), false)
  cards.dispose?.()
})
