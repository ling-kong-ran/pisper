import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { afterEach, test } from 'node:test'
import { runInNewContext } from 'node:vm'
import * as reactQuery from '@tanstack/react-query'
import { transformSync } from 'esbuild'
import { BROWSER_PREFERENCE_KEYS } from '../../shared/browser-preferences.mjs'
import { chatApi } from '../../src/features/chat/chat-api.ts'
import * as sessionList from '../../src/features/chat/session-list.ts'
import * as sessionState from '../../src/lib/session-state.ts'
import { streamEventsWithResume } from '../../src/lib/api.ts'
import { queryClient, SESSIONS_QUERY_KEY } from '../../src/lib/startup-queries.ts'
import {
  approveBoardApproval,
  denyBoardApproval,
  markBoardSessionHandled,
  pinBoardSession,
  retryBoardSession,
} from '../../src/features/board/board-actions.ts'
import { parseBoardLiveDetail, truncateCodePoints } from '../../src/features/board/board-api.ts'
import {
  boardProjects,
  boardSessionTitle,
  buildBoardItems,
  filterBoardItems,
  groupBoardLanes,
  summarizeFleet,
} from '../../src/features/board/board-model.ts'
import {
  BOARD_CONNECTION_BUDGET,
  BOARD_HELD_STREAM_LIMIT,
  BOARD_LIVE_LIMIT,
  BOARD_LIVE_STAGGER_MS,
  BOARD_SESSIONS_QUERY_KEY,
  boardHeldStreamCount,
  applyBoardPatch,
  boardActionKey,
  boardSyncActive,
  resetBoardStore,
  retainBoardSync,
  useBoardOverlay,
} from '../../src/features/board/board-store.ts'
import {
  BOARD_VIEW_PREFERENCE_KEY,
  normalizeBoardView,
} from '../../src/features/board/board-view-store.ts'
import {
  LAUNCH_DEFAULTS_PREFERENCE_KEY,
  normalizeLaunchDefaults,
} from '../../src/features/board/launch-defaults-store.ts'

const originalFetch = globalThis.fetch
const NOW = Date.parse('2026-10-29T10:00:00.000Z')
const iso = (offsetMs) => new Date(NOW + offsetMs).toISOString()
const tick = () => new Promise((resolve) => setImmediate(resolve))
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
// 定时器驱动的断言用条件等待，避免 setImmediate 与 setTimeout(0) 的先后不确定导致偶发失败。
async function until(predicate, timeoutMs = 2_000) {
  const deadline = Date.now() + timeoutMs
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error('condition was not met in time')
    await wait(5)
  }
}
const json = (value, status = 200) => new Response(JSON.stringify(value), { status })
const emptyOverlay = () => ({ live: {}, localRuns: {}, runningSince: {}, patches: {} })

function sseResponse(frames, { hold } = {}) {
  const encoder = new TextEncoder()
  let index = 0
  const stream = new ReadableStream({
    async pull(controller) {
      if (index < frames.length) {
        controller.enqueue(encoder.encode(frames[index]))
        index += 1
        return
      }
      if (hold) await hold
      controller.close()
    },
  })
  return new Response(stream, { status: 200 })
}

function deferred() {
  let resolve
  const promise = new Promise((done) => {
    resolve = done
  })
  return { promise, resolve }
}

function installFetch(handler) {
  const calls = []
  globalThis.fetch = async (path, init = {}) => {
    const method = init.method || 'GET'
    const body = typeof init.body === 'string' ? JSON.parse(init.body) : undefined
    calls.push({ path: String(path), method, body })
    return handler(String(path), method, body, init)
  }
  return calls
}

afterEach(async () => {
  resetBoardStore()
  queryClient.clear()
  globalThis.fetch = originalFetch
  await tick()
})

test('board preferences are registered in the shared browser preference whitelist', () => {
  assert.ok(BROWSER_PREFERENCE_KEYS.includes(BOARD_VIEW_PREFERENCE_KEY))
  assert.ok(BROWSER_PREFERENCE_KEYS.includes(LAUNCH_DEFAULTS_PREFERENCE_KEY))
  assert.equal(BOARD_VIEW_PREFERENCE_KEY, 'pisper-board-view')
  assert.equal(LAUNCH_DEFAULTS_PREFERENCE_KEY, 'pisper-launch-defaults')
  // 看板与会话目录共用同一份权威缓存键。
  assert.deepEqual([...BOARD_SESSIONS_QUERY_KEY], ['sessions', 'board'])
  assert.equal(BOARD_SESSIONS_QUERY_KEY, SESSIONS_QUERY_KEY)
})

