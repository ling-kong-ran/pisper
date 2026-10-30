// 看板冒烟的固定数据：只在浏览器边界用 Playwright 路由提供，不写 Runtime 数据目录。
// 覆盖：需要你（standard 审批、critical 审批与未读失败）、运行中（其中一个带子代理）、
// 已完成（含未读）和一张置顶卡片。critical 的 reason 使用 Runtime 真实的命令守卫前缀，
// 这样降级规则（没有 severity 字段时按前缀判定）走的是生产路径。
// 协议有两种：board（默认，与被测 Runtime 声明的 board.view / board.events 一致，额外模拟
// GET /api/sessions?view=board 与 /api/board/events，card 由同一份数据推出）和 legacy（把能力里的
// board.* 去掉，模拟旧桌面 Runtime，看板走第 1 期的 /api/sessions + /live 降级路径）。
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { deriveApprovalSeverity, deriveBoardStatus } from '../../shared/session-board-status.mjs'

export const GUARD_PREFIX = '命令守卫拦截'

export function boardFixture(now = Date.now()) {
  // 已完成泳道默认只显示“今天”：刚过本地零点时把时间夹到当天，避免固定数据跨日掉出看板。
  const dayStart = new Date(now).setHours(0, 0, 0, 0)
  const earliest = Math.min(now, dayStart + 1000)
  const iso = (offset) => new Date(Math.max(now - offset, earliest)).toISOString()
  const sessions = [
    {
      id: 'fx-critical',
      name: '修复 CI 失败',
      firstMessage: '修复 CI 失败',
      messageCount: 8,
      cwd: '/home/dev/billing-service',
      model: 'openai/gpt-5.6-luna',
      created: iso(3_600_000),
      modified: iso(84_000),
      streaming: false,
      needsAttention: true,
      attentionReason: 'approval',
      unread: true,
    },
    {
      id: 'fx-standard',
      name: '补充登录模块的单元测试',
      firstMessage: '补充登录模块的单元测试',
      messageCount: 4,
      cwd: '/home/dev/pisper',
      model: 'openai/gpt-5.6-luna',
      created: iso(1_800_000),
      modified: iso(31_000),
      streaming: false,
      needsAttention: true,
      attentionReason: 'approval',
      unread: true,
    },
    {
      id: 'fx-failed',
      name: '迁移支付回调到队列',
      firstMessage: '迁移支付回调到队列',
      messageCount: 11,
      cwd: '/home/dev/billing-service',
      model: 'anthropic/claude-sonnet-5',
      created: iso(7_200_000),
      modified: iso(1_080_000),
      lastCompletedAt: iso(1_080_000),
      streaming: false,
      needsAttention: true,
      attentionReason: 'failure',
      unread: true,
    },
    {
      id: 'fx-running',
      name: '重构登录流程',
      firstMessage: '重构登录流程',
      messageCount: 6,
      cwd: '/home/dev/pisper',
      model: 'openai/gpt-5.6-luna',
      created: iso(600_000),
      modified: iso(12_000),
      streaming: true,
      needsAttention: false,
      attentionReason: null,
      unread: false,
    },
    {
      id: 'fx-running-agents',
      name: '为结算接口补单元测试，覆盖退款、部分退款与重复回调三种边界情况',
      firstMessage: '为结算接口补单元测试',
      messageCount: 14,
      cwd: '/home/dev/billing-service/packages/settlement-api-third-party-adapters',
      model: 'openai/gpt-5.6-luna-long-context-preview',
      created: iso(900_000),
      modified: iso(5_000),
      streaming: true,
      needsAttention: false,
      attentionReason: null,
      unread: false,
      agents: [
        { id: 'agent-1', taskName: '生成发票测试夹具', status: 'running' },
        { id: 'agent-2', taskName: '核对退款金额', status: 'queued' },
      ],
    },
    {
      id: 'fx-done-unread',
      name: '优化首页首屏性能',
      firstMessage: '优化首页首屏性能',
      messageCount: 12,
      cwd: '/home/dev/pisper',
      model: 'openai/gpt-5.6-luna',
      created: iso(5_400_000),
      modified: iso(1_560_000),
      lastCompletedAt: iso(1_560_000),
      streaming: false,
      needsAttention: false,
      attentionReason: null,
      unread: true,
    },
    {
      id: 'fx-done-pinned',
      name: '整理依赖许可证清单',
      firstMessage: '整理依赖许可证清单',
      messageCount: 5,
      cwd: '/home/dev/mobile-shell',
      model: 'anthropic/claude-sonnet-5',
      created: iso(4_000_000),
      modified: iso(2_400_000),
      lastCompletedAt: iso(2_400_000),
      streaming: false,
      needsAttention: false,
      attentionReason: null,
      unread: false,
      pinned: true,
    },
  ]
  const live = {
    'fx-critical': {
      streaming: false,
      approvals: [
        {
          id: 'approval-critical',
          toolName: 'bash',
          toolCallId: 'call-critical',
          args: { command: 'git push --force-with-lease origin fix/ci' },
          reason: `${GUARD_PREFIX}：改写远端分支`,
          risk: 'high',
          createdAt: iso(84_000),
        },
      ],
    },
    'fx-standard': {
      streaming: false,
      approvals: [
        {
          id: 'approval-standard',
          toolName: 'bash',
          toolCallId: 'call-standard',
          args: {
            command:
              'npm test -- --run src/features/auth/login.test.ts src/features/auth/session-refresh.test.ts --reporter=verbose',
          },
          reason: '',
          risk: 'high',
          createdAt: iso(31_000),
        },
      ],
    },
  }
  // 第 2 期 card 摘要（只在 board 协议下发给前端）：运行中尾巴与活动、失败摘要、结果摘要与改动统计。
  const cards = {
    'fx-running': {
      runId: 'run_fixture_login',
      startedAt: iso(140_000),
      activity: {
        type: 'tool',
        name: 'bash',
        argsSummary: 'npm test -- --runInBand',
        startedAt: iso(12_000),
      },
      tail: [
        '✓ login › accepts valid token (38 ms)',
        '✓ login › refreshes sliding session',
        '✗ login › rejects expired token',
      ],
      contextPct: 41,
    },
    'fx-running-agents': {
      runId: 'run_fixture_invoice',
      startedAt: iso(188_000),
      activity: {
        type: 'tool',
        name: 'edit',
        argsSummary: 'src/invoice.ts',
        startedAt: iso(4_000),
      },
      tail: ['PASS test/refund.test.ts (12 tests)', 'RUNS test/partial-refund.test.ts'],
      contextPct: 28,
      agents: { running: 1, total: 2 },
    },
    'fx-failed': {
      outcome: 'failed',
      finishedAt: iso(1_080_000),
      error: { message: 'Tests: 3 failed, 41 passed · exit 1', kind: 'runtime' },
      tail: ['✗ webhook › enqueues payment.succeeded', 'Tests: 3 failed, 41 passed'],
      contextPct: 57,
    },
    'fx-done-unread': {
      outcome: 'completed',
      finishedAt: iso(1_560_000),
      lastAssistantPreview:
        '首屏 LCP 从 2.8s 降到 1.6s：拆分路由、预加载字体，并把图表库改为按需加载。',
      changeStats: { added: 214, removed: 97, files: 6, status: 'known' },
      contextPct: 63,
    },
    'fx-done-pinned': {
      outcome: 'completed',
      finishedAt: iso(2_400_000),
      lastAssistantPreview: '已生成 THIRD_PARTY.md，列出 128 个依赖及其许可证。',
      changeStats: { added: 38, removed: 2, files: 1, status: 'known' },
    },
  }
  const changeSummary = {
    'fx-done-unread': {
      status: 'known',
      changedFiles: 6,
      pendingFiles: 0,
      added: 214,
      removed: 97,
      unknownFiles: 0,
      capped: false,
    },
    'fx-done-pinned': {
      status: 'known',
      changedFiles: 1,
      pendingFiles: 0,
      added: 38,
      removed: 2,
      unknownFiles: 0,
      capped: false,
    },
  }
  return { sessions, live, changeSummary, cards }
}

