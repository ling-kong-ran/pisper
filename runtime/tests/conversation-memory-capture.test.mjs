import assert from 'node:assert/strict'
import { getEventListeners } from 'node:events'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { ConversationMemoryCapture } from '../runtime/conversation-memory-capture.mjs'
import { extractConversationMemories } from '../services/memory/conversation-memory.mjs'
import { awaitMemoryOperation } from '../services/memory/abortable-memory-operation.mjs'

const user = '记住以后默认使用中文回答。'
const candidate = {
  title: '语言偏好',
  content: '默认使用中文回答',
  topic: 'user.language',
  type: 'preference',
  scope: 'global',
  evidence: user,
  confidence: 0.9,
}
const usage = {
  input: 12,
  output: 4800,
  cacheRead: 0,
  cacheWrite: 0,
  totalTokens: 4812,
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
}
const response = (text = JSON.stringify([candidate])) => ({
  content: [{ type: 'text', text }],
  usage,
  timestamp: Date.now(),
  stopReason: 'stop',
})
const input = {
  sessionId: 'test-session',
  cwd: '.',
  model: { reasoning: true, maxTokens: 16000 },
  user,
  assistant: '好的。',
}

for (const [model, expected] of [
  [{ reasoning: true, maxTokens: 16000 }, 8192],
  [{ reasoning: false, maxTokens: 16000 }, 2048],
  [{ reasoning: true, maxTokens: 3000 }, 3000],
]) {
  test(`memory output budget preserves reasoning room within model cap ${expected}`, async () => {
    let options
    const result = await extractConversationMemories({
      ...input,
      model,
      modelRuntime: {
        async completeSimple(_model, _context, value) {
          options = value
          return response()
        },
      },
    })
    assert.equal(options.maxTokens, expected)
    assert.equal(result.memories.length, 1)
    assert.equal(result.usage.output, 4800)
  })
}

for (const [name, value, code] of [
  ['truncated JSON', { ...response('[{"title":'), stopReason: 'length' }, 'token_limit'],
  [
    'reasoning without content',
    { ...response(), content: [{ type: 'thinking', thinking: 'private reasoning' }] },
    'empty_response',
  ],
  ['empty result', undefined, 'empty_response'],
  ['invalid JSON', response('[broken]'), 'invalid_response'],
  [
    'model error',
    { ...response(), errorMessage: 'secret-key private source', stopReason: 'error' },
    'model_error',
  ],
  ['error stop without message', { ...response(), stopReason: 'error' }, 'model_error'],
]) {
  test(`memory extraction reports ${name} instead of silent empty success`, async () => {
    const result = await extractConversationMemories({
      ...input,
      modelRuntime: {
        async completeSimple() {
          return value
        },
      },
    })
    assert.equal(result.errorCode, code)
    assert.deepEqual(result.memories, [])
    assert.equal(result.usage, value?.usage || null)
    assert.doesNotMatch(JSON.stringify(result), /secret-key|private source|private reasoning/)
  })
}

test('normal empty memory candidates and messages without memory intent remain quiet', async () => {
  let calls = 0
  const modelRuntime = {
    async completeSimple() {
      calls++
      return response('[]')
    },
  }
  assert.equal((await extractConversationMemories({ ...input, modelRuntime })).errorCode, undefined)
  assert.equal(
    (await extractConversationMemories({ ...input, modelRuntime, user: '你好，请解释这段代码。' }))
      .errorCode,
    undefined,
  )
  assert.equal(calls, 1)
})

function fixture(overrides = {}) {
  const diagnostics = []
  const recorded = []
  const proposed = []
  const capture = new ConversationMemoryCapture({
    getModelRuntime: () => ({
      async completeSimple() {
        return response()
      },
    }),
    waitForInitialization: async () => {},
    memory: {
      ensureWorkspaceSpace: async () => 'project',
      propose: (value) => {
        proposed.push(value)
        return value
      },
    },
    recordUsage: async (...args) => recorded.push(args),
    reportFailure: (value) => diagnostics.push(value),
    ...overrides,
  })
  return { capture, diagnostics, recorded, proposed }
}

test('failed extraction still accounts for usage and emits only a stable private diagnostic', async () => {
  const f = fixture({
    getModelRuntime: () => ({
      async completeSimple() {
        return { ...response(), stopReason: 'length' }
      },
    }),
  })
  assert.deepEqual(await f.capture.capture({ ...input, sessionId: 'private/path/or/session' }), [])
  assert.equal(f.recorded[0][2], usage)
  assert.deepEqual(
    f.diagnostics.map((x) => x.code),
    ['token_limit'],
  )
  assert.match(f.diagnostics[0].session, /^[a-f0-9]{12}$/)
  assert.doesNotMatch(JSON.stringify(f.diagnostics), /private|path|key/)
  assert.equal(f.proposed.length, 0)
  await f.capture.dispose()
})

test('usage persistence failure does not discard a supported memory candidate', async () => {
  const f = fixture({
    recordUsage: async () => {
      throw new Error('private database path')
    },
  })
  assert.equal((await f.capture.capture(input)).length, 1)
  assert.equal(f.proposed[0].sourceType, 'conversation')
  assert.deepEqual(
    f.diagnostics.map((x) => x.code),
    ['usage_write_failed'],
  )
  await f.capture.dispose()
})

