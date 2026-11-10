import assert from 'node:assert/strict'
import { appendFile, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

// 受理一条消息到建立 live 之间（装配会话运行时、选工具等）的“准备阶段”：
// 对外算运行中，/live 投影待发消息，停止与并发 prompt 在这段时间里也要有确定的结果。

function residentSession(runtime, id, { messages = [] } = {}) {
  const value = {
    cwd: runtime.cwd,
    name: '准备阶段',
    session: {
      sessionId: id,
      isStreaming: false,
      model: { provider: 'openai', id: 'gpt-5.4' },
      messages,
      abort: async () => {},
      clearQueue: () => {},
      setActiveToolsByName: () => {},
      dispose: () => {},
    },
  }
  runtime.sessions.set(id, value)
  return value
}

async function preparingFixture(t, id, options) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-run-preparing-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.listStoredSessions = async () => []
  runtime.installWorkspaceAssetCapture = () => {}
  runtime.settingsManager = {
    getGlobalSettings: () => ({ defaultProvider: 'openai', defaultModel: 'gpt-5.4' }),
  }
  const value = residentSession(runtime, id, options)
  const ready = Promise.withResolvers()
  // 装配运行时被挂住：模拟新会话首轮加载扩展、等 MCP 的那几秒。
  runtime.getOrCreateSession = async () => {
    await ready.promise
    return value
  }
  // 代替真实的模型运行：和 runSessionPrompt 一样，先看停止标记，再建立 live 并移除准备标记。
  runtime.runSessionPrompt = async (_value, { sessionId }) => {
    if (runtime.startingRuns.get(sessionId)?.aborted) throw new Error('Session was stopped.')
    runtime.liveSessions.set(sessionId, {
      streaming: true,
      text: '',
      tools: [],
      assets: [],
      error: '',
    })
    runtime.startingRuns.delete(sessionId)
    runtime.liveSessions.get(sessionId).streaming = false
  }
  t.after(() => ready.resolve())
  return { runtime, value, ready }
}

test('an accepted prompt reports running and projects its message before the runtime is ready', async (t) => {
  const id = 'session-preparing'
  const { runtime, ready } = await preparingFixture(t, id)
  const run = runtime.streamPrompt({ sessionId: id, message: '帮我看看', send: () => {} })

  assert.equal(runtime.sessionRunIsActive(id), true)
  assert.equal(runtime.streamProjection.sessionStreaming(id), true)
  const live = await runtime.getSessionLive(id)
  assert.equal(live.streaming, true)
  assert.equal(live.configurationBusy, true)
  assert.deepEqual(
    live.messages.map((message) => [message.role, message.text]),
    [['user', '帮我看看']],
  )
  const listed = (await runtime.listSessions()).find((session) => session.id === id)
  assert.equal(listed?.streaming, true)

  ready.resolve()
  await run
  assert.equal(runtime.startingRuns.has(id), false)
  assert.equal(runtime.sessionRunIsActive(id), false)
})

test('a second prompt is rejected while the first one is still preparing', async (t) => {
  const id = 'session-preparing-twice'
  const { runtime, ready } = await preparingFixture(t, id)
  const first = runtime.streamPrompt({ sessionId: id, message: '第一条', send: () => {} })
  await assert.rejects(runtime.streamPrompt({ sessionId: id, message: '第二条', send: () => {} }), {
    code: 'busy',
    params: { subject: 'session' },
  })
  ready.resolve()
  await first
})

test('stopping during preparation ends the run before it starts', async (t) => {
  const id = 'session-preparing-stop'
  const { runtime, ready } = await preparingFixture(t, id)
  const run = runtime.streamPrompt({ sessionId: id, message: '马上停', send: () => {} })
  assert.equal(await runtime.abortSession(id), true)
  // 停止后立刻对读者显示“未运行”，不再投影待发消息。
  const live = await runtime.getSessionLive(id)
  assert.equal(live.streaming, false)
  assert.deepEqual(live.messages, [])

  ready.resolve()
  await assert.rejects(run, /Session was stopped/)
  assert.equal(runtime.startingRuns.has(id), false)
  assert.equal(runtime.liveSessions.has(id), false)
})