// 审批的 card 形态：与 Runtime 的 approvalArgsPreview 同一格式（bash 是命令，其余“键: 值”逐行）。
function cardApproval(approval) {
  const args = approval.args ?? {}
  const preview =
    approval.toolName === 'bash' && typeof args.command === 'string'
      ? args.command
      : Object.entries(args)
          .map(
            ([key, value]) =>
              `${key}: ${typeof value === 'string' ? value : JSON.stringify(value)}`,
          )
          .join('\n')
  return {
    id: approval.id,
    toolName: approval.toolName,
    argsPreview: preview.slice(0, 512),
    ...(preview.length > 512 ? { argsTruncated: true } : {}),
    risk: approval.risk || 'high',
    severity: deriveApprovalSeverity(approval),
    reason: approval.reason || '',
    createdAt: approval.createdAt || new Date().toISOString(),
  }
}

// 按 Runtime 同一份规则推出一条看板条目；不上看板（7 天窗口外的空闲会话、已归档）时返回 null。
function fixtureBoardItem(state, session) {
  const approvals = (state.live[session.id]?.approvals ?? []).map(cardApproval)
  const derived = deriveBoardStatus(
    { ...session, card: { approvals, outcome: state.cards[session.id]?.outcome } },
    { now: Date.now(), doneWindow: '7d' },
  )
  if (!derived.lane) return null
  const extra = { ...(state.cards[session.id] ?? {}) }
  // 运行结束（批准被拒、重试完成）后不再带运行中才有的字段，与 Runtime 的投影一致。
  if (session.streaming !== true) {
    delete extra.activity
    if (extra.outcome !== 'failed') delete extra.tail
  }
  return {
    ...session,
    card: { boardStatus: derived.lane, ...extra, ...(approvals.length ? { approvals } : {}) },
  }
}

