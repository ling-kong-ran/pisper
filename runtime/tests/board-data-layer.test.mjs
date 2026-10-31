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
  DEFAULT_LAUNCH_DEFAULTS,
  LAUNCH_DEFAULTS_PREFERENCE_KEY,
  mergeLaunchChoice,
  normalizeLaunchDefaults,
  safeLaunchExecutionMode,
  useLaunchDefaultsStore,
} from '../../src/features/board/launch-defaults-store.ts'
import {
  BoardLaunchError,
  discardUnsentLaunchSession,
  isUnsentLaunchSession,
  launchBoardTask,
  sendBoardTaskMessage,
} from '../../src/features/board/board-launch.ts'
import { sortProjectsByOrder } from '../../src/features/board/board-model.ts'

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

test('full access applies to one draft only and is never remembered as a launch default', () => {
  // 旧版本写进偏好的完全访问，读取时回落到 Runtime 默认的审批模式。
  assert.equal(normalizeLaunchDefaults({ executionMode: 'full-access' }).executionMode, '')
  assert.equal(
    useLaunchDefaultsStore.persist
      .getOptions()
      .merge({ executionMode: 'full-access', cwd: '/work/a' }, useLaunchDefaultsStore.getState())
      .executionMode,
    '',
  )
  assert.equal(safeLaunchExecutionMode('workspace-write'), 'workspace-write')
  assert.equal(safeLaunchExecutionMode('full-access'), '')
  // 记住一次带完全访问的派发：其余字段照常更新，审批模式保留上一次的安全模式。
  const remembered = { ...DEFAULT_LAUNCH_DEFAULTS, executionMode: 'workspace-write' }
  assert.deepEqual(
    mergeLaunchChoice(remembered, {
      cwd: '/work/next',
      executionMode: 'full-access',
      runMode: 'goal',
    }),
    { ...remembered, cwd: '/work/next', runMode: 'goal' },
  )
  assert.equal(
    mergeLaunchChoice(DEFAULT_LAUNCH_DEFAULTS, { executionMode: 'full-access' }).executionMode,
    '',
  )
  assert.equal(
    mergeLaunchChoice(remembered, { executionMode: 'approval-required' }).executionMode,
    'approval-required',
  )
  // 经 store 的写入同样不会落下完全访问。
  const store = useLaunchDefaultsStore.getState()
  try {
    store.rememberLaunch({ executionMode: 'workspace-write' })
    useLaunchDefaultsStore.getState().rememberLaunch({ executionMode: 'full-access', cwd: '/w' })
    assert.equal(useLaunchDefaultsStore.getState().executionMode, 'workspace-write')
    assert.equal(useLaunchDefaultsStore.getState().cwd, '/w')
  } finally {
    useLaunchDefaultsStore.getState().resetLaunchDefaults()
  }
})

test('project order follows remembered positions, then card counts', () => {
  const projects = [
    { key: '/a', count: 1 },
    { key: '/b', count: 5 },
    { key: '/c', count: 2 },
    { key: '/d', count: 9 },
  ]
  assert.deepEqual(
    sortProjectsByOrder(projects, ['/c', '/a']).map((project) => project.key),
    ['/c', '/a', '/d', '/b'],
  )
})

test('an unsent launch session is recognised only from a confirmed empty, idle snapshot', () => {
  assert.equal(
    isUnsentLaunchSession({ streaming: false, configurationBusy: false, messages: [] }),
    true,
  )
  assert.equal(isUnsentLaunchSession({ streaming: false, messages: [], queuedInputs: [] }), true)
  for (const live of [
    null,
    'empty',
    { streaming: false, messages: [{ id: 'u', role: 'user' }] },
    { streaming: true, messages: [] },
    { streaming: false, configurationBusy: true, messages: [] },
    { streaming: false, messages: [], queuedInputs: [{ id: 'q' }] },
    { streaming: false },
    { messages: [] },
  ])
    assert.equal(isUnsentLaunchSession(live), false, JSON.stringify(live))
})

function launchRequest(overrides = {}) {
  return {
    prompt: '修复 CI',
    cwd: '/work/pisper',
    model: '',
    thinkingLevel: '',
    executionMode: '',
    runMode: 'plan',
    ...overrides,
  }
}

const createdSession = {
  id: 'fresh',
  cwd: '/work/pisper',
  model: 'openai/gpt-5',
  thinkingLevel: 'medium',
  executionMode: 'approval-required',
  runMode: 'plan',
  messageCount: 0,
  modified: iso(0),
}