test('a failed preparation clears the running state', async (t) => {
  const id = 'session-preparing-fails'
  const { runtime } = await preparingFixture(t, id)
  runtime.getOrCreateSession = async () => {
    throw new Error('运行时装配失败')
  }
  await assert.rejects(
    runtime.streamPrompt({ sessionId: id, message: '装配失败', send: () => {} }),
    /装配失败/,
  )
  assert.equal(runtime.startingRuns.has(id), false)
  assert.equal(runtime.streamProjection.sessionStreaming(id), false)
})

test('the previous run’s live state is not projected while the next one prepares', async (t) => {
  const id = 'session-preparing-stale-live'
  const { runtime, ready } = await preparingFixture(t, id, {
    messages: [
      { role: 'user', content: '上一轮', timestamp: 1 },
      { role: 'assistant', content: [{ type: 'text', text: '上一轮的回答' }], timestamp: 2 },
    ],
  })
  runtime.liveSessions.set(id, {
    streaming: false,
    text: '上一轮的回答',
    tools: [{ id: 'old-tool', name: 'read', status: 'done' }],
    assets: [],
    error: '',
    finishedAt: new Date(2).toISOString(),
  })
  const run = runtime.streamPrompt({ sessionId: id, message: '下一轮', send: () => {} })
  const live = await runtime.getSessionLive(id)
  assert.equal(live.streaming, true)
  assert.deepEqual(live.tools, [])
  assert.equal(
    live.messages.some((message) => message.streaming),
    false,
    'the previous answer must not be shown as streaming',
  )
  assert.equal(live.messages.at(-1).text, '下一轮')
  ready.resolve()
  await run
})

test('a preparing prompt still rebuilds an outdated session runtime', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-run-preparing-version-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  const id = 'session-outdated-runtime'
  let disposed = 0
  const stale = residentSession(runtime, id)
  stale.runtimeVersion = 0
  stale.session.dispose = () => (disposed += 1)
  runtime.sessionRuntimeVersion = 1
  // 自己这次 prompt 的准备标记不能让过期运行时（技能、插件变更后）被沿用。
  runtime.startingRuns.set(id, { aborted: false, message: '', acceptedAt: '' })
  const fresh = { session: { sessionId: id }, runtimeVersion: 1 }
  runtime.sessionLifecycle.findSessionInfo = async () => ({ path: join(directory, 's.jsonl') })
  runtime.sessionLifecycle.openStoredSession = async () => ({ manager: {}, name: '' })
  runtime.createSessionRuntime = async () => fresh
  const value = await runtime.sessionLifecycle.getOrCreateSession(id).catch((error) => error)
  assert.equal(disposed, 1)
  assert.notEqual(value, stale)
})

test('a new session stopped before its first prompt starts is discarded, one with history is kept', async (t) => {
  for (const [id, messages, expected] of [
    ['session-stopped-empty', [], ['session-stopped-empty']],
    ['session-stopped-history', [{ role: 'user', content: '之前的', timestamp: 1 }], []],
  ]) {
    const { runtime, ready } = await preparingFixture(t, id, { messages })
    const deleted = []
    runtime.deleteSession = async (sessionId) => {
      deleted.push(sessionId)
      return true
    }
    const run = runtime.streamPrompt({ sessionId: id, message: '马上停', send: () => {} })
    await runtime.abortSession(id)
    ready.resolve()
    await assert.rejects(run, /Session was stopped/)
    await new Promise((resolve) => setImmediate(resolve))
    assert.deepEqual(deleted, expected)
  }
})

async function bareRuntime(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-review-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.listStoredSessions = async () => []
  runtime.settingsManager = {
    getGlobalSettings: () => ({ defaultProvider: 'openai', defaultModel: 'gpt-5.4' }),
  }
  return { runtime, directory }
}