test('persisted board preferences are validated as untrusted data', () => {
  assert.deepEqual(normalizeBoardView(null), {
    groupBy: 'lane',
    projects: [],
    lanes: [],
    collapsedLanes: [],
    doneWindow: 'today',
  })
  assert.deepEqual(
    normalizeBoardView({
      groupBy: 'project',
      projects: ['/a', '/a', 7, 'x'.repeat(5000), '\0'],
      lanes: ['running', 'idle', 'running', 'done'],
      collapsedLanes: 'done',
      doneWindow: '7d',
    }),
    {
      groupBy: 'project',
      projects: ['/a', '\0'],
      lanes: ['running', 'done'],
      collapsedLanes: [],
      doneWindow: '7d',
    },
  )
  assert.equal(normalizeBoardView({ groupBy: 'grid', doneWindow: 'forever' }).groupBy, 'lane')
  assert.deepEqual(
    normalizeLaunchDefaults({
      cwd: ' /work/pisper ',
      model: 'openai/gpt-5',
      thinkingLevel: 'HIGH; rm -rf',
      executionMode: 'approval-required',
      runMode: 'team',
    }),
    {
      cwd: '/work/pisper',
      model: 'openai/gpt-5',
      thinkingLevel: '',
      executionMode: 'approval-required',
      runMode: 'team',
    },
  )
  assert.deepEqual(normalizeLaunchDefaults({ executionMode: 'yolo', runMode: 'chaos' }), {
    cwd: '',
    model: '',
    thinkingLevel: '',
    executionMode: '',
    runMode: 'plan',
  })
})

test('live detail parsing keeps full commands and derives severity from the guard reason', () => {
  const detail = parseBoardLiveDetail({
    streaming: true,
    messages: [{ id: 'm', text: 'x'.repeat(10_000) }],
    approvals: [
      {
        id: 'a1',
        toolName: 'bash',
        args: { command: `git reset --hard ${'x'.repeat(700)}` },
        risk: 'high',
        reason: '命令守卫拦截：git reset --hard 丢弃未提交改动。请确认后执行。',
        createdAt: iso(-60_000),
      },
      { id: 'a1', toolName: 'bash' },
      { toolName: 'missing-id' },
      { id: 'a2', toolName: 'edit', fileChange: { path: 'src/a.ts' }, createdAt: 'bad' },
    ],
    currentActivity: { type: 'tool', name: 'bash', args: { command: 'npm test' } },
    error: 7,
  })
  assert.equal(detail.streaming, true)
  assert.deepEqual(
    detail.approvals.map((approval) => [approval.id, approval.severity, approval.createdAt]),
    [
      ['a1', 'critical', iso(-60_000)],
      ['a2', 'standard', null],
    ],
  )
  assert.equal(detail.approvals[0].command.length, 'git reset --hard '.length + 700)
  assert.equal(detail.approvals[1].path, 'src/a.ts')
  assert.deepEqual(detail.activity, {
    type: 'tool',
    name: 'bash',
    summary: 'npm test',
    startedAt: null,
  })
  assert.equal(detail.error, '')
  assert.equal('messages' in detail, false)
  assert.equal(truncateCodePoints('😀😀😀', 2), '😀…')
})