test('a dispatch rejected before the first message is accepted deletes the empty session', async (t) => {
  globalThis.window = new EventTarget()
  t.after(() => {
    delete globalThis.window
  })
  const deletions = []
  window.addEventListener('pisper:sessions-updated', (event) => deletions.push(event.detail))
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [] })
  const calls = installFetch((path, method) => {
    if (path === '/api/sessions' && method === 'POST') return json(createdSession)
    if (path === '/api/chat')
      return json({ error: '当前 Runtime 不支持 goals 能力。', capability: 'goals' }, 409)
    if (path === '/api/sessions/fresh/live')
      return json({ id: 'fresh', streaming: false, configurationBusy: false, messages: [] })
    if (path === '/api/sessions/fresh' && method === 'DELETE')
      return json({ deleted: true, id: 'fresh' })
    return json({ sessions: [] })
  })
  await assert.rejects(
    launchBoardTask(launchRequest()),
    (error) => error instanceof BoardLaunchError && error.stage === 'send',
  )
  assert.equal(
    calls.filter((call) => call.path === '/api/sessions/fresh' && call.method === 'DELETE').length,
    1,
  )
  // 先确认会话确实是空的，再删除。
  assert.ok(
    calls.findIndex((call) => call.path === '/api/sessions/fresh/live') <
      calls.findIndex((call) => call.method === 'DELETE'),
  )
  assert.ok(
    !queryClient
      .getQueryData(BOARD_SESSIONS_QUERY_KEY)
      .sessions.some((session) => session.id === 'fresh'),
  )
  assert.deepEqual(deletions, [{ deletedIds: ['fresh'] }])
  assert.deepEqual(useBoardOverlay.getState().patches, {})
})

test('a failure while configuring the new session also removes it', async (t) => {
  globalThis.window = new EventTarget()
  t.after(() => {
    delete globalThis.window
  })
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [] })
  const calls = installFetch((path, method) => {
    if (path === '/api/sessions' && method === 'POST') return json(createdSession)
    if (path === '/api/sessions/fresh/model') return json({ error: 'model unavailable' }, 400)
    if (path === '/api/sessions/fresh/live')
      return json({ id: 'fresh', streaming: false, messages: [] })
    if (path === '/api/sessions/fresh' && method === 'DELETE')
      return json({ deleted: true, id: 'fresh' })
    return json({ sessions: [] })
  })
  await assert.rejects(
    launchBoardTask(launchRequest({ model: 'anthropic/claude-x' })),
    (error) => error instanceof BoardLaunchError && error.stage === 'configure',
  )
  assert.ok(!calls.some((call) => call.path === '/api/chat'))
  assert.ok(calls.some((call) => call.path === '/api/sessions/fresh' && call.method === 'DELETE'))
})

test('a launch that may have reached the runtime keeps its session', async (t) => {
  globalThis.window = new EventTarget()
  t.after(() => {
    delete globalThis.window
  })
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [] })
  // 断网：说不清 Runtime 是否已受理，连 /live 都不查。
  let calls = installFetch((path, method) => {
    if (path === '/api/sessions' && method === 'POST') return json(createdSession)
    if (path === '/api/chat') throw new TypeError('fetch failed')
    return json({ sessions: [] })
  })
  await assert.rejects(launchBoardTask(launchRequest()), BoardLaunchError)
  assert.ok(!calls.some((call) => call.path.endsWith('/live') || call.method === 'DELETE'))

  // 被拒绝，但 /live 显示已经有消息（例如别的客户端抢先发了）：保留。
  calls = installFetch((path, method) => {
    if (path === '/api/sessions' && method === 'POST') return json(createdSession)
    if (path === '/api/chat') return json({ error: 'busy' }, 409)
    if (path === '/api/sessions/fresh/live')
      return json({ streaming: false, messages: [{ id: 'm', role: 'user', text: 'x' }] })
    return json({ sessions: [] })
  })
  await assert.rejects(launchBoardTask(launchRequest()), BoardLaunchError)
  assert.ok(calls.some((call) => call.path === '/api/sessions/fresh/live'))
  assert.ok(!calls.some((call) => call.method === 'DELETE'))
})

test('a failed cleanup keeps the session silently and logs no path or user content', async (t) => {
  globalThis.window = new EventTarget()
  t.after(() => {
    delete globalThis.window
  })
  const warn = t.mock.method(console, 'warn', () => {})
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [{ id: 'fresh' }] })
  installFetch((path, method) => {
    if (path === '/api/sessions/fresh/live') return json({ streaming: false, messages: [] })
    if (method === 'DELETE') return json({ error: '无法删除 /home/alice/secret/修复 CI' }, 500)
    return json({ sessions: [] })
  })
  assert.equal(await discardUnsentLaunchSession('fresh'), false)
  assert.equal(queryClient.getQueryData(BOARD_SESSIONS_QUERY_KEY).sessions[0].id, 'fresh')
  assert.equal(warn.mock.callCount(), 1)
  const logged = warn.mock.calls[0].arguments.join(' ')
  assert.match(logged, /http:500/)
  for (const secret of ['fresh', '/home/alice', '修复 CI', '/api/sessions'])
    assert.doesNotMatch(logged, new RegExp(secret.replace(/[/.*+?^${}()|[\]\\]/g, '\\$&')))
})

// —— 第 2 期：看板事件流 / 看板视图（Runtime 声明 board.events / board.view 时） ——
const { useRuntimeCapabilitiesStore } =
  await import('../../src/stores/runtime-capabilities-store.ts')