test('a stop that lands while attachments and memory are prepared keeps the model call from starting', async () => {
  // runSessionPrompt 要一整套引擎才能跑起来，这里守住源码里的顺序：发请求前再看一次停止标记，
  // 起名没存上不会在收尾的 try 之外抛出去（否则会话永远停在“运行中”）。
  const source = await readFile(new URL('../runtime/agent-runtime.mjs', import.meta.url), 'utf8')
  assert.match(
    source,
    /if \(value\.abortedAt\) throw pisperError\('cancelled', 'session'\)\s+await runPromptWithAbortGuard\(value, \(\) => session\.prompt\(prompt, \{ images \}\)\)/,
  )
  assert.match(
    source,
    /await this\.markSessionTitle\(session\.sessionId, firstMessageTitle, false\)\.catch\(/,
  )
})

test('deleting a running session does not wait forever for an engine that never stops', async (t) => {
  const { runtime } = await bareRuntime(t)
  const id = 'session-hung'
  let disposed = 0
  const value = residentSession(runtime, id)
  value.session.isStreaming = true
  value.session.abort = () => new Promise(() => {})
  value.session.dispose = () => (disposed += 1)
  runtime.sessionLifecycle.deleteAbortWaitMs = 40
  const started = Date.now()
  assert.equal(await runtime.deleteSession(id), true)
  assert.ok(Date.now() - started < 2_000)
  assert.equal(disposed, 1)
  assert.equal(runtime.sessions.has(id), false)
})

test('session metadata saves pile into one pending write that carries the latest changes', async (t) => {
  const { runtime, directory } = await bareRuntime(t)
  runtime.sessionMeta.a = { name: 'first' }
  const first = runtime.saveSessionMeta()
  // 第一次还没开始写：后面的保存不再各排一次，开始写时取到的是最新内容。
  runtime.sessionMeta.b = { name: 'second' }
  const second = runtime.saveSessionMeta()
  runtime.sessionMeta.c = { name: 'third' }
  assert.equal(runtime.saveSessionMeta(), second)
  assert.equal(first, second)
  await second
  const stored = JSON.parse(await readFile(join(directory, 'pisper-sessions.json'), 'utf8'))
  assert.deepEqual(Object.keys(stored).sort(), ['a', 'b', 'c'])
  // 落定之后的保存是新的一次写，改动不会丢。
  runtime.sessionMeta.d = { name: 'fourth' }
  const later = runtime.saveSessionMeta()
  assert.notEqual(later, second)
  await later
  assert.ok(
    JSON.parse(await readFile(join(directory, 'pisper-sessions.json'), 'utf8')).d,
    'a save after the write settled must reach the disk',
  )
})

test('board deltas project only the sessions that changed', async (t) => {
  const { runtime } = await bareRuntime(t)
  residentSession(runtime, 'session-a')
  residentSession(runtime, 'session-b')
  const asked = []
  const original = runtime.sessionLifecycle.listSessions.bind(runtime.sessionLifecycle)
  runtime.sessionLifecycle.listSessions = (options) => {
    asked.push(options?.ids || null)
    return original(options)
  }
  const one = await runtime.boardHubItems(['session-a', 'session-gone'])
  assert.deepEqual(asked, [['session-a', 'session-gone']])
  assert.deepEqual([...one.keys()].sort(), ['session-a', 'session-gone'])
  assert.equal(one.get('session-gone'), null)
  assert.deepEqual(
    (await runtime.listSessions({ ids: ['session-b'] })).map((session) => session.id),
    ['session-b'],
  )
  // 快照仍是全部。
  const all = await runtime.boardHubItems()
  assert.deepEqual([...all.keys()].sort(), ['session-a', 'session-b'])
})

test('session usage adds only the history read since last time', async (t) => {
  const { runtime, directory } = await bareRuntime(t)
  const path = join(directory, 'usage.jsonl')
  const line = (input) =>
    `${JSON.stringify({
      type: 'message',
      id: `m${input}`,
      message: { role: 'assistant', usage: { input, output: 1, totalTokens: input + 1 } },
    })}\n`
  await writeFile(path, line(10) + line(20))
  runtime.sessionHistoryPaths.set('session-usage', path)
  const first = await runtime.streamProjection.getSessionTokenUsage('session-usage')
  const cached = runtime.sessionHistoryCache.get(path)
  assert.equal(cached.usageCount, 2)
  // 返回的是副本：调用方往上挂 timing、改数字都不影响累计值。
  first.totalTokens = -1
  await appendFile(path, line(30))
  const second = await runtime.streamProjection.getSessionTokenUsage('session-usage')
  assert.equal(cached.usageCount, 3)
  const fresh = await runtime.streamProjection.getSessionTokenUsage('session-usage')
  assert.deepEqual(second, fresh)
  // 和从头汇总的结果一致。
  const { summarizeSessionUsage } = await import('../runtime/stream-projection.mjs')
  assert.deepEqual(
    { ...second, timing: undefined },
    {
      ...summarizeSessionUsage(cached.entries.map((entry) => entry.message)),
      timing: undefined,
    },
  )
})
