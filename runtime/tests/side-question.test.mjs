// 旁问（/btw）：带主会话上下文的一次性无头请求，不改主会话，SSE 只回给提问的客户端。
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { sessionRuntimeRoutes } from '../http/routes/sessions-runtime.mjs'
import {
  normalizeSideQuestion,
  SIDE_QUESTION_MAX_CHARS,
  sideQuestionContext,
  streamSideQuestion,
} from '../runtime/side-question.mjs'

function fakeSession(overrides = {}) {
  return {
    sessionId: 'parent',
    model: { provider: 'openai', id: 'gpt-5' },
    thinkingLevel: 'high',
    systemPrompt: 'You are Pisper.',
    messages: [
      { role: 'user', content: [{ type: 'text', text: '重构登录模块' }], timestamp: 1 },
      {
        role: 'assistant',
        content: [{ type: 'text', text: '好的，先看 auth.ts' }],
        stopReason: 'stop',
        timestamp: 2,
      },
    ],
    agent: {
      state: {
        tools: [{ name: 'read', description: 'Read a file', parameters: {}, execute: () => {} }],
      },
    },
    ...overrides,
  }
}

function fakeModelRuntime(events, final = { stopReason: 'stop', usage: { totalTokens: 9 } }) {
  const calls = []
  return {
    calls,
    streamSimple(model, context, options) {
      calls.push({ model, context, options })
      return {
        async *[Symbol.asyncIterator]() {
          for (const event of events) yield event
        },
        result: async () => final,
      }
    },
  }
}

test('side questions reuse the parent prefix and leave the parent session untouched', async () => {
  const session = fakeSession()
  const before = JSON.stringify(session.messages)
  const modelRuntime = fakeModelRuntime([
    { type: 'text_delta', delta: '在 ' },
    { type: 'text_delta', delta: 'auth.ts' },
  ])
  const deltas = []
  const result = await streamSideQuestion({
    modelRuntime,
    session,
    question: '刚才说的是哪个文件？',
    onDelta: (text) => deltas.push(text),
  })

  assert.deepEqual(deltas, ['在 ', 'auth.ts'])
  assert.equal(result.text, '在 auth.ts')
  assert.equal(result.toolCallIgnored, false)
  const [call] = modelRuntime.calls
  // 同一模型、思考档位和会话标识；系统提示词与工具定义原样复用，只追加一条旁问消息。
  assert.equal(call.model, session.model)
  assert.equal(call.options.reasoning, 'high')
  assert.equal(call.options.sessionId, 'parent')
  assert.equal(call.context.systemPrompt, 'You are Pisper.')
  assert.deepEqual(call.context.tools, [
    { name: 'read', description: 'Read a file', parameters: {} },
  ])
  assert.equal(call.context.messages.length, 3)
  assert.match(call.context.messages[2].content[0].text, /<side_question>\n刚才说的是哪个文件？/)
  assert.equal(JSON.stringify(session.messages), before)
})

test('side questions never run tools and surface model errors', async () => {
  const toolCall = await streamSideQuestion({
    modelRuntime: fakeModelRuntime([{ type: 'toolcall_start' }], { stopReason: 'toolUse' }),
    session: fakeSession(),
    question: '看看 README',
  })
  assert.deepEqual([toolCall.text, toolCall.toolCallIgnored], ['', true])

  await assert.rejects(
    streamSideQuestion({
      modelRuntime: fakeModelRuntime([], { stopReason: 'error', errorMessage: '额度不足' }),
      session: fakeSession(),
      question: '在吗',
    }),
    /额度不足/,
  )
  await assert.rejects(
    streamSideQuestion({
      modelRuntime: fakeModelRuntime([]),
      session: fakeSession({ model: null }),
      question: '在吗',
    }),
    { code: 'unavailable', params: { subject: 'model', detail: 'pick a model first' } },
  )
  // 思考关闭时不传 reasoning。
  const off = fakeModelRuntime([])
  await streamSideQuestion({
    modelRuntime: off,
    session: fakeSession({ thinkingLevel: 'off' }),
    question: 'x',
  })
  assert.equal('reasoning' in off.calls[0].options, false)
})

test('side question input is trimmed and bounded', () => {
  assert.equal(normalizeSideQuestion('  问题  '), '问题')
  assert.throws(() => normalizeSideQuestion('   '), /question is required/)
  assert.throws(() => normalizeSideQuestion(42), /question is required/)
  assert.throws(() => normalizeSideQuestion('x'.repeat(SIDE_QUESTION_MAX_CHARS + 1)), {
    code: 'too_large',
  })
  // 正在流式输出的会话也只取已完成的消息快照。
  const session = fakeSession({ messages: [] })
  assert.equal(sideQuestionContext(session, 'q').messages.length, 1)
})