test('board items merge the authoritative list with the realtime overlay', () => {
  const sessions = [
    {
      id: 'wait',
      name: '修复 CI',
      cwd: '/w/pisper',
      needsAttention: true,
      attentionReason: 'approval',
      streaming: true,
      modified: iso(-120_000),
    },
    {
      id: 'fail',
      name: '',
      firstMessage: '升级 Tauri 到 2.5。然后跑测试',
      cwd: '/w/app',
      needsAttention: true,
      attentionReason: 'failure',
      unread: true,
      lastCompletedAt: iso(-30_000),
    },
    { id: 'run', name: '生成夹具', cwd: '/w/pisper', streaming: true, modified: iso(-1_000) },
    {
      id: 'done',
      name: '写文档',
      cwd: '/w/pisper',
      unread: true,
      lastCompletedAt: iso(-5_000),
      pinned: true,
    },
    {
      id: 'old',
      name: '旧会话',
      cwd: '/w/pisper',
      modified: iso(-10 * 86_400_000),
      messageCount: 3,
    },
    { id: 'archived', name: '归档', archived: true, streaming: true },
    { name: 'no id' },
  ]
  const overlay = emptyOverlay()
  overlay.live.wait = {
    status: 'ready',
    approvals: parseBoardLiveDetail({
      approvals: [
        {
          id: 'p1',
          toolName: 'bash',
          args: { command: 'npm test' },
          reason: 'Shell',
          createdAt: iso(-90_000),
        },
      ],
    }).approvals,
    error: '',
    activity: null,
    startedAt: null,
    fetchedAt: NOW,
    failures: 0,
  }
  overlay.localRuns.run = {
    token: 1,
    kind: 'retry',
    runId: 'r',
    startedAt: NOW - 4_000,
    activity: { type: 'tool', name: 'bash', summary: 'npm test', startedAt: null },
  }
  const items = buildBoardItems(sessions, overlay, { now: NOW, timeZoneOffsetMinutes: 0 })
  assert.deepEqual(
    items.map((item) => [item.id, item.lane, item.status]),
    [
      ['wait', 'attention', 'approval'],
      ['fail', 'attention', 'failed'],
      ['run', 'running', 'running'],
      ['done', 'done', 'done'],
    ],
  )
  const [wait, fail, run, done] = items
  // 审批详情的 createdAt 比会话修改时间更接近真实等待起点。
  assert.equal(wait.since, NOW - 90_000)
  assert.equal(wait.severity, 'standard')
  assert.equal(wait.approvalsState, 'ready')
  assert.equal(wait.activity, null)
  assert.equal(fail.title, '升级 Tauri 到 2.5。')
  assert.equal(fail.approvalsState, 'none')
  assert.equal(fail.severity, null)
  assert.equal(run.startedAt, NOW - 4_000)
  assert.deepEqual(run.activity, overlay.localRuns.run.activity)
  assert.equal(run.localRun, true)
  assert.equal(done.unread, true)
  assert.equal(done.pinned, true)

  // 乐观补丁只叠加在派生视图上：标记已处理后失败离开“需要你”。
  overlay.patches.fail = [{ token: 1, patch: { unread: false } }]
  const handled = buildBoardItems(sessions, overlay, { now: NOW, timeZoneOffsetMinutes: 0 })
  const handledFail = handled.find((item) => item.id === 'fail')
  assert.deepEqual(
    [handledFail.lane, handledFail.status, handledFail.handled],
    ['done', 'failed', true],
  )

  // 审批卡还没有详情时显示加载，而不是空命令。
  const loading = buildBoardItems(sessions, emptyOverlay(), { now: NOW, timeZoneOffsetMinutes: 0 })
  assert.equal(loading.find((item) => item.id === 'wait').approvalsState, 'loading')

  const pisper = filterBoardItems(items, { projects: ['/w/pisper'] })
  assert.deepEqual(
    pisper.map((item) => item.id),
    ['wait', 'run', 'done'],
  )
  assert.deepEqual(
    filterBoardItems(items, { lanes: ['attention'] }).map((item) => item.id),
    ['wait', 'fail'],
  )
  assert.deepEqual(filterBoardItems(items, {}).length, 4)
  const lanes = groupBoardLanes(items)
  assert.deepEqual(Object.keys(lanes), ['attention', 'running', 'queued', 'done'])
  assert.equal(lanes.queued.length, 0)
  assert.deepEqual(boardProjects(items), [
    { key: '/w/pisper', cwd: '/w/pisper', count: 3 },
    { key: '/w/app', cwd: '/w/app', count: 1 },
  ])

  const fleet = summarizeFleet(items)
  assert.deepEqual(
    [
      fleet.running,
      fleet.queued,
      fleet.attention,
      fleet.approvals,
      fleet.failures,
      fleet.questions,
    ],
    [1, 0, 2, 1, 1, 0],
  )
  assert.equal(fleet.longestWaiting.sessionId, 'wait')
  assert.equal(fleet.longestWaiting.summary, 'npm test')
  assert.equal(fleet.latest.sessionId, 'fail')
  assert.equal(
    boardSessionTitle({ id: 'x', firstMessage: 'Fix the flaky test. Then ship' }),
    'Fix the flaky test.',
  )
  assert.equal(boardSessionTitle({ id: 'x', firstMessage: 'line one\nline two' }), 'line one')
})

