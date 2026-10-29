// 看板冒烟的固定数据：只在浏览器边界用 Playwright 路由提供，不写 Runtime 数据目录。
// 覆盖：需要你（standard 审批、critical 审批与未读失败）、运行中（其中一个带子代理）、
// 已完成（含未读）和一张置顶卡片。critical 的 reason 使用 Runtime 真实的命令守卫前缀，
// 这样降级规则（没有 severity 字段时按前缀判定）走的是生产路径。
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
  return { sessions, live, changeSummary }
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
export async function installBoardFixture(context, { now = Date.now(), sessions } = {}) {
  const fixture = boardFixture(now)
  const state = {
    sessions: structuredClone(sessions ?? fixture.sessions),
    live: structuredClone(fixture.live),
    posts: [],
  }
  const update = (id, patch) => {
    state.sessions = state.sessions.map((session) =>
      session.id === id ? { ...session, ...patch } : session,
    )
  }
  await context.route(/\/api\/sessions(\?.*)?$/, async (route) => {
    if (route.request().method() !== 'GET') return route.continue()
    await route.fulfill({ json: { sessions: state.sessions } })
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
    update(
      id,
      body.approved === true
        ? { ...settled, streaming: true }
        : { ...settled, streaming: false, lastCompletedAt: new Date().toISOString() },
    )
    delete state.live[id]
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