function sseFrame(event, data) {
  return `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`
}

const UNAVAILABLE_CHANGES = {
  status: 'unavailable',
  changedFiles: null,
  pendingFiles: null,
  added: null,
  removed: null,
  unknownFiles: 0,
  capped: false,
}

const sessionIdFrom = (request) =>
  decodeURIComponent(new URL(request.url()).pathname.split('/')[3] || '')

// 在浏览器上下文安装拦截。状态是可变的：批准后会话进入运行中，拒绝后进入已完成，
// 重试后失败卡片进入运行中，这样冒烟能看到真实的换泳道结果，而不只是请求发出。
// 所有写请求记录在 posts 里，供冒烟断言请求路径与请求体。
export async function installBoardFixture(
  context,
  { now = Date.now(), sessions, protocol = 'board' } = {},
) {
  const fixture = boardFixture(now)
  const baseRevision = 1_000
  const state = {
    protocol,
    sessions: structuredClone(sessions ?? fixture.sessions),
    live: structuredClone(fixture.live),
    cards: structuredClone(fixture.cards),
    posts: [],
    // 事件流的修订号与每个会话最后一次变化的修订号；eventRequests 记录每次连接带的 after。
    revision: baseRevision,
    changed: {},
    eventRequests: [],
    viewRequests: 0,
  }
  // 任何改动都推进修订号：下一次带 after 的事件流请求会补发这个会话的最新卡片。
  state.touch = (id) => {
    state.revision += 1
    state.changed[id] = state.revision
  }
  const update = (id, patch) => {
    state.sessions = state.sessions.map((session) =>
      session.id === id ? { ...session, ...patch } : session,
    )
    state.touch(id)
  }
  state.update = update
  if (protocol === 'legacy') {
    // 旧桌面 Runtime：能力接口里没有 board.view / board.events。
    await context.route(/\/api\/runtime\/capabilities(\?.*)?$/, async (route) => {
      const response = await route.fetch()
      const data = await response.json().catch(() => ({}))
      const features = { ...(data.features ?? {}) }
      delete features['board.view']
      delete features['board.events']
      await route.fulfill({ response, json: { ...data, features } })
    })
  }
  await context.route(/\/api\/sessions(\?.*)?$/, async (route) => {
    if (route.request().method() !== 'GET') return route.continue()
    const url = new URL(route.request().url())
    if (url.searchParams.get('view') === 'board' && protocol === 'board') {
      state.viewRequests += 1
      const items = state.sessions
        .map((session) => fixtureBoardItem(state, session))
        .filter(Boolean)
      await route.fulfill({
        json: {
          view: 'board',
          sessions: items,
          nextCursor: null,
          total: items.length,
          revision: state.revision,
        },
      })
      return
    }
    await route.fulfill({ json: { sessions: state.sessions } })
  })
  // 看板事件流：Playwright 只能一次性返回完整响应体，所以每次连接发完快照或补发帧就结束，
  // 前端按“正常结束”在 1 秒后带 after 重连——相当于 1 秒一次的长轮询，足以覆盖动作后的换泳道。
  await context.route(/\/api\/board\/events(\?.*)?$/, async (route) => {
    if (protocol !== 'board') return route.fulfill({ status: 404, json: { error: 'not found' } })
    const after = new URL(route.request().url()).searchParams.get('after')
    const afterRevision = after && /^\d+$/.test(after) ? Number(after) : null
    state.eventRequests.push(afterRevision)
    const frames = []
    if (afterRevision === null || afterRevision < baseRevision || afterRevision > state.revision) {
      if (afterRevision !== null)
        frames.push(sseFrame('resync_required', { revision: state.revision, reason: 'gap' }))
      frames.push(
        sseFrame('snapshot', {
          revision: state.revision,
          items: state.sessions.map((session) => fixtureBoardItem(state, session)).filter(Boolean),
        }),
      )
    } else {
      for (const [id, revision] of Object.entries(state.changed)) {
        if (revision <= afterRevision) continue
        const session = state.sessions.find((item) => item.id === id)
        const item = session ? fixtureBoardItem(state, session) : null
        const { card, ...row } = item ?? {}
        frames.push(
          sseFrame(
            'session_card',
            item ? { revision, id, card, session: row } : { revision, id, removed: true },
          ),
        )
      }
      frames.push(sseFrame('heartbeat', { revision: state.revision, at: new Date().toISOString() }))
    }
    await route.fulfill({
      status: 200,
      headers: { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' },
      body: frames.join(''),
    })
  })
  await context.route(/\/api\/sessions\/[^/]+\/live(\?.*)?$/, async (route) => {
    const live = state.live[sessionIdFrom(route.request())]
    await route.fulfill({ json: live ?? { streaming: false, approvals: [] } })
  })
  await context.route(/\/api\/sessions\/[^/]+\/change-summary(\?.*)?$/, async (route) => {
    const summary = fixture.changeSummary[sessionIdFrom(route.request())]
    await route.fulfill({ json: summary ?? UNAVAILABLE_CHANGES })
  })
  await context.route(/\/api\/sessions\/[^/]+\/approvals\/[^/]+$/, async (route) => {
    const request = route.request()
    const id = sessionIdFrom(request)
    const body = JSON.parse(request.postData() || '{}')
    state.posts.push({ method: request.method(), path: new URL(request.url()).pathname, body })
    const settled = { needsAttention: false, attentionReason: null }
    delete state.live[id]
    update(
      id,
      body.approved === true
        ? { ...settled, streaming: true }
        : { ...settled, streaming: false, lastCompletedAt: new Date().toISOString() },
    )
    await route.fulfill({ json: { ok: true } })
  })
  await context.route(/\/api\/sessions\/[^/]+\/retry$/, async (route) => {
    const request = route.request()
    const id = sessionIdFrom(request)
    state.posts.push({
      method: request.method(),
      path: new URL(request.url()).pathname,
      body: null,
    })
    // 重试开始：失败摘要换成新一轮的运行状态。
    state.cards[id] = {
      runId: 'run_fixture_retry',
      startedAt: new Date().toISOString(),
      activity: { type: 'model', stage: 'thinking' },
    }
    update(id, { needsAttention: false, attentionReason: null, streaming: true })
    const header = { runId: 'run-retry', kind: 'chat', sessionId: id, cursor: 0 }
    await route.fulfill({
      status: 200,
      headers: { 'Content-Type': 'text/event-stream' },
      body: `event: run\ndata: ${JSON.stringify(header)}\n\nevent: done\ndata: {}\n\n`,
    })
  })
  // 注入一个已配置的对话模型，让启动台芯片有内容；真实配置的其余字段原样保留。
  await context.route(/\/api\/config(\?.*)?$/, async (route) => {
    if (route.request().method() !== 'GET') return route.continue()
    const response = await route.fetch()
    const data = await response.json().catch(() => ({}))
    const providers = Array.isArray(data.providers) ? data.providers : []
    await route.fulfill({
      response,
      json: {
        ...data,
        provider: 'openai',
        model: 'gpt-5.6-luna',
        providers: [
          {
            id: 'openai',
            name: 'OpenAI',
            configured: true,
            enabled: true,
            models: [
              {
                id: 'gpt-5.6-luna',
                name: 'GPT-5.6 Luna',
                kind: 'chat',
                thinkingLevels: ['off', 'low', 'medium', 'high'],
              },
            ],
          },
          ...providers.filter((provider) => provider?.id !== 'openai'),
        ],
      },
    })
  })
  return state
}

// Windows 自绘标题栏的桌面桥接（浏览器模拟，不代表 WebView2 验证）。
export const desktopBridgeScript = `
  window.pisperDesktop = {
    customTitlebar: true,
    platform: 'win32',
    windowAction: () => Promise.resolve(),
    getAppInfo: () => Promise.resolve({ version: '0.0.0', desktop: true }),
    openReleases: () => Promise.resolve(false),
  }
`

// 没有自绘标题栏的桌面桥接（macOS / Linux 原生标题栏），只用来证明窄窗口不会进入手机外壳。
export const plainDesktopBridgeScript = `
  window.pisperDesktop = {
    platform: 'linux',
    getAppInfo: () => Promise.resolve({ version: '0.0.0', desktop: true }),
    openReleases: () => Promise.resolve(false),
  }
`

// 第 3 期会话页冒烟用的一条真实会话：写进隔离 Runtime 的 sessions 目录（Pi 会话 JSONL 格式），
// 这样会话页的转录、模型与 /live 都来自真实 Runtime；看板与泳道条仍由上面的固定数据提供。
// 只在调用方创建的临时数据目录里写入，不碰真实数据；内容不含任何个人信息。
export const BOARD_SESSION_ID = '01a19000-1111-7222-8333-444455556666'
export const BOARD_SESSION_TITLE = '重构登录流程（会话页）'

export async function writeBoardSessionFixture(sessionsDir, cwd, now = Date.now()) {
  await mkdir(sessionsDir, { recursive: true })
  // 泳道条与看板只显示“今天”：刚过本地零点时把起点夹到当天（条目最晚在 t0 + 30s），
  // 否则 00:00–00:25 运行冒烟时这条会话会掉出时间窗。
  const dayStart = new Date(now).setHours(0, 0, 0, 0)
  const t0 = Math.max(now - 25 * 60_000, Math.min(now - 30_000, dayStart + 1000))
  const ts = (offset) => new Date(t0 + offset * 1000).toISOString()
  const usage = {
    input: 12000,
    output: 800,
    cacheRead: 9000,
    cacheWrite: 0,
    totalTokens: 21800,
    cost: { input: 0.01, output: 0.01, cacheRead: 0, cacheWrite: 0, total: 0.02 },
  }
  const assistant = (content, stopReason, offset) => ({
    role: 'assistant',
    content,
    api: 'openai-responses',
    provider: 'openai',
    model: 'gpt-5.6-luna',
    usage,
    stopReason,
    timestamp: t0 + offset * 1000,
  })
  const entries = [
    { type: 'session', version: 3, id: BOARD_SESSION_ID, timestamp: ts(0), cwd },
    {
      type: 'session_info',
      id: 'a0000001',
      parentId: null,
      timestamp: ts(0),
      name: BOARD_SESSION_TITLE,
    },
    {
      type: 'model_change',
      id: 'a0000002',
      parentId: 'a0000001',
      timestamp: ts(1),
      provider: 'openai',
      modelId: 'gpt-5.6-luna',
    },
    {
      type: 'message',
      id: 'a0000003',
      parentId: 'a0000002',
      timestamp: ts(2),
      message: {
        role: 'user',
        content: [{ type: 'text', text: '重构登录流程：把会话刷新逻辑抽到独立模块。' }],
        timestamp: t0 + 2000,
      },
    },
    {
      type: 'message',
      id: 'a0000004',
      parentId: 'a0000003',
      timestamp: ts(8),
      message: assistant(
        [
          { type: 'text', text: '先运行现有测试。' },
          {
            type: 'toolCall',
            id: 'call_bash_1',
            name: 'bash',
            arguments: { command: 'npm test -- --run src/features/auth' },
          },
        ],
        'toolUse',
        8,
      ),
    },
    {
      type: 'message',
      id: 'a0000005',
      parentId: 'a0000004',
      timestamp: ts(20),
      message: {
        role: 'toolResult',
        toolCallId: 'call_bash_1',
        toolName: 'bash',
        content: [
          { type: 'text', text: ' ✓ login › accepts valid token\n     Tests  1 passed (1)' },
        ],
        isError: false,
        timestamp: t0 + 20000,
      },
    },
    {
      type: 'message',
      id: 'a0000006',
      parentId: 'a0000005',
      timestamp: ts(30),
      message: assistant([{ type: 'text', text: '已完成重构，全部测试通过。' }], 'stop', 30),
    },
  ]
  const name = `${ts(0).replace(/[:.]/g, '-')}_${BOARD_SESSION_ID}.jsonl`
  await writeFile(
    join(sessionsDir, name),
    entries.map((entry) => JSON.stringify(entry)).join('\n') + '\n',
  )
}