test('one retained sync owns polling and shares the authoritative session cache', async () => {
  const calls = installFetch((path) =>
    path === '/api/sessions' ? json({ sessions: [{ id: 's', streaming: true }] }) : json({}),
  )
  const releaseA = retainBoardSync()
  const releaseB = retainBoardSync()
  await until(() => queryClient.getQueryData(BOARD_SESSIONS_QUERY_KEY) !== undefined)
  assert.equal(boardSyncActive(), true)
  assert.equal(calls.filter((call) => call.path === '/api/sessions').length, 1)
  const cached = queryClient.getQueryData(BOARD_SESSIONS_QUERY_KEY)
  assert.deepEqual(cached, { sessions: [{ id: 's', streaming: true }] })
  // 聊天目录读取命中同一份缓存，不再出现第二个列表所有者。
  assert.equal(await chatApi.listSessions({ refresh: false }), cached)
  // 首次观察到运行即记录起点，保持运行中泳道的稳定顺序。
  assert.equal(typeof useBoardOverlay.getState().runningSince.s, 'number')
  releaseA()
  releaseA()
  releaseB()
  // 释放后有短暂宽限，路由切换的卸载/挂载不会把同步停掉再重开。
  assert.equal(boardSyncActive(), true)
})

test('polling stops while the page is hidden and refreshes as soon as it is visible again', async (t) => {
  const fakeDocument = Object.assign(new EventTarget(), { visibilityState: 'visible' })
  globalThis.document = fakeDocument
  t.after(() => {
    delete globalThis.document
  })
  const calls = installFetch(() => json({ sessions: [] }))
  const listCalls = () => calls.filter((call) => call.path === '/api/sessions').length
  retainBoardSync()
  await until(() => listCalls() === 1)
  fakeDocument.visibilityState = 'hidden'
  fakeDocument.dispatchEvent(new Event('visibilitychange'))
  await wait(3_300)
  assert.equal(listCalls(), 1, 'a hidden page must not keep polling')
  fakeDocument.visibilityState = 'visible'
  fakeDocument.dispatchEvent(new Event('visibilitychange'))
  await until(() => listCalls() === 2)
})

test('approval cards fetch /live with a concurrency cap and drop details once handled', async () => {
  const sessions = Array.from({ length: 8 }, (_, index) => ({
    id: `s${index}`,
    needsAttention: true,
    attentionReason: 'approval',
    streaming: true,
  }))
  sessions.push({ id: 'failed', needsAttention: true, attentionReason: 'failure', unread: true })
  const live = []
  installFetch((path) => {
    if (path === '/api/sessions') return json({ sessions })
    const match = /^\/api\/sessions\/([^/]+)\/live$/.exec(path)
    if (match) {
      live.push(match[1])
      return json({
        streaming: true,
        approvals: [
          { id: `p-${match[1]}`, toolName: 'bash', args: { command: 'ls' }, reason: 'x' },
        ],
      })
    }
    return json({})
  })
  retainBoardSync()
  // 第一条立即发出，其余错开；单轮最多 BOARD_LIVE_LIMIT 个，失败卡不取 /live。
  await until(() => live.length >= BOARD_LIVE_LIMIT, 4_000)
  await until(() => useBoardOverlay.getState().live[`s${BOARD_LIVE_LIMIT - 1}`]?.status === 'ready')
  await wait(300)
  assert.equal(live.length, BOARD_LIVE_LIMIT)
  assert.equal(new Set(live).size, BOARD_LIVE_LIMIT)
  assert.ok(!live.includes('failed'))
  const detail = useBoardOverlay.getState().live.s0
  assert.equal(detail.status, 'ready')
  assert.deepEqual(
    detail.approvals.map((approval) => approval.id),
    ['p-s0'],
  )

  // 会话离开“需要你”后清掉审批详情。
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, {
    sessions: sessions.map((session) => ({
      ...session,
      needsAttention: false,
      attentionReason: null,
    })),
  })
  assert.deepEqual(Object.keys(useBoardOverlay.getState().live), [])
})

