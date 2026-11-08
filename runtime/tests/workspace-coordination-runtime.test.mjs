// 真实 Runtime：同一文件夹里的两个会话。模型只是按用户消息里的指令调工具的替身。
// 覆盖：读过之后被隔壁改了 → 写入被拒、提示点名隔壁；session_message 经 call_tool 留言、
// 对方空闲时进留言箱（会话页看得到）、“现在处理”开一轮交给它，转录里标着来自谁；会话页显示谁在这里改文件。
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

const chunk = (delta, finish_reason = null) => ({
  id: 'test',
  object: 'chat.completion.chunk',
  created: 0,
  model: 'fixture',
  choices: [{ index: 0, delta, finish_reason }],
})

function textOf(content) {
  return typeof content === 'string'
    ? content
    : (content || []).map((part) => part?.text || '').join('')
}

// 指令：READ、WRITE:<内容>、TELL:<会话>:<话>；其余（包括别的会话的留言）只回一句话。
// Runtime 会在用户消息前加发送时间。
function toolCallFor(raw) {
  const text = raw.replace(/^<sent_at>[^<]*<\/sent_at>\n/, '')
  if (text.startsWith('READ')) return { name: 'read', arguments: { path: 'notes.txt' } }
  if (text.startsWith('WRITE:'))
    return { name: 'write', arguments: { path: 'notes.txt', content: text.slice(6).trim() } }
  if (text.startsWith('TELL:')) {
    const [, session, message] = text.split(':')
    return {
      name: 'call_tool',
      arguments: { name: 'session_message', arguments: { session, text: message } },
    }
  }
  return null
}

async function waitFor(check, label) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const value = await check()
    if (value) return value
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
  throw new Error(`timed out waiting for ${label}`)
}