test('side question route streams deltas then done, and aborts when the client leaves', async () => {
  const route = sessionRuntimeRoutes.find(
    (item) => item.method === 'POST' && item.path === '/api/sessions/:sessionId/side-question',
  )
  assert.ok(route)
  const frames = []
  let signal
  const listeners = new Map()
  const res = {
    once: (event, listener) => listeners.set(event, listener),
    off: (event) => listeners.delete(event),
  }
  const runtime = {
    async askSideQuestion(sessionId, question, options) {
      assert.deepEqual([sessionId, question], ['s1', '问一下'])
      signal = options.signal
      options.onStart()
      options.onDelta('答')
      return { text: '答', toolCallIgnored: false, usage: null }
    },
  }
  await route.handler({
    runtime,
    res,
    params: { sessionId: 's1' },
    body: async () => ({ question: '问一下' }),
    startSse: () => frames.push(['start']),
    sendSse: (event, data) => frames.push([event, data]),
  })
  assert.deepEqual(frames, [
    ['start'],
    ['delta', { text: '答' }],
    ['done', { text: '答', toolCallIgnored: false, usage: null }],
  ])
  assert.equal(listeners.size, 0)
  assert.equal(signal.aborted, false)

  // 客户端断开：上游请求随之取消。
  let aborted = null
  await route.handler({
    runtime: {
      async askSideQuestion(_id, _question, options) {
        listeners.get('close')()
        aborted = options.signal.aborted
        return { text: '', toolCallIgnored: false, usage: null }
      },
    },
    res,
    params: { sessionId: 's1' },
    body: async () => ({ question: 'q' }),
    startSse: () => {},
    sendSse: () => {},
  })
  assert.equal(aborted, true)
})

test('a real runtime answers a side question from the session context without touching it', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-side-question-'))
  const requests = []
  const server = createServer(async (request, reply) => {
    if (request.method === 'GET' && request.url === '/v1/models') {
      reply.writeHead(200, { 'content-type': 'application/json' })
      reply.end(JSON.stringify({ data: [{ id: 'fixture' }] }))
      return
    }
    const chunks = []
    for await (const chunk of request) chunks.push(chunk)
    const body = JSON.parse(Buffer.concat(chunks).toString('utf8'))
    requests.push(body)
    const answer = requests.length === 1 ? '主任务：先看 auth.ts。' : '你刚才让我看 auth.ts。'
    const chunk = (delta, finish_reason = null) => ({
      id: 'test',
      object: 'chat.completion.chunk',
      created: 0,
      model: 'fixture',
      choices: [{ index: 0, delta, finish_reason }],
    })
    reply.writeHead(200, { 'content-type': 'text/event-stream' })
    reply.write(`data: ${JSON.stringify(chunk({ role: 'assistant', content: answer }))}\n\n`)
    reply.write(
      `data: ${JSON.stringify({ ...chunk({}, 'stop'), usage: { prompt_tokens: 30, completion_tokens: 7, total_tokens: 37 } })}\n\n`,
    )
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
        'test-side': {
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
    JSON.stringify({ 'test-side': { type: 'api_key', key: 'test-only-side-key' } }),
  )
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.memorySummarizer = { summarize: async (entries) => entries.map(() => '') }
  await runtime.init()
  runtime.memory.setSemanticSummarizer(null)
  const created = await runtime.createSession('主任务', directory)
  await runtime.setSessionModel(created.id, 'test-side', 'fixture')
  await runtime.streamPrompt({ sessionId: created.id, message: '重构登录模块', send: () => {} })

  const live = await runtime.getExistingSession(created.id)
  const messagesBefore = JSON.stringify(live.session.messages)
  const fileBefore = await readFile(live.session.sessionFile, 'utf8')
  const deltas = []
  let started = false
  const result = await runtime.askSideQuestion(created.id, '刚才说先看哪个文件？', {
    onStart: () => {
      started = true
    },
    onDelta: (text) => deltas.push(text),
  })

  assert.equal(started, true)
  assert.equal(result.text, '你刚才让我看 auth.ts。')
  assert.equal(deltas.join(''), result.text)
  // 旁问请求带着主会话的完整上下文，最后一条是旁问本身。
  const sideRequest = requests[1]
  const serialized = JSON.stringify(sideRequest.messages)
  assert.match(serialized, /重构登录模块/)
  assert.match(serialized, /主任务：先看 auth\.ts。/)
  assert.match(JSON.stringify(sideRequest.messages.at(-1)), /side_question/)
  // 主会话的消息和会话文件都没有变化。
  assert.equal(JSON.stringify(live.session.messages), messagesBefore)
  assert.equal(await readFile(live.session.sessionFile, 'utf8'), fileBefore)
  // 用量单独记一条，统计不漏算。
  const keys = Object.values(runtime.usageLedger.days).flatMap((day) => Object.keys(day.records))
  assert.ok(keys.some((key) => key.startsWith(`side-question:${created.id}:`)))

  await assert.rejects(runtime.askSideQuestion('missing', '在吗'), /Session not found/)
  await assert.rejects(runtime.askSideQuestion(created.id, '  '), /question is required/)
})