function seedApproval(severity = 'standard') {
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, {
    sessions: [{ id: 's', needsAttention: true, attentionReason: 'approval', streaming: true }],
  })
  useBoardOverlay.setState({
    live: {
      s: {
        status: 'ready',
        approvals: parseBoardLiveDetail({
          approvals: [
            {
              id: 'p1',
              toolName: 'bash',
              args: { command: 'git reset --hard' },
              ...(severity === 'critical'
                ? { reason: '命令守卫拦截：丢弃改动。' }
                : { reason: 'Shell' }),
            },
          ],
        }).approvals,
        error: '',
        activity: null,
        startedAt: null,
        fetchedAt: NOW,
        failures: 0,
      },
    },
  })
}

test('approving applies an optimistic patch, posts the decision and clears the patch after refresh', async () => {
  seedApproval()
  const posted = deferred()
  const calls = installFetch(async (path, method) => {
    if (method === 'POST') {
      await posted.promise
      return json({ found: true, approved: true })
    }
    return json({
      sessions: [{ id: 's', streaming: true, needsAttention: false, attentionReason: null }],
    })
  })
  const result = approveBoardApproval('s', 'p1')
  await tick()
  const state = useBoardOverlay.getState()
  assert.equal(state.pending[boardActionKey('approve', 's', 'p1')], 'approve')
  assert.deepEqual(state.patches.s[0].patch, {
    needsAttention: false,
    attentionReason: null,
    streaming: true,
  })
  assert.deepEqual(state.live.s.approvals, [])
  // 同一条审批的允许与拒绝互斥。
  assert.deepEqual(await denyBoardApproval('s', 'p1'), {
    ok: false,
    reason: 'pending',
    error: null,
  })
  posted.resolve()
  assert.deepEqual(await result, { ok: true })
  const post = calls.find((call) => call.method === 'POST')
  assert.equal(post.path, '/api/sessions/s/approvals/p1')
  assert.deepEqual(post.body, { approved: true })
  assert.deepEqual(useBoardOverlay.getState().patches, {})
  assert.deepEqual(useBoardOverlay.getState().pending, {})
})

test('a failed approval rolls back the optimistic state', async () => {
  seedApproval()
  installFetch((path, method) =>
    method === 'POST'
      ? json({ error: 'boom' }, 500)
      : json({ sessions: [{ id: 's', needsAttention: true, attentionReason: 'approval' }] }),
  )
  const result = await approveBoardApproval('s', 'p1')
  assert.equal(result.ok, false)
  assert.equal(result.reason, 'failed')
  const state = useBoardOverlay.getState()
  assert.deepEqual(state.patches, {})
  assert.deepEqual(state.pending, {})
  assert.deepEqual(
    state.live.s.approvals.map((approval) => approval.id),
    ['p1'],
  )
})

test('critical or unknown approvals require the confirmation layer before posting', async () => {
  seedApproval('critical')
  const calls = installFetch(() => json({ found: true }))
  const blocked = await approveBoardApproval('s', 'p1')
  assert.equal(blocked.ok, false)
  assert.equal(blocked.reason, 'confirmation-required')
  assert.equal((await approveBoardApproval('s', 'unknown')).reason, 'confirmation-required')
  assert.equal(calls.filter((call) => call.method === 'POST').length, 0)
  // 拒绝永远可以一步完成。
  assert.deepEqual(await denyBoardApproval('s', 'p1'), { ok: true })
  installFetch(() => json({ found: true }))
  seedApproval('critical')
  assert.deepEqual(await approveBoardApproval('s', 'p1', { confirmCritical: true }), { ok: true })
})

test('an approval already handled elsewhere is not reported as a failure', async () => {
  seedApproval()
  installFetch((path, method) =>
    method === 'POST' ? json({ error: '授权请求不存在。' }, 404) : json({ sessions: [] }),
  )
  assert.deepEqual(await approveBoardApproval('s', 'p1'), { ok: true })
})