const { LEGACY_RUNTIME_CAPABILITIES } = await import('../../src/types/runtime-capabilities.ts')
const {
  BOARD_EVENTS_RETRY_BASE_MS,
  BOARD_VIEW_POLL_INTERVAL_MS,
  boardEventsConnected,
  boardFeedMode,
  BOARD_EVENTS_IDLE_TIMEOUT_MS,
  readBoardSessions,
  refreshBoardSessions,
} = await import('../../src/features/board/board-store.ts')
const { applyBoardFeedEvent, emptyBoardFeed } =
  await import('../../src/features/board/board-feed.ts')

function setBoardCapabilities({ view = false, events = false } = {}) {
  useRuntimeCapabilitiesStore.setState({
    capabilities: {
      ...LEGACY_RUNTIME_CAPABILITIES,
      features: {
        ...LEGACY_RUNTIME_CAPABILITIES.features,
        'board.view': view,
        'board.events': events,
      },
    },
    loaded: true,
  })
}

afterEach(() => {
  useRuntimeCapabilitiesStore.setState({ capabilities: LEGACY_RUNTIME_CAPABILITIES, loaded: false })
})

// 可控的 SSE：测试按需推帧；发起方中止时与真实 fetch 一样让读循环报错退出。
function controllableSse(init) {
  const encoder = new TextEncoder()
  let controller
  let closed = false
  const stream = new ReadableStream({
    start(value) {
      controller = value
    },
  })
  init?.signal?.addEventListener(
    'abort',
    () => {
      if (closed) return
      closed = true
      try {
        controller.error(new DOMException('aborted', 'AbortError'))
      } catch {}
    },
    { once: true },
  )
  return {
    response: new Response(stream, { status: 200 }),
    send(event, data) {
      if (!closed)
        controller.enqueue(encoder.encode(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`))
    },
    end() {
      if (closed) return
      closed = true
      controller.close()
    },
    get aborted() {
      return init?.signal?.aborted === true
    },
  }
}

const cardSession = (id, extra = {}) => ({
  id,
  name: id,
  cwd: '/w/pisper',
  modified: new Date().toISOString(),
  messageCount: 3,
  ...extra,
})

test('the feed reducer replaces on snapshots and drops duplicate, out-of-order and post-resync frames', () => {
  let feed = emptyBoardFeed('events')
  const run = cardSession('run', { streaming: true })
  ;({ feed } = applyBoardFeedEvent(feed, 'snapshot', {
    revision: 10,
    items: [
      { ...run, card: { boardStatus: 'running', tail: ['a', 'b\u001b[31m', 'c', 'd'] } },
      { id: '' },
      'bad',
    ],
  }))
  assert.equal(feed.ready, true)
  assert.equal(feed.revision, 10)
  assert.deepEqual(Object.keys(feed.entries), ['run'])
  // 尾巴最多 3 行，控制字符去掉；会话行不带 card。
  assert.deepEqual(feed.entries.run.card.tail, ['b [31m', 'c', 'd'])
  assert.equal('card' in feed.entries.run.session, false)

  const frame = (revision, extra) => ({
    revision,
    id: 'run',
    session: { ...run, streaming: false },
    card: { boardStatus: 'done', outcome: 'completed', lastAssistantPreview: '完成' },
    ...extra,
  })
  let result = applyBoardFeedEvent(feed, 'session_card', frame(12))
  assert.equal(result.effects.upserts[0].streaming, false)
  feed = result.feed
  assert.equal(feed.entries.run.card.preview, '完成')
  // 重复与乱序：修订号不超过已见修订号的帧被丢弃。
  for (const revision of [12, 11, 5])
    assert.equal(applyBoardFeedEvent(feed, 'session_card', frame(revision)).feed, feed)
  // 帧 id 与会话 id 不一致：只推进修订号，不改条目。
  result = applyBoardFeedEvent(feed, 'session_card', frame(13, { session: cardSession('x') }))
  assert.equal(result.feed.entries.run, feed.entries.run)
  assert.equal(result.feed.revision, 13)
  feed = result.feed
  // 心跳推进修订号；removed 删掉条目并要求补读目录。
  feed = applyBoardFeedEvent(feed, 'heartbeat', { revision: 18, at: 'x' }).feed
  assert.equal(feed.revision, 18)
  // catalog_changed（A9）：只推进修订号、要求补读目录，条目不变；旧修订号的重复帧忽略。
  result = applyBoardFeedEvent(feed, 'catalog_changed', { revision: 20 })
  assert.equal(result.effects.catalog, true)
  assert.equal(result.feed.entries, feed.entries)
  assert.equal(result.feed.revision, 20)
  assert.equal(
    applyBoardFeedEvent(result.feed, 'catalog_changed', { revision: 19 }).effects.catalog,
    false,
  )
  feed = result.feed
  result = applyBoardFeedEvent(feed, 'session_card', { revision: 21, id: 'run', removed: true })
  assert.deepEqual(result.effects.removed, ['run'])
  feed = result.feed
  assert.deepEqual(feed.entries, {})
  // resync_required 之后、新快照之前的增量全部忽略；flush_failed 还要求重连。
  result = applyBoardFeedEvent(feed, 'resync_required', { revision: 30, reason: 'flush_failed' })
  assert.equal(result.effects.resync, true)
  assert.equal(result.effects.reconnect, true)
  feed = result.feed
  assert.equal(applyBoardFeedEvent(feed, 'session_card', frame(40)).feed, feed)
  result = applyBoardFeedEvent(feed, 'snapshot', { revision: 50, items: [] })
  assert.equal(result.feed.awaitingSnapshot, false)
  assert.equal(result.feed.revision, 50)
  // 未知事件与没有修订号的帧不改变状态。
  assert.equal(applyBoardFeedEvent(result.feed, 'mystery', { revision: 60 }).feed, result.feed)
  assert.equal(applyBoardFeedEvent(result.feed, 'session_card', { id: 'run' }).feed, result.feed)
})

test('an old runtime keeps the phase 1 path and never opens the board event stream', async () => {
  setBoardCapabilities({ view: false, events: false })
  const calls = installFetch((path) =>
    path === '/api/sessions'
      ? json({ sessions: [{ id: 'a', needsAttention: true, attentionReason: 'approval' }] })
      : json({ approvals: [{ id: 'p', toolName: 'bash', args: { command: 'ls' } }] }),
  )
  retainBoardSync()
  await until(() => calls.some((call) => /\/live$/.test(call.path)))
  assert.equal(boardFeedMode(), 'legacy')
  assert.equal(
    calls.some((call) => call.path.startsWith('/api/board/events')),
    false,
  )
  assert.equal(
    calls.some((call) => call.path.includes('view=board')),
    false,
  )
  assert.equal(useBoardOverlay.getState().feed.ready, false)
})

test('board.view without events polls the board view and takes approvals from cards', async () => {
  setBoardCapabilities({ view: true, events: false })
  const approval = {
    id: 'p1',
    toolName: 'bash',
    argsPreview: 'npm test',
    risk: 'high',
    severity: 'standard',
    reason: '',
    createdAt: new Date().toISOString(),
  }
  const calls = installFetch((path) => {
    if (path.startsWith('/api/sessions?'))
      return json({
        view: 'board',
        sessions: [
          {
            ...cardSession('wait', { needsAttention: true, attentionReason: 'approval' }),
            card: { boardStatus: 'attention', approvals: [approval] },
          },
        ],
        nextCursor: null,
        total: 1,
        revision: 7,
      })
    return json({ sessions: [] })
  })
  retainBoardSync()
  await until(() => useBoardOverlay.getState().feed.ready)
  assert.equal(boardFeedMode(), 'view')
  const viewCall = calls.find((call) => call.path.startsWith('/api/sessions?'))
  const params = new URL(viewCall.path, 'http://x').searchParams
  assert.equal(params.get('view'), 'board')
  assert.equal(params.get('window'), '7d')
  assert.ok(params.has('tzOffset'))
  // 预览完整的审批不再单独取 /live；看板视图按 2 秒间隔轮询，不再轮询完整目录。
  await wait(BOARD_VIEW_POLL_INTERVAL_MS + 300)
  assert.equal(
    calls.some((call) => /\/live$/.test(call.path)),
    false,
  )
  assert.ok(calls.filter((call) => call.path.startsWith('/api/sessions?')).length >= 2)
  assert.equal(
    calls.some((call) => call.path === '/api/sessions'),
    false,
  )
  const items = buildBoardItems([], useBoardOverlay.getState(), { now: Date.now() })
  assert.equal(items[0].approvalsState, 'ready')
  assert.equal(items[0].approvals[0].command, 'npm test')
})

test('one board event stream per tab owns the feed, reconnects with after and resyncs on gaps', async () => {
  setBoardCapabilities({ view: true, events: true })
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [cardSession('old')] })
  const streams = []
  const calls = installFetch((path, _method, _body, init) => {
    if (path.startsWith('/api/board/events')) {
      const stream = controllableSse(init)
      streams.push({ path, stream })
      return stream.response
    }
    if (path === '/api/sessions') return json({ sessions: [cardSession('old')] })
    return json({})
  })
  const releaseA = retainBoardSync()
  const releaseB = retainBoardSync()
  await until(() => streams.length === 1)
  // 多个持有者共用一条连接；第一次连接不带 after。
  assert.equal(streams[0].path, '/api/board/events')
  assert.equal(boardEventsConnected(), true)
  const run = cardSession('run', { streaming: true })
  streams[0].stream.send('snapshot', {
    revision: 100,
    items: [{ ...run, card: { boardStatus: 'running', tail: ['line'], runId: 'run_1' } }],
  })
  await until(() => useBoardOverlay.getState().feed.ready)
  // 新会话行写回权威目录，供历史、会话页读取。
  assert.ok(queryClient.getQueryData(BOARD_SESSIONS_QUERY_KEY).sessions.some((s) => s.id === 'run'))
  streams[0].stream.send('session_card', {
    revision: 101,
    id: 'run',
    session: run,
    card: { boardStatus: 'running', tail: ['line', 'next'] },
  })
  await until(() => useBoardOverlay.getState().feed.entries.run.card.tail.length === 2)
  // 乱序帧被丢弃。
  streams[0].stream.send('session_card', {
    revision: 99,
    id: 'run',
    session: run,
    card: { boardStatus: 'running', tail: ['stale'] },
  })
  streams[0].stream.send('heartbeat', { revision: 105, at: 'now' })
  await until(() => useBoardOverlay.getState().feed.revision === 105)
  assert.deepEqual(useBoardOverlay.getState().feed.entries.run.card.tail, ['line', 'next'])
  // 断线：带 after=最近修订号重连，期间只有一条连接。
  streams[0].stream.end()
  await until(() => streams.length === 2, BOARD_EVENTS_RETRY_BASE_MS + 2_000)
  assert.equal(streams[1].path, '/api/board/events?after=105')
  // 缺口：resync_required + 新快照整体替换覆盖层，并补读一次完整目录。
  const catalogReads = () => calls.filter((call) => call.path === '/api/sessions').length
  const before = catalogReads()
  streams[1].stream.send('resync_required', { revision: 200, reason: 'gap' })
  streams[1].stream.send('snapshot', { revision: 200, items: [] })
  await until(() => useBoardOverlay.getState().feed.revision === 200)
  assert.deepEqual(useBoardOverlay.getState().feed.entries, {})
  await until(() => catalogReads() > before)
  // flush_failed：断开并不带 after 重连，等新快照。
  streams[1].stream.send('resync_required', { revision: 201, reason: 'flush_failed' })
  await until(() => streams.length === 3)
  assert.equal(streams[2].path, '/api/board/events')
  assert.equal(streams.filter(({ stream }) => !stream.aborted).length >= 1, true)
  releaseA()
  releaseB()
  resetBoardStore()
  await tick()
  assert.equal(streams[2].stream.aborted, true)
  assert.equal(boardEventsConnected(), false)
})

test('the event stream disconnects while the page is hidden and catches up with after', async (t) => {
  const fakeDocument = Object.assign(new EventTarget(), { visibilityState: 'visible' })
  globalThis.document = fakeDocument
  t.after(() => {
    delete globalThis.document
  })
  setBoardCapabilities({ view: true, events: true })
  const streams = []
  installFetch((path, _method, _body, init) => {
    if (path.startsWith('/api/board/events')) {
      const stream = controllableSse(init)
      streams.push({ path, stream })
      return stream.response
    }
    return json({ sessions: [] })
  })
  retainBoardSync()
  await until(() => streams.length === 1)
  streams[0].stream.send('snapshot', { revision: 40, items: [] })
  await until(() => useBoardOverlay.getState().feed.revision === 40)
  fakeDocument.visibilityState = 'hidden'
  fakeDocument.dispatchEvent(new Event('visibilitychange'))
  assert.equal(streams[0].stream.aborted, true)
  assert.equal(boardEventsConnected(), false)
  await wait(BOARD_EVENTS_RETRY_BASE_MS + 200)
  assert.equal(streams.length, 1, 'a hidden page must not reconnect')
  fakeDocument.visibilityState = 'visible'
  fakeDocument.dispatchEvent(new Event('visibilitychange'))
  await until(() => streams.length === 2)
  assert.equal(streams[1].path, '/api/board/events?after=40')
})

test('a rejected event stream falls back to board view polling inside the connection budget', async () => {
  setBoardCapabilities({ view: true, events: true })
  const calls = installFetch((path) => {
    if (path.startsWith('/api/board/events'))
      return json({ error: '看板事件流连接数已达上限。', code: 'board_events_limit' }, 429)
    if (path.startsWith('/api/sessions?'))
      return json({ view: 'board', sessions: [], nextCursor: null, total: 0, revision: 3 })
    return json({ sessions: [] })
  })
  retainBoardSync()
  await until(() => boardFeedMode() === 'view')
  await until(() => calls.some((call) => call.path.startsWith('/api/sessions?')))
  assert.equal(calls.filter((call) => call.path.startsWith('/api/board/events')).length, 1)
  assert.equal(useBoardOverlay.getState().feed.ready, true)
})

test('truncated approval previews fetch /live, and /live shares the budget with the event stream', async () => {
  setBoardCapabilities({ view: true, events: true })
  const sessions = Array.from({ length: 5 }, (_, index) =>
    cardSession(`t${index}`, { needsAttention: true, attentionReason: 'approval' }),
  )
  const streams = []
  const liveGate = deferred()
  let liveInFlight = 0
  let livePeak = 0
  const calls = installFetch(async (path, _method, _body, init) => {
    if (path.startsWith('/api/board/events')) {
      const stream = controllableSse(init)
      streams.push(stream)
      return stream.response
    }
    const match = /^\/api\/sessions\/([^/]+)\/live$/.exec(path)
    if (match) {
      liveInFlight += 1
      livePeak = Math.max(livePeak, liveInFlight)
      await liveGate.promise
      liveInFlight -= 1
      return json({
        approvals: [
          { id: `p-${match[1]}`, toolName: 'bash', args: { command: 'x'.repeat(600) }, reason: '' },
        ],
      })
    }
    return json({ sessions: [] })
  })
  retainBoardSync()
  await until(() => streams.length === 1)
  streams[0].send('snapshot', {
    revision: 1,
    items: [
      ...sessions.map((session) => ({
        ...session,
        card: {
          boardStatus: 'attention',
          approvals: [
            {
              id: `p-${session.id}`,
              toolName: 'bash',
              argsPreview: 'x'.repeat(512),
              argsTruncated: true,
              severity: 'standard',
              createdAt: new Date().toISOString(),
            },
          ],
        },
      })),
      {
        ...cardSession('ok', { needsAttention: true, attentionReason: 'approval' }),
        card: {
          boardStatus: 'attention',
          approvals: [{ id: 'p-ok', toolName: 'bash', argsPreview: 'ls', severity: 'standard' }],
        },
      },
    ],
  })
  await until(() => livePeak >= 1)
  await wait(BOARD_LIVE_STAGGER_MS * BOARD_LIVE_LIMIT + 100)
  // 事件流 1 条 + /live 不超过连接预算；预览完整的卡片不取 /live。
  assert.ok(livePeak + 1 <= BOARD_CONNECTION_BUDGET, `peak ${livePeak}`)
  assert.ok(!Object.keys(useBoardOverlay.getState().live).includes('ok'))
  // 截断的审批在拿到完整参数之前不能一步批准：卡片显示加载中，批准要求确认层。
  let items = buildBoardItems([], useBoardOverlay.getState(), { now: Date.now() })
  assert.equal(items.find((item) => item.id === 't0').approvalsState, 'loading')
  assert.equal(items.find((item) => item.id === 'ok').approvalsState, 'ready')
  assert.equal((await approveBoardApproval('t0', 'p-t0')).reason, 'confirmation-required')
  // 即使经过确认层（confirmCritical），没有完整参数也绝不发批准请求。
  assert.equal(
    (await approveBoardApproval('t0', 'p-t0', { confirmCritical: true })).reason,
    'confirmation-required',
  )
  assert.equal(calls.filter((call) => call.method === 'POST').length, 0)
  liveGate.resolve()
  await until(() => useBoardOverlay.getState().live.t0?.status === 'ready')
  items = buildBoardItems([], useBoardOverlay.getState(), { now: Date.now() })
  const full = items.find((item) => item.id === 't0')
  assert.equal(full.approvalsState, 'ready')
  assert.equal(full.approvals[0].command.length, 600)
})

test('approving with the event stream waits for the confirming frame before clearing the patch', async () => {
  setBoardCapabilities({ view: true, events: true })
  const streams = []
  installFetch((path, method, _body, init) => {
    if (path.startsWith('/api/board/events')) {
      const stream = controllableSse(init)
      streams.push(stream)
      return stream.response
    }
    if (method === 'POST') return json({ found: true, approved: true })
    return json({ sessions: [] })
  })
  retainBoardSync()
  await until(() => streams.length === 1)
  const waiting = cardSession('s', { needsAttention: true, attentionReason: 'approval' })
  const approval = {
    id: 'p1',
    toolName: 'bash',
    argsPreview: 'npm test',
    severity: 'standard',
    createdAt: new Date().toISOString(),
  }
  streams[0].send('snapshot', {
    revision: 1,
    items: [{ ...waiting, card: { boardStatus: 'attention', approvals: [approval] } }],
  })
  await until(() => useBoardOverlay.getState().feed.ready)
  const result = approveBoardApproval('s', 'p1')
  await until(() => Object.keys(useBoardOverlay.getState().patches).length === 1)
  await wait(50)
  // 请求途中算出的旧帧仍带着这条审批：不算确认，补丁保留，卡片不闪回“需要你”。
  streams[0].send('session_card', {
    revision: 2,
    id: 's',
    session: waiting,
    card: { boardStatus: 'attention', approvals: [approval] },
  })
  await wait(50)
  assert.equal(Object.keys(useBoardOverlay.getState().patches).length, 1)
  let items = buildBoardItems([], useBoardOverlay.getState(), { now: Date.now() })
  assert.equal(items[0].lane, 'running')
  streams[0].send('session_card', {
    revision: 3,
    id: 's',
    session: { ...waiting, needsAttention: false, attentionReason: null, streaming: true },
    card: { boardStatus: 'running' },
  })
  assert.deepEqual(await result, { ok: true })
  assert.deepEqual(useBoardOverlay.getState().patches, {})
  items = buildBoardItems([], useBoardOverlay.getState(), { now: Date.now() })
  assert.equal(items[0].lane, 'running')
})

test('mark handled dismisses the failure attention when the runtime supports the board view', async (t) => {
  globalThis.window = new EventTarget()
  t.after(() => {
    delete globalThis.window
  })
  setBoardCapabilities({ view: true, events: false })
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, {
    sessions: [{ id: 's', needsAttention: true, attentionReason: 'failure', unread: true }],
  })
  const calls = installFetch((path, method) =>
    method === 'PATCH'
      ? json({
          id: 's',
          pinned: false,
          archived: false,
          unread: false,
          needsAttention: false,
          attentionReason: null,
        })
      : json({}),
  )
  assert.deepEqual(await markBoardSessionHandled('s'), { ok: true })
  assert.deepEqual(calls.at(-1).body, { read: true, dismissAttention: true })
  assert.equal(queryClient.getQueryData(BOARD_SESSIONS_QUERY_KEY).sessions[0].attentionReason, null)
})

test('the catalog and board rows settle on whichever is newer instead of the feed always winning', async () => {
  setBoardCapabilities({ view: true, events: true })
  const streams = []
  let catalogName = 'Old name'
  installFetch((path, _method, _body, init) => {
    if (path.startsWith('/api/board/events')) {
      const stream = controllableSse(init)
      streams.push(stream)
      return stream.response
    }
    if (path === '/api/sessions')
      return json({ sessions: [cardSession('s', { name: catalogName, streaming: false })] })
    return json({})
  })
  retainBoardSync()
  await until(() => streams.length === 1)
  streams[0].send('snapshot', {
    revision: 1,
    items: [
      {
        ...cardSession('s', { name: 'Old name', streaming: true }),
        card: { boardStatus: 'running' },
      },
    ],
  })
  await until(() => useBoardOverlay.getState().feed.ready)
  await wait(5)
  // 改名（或运行结束）之后的目录重读比条目新：目录保持新值，条目反过来被目录刷新。
  catalogName = 'New name'
  await refreshBoardSessions()
  await tick()
  const row = () => readBoardSessions().find((session) => session.id === 's')
  assert.equal(row().name, 'New name')
  assert.equal(row().streaming, false)
  assert.equal(useBoardOverlay.getState().feed.entries.s.session.name, 'New name')
  assert.equal(useBoardOverlay.getState().feed.entries.s.session.streaming, false)
  // 目录请求之后才收到的帧仍然写回目录。
  await wait(5)
  streams[0].send('session_card', {
    revision: 2,
    id: 's',
    session: cardSession('s', { name: 'Newest', streaming: false }),
    card: { boardStatus: 'done' },
  })
  await until(() => row().name === 'Newest')
  assert.equal(useBoardOverlay.getState().feed.entries.s.session.name, 'Newest')
})

test('a silent half-open event stream is dropped by the idle watchdog and resumes with after', async (t) => {
  // 只模拟 setTimeout：看门狗与重连计时可控，fetch 与流读取仍走真实的微任务。
  t.mock.timers.enable({ apis: ['setTimeout'] })
  setBoardCapabilities({ view: true, events: true })
  const streams = []
  const calls = installFetch((path, _method, _body, init) => {
    if (path.startsWith('/api/board/events')) {
      const stream = controllableSse(init)
      streams.push(stream)
      return stream.response
    }
    return json({ sessions: [] })
  })
  const settle = async () => {
    for (let index = 0; index < 20; index += 1) await tick()
  }
  retainBoardSync()
  await settle()
  assert.equal(streams.length, 1)
  streams[0].send('snapshot', { revision: 7, items: [] })
  await settle()
  assert.equal(useBoardOverlay.getState().feed.revision, 7)
  // 心跳也算活着：每一帧都重新计时。
  t.mock.timers.tick(BOARD_EVENTS_IDLE_TIMEOUT_MS - 1_000)
  streams[0].send('heartbeat', { revision: 8, at: 'x' })
  await settle()
  t.mock.timers.tick(BOARD_EVENTS_IDLE_TIMEOUT_MS - 1_000)
  await settle()
  assert.equal(streams.length, 1)
  assert.equal(streams[0].aborted, false)
  assert.equal(boardEventsConnected(), true)
  // 之后一直静默：超过阈值就中止这条连接，并立即带 after 重连。
  t.mock.timers.tick(1_000)
  await settle()
  assert.equal(streams[0].aborted, true)
  t.mock.timers.tick(0)
  await settle()
  assert.equal(streams.length, 2)
  const opened = calls.filter((call) => call.path.startsWith('/api/board/events'))
  assert.equal(opened.at(-1).path, '/api/board/events?after=8')
  assert.equal(boardEventsConnected(), true)
})

// —— 给已有会话发任务消息（组件工作室的“生成”“让代理修复”，计划 §5.4）——

test('a task message to an empty bound session starts with the skill command and a space, and runs on the board', async (t) => {
  globalThis.window = new EventTarget()
  t.after(() => {
    delete globalThis.window
  })
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [] })
  const finish = deferred()
  const calls = installFetch((path) => {
    if (path === '/api/sessions/bound/live')
      return json({ id: 'bound', streaming: false, messages: [] })
    if (path === '/api/chat')
      return sseResponse(['event: run\ndata: {"runId":"r","sessionId":"bound"}\n\n'], {
        hold: finish.promise,
      })
    return json({ sessions: [] })
  })
  const result = await sendBoardTaskMessage({
    sessionId: 'bound',
    prompt: '  做一个 CI 状态卡片  ',
    commandWhenEmpty: '/skill:pisper-component',
    created: { ...createdSession, id: 'bound', cwd: '/data/custom-ui/ci' },
  })
  assert.deepEqual(result, { sessionId: 'bound', queued: false })
  const chat = calls.find((call) => call.path === '/api/chat')
  // 命令后必须是空格：Pi 按第一个空格切技能名，换行会把提示词也并进技能名。
  assert.equal(chat.body.message, '/skill:pisper-component 做一个 CI 状态卡片')
  assert.equal(chat.body.sessionId, 'bound')
  assert.equal(chat.body.goalMode, false)
  // 新会话立即进入权威缓存，并由本客户端持有运行流（侧栏“运行中”）。
  const cached = queryClient.getQueryData(BOARD_SESSIONS_QUERY_KEY).sessions
  assert.equal(cached[0].id, 'bound')
  assert.equal(cached[0].firstMessage, '做一个 CI 状态卡片')
  assert.ok(useBoardOverlay.getState().localRuns.bound)
  finish.resolve()
})

test('a task message to a session that already has messages is sent without the skill command', async (t) => {
  globalThis.window = new EventTarget()
  t.after(() => {
    delete globalThis.window
  })
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, {
    sessions: [{ ...createdSession, id: 'bound' }],
  })
  const finish = deferred()
  const calls = installFetch((path) => {
    if (path === '/api/sessions/bound/live')
      return json({ id: 'bound', streaming: false, messages: [{ id: 'm', role: 'user' }] })
    if (path === '/api/chat')
      return sseResponse(['event: run\ndata: {"runId":"r","sessionId":"bound"}\n\n'], {
        hold: finish.promise,
      })
    return json({ sessions: [] })
  })
  await sendBoardTaskMessage({
    sessionId: 'bound',
    prompt: '修复以下错误：\nTypeError: x',
    commandWhenEmpty: '/skill:pisper-component',
  })
  assert.equal(
    calls.find((call) => call.path === '/api/chat').body.message,
    '修复以下错误：\nTypeError: x',
  )
  finish.resolve()
})

test('a task message to a running session is queued as a follow-up instead of interrupting it', async (t) => {
  globalThis.window = new EventTarget()
  t.after(() => {
    delete globalThis.window
  })
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [] })
  const calls = installFetch((path) => {
    if (path === '/api/sessions/bound/live')
      return json({ id: 'bound', streaming: true, messages: [{ id: 'm', role: 'user' }] })
    if (path === '/api/sessions/bound/input')
      return json({ queued: true, inputId: 'i', pendingMessageCount: 1 })
    return json({ sessions: [] })
  })
  const result = await sendBoardTaskMessage({ sessionId: 'bound', prompt: '再加一个图例' })
  assert.deepEqual(result, { sessionId: 'bound', queued: true })
  const input = calls.find((call) => call.path === '/api/sessions/bound/input')
  assert.deepEqual(input.body, { message: '再加一个图例', attachments: [], behavior: 'followUp' })
  assert.ok(!calls.some((call) => call.path === '/api/chat'))
  assert.deepEqual(useBoardOverlay.getState().patches, {})
})

test("a rejected task message keeps the caller's session and rejects with the send stage", async (t) => {
  globalThis.window = new EventTarget()
  t.after(() => {
    delete globalThis.window
  })
  queryClient.setQueryData(BOARD_SESSIONS_QUERY_KEY, { sessions: [] })
  const calls = installFetch((path) => {
    if (path === '/api/sessions/bound/live')
      return json({ id: 'bound', streaming: false, messages: [] })
    if (path === '/api/chat') return json({ error: 'busy' }, 409)
    return json({ sessions: [] })
  })
  await assert.rejects(
    sendBoardTaskMessage({ sessionId: 'bound', prompt: 'x' }),
    (error) => error instanceof BoardLaunchError && error.stage === 'send',
  )
  assert.ok(!calls.some((call) => call.method === 'DELETE'))
  assert.deepEqual(useBoardOverlay.getState().patches, {})
  // 空提示词与不像技能命令的前缀直接拒绝，不发请求。
  const before = calls.length
  await assert.rejects(
    sendBoardTaskMessage({ sessionId: 'bound', prompt: '   ' }),
    BoardLaunchError,
  )
  await assert.rejects(
    sendBoardTaskMessage({ sessionId: 'bound', prompt: 'x', commandWhenEmpty: '/skill:a\nb' }),
    BoardLaunchError,
  )
  assert.equal(calls.length, before)
})
