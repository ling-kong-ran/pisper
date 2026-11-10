// 子代理过程（只读）：对话记录跟着父会话落盘、按父会话与登记表清理，投影成界面消息，
// 经 GET /api/sessions/:id/agents/:agentId/transcript 提供，由 agentTranscripts 能力把关。
import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { mkdir, mkdtemp, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { SessionManager } from '@earendil-works/pi-coding-agent'
import { createApiHandler, requiredRuntimeFeature } from '../http/api-handler.mjs'
import { resolveRuntimeCapabilities } from '../runtime-capabilities.mjs'
import { projectAgentTranscript } from '../runtime/agent-transcript.mjs'
import { MultiAgentService } from '../services/multi-agent-service.mjs'

async function tempDir(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-agent-transcripts-'))
  t.after(() => rm(directory, { recursive: true, force: true }).catch(() => {}))
  return directory
}

async function waitFor(predicate, message = 'condition') {
  const deadline = Date.now() + 2_000
  while (Date.now() < deadline) {
    const value = predicate()
    if (value) return value
    await new Promise((resolve) => setTimeout(resolve, 0))
  }
  throw new Error(`Timed out waiting for ${message}.`)
}

function fakeSession(onPrompt) {
  const session = {
    agent: { state: { systemPrompt: 'Base' } },
    messages: [],
    subscribe: () => () => {},
    async prompt(message) {
      session.messages.push({ role: 'user', content: [{ type: 'text', text: message }] })
      await onPrompt?.(session)
    },
    async abort() {},
    dispose() {},
  }
  return session
}

function service(directory, session, overrides = {}) {
  const files = []
  const value = new MultiAgentService({
    path: join(directory, 'pisper-agents.json'),
    transcriptDir: join(directory, 'transcripts'),
    getModelRuntime: () => ({}),
    getSettingsManager: () => ({}),
    createResourceLoader: async () => ({}),
    createSessionManager: (_cwd, file) => {
      files.push(file)
      return {}
    },
    createSession: async () => ({ session }),
    ...overrides,
  })
  return { service: value, files }
}

const MODEL = { provider: 'openai', id: 'gpt-5', contextWindow: 200_000, maxTokens: 128_000 }

function input(overrides = {}) {
  return {
    parentSessionId: 'parent/1',
    cwd: process.cwd(),
    model: MODEL,
    taskName: 'inspect-recall',
    message: '查撤回实现：只读，不要改文件。',
    allowedTools: ['read', 'bash'],
    toolCallId: 'call_spawn_1',
    ...overrides,
  }
}

const assistantWithTool = (id) => ({
  role: 'assistant',
  content: [
    { type: 'text', text: '先找撤回在哪儿实现。' },
    { type: 'toolCall', id, name: 'bash', arguments: { command: 'grep -rn recall' } },
  ],
  stopReason: 'toolUse',
  timestamp: Date.parse('2026-11-10T06:00:01Z'),
})
const toolResult = (id) => ({
  role: 'toolResult',
  toolCallId: id,
  content: [{ type: 'text', text: 'room-log.mjs:8: recall' }],
  isError: false,
  timestamp: Date.parse('2026-11-10T06:00:02Z'),
})
const assistantText = (text) => ({
  role: 'assistant',
  content: [{ type: 'text', text }],
  stopReason: 'stop',
  timestamp: Date.parse('2026-11-10T06:00:03Z'),
})

test('a sub-agent writes its conversation under the parent session and remembers the spawning call', async (t) => {
  const directory = await tempDir(t)
  const gate = Promise.withResolvers()
  const session = fakeSession(async (active) => {
    active.messages.push(assistantWithTool('call_bash'))
    await gate.promise
    active.messages.push(toolResult('call_bash'), assistantText('撤回只追加一条记录。'))
  })
  const { service: agents, files } = service(directory, session)
  const started = await agents.spawn(input())
  await waitFor(() => files.length === 1, 'session manager')
  // 父会话 id 里的斜杠不会跳出记录目录。
  assert.equal(files[0], join(directory, 'transcripts', 'parent_1', `${started.id}.jsonl`))
  assert.equal(agents.summaries('parent/1')[0].toolCallId, 'call_spawn_1')

  // 运行中：读内存里的会话，最后一步还没结果的工具是“正在执行”。
  await waitFor(() => session.messages.length === 2, 'tool call')
  const running = projectAgentTranscript(agents.transcriptSource('parent/1', started.id))
  assert.equal(running.active, true)
  assert.equal(running.available, true)
  assert.equal(running.task, '查撤回实现：只读，不要改文件。')
  assert.equal(running.taskShown, true)
  const tool = running.messages.at(-1).runActivity.tools[0]
  assert.equal(tool.status, 'running')
  assert.equal(tool.message, '')

  gate.resolve()
  await waitFor(() => agents.summaries('parent/1')[0].status === 'completed', 'completion')
  const done = projectAgentTranscript(agents.transcriptSource('parent/1', started.id))
  assert.equal(done.active, false)
  assert.equal(done.messages.at(-1).text, '撤回只追加一条记录。')
  assert.equal(done.messages.at(-1).runActivity.tools[0].status, 'done')

  // 不是这个父会话的子代理看不到。
  assert.equal(agents.transcriptSource('parent-2', started.id), null)
  await agents.dispose()
})

test('after the session is released the transcript is read back from its file', async (t) => {
  const directory = await tempDir(t)
  const session = fakeSession(async (active) => {
    active.messages.push(assistantText('UI 冒烟 3 个用例全过。'))
  })
  const { service: agents } = service(directory, session, {
    terminalSessionRetentionMs: 0,
    createSessionManager: (cwd, file) => {
      // 用真的 Pi 会话文件：写进同样的消息，模拟引擎落盘。
      const manager = SessionManager.open(file, join(file, '..'), cwd)
      manager.appendMessage({ role: 'user', content: [{ type: 'text', text: '跑一遍 UI 冒烟' }] })
      manager.appendMessage(assistantText('UI 冒烟 3 个用例全过。'))
      return manager
    },
  })
  const started = await agents.spawn(input({ taskName: 'ui-smoke', message: '跑一遍 UI 冒烟' }))
  await waitFor(() => agents.summaries('parent/1')[0].status === 'completed', 'completion')
  await waitFor(() => !agents.records.get(started.id).session, 'release')
  const source = agents.transcriptSource('parent/1', started.id)
  assert.ok(Array.isArray(source.messages))
  const projected = projectAgentTranscript(source)
  assert.equal(projected.task, '跑一遍 UI 冒烟')
  assert.equal(projected.messages.at(-1).text, 'UI 冒烟 3 个用例全过。')
  await agents.dispose()
})

test('agents spawned before transcripts existed report that no transcript was kept', () => {
  const projected = projectAgentTranscript({
    agent: { id: 'a1', message: 'old' },
    active: false,
    messages: null,
  })
  assert.deepEqual(projected, {
    agent: { id: 'a1', message: 'old' },
    active: false,
    available: false,
  })
  assert.equal(projectAgentTranscript(null), null)
})

test('long transcripts keep the latest messages and say how many were left out', () => {
  const raw = [{ role: 'user', content: [{ type: 'text', text: '任务' }] }]
  for (let index = 0; index < 5; index += 1) raw.push(assistantText(`第 ${index} 段`))
  const projected = projectAgentTranscript(
    { agent: { id: 'a1' }, active: false, messages: raw },
    { limit: 3 },
  )
  assert.equal(projected.omitted, 3)
  assert.equal(projected.taskShown, false)
  assert.deepEqual(
    projected.messages.map((message) => message.text),
    ['第 2 段', '第 3 段', '第 4 段'],
  )
  assert.equal(projected.task, '任务')
})

test('transcripts are deleted with the parent session, with pruned records and as orphans at startup', async (t) => {
  const directory = await tempDir(t)
  const transcripts = join(directory, 'transcripts')
  const { service: agents } = service(directory, fakeSession())
  await agents.init()
  const first = await agents.spawn(input())
  const second = await agents.spawn(input({ parentSessionId: 'parent-2', toolCallId: '' }))
  await waitFor(
    () => agents.summaries().every((agent) => agent.status === 'completed'),
    'completion',
  )
  for (const agent of [first, second]) {
    const file = agents.transcriptFile(agents.records.get(agent.id))
    await mkdir(join(file, '..'), { recursive: true })
    await writeFile(file, '{}\n')
  }

  await agents.removeParent('parent/1')
  assert.equal(existsSync(join(transcripts, 'parent_1')), false)
  assert.equal(existsSync(join(transcripts, 'parent-2', `${second.id}.jsonl`)), true)

  agents.removeRecord(agents.records.get(second.id))
  await waitFor(
    () => !existsSync(join(transcripts, 'parent-2', `${second.id}.jsonl`)),
    'pruned transcript',
  )

  // 登记表里没有的文件（进程不在时删掉的父会话）在下次启动时清掉，登记过的留着。
  const kept = await agents.spawn(input({ parentSessionId: 'parent-3' }))
  await waitFor(() => agents.summaries('parent-3')[0]?.status === 'completed', 'completion')
  await agents.save()
  const keptFile = agents.transcriptFile(agents.records.get(kept.id))
  await mkdir(join(transcripts, 'gone'), { recursive: true })
  await writeFile(join(transcripts, 'gone', 'old.jsonl'), '{}\n')
  await mkdir(join(keptFile, '..'), { recursive: true })
  await writeFile(keptFile, '{}\n')
  await writeFile(join(transcripts, 'parent-3', 'stray.jsonl'), '{}\n')
  const { service: restarted } = service(directory, fakeSession())
  await restarted.init()
  await waitFor(() => !existsSync(join(transcripts, 'gone')), 'orphan sweep')
  await waitFor(() => !existsSync(join(transcripts, 'parent-3', 'stray.jsonl')), 'stray file')
  assert.deepEqual(await readdir(join(transcripts, 'parent-3')), [`${kept.id}.jsonl`])
  await agents.dispose()
  await restarted.dispose()
})

test('the transcript route is gated by agentTranscripts and only serves the session own agents', async () => {
  assert.equal(
    requiredRuntimeFeature('/api/sessions/s1/agents/a1/transcript', 'GET'),
    'agentTranscripts',
  )
  assert.equal(requiredRuntimeFeature('/api/sessions/s1/agents', 'GET'), '')
  const capabilities = (profile) =>
    resolveRuntimeCapabilities({
      environment: { PISPER_RUNTIME_PROFILE: profile },
      moduleSupport: { childProcess: true, workerThreads: true, sqlite: true, wasm: true },
    })
  // 与多 Agent 一起提供：手机端没有子代理，也就没有过程可看。
  assert.equal((await capabilities('desktop')).features.agentTranscripts, true)
  assert.equal((await capabilities('mobile-embedded')).features.agentTranscripts, false)

  const calls = []
  const runtime = {
    capabilities: { features: { agentTranscripts: true } },
    getSessionAgentTranscript: (sessionId, agentId) => {
      calls.push([sessionId, agentId])
      return agentId === 'a1' ? { agent: { id: 'a1' }, active: false, available: false } : null
    },
  }
  const handler = createApiHandler(runtime)
  const get = async (path) => {
    const response = {
      status: 0,
      body: '',
      writeHead(status) {
        this.status = status
      },
      setHeader() {},
      end(body) {
        this.body = body
      },
    }
    const request = { method: 'GET', headers: {}, async *[Symbol.asyncIterator]() {} }
    await handler(request, response, new URL(`http://localhost${path}`))
    return { status: response.status, body: JSON.parse(response.body) }
  }
  assert.deepEqual((await get('/api/sessions/s1/agents/a1/transcript')).body.agent, { id: 'a1' })
  const missing = await get('/api/sessions/s1/agents/other/transcript')
  assert.equal(missing.status, 404)
  assert.equal(missing.body.code, 'agent_not_found')
  assert.deepEqual(calls, [
    ['s1', 'a1'],
    ['s1', 'other'],
  ])
})

test('the client parses transcripts defensively and matches spawn calls to their sub-agents', async () => {
  const { agentForSpawnCall, parseAgentTranscript, parseSessionAgents, sessionHasSubagents } =
    await import('../../src/features/chat/session-agents.ts')
  const parsed = parseAgentTranscript({
    available: true,
    active: 'yes',
    task: '任务',
    taskShown: true,
    omitted: -3,
    messages: [{ id: 'm1', role: 'user', text: '任务' }, { role: 'agent' }, null, 'x'],
  })
  assert.equal(parsed.active, false)
  assert.equal(parsed.omitted, 0)
  assert.deepEqual(
    parsed.messages.map((message) => message.id),
    ['m1'],
  )
  assert.equal(parseAgentTranscript(null).available, false)

  const agents = parseSessionAgents({
    agents: [
      { id: 'old', taskName: 'recall-test', status: 'completed' },
      { id: 'a1', taskName: 'recall-test', status: 'running', toolCallId: 'call_1' },
      { id: 'a2', taskName: 'recall-test', status: 'completed', toolCallId: 'call_2' },
    ],
  })
  // 同名任务派了几次：按调用 id 对上；旧记录没有调用 id 时按任务名找最近一个。
  assert.equal(agentForSpawnCall(agents, 'call_2', 'recall-test')?.id, 'a2')
  assert.equal(agentForSpawnCall(agents, 'call_x', 'recall-test')?.id, 'old')
  assert.equal(agentForSpawnCall(agents, '', 'other'), undefined)

  const spawn = { type: 'tool', name: 'spawn_agent', id: 'call_1' }
  assert.equal(sessionHasSubagents([], []), false)
  assert.equal(sessionHasSubagents([], [{ type: 'agent', agent: { id: 'a1' } }]), true)
  assert.equal(
    sessionHasSubagents([{ id: 'r', role: 'agent', runActivity: { tools: [spawn] } }], []),
    true,
  )
  assert.equal(
    sessionHasSubagents(
      [{ id: 'r', role: 'agent', runActivity: { tools: [{ type: 'tool', name: 'bash' }] } }],
      [],
    ),
    false,
  )
})