test('retry is owned by the board store until the stream ends', async () => {
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, {
    sessions: [{ id: 's', needsAttention: true, attentionReason: 'failure', unread: true }],
  })
  retainBoardSync()
  const finish = deferred()
  const calls = installFetch((path, method) => {
    if (path === '/api/sessions/s/retry' && method === 'POST')
      return sseResponse(
        [
          'event: run\ndata: {"runId":"r1","sessionId":"s"}\n\n',
          'id: 1\nevent: tool_start\ndata: {"id":"t","name":"bash","args":{"command":"npm test"}}\n\n',
        ],
        { hold: finish.promise },
      )
    return json({
      sessions: [{ id: 's', streaming: true, needsAttention: true, attentionReason: 'failure' }],
    })
  })
  assert.deepEqual(await retryBoardSession('s'), { ok: true })
  assert.ok(calls.some((call) => call.path === '/api/sessions/s/retry'))
  await until(() => useBoardOverlay.getState().localRuns.s?.activity?.type === 'tool')
  const run = useBoardOverlay.getState().localRuns.s
  assert.equal(run.kind, 'retry')
  assert.equal(run.runId, 'r1')
  assert.deepEqual(run.activity, {
    type: 'tool',
    name: 'bash',
    summary: 'npm test',
    startedAt: run.activity.startedAt,
  })
  // 同一会话不能同时由看板持有两条运行流。
  assert.equal((await retryBoardSession('s')).ok, false)
  finish.resolve()
  // 干净 EOF 没有终态帧时按传输中断重挂；这里停止同步，确认中断后不残留运行条目。
  resetBoardStore()
  await wait(50)
  assert.deepEqual(useBoardOverlay.getState().localRuns, {})
})

test('held run streams stay inside the connection budget and the oldest is released once accepted', async () => {
  // 浏览器对同一主机最多 6 条连接：看板最多持有 2 条运行流，/live 与持有的流合计不超过 4 条，
  // 给轮询与审批提交留出连接。第三条流受理后，最早的那条被释放，run 在 Runtime 继续。
  assert.ok(BOARD_HELD_STREAM_LIMIT + BOARD_LIVE_LIMIT >= BOARD_CONNECTION_BUDGET)
  assert.ok(BOARD_CONNECTION_BUDGET <= 4)
  assert.ok(BOARD_HELD_STREAM_LIMIT <= 2)
  const ids = ['s1', 's2', 's3']
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, {
    sessions: ids.map((id) => ({
      id,
      needsAttention: true,
      attentionReason: 'failure',
      unread: true,
    })),
  })
  retainBoardSync()
  const finish = deferred()
  const signals = new Map()
  installFetch((path, method, _body, init) => {
    const match = /^\/api\/sessions\/([^/]+)\/retry$/.exec(path)
    if (match && method === 'POST') {
      signals.set(match[1], init.signal)
      // 与真实 fetch 一致：发起方中止时响应体随之报错，读循环立即结束。
      const aborted = new Promise((resolve) =>
        init.signal?.addEventListener('abort', resolve, { once: true }),
      )
      const encoder = new TextEncoder()
      let sent = false
      const stream = new ReadableStream({
        async pull(controller) {
          if (!sent) {
            sent = true
            controller.enqueue(
              encoder.encode(
                `event: run\ndata: {"runId":"r-${match[1]}","sessionId":"${match[1]}"}\n\n`,
              ),
            )
            return
          }
          await Promise.race([finish.promise, aborted])
          if (init.signal?.aborted) controller.error(new DOMException('aborted', 'AbortError'))
          else controller.close()
        },
      })
      return new Response(stream, { status: 200 })
    }
    return json({ sessions: ids.map((id) => ({ id, streaming: true })) })
  })
  for (const id of ids) assert.deepEqual(await retryBoardSession(id), { ok: true })
  await until(() => boardHeldStreamCount() === BOARD_HELD_STREAM_LIMIT)
  assert.equal(signals.get('s1').aborted, true, 'the oldest accepted stream is released')
  assert.equal(signals.get('s2').aborted, false)
  assert.equal(signals.get('s3').aborted, false)
  await until(() => !useBoardOverlay.getState().localRuns.s1)
  assert.deepEqual(Object.keys(useBoardOverlay.getState().localRuns).sort(), ['s2', 's3'])
  finish.resolve()
  resetBoardStore()
  await wait(20)
  assert.equal(boardHeldStreamCount(), 0)
})