test('two sessions in one folder coordinate through the runtime', async (t) => {
  const { AgentRuntimeService } = await import('../runtime/agent-runtime.mjs')
  const directory = await mkdtemp(join(tmpdir(), 'pisper-coordination-runtime-'))
  const workspace = join(directory, 'ws')
  await mkdir(workspace)
  await writeFile(join(workspace, 'notes.txt'), 'v0')
  let calls = 0
  // WAIT：这一轮的回复先挂着，测试放行后才回（模拟对方正在跑）。
  let release = () => {}
  const held = new Promise((resolve) => (release = resolve))
  const server = createServer(async (request, reply) => {
    if (request.method === 'GET' && request.url.endsWith('/models')) {
      reply.writeHead(200, { 'content-type': 'application/json' })
      reply.end(JSON.stringify({ data: [{ id: 'fixture' }] }))
      return
    }
    const parts = []
    for await (const part of request) parts.push(part)
    const body = JSON.parse(Buffer.concat(parts).toString('utf8'))
    const last = body.messages.at(-1)
    reply.writeHead(200, { 'content-type': 'text/event-stream' })
    const send = (value) => reply.write(`data: ${JSON.stringify(value)}\n\n`)
    if (last.role === 'user' && textOf(last.content).endsWith('WAIT')) await held
    const call = last.role === 'user' ? toolCallFor(textOf(last.content)) : null
    if (call) {
      calls += 1
      send(
        chunk({
          role: 'assistant',
          tool_calls: [
            {
              index: 0,
              id: `call_${calls}`,
              type: 'function',
              function: { name: call.name, arguments: JSON.stringify(call.arguments) },
            },
          ],
        }),
      )
      send(chunk({}, 'tool_calls'))
    } else {
      send(chunk({ role: 'assistant', content: 'ok' }))
      send(chunk({}, 'stop'))
    }
    reply.end('data: [DONE]\n\n')
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  let runtime
  t.after(async () => {
    await runtime?.dispose()
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
    await rm(directory, { recursive: true, force: true })
  })
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        'test-coordination': {
          api: 'openai-completions',
          baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
          models: [
            {
              id: 'fixture',
              name: 'Fixture',
              reasoning: false,
              input: ['text'],
              contextWindow: 200000,
              maxTokens: 8192,
            },
          ],
        },
      },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({ 'test-coordination': { type: 'api_key', key: 'test-only-coordination-key' } }),
  )
  runtime = new AgentRuntimeService({ cwd: workspace, dataDir: directory })
  runtime.memorySummarizer = { summarize: async (entries) => entries.map(() => '') }
  await runtime.init()
  runtime.memory.setSemanticSummarizer(null)
  const open = async (title) => {
    const created = await runtime.createSession(title, workspace)
    await runtime.setSessionModel(created.id, 'test-coordination', 'fixture')
    // 审批后写入模式：协调工具不需要批准。
    await runtime.setSessionExecutionMode(created.id, 'approval-required')
    return created.id
  }
  const alpha = await open('Alpha')
  const beta = await open('Beta')
  await runtime.setSessionExecutionMode(alpha, 'full-access')
  await runtime.setSessionExecutionMode(beta, 'full-access')
  const prompt = (sessionId, message) =>
    runtime.streamPrompt({ sessionId, message, send: () => {} })
  const toolResults = async (sessionId) =>
    (await runtime.getExistingSession(sessionId)).session.messages
      .filter((message) => message.role === 'toolResult')
      .map((message) => ({ isError: message.isError, text: textOf(message.content) }))

  await prompt(beta, 'READ')
  await prompt(alpha, 'WRITE: from alpha')
  await prompt(beta, 'WRITE: from beta')
  assert.equal(await readFile(join(workspace, 'notes.txt'), 'utf8'), 'from alpha')
  const refused = (await toolResults(beta)).at(-1)
  assert.equal(refused.isError, true)
  assert.match(refused.text, /notes\.txt has changed since you last read or wrote it/)
  assert.match(refused.text, /session "Alpha" wrote it/)

  // 会话页：Beta 看得到 Alpha 最近在这里改了文件。
  const betaLive = await runtime.getSessionLive(beta)
  assert.equal(betaLive.workspace.neighbours[0].title, 'Alpha')
  assert.deepEqual(betaLive.workspace.neighbours[0].files, ['notes.txt'])

  // 开着的会话页靠看板事件流知道留言来了（空闲会话不轮询 /live）。
  const workspaceEvents = []
  const notify = runtime.boardEvents.sessionWorkspaceChanged.bind(runtime.boardEvents)
  runtime.boardEvents.sessionWorkspaceChanged = (id) => {
    workspaceEvents.push(id)
    notify(id)
  }
  // Alpha 给空闲的 Beta 留言：不替 Beta 开一轮，留言进它的留言箱。
  await prompt(alpha, 'TELL:Beta:I am rewriting notes.txt, please wait')
  assert.match((await toolResults(alpha)).at(-1).text, /Left for "Beta"; it is idle/)
  assert.ok(workspaceEvents.includes(beta))
  const waiting = await runtime.getSessionLive(beta)
  assert.equal(waiting.streaming, false)
  assert.deepEqual(
    waiting.workspace.messages.map((message) => [message.from.title, message.text]),
    [['Alpha', 'I am rewriting notes.txt, please wait']],
  )

  // “现在处理”：后台开一轮，留言成了 Beta 转录里标着来自 Alpha 的一条消息。
  assert.deepEqual(await runtime.deliverSessionMessages(beta), { started: true })
  const delivered = await waitFor(async () => {
    const live = await runtime.getSessionLive(beta)
    const message = live.messages.find((item) => item.fromSessions)
    return !live.streaming && message && live.messages.at(-1)?.role === 'agent' ? live : null
  }, 'the delivered message')
  const message = delivered.messages.find((item) => item.fromSessions)
  assert.equal(message.text, 'I am rewriting notes.txt, please wait')
  assert.deepEqual(message.fromSessions, [{ id: alpha, title: 'Alpha' }])
  assert.equal(delivered.workspace?.messages?.length ?? 0, 0)

  // Beta 正在跑：留言插进它的下一步，不进留言箱。先等上一轮彻底收尾（运行标记放下）。
  await waitFor(
    () => !runtime.sessionRunIsActive(beta, runtime.sessions.get(beta)),
    'the delivered turn to settle',
  )
  const running = prompt(beta, 'WAIT')
  await waitFor(async () => (await runtime.getSessionLive(beta)).streaming, 'beta running')
  await waitFor(
    async () => (await runtime.getExistingSession(beta)).session.isStreaming,
    'beta streaming',
  )
  const now = await runtime.sendSessionMessage(alpha, { session: 'Beta', text: 'heads up' })
  assert.equal(now.delivered, 'now')
  release()
  await running
  const after = await runtime.getSessionLive(beta)
  assert.ok(after.messages.some((item) => item.fromSessions && item.text === 'heads up'))
  assert.equal(after.workspace?.messages?.length ?? 0, 0)

  // 留言只能发给同一文件夹里的会话，找不到时列出这里有哪些。
  await assert.rejects(
    runtime.sendSessionMessage(alpha, { session: 'Nobody', text: 'hi' }),
    /No session in this folder matches "Nobody". Sessions here: "Beta"/,
  )
  const listed = await runtime.listWorkspaceSessions(beta)
  assert.deepEqual(
    listed.map((item) => [item.title, item.recentFiles]),
    [['Alpha', ['notes.txt']]],
  )
})
