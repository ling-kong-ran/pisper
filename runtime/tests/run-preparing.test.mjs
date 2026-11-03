import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
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
    if (runtime.startingRuns.get(sessionId)?.aborted) throw new Error('会话已停止。')
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
  await assert.rejects(
    runtime.streamPrompt({ sessionId: id, message: '第二条', send: () => {} }),
    /仍在运行/,
  )
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
  await assert.rejects(run, /会话已停止/)
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