test('/live requests share the connection budget with held run streams', async () => {
  const sessions = Array.from({ length: 6 }, (_, index) => ({
    id: `a${index}`,
    needsAttention: true,
    attentionReason: 'approval',
    streaming: true,
  }))
  sessions.push({ id: 'f', needsAttention: true, attentionReason: 'failure', unread: true })
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions })
  const finish = deferred()
  const liveGate = deferred()
  let liveInFlight = 0
  let livePeak = 0
  installFetch(async (path, method) => {
    if (path === '/api/sessions/f/retry' && method === 'POST')
      return sseResponse(['event: run\ndata: {"runId":"r","sessionId":"f"}\n\n'], {
        hold: finish.promise,
      })
    if (/\/live$/.test(path)) {
      liveInFlight += 1
      livePeak = Math.max(livePeak, liveInFlight)
      await liveGate.promise
      liveInFlight -= 1
      return json({ approvals: [] })
    }
    return json({ sessions })
  })
  assert.deepEqual(await retryBoardSession('f'), { ok: true })
  retainBoardSync()
  await wait(BOARD_LIVE_STAGGER_MS * BOARD_LIVE_LIMIT + 100)
  // 1 条持有的流 + /live 不超过连接预算。
  assert.ok(livePeak >= 1)
  assert.ok(livePeak + boardHeldStreamCount() <= BOARD_CONNECTION_BUDGET, `peak ${livePeak}`)
  assert.ok(livePeak <= BOARD_LIVE_LIMIT)
  liveGate.resolve()
  finish.resolve()
})

test('a retry rejected before the run starts rolls back the running patch', async () => {
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [{ id: 's' }] })
  installFetch((path, method) =>
    method === 'POST'
      ? json({ error: '没有可重试的内容。' }, 400)
      : json({ sessions: [{ id: 's' }] }),
  )
  const result = await retryBoardSession('s')
  assert.equal(result.ok, false)
  assert.equal(result.reason, 'failed')
  await tick()
  assert.deepEqual(useBoardOverlay.getState().patches, {})
  assert.deepEqual(useBoardOverlay.getState().localRuns, {})
})

test('organization actions write confirmed fields back and roll back on failure', async (t) => {
  // 整理接口成功后会广播会话更新事件；Node 里用 EventTarget 充当 window。
  globalThis.window = new EventTarget()
  t.after(() => {
    delete globalThis.window
  })
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, {
    sessions: [
      { id: 's', needsAttention: true, attentionReason: 'failure', unread: true, pinned: false },
    ],
  })
  const calls = installFetch((path, method, body) =>
    method === 'PATCH'
      ? json({
          id: 's',
          pinned: body.pinned === true,
          archived: false,
          unread: body.read === true ? false : true,
          needsAttention: true,
          attentionReason: 'failure',
        })
      : json({}),
  )
  assert.deepEqual(await markBoardSessionHandled('s'), { ok: true })
  assert.deepEqual(calls.at(-1).body, { read: true })
  assert.equal(queryClient.getQueryData(BOARD_SESSIONS_QUERY_KEY).sessions[0].unread, false)
  assert.deepEqual(await pinBoardSession('s'), { ok: true })
  assert.equal(queryClient.getQueryData(BOARD_SESSIONS_QUERY_KEY).sessions[0].pinned, true)

  installFetch((path, method) => (method === 'PATCH' ? json({ error: 'nope' }, 500) : json({})))
  const token = applyBoardPatch('other', { pinned: true })
  const failed = await pinBoardSession('s', false)
  assert.equal(failed.ok, false)
  assert.deepEqual(Object.keys(useBoardOverlay.getState().patches), ['other'])
  assert.equal(useBoardOverlay.getState().patches.other[0].token, token)
})

test('stream resume stops promptly when its owner aborts', async () => {
  const controller = new AbortController()
  let resumes = 0
  const started = Date.now()
  const consumption = streamEventsWithResume({
    open: async () => sseResponse(['event: run\ndata: {"runId":"r1"}\n\n']),
    resume: async () => {
      resumes += 1
      return sseResponse([])
    },
    onEvent: () => undefined,
    baseDelayMs: 10_000,
    signal: controller.signal,
  })
  await tick()
  controller.abort()
  await assert.rejects(consumption, (error) => error?.name === 'AbortError')
  assert.equal(resumes, 0)
  assert.ok(Date.now() - started < 5_000)
})

// —— 聊天目录垫片：会话列表读写同一份权威缓存 ——
const catalogCode = transformSync(
  await readFile('src/features/chat/use-session-catalog.ts', 'utf8'),
  { loader: 'ts', format: 'cjs' },
).code