test('shutdown cancels capture and ignores late SDK results before memory storage closes', async () => {
  let resolveRequest
  let requestSignal
  const started = Promise.withResolvers()
  const f = fixture({
    getModelRuntime: () => ({
      completeSimple(_model, _context, options) {
        requestSignal = options.signal
        started.resolve()
        return new Promise((resolve) => {
          resolveRequest = resolve
        })
      },
    }),
  })
  const task = f.capture.capture(input)
  await started.promise
  await f.capture.dispose()
  assert.equal(requestSignal.aborted, true)
  assert.deepEqual(await task, [])
  resolveRequest(response())
  await Promise.resolve()
  assert.deepEqual(f.proposed, [])
  assert.deepEqual(f.recorded, [])
  assert.deepEqual(f.diagnostics, [])
  assert.equal(f.capture.tasks.size, 0)
  assert.deepEqual(await f.capture.capture(input), [])
})

test('capture deadline settles an unresponsive SDK without leaking its late rejection', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const started = Promise.withResolvers()
  const pending = Promise.withResolvers()
  const f = fixture({
    timeoutMs: 100,
    getModelRuntime: () => ({
      completeSimple() {
        started.resolve()
        return pending.promise
      },
    }),
  })
  const task = f.capture.capture(input)
  await started.promise
  t.mock.timers.tick(100)
  assert.deepEqual(await task, [])
  pending.reject(new Error('late private SDK error'))
  await Promise.resolve()
  assert.deepEqual(
    f.diagnostics.map((x) => x.code),
    ['timeout'],
  )
  assert.equal(f.capture.tasks.size, 0)
  await f.capture.dispose()
})

test('shutdown does not wait forever for shared initialization and leaves it uncancelled', async () => {
  const ready = Promise.withResolvers()
  const f = fixture({ waitForInitialization: () => ready.promise })
  const task = f.capture.capture(input)
  await f.capture.dispose()
  assert.deepEqual(await task, [])
  ready.resolve()
  await Promise.resolve()
  assert.deepEqual(f.proposed, [])
})

test('abortable memory waits release abort listeners on success, failure and cancellation', async () => {
  for (const outcome of ['success', 'failure', 'cancel']) {
    const controller = new AbortController()
    const operation = Promise.withResolvers()
    const wait = awaitMemoryOperation(operation.promise, controller.signal)
    if (outcome === 'success') {
      operation.resolve(42)
      assert.equal(await wait, 42)
    } else if (outcome === 'failure') {
      operation.reject(new Error('failed'))
      await assert.rejects(wait)
    } else {
      controller.abort()
      await assert.rejects(wait)
    }
    assert.equal(getEventListeners(controller.signal, 'abort').length, 0)
  }
})

test('saved GLM-compatible provider survives Runtime restart and captures real SSE content and usage', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-memory-restart-'))
  const requests = []
  const server = createServer(async (request, reply) => {
    if (request.method === 'GET' && request.url === '/v1/models') {
      reply.writeHead(200, { 'content-type': 'application/json' })
      reply.end(JSON.stringify({ data: [{ id: 'glm-5.3' }] }))
      return
    }
    const chunks = []
    for await (const chunk of request) chunks.push(chunk)
    const body = JSON.parse(Buffer.concat(chunks).toString('utf8'))
    requests.push({ body, authorization: request.headers.authorization })
    const chunk = (delta, finish_reason = null) => ({
      id: 'test',
      object: 'chat.completion.chunk',
      created: 0,
      model: 'glm-5.3',
      choices: [{ index: 0, delta, finish_reason }],
    })
    reply.writeHead(200, { 'content-type': 'text/event-stream' })
    reply.write(
      `data: ${JSON.stringify(chunk({ role: 'assistant', reasoning_content: 'Reasoning about a preference.' }))}\n\n`,
    )
    reply.write(`data: ${JSON.stringify(chunk({ content: JSON.stringify([candidate]) }))}\n\n`)
    reply.write(
      `data: ${JSON.stringify({ ...chunk({}, 'stop'), usage: { prompt_tokens: 12, completion_tokens: 4800, total_tokens: 4812 } })}\n\n`,
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
        'test-zai': {
          api: 'openai-completions',
          baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
          models: [
            {
              id: 'glm-5.3',
              name: 'GLM fixture',
              reasoning: true,
              input: ['text'],
              contextWindow: 200000,
              maxTokens: 128000,
            },
          ],
        },
      },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({ 'test-zai': { type: 'api_key', key: 'test-only-memory-key' } }),
  )
  for (let generation = 0; generation < 2; generation++) {
    runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
    // 语义索引是另一条后台模型调用；本测试只验证会话提取的真实传输和重启。
    runtime.memorySummarizer = { summarize: async (entries) => entries.map(() => '') }
    await runtime.init()
    runtime.memory.setSemanticSummarizer(null)
    const captured = await runtime.captureConversationMemory({
      ...input,
      cwd: directory,
      model: runtime.modelRuntime.getModel('test-zai', 'glm-5.3'),
      sessionId: `session-${generation}`,
    })
    assert.equal(captured.length, 1)
    assert.ok(
      runtime.memory
        .listMemories({ spaceId: 'global' })
        .some((memory) => memory.content === candidate.content),
    )
    const records = Object.values(runtime.usageLedger.days).flatMap((day) =>
      Object.entries(day.records),
    )
    const record = records.find(([key]) => key.startsWith(`memory:session-${generation}:`))
    assert.equal(record?.[1].output, 4800)
    if (generation === 1) assert.ok(records.some(([key]) => key.startsWith('memory:session-0:')))
    assert.equal(requests[generation].authorization, 'Bearer test-only-memory-key')
    assert.ok(
      (requests[generation].body.max_tokens || requests[generation].body.max_completion_tokens) >=
        8192,
    )
    await runtime.dispose()
    runtime = null
  }
  assert.equal(requests.length, 2)
})