function catalogFixture({ listSessions }) {
  const modules = {
    '@tanstack/react-query': { ...reactQuery, useQueryClient: () => queryClient },
    react: {
      useState: (value) => [typeof value === 'function' ? value() : value, () => {}],
      useRef: (value) => ({ current: value }),
      useCallback: (callback) => callback,
      // 只运行一次 effect，足以建立缓存订阅与初次加载。
      useEffect: (effect) => {
        effect()
      },
    },
    '@/app/brand': { APP_NAME: 'Pisper' },
    '@/app/storage': { STORAGE_KEYS: { activeSession: 'test-active' } },
    '@/app/use-i18n': { useI18n: () => ({ t: (key) => key }) },
    '@/lib/page-state-storage': {
      pageStateStorage: { getItem: () => null, setItem: () => {}, removeItem: () => {} },
    },
    '@/lib/session-state': sessionState,
    '@/lib/plan-protocol': { planFromPayloadOr: (_value, fallback) => fallback },
    './chat-api': {
      SESSIONS_QUERY_KEY,
      chatApi: { listSessions, getConfig: async () => ({ providers: [] }) },
    },
    './chat-errors': { chatErrorMessage: (error) => String(error) },
    './events': {
      announceActiveSession: () => {},
      announceSessionsUpdated: () => {},
      subscribeSessionDeletionUpdates: () => () => {},
      subscribeSessionOrganizationUpdates: () => () => {},
      subscribeSessionTitleUpdates: () => () => {},
    },
    './session-list': sessionList,
  }
  const module = { exports: {} }
  runInNewContext(catalogCode, {
    module,
    exports: module.exports,
    document: {},
    window: {},
    require(id) {
      assert.ok(Object.hasOwn(modules, id), id)
      return modules[id]
    },
  })
  return module.exports.useSessionCatalog({ notify: () => {} })
}

test('the chat catalog mirrors the authoritative cache instead of owning a second list', async () => {
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [{ id: 'a', name: 'A' }] })
  const catalog = catalogFixture({
    listSessions: async () => queryClient.getQueryData(BOARD_SESSIONS_QUERY_KEY),
  })
  await tick()
  const ids = () => catalog.sessionsRef.current.map((session) => session.id)
  assert.deepEqual(ids(), ['a'])
  // 其他所有者（看板轮询、侧栏）写入缓存后，目录镜像同步可见。
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, {
    sessions: [
      { id: 'a', name: 'A2' },
      { id: 'b', name: 'B' },
    ],
  })
  assert.deepEqual(
    catalog.sessionsRef.current.map((session) => session.name),
    ['A2', 'B'],
  )
  // 目录的本地写入回到同一份缓存。
  catalog.updateSessions((current) =>
    current.map((session) => (session.id === 'b' ? { ...session, streaming: true } : session)),
  )
  assert.equal(queryClient.getQueryData(BOARD_SESSIONS_QUERY_KEY).sessions[1].streaming, true)
})

test('foreground recovery keeps the catalog when the runtime briefly reports no sessions', async () => {
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [{ id: 'keep' }] })
  let reads = 0
  const catalog = catalogFixture({
    listSessions: async ({ refresh = true } = {}) => {
      if (!refresh) return queryClient.getQueryData(BOARD_SESSIONS_QUERY_KEY)
      reads += 1
      // 模拟 fetchStartupQuery：请求落地时缓存先同步收到空快照。
      queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [] })
      return { sessions: [] }
    },
  })
  await tick()
  const kept = await catalog.refreshSessions(undefined, { preserveExistingOnEmpty: true })
  assert.equal(reads, 1)
  const ids = () => catalog.sessionsRef.current.map((session) => session.id)
  assert.deepEqual(
    kept.map((session) => session.id),
    ['keep'],
  )
  assert.deepEqual(ids(), ['keep'])
  // 随后的轮询仍是空目录时，短暂窗口内镜像不被清空。
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [] })
  assert.deepEqual(ids(), ['keep'])
  // 普通刷新不保留空目录。
  await catalog.refreshSessions()
  assert.deepEqual(ids(), [])
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [{ id: 'fresh' }] })
  assert.deepEqual(ids(), ['fresh'])
})
