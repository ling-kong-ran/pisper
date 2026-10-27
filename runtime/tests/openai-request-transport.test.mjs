import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { createServer } from 'node:http'
import { fork } from 'node:child_process'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createOpenAIRequestFetch } from '../services/openai-request-transport.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { generateOpenAICompatible } from '../services/visual-generation/openai-compatible.mjs'

function responseFrames(text, terminal = {}) {
  const item = {
    id: 'message-fixture',
    type: 'message',
    role: 'assistant',
    content: [{ type: 'output_text', text, annotations: [] }],
  }
  return [
    { type: 'response.created', response: { id: 'response-fixture' } },
    { type: 'response.output_item.added', output_index: 0, item: { ...item, content: [] } },
    { type: 'response.output_text.delta', output_index: 0, content_index: 0, delta: text },
    { type: 'response.output_item.done', output_index: 0, item },
    {
      type: 'response.completed',
      ...terminal,
      response: {
        id: 'response-fixture',
        status: 'completed',
        output: [item],
        usage: { input_tokens: 2, output_tokens: 3, total_tokens: 5 },
        ...terminal.response,
      },
    },
  ]
    .map((event) => `data: ${JSON.stringify(event)}\n\n`)
    .join('')
}

async function within(promise, message, milliseconds = 10_000) {
  let timer
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(message)), milliseconds)
      }),
    ])
  } finally {
    clearTimeout(timer)
  }
}

test('independent runtimes in one workspace finish after gateway terminal frames without HTTP EOF', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-parallel-provider-'))
  const children = []
  let calls = 0
  let activeGatewayRequests = 0
  let bothRequestsFinished
  const gatewayFinished = new Promise((resolve) => {
    bothRequestsFinished = resolve
  })
  const heldConnectionClosed = Promise.withResolvers()
  async function serveGateway(request, response) {
    for await (const _ of request) {
      /* 先消费请求，避免上传与响应清理竞态。 */
    }
    if (request.method !== 'POST' || !request.url.endsWith('/responses')) {
      response.writeHead(200, { 'Content-Type': 'application/json' })
      response.end(JSON.stringify({ data: [] }))
      return
    }
    calls++
    activeGatewayRequests++
    const index = calls
    response.writeHead(200, { 'Content-Type': 'text/event-stream' })
    // 模型已经结算；第一条 HTTP 响应故意保持打开，第二条正常关闭。
    activeGatewayRequests--
    const frames = responseFrames(`finished-${index}`)
    if (index === 1) {
      response.once('close', heldConnectionClosed.resolve)
      response.write(frames)
    } else response.end(frames)
    if (calls === 2) bothRequestsFinished()
  }
  const server = createServer((request, response) => {
    void serveGateway(request, response).catch(() => response.destroy())
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  t.after(async () => {
    for (const { child } of children) if (child.exitCode === null) child.kill()
    await Promise.all(children.map(({ exited }) => exited))
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
    await rm(directory, { recursive: true, force: true })
  })
  const setup = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  try {
    await setup.init()
    await setup.createProvider({
      id: 'parallel-fixture',
      name: 'Parallel fixture',
      api: 'openai-responses',
      baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
      apiKey: 'fixture-key',
      model: 'fixture-model',
    })
  } finally {
    await setup.dispose()
  }
  for (const name of ['terminal-a', 'terminal-b']) {
    const child = fork(new URL('./fixtures/provider-stream-worker.mjs', import.meta.url), [], {
      cwd: directory,
      execArgv: [],
      silent: true,
      env: {
        ...process.env,
        PISPER_AGENT_DIR: directory,
        PI_SKIP_VERSION_CHECK: '1',
        PI_TELEMETRY: '0',
      },
    })
    child.stdout.resume()
    child.stderr.resume()
    const exited = new Promise((resolve) => child.once('exit', resolve))
    const finished = new Promise((resolve) => child.once('message', resolve))
    children.push({ child, exited, finished })
    child.send({ workspace: directory, dataDir: directory, name })
  }
  await within(gatewayFinished, 'both isolated runtimes must reach the gateway')
  assert.equal(activeGatewayRequests, 0)
  const results = await within(
    Promise.all(children.map(({ finished }) => finished)),
    'gateway has no active requests, but an isolated Runtime still waits for HTTP EOF',
    5_000,
  )
  assert.equal(calls, 2, 'completion must not resubmit either model request')
  assert.deepEqual(
    new Set(results.map((result) => result.events?.[0]?.text)),
    new Set(['finished-1', 'finished-2']),
  )
  for (const result of results) {
    assert.equal(result.type, 'finished')
    assert.equal(result.streaming, false)
    assert.equal(result.events.length, 1)
    assert.equal(result.events[0].event, 'done')
  }
  for (const { exited } of children) assert.equal(await within(exited, 'worker did not exit'), 0)
  await within(heldConnectionClosed.promise, 'completed upstream HTTP response was not cancelled')
})

// 网关已结束推理并发出终态，但故意不结束 HTTP 响应，复现 SDK 等待 EOF 的挂起。
function unfinishedHttpResponse(t, frames) {
  let source
  let cancelled = false
  const deadline = setTimeout(() => {
    source.error(new Error('fixture: provider finished but HTTP body never closed'))
  }, 5_000)
  t.after(() => clearTimeout(deadline))
  const response = new Response(
    new ReadableStream({
      start(controller) {
        source = controller
        const chunks = typeof frames === 'string' ? [new TextEncoder().encode(frames)] : frames
        for (const chunk of chunks) controller.enqueue(chunk)
      },
      cancel() {
        cancelled = true
        clearTimeout(deadline)
      },
    }),
    { headers: { 'Content-Type': 'text/event-stream' } },
  )
  return { response, cancelled: () => cancelled }
}

for (const ending of ['\n', '\r\n', '\r']) {
  test(`Responses terminal framing survives byte splits with ${JSON.stringify(ending)} lines`, async (t) => {
    const prefix = `event: response.completed${ending}`
    const frame = `${prefix}data: {"type":"response.completed",${ending}data: "response":{"status":"completed","metadata":{"text":"中文"}}}${ending}${ending}`
    const bytes = new TextEncoder().encode(frame)
    const source = unfinishedHttpResponse(
      t,
      [...bytes].map((byte) => Uint8Array.of(byte)),
    )
    const response = await createOpenAIRequestFetch(async () => source.response, {
      normalizeResponses: true,
    })('https://relay.example/v1', {})
    const result = await response.text()
    assert.equal(result.trimEnd(), frame.trimEnd())
  })
}

for (const type of ['response.incomplete', 'response.failed', 'error', '[DONE]']) {
  test(`Responses ${type} closes its projection and keeps the terminal payload`, async (t) => {
    const data =
      type === '[DONE]' ? type : JSON.stringify({ type, response: { status: 'fixture' } })
    const frame = `data: ${data}\n\n`
    const source = unfinishedHttpResponse(t, frame + 'data: {"type":"unexpected-late-frame"}\n\n')
    const response = await createOpenAIRequestFetch(async () => source.response, {
      normalizeResponses: true,
    })('https://relay.example/v1', {})
    assert.equal(await response.text(), frame)
  })
}

test('Responses tool argument done is not a response terminal and consumer cancellation reaches upstream', async (t) => {
  const frame =
    'data: {"type":"response.function_call_arguments.done","output_index":0,"arguments":"{}"}\n\n'
  const source = unfinishedHttpResponse(t, frame)
  const response = await createOpenAIRequestFetch(async () => source.response, {
    normalizeResponses: true,
  })('https://relay.example/v1', {})
  const reader = response.body.getReader()
  assert.equal((await reader.read()).done, false)
  assert.equal(source.cancelled(), false)
  await reader.cancel()
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(source.cancelled(), true)
})

for (const api of ['openai-responses', 'openai-completions']) {
  if (api === 'openai-responses') {
    test(`${api} terminal frames finish without waiting for an HTTP EOF`, async (t) => {
      const terminal = 'data: {"type":"response.completed","response":{"status":"completed"}}\n\n'
      const source = unfinishedHttpResponse(t, terminal)
      const response = await createOpenAIRequestFetch(async () => source.response, {
        normalizeResponses: api === 'openai-responses',
      })('https://relay.example/v1', {})
      assert.equal(await response.text(), terminal)
      // pipeThrough 的取消在微任务中传播；读取结束后等待源取消信号而不是任意睡眠。
      await new Promise((resolve) => setImmediate(resolve))
      assert.equal(source.cancelled(), true)
    })
  }

  test(`${api} real model SDK settles when the gateway leaves a completed response open`, async (t) => {
    const directory = await mkdtemp(join(tmpdir(), 'pisper-terminal-frame-'))
    const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
    t.after(async () => {
      await runtime.dispose()
      await rm(directory, { recursive: true, force: true })
    })
    await runtime.init()
    await runtime.createProvider({
      id: 'terminal-frame',
      name: 'Terminal fixture',
      api,
      baseUrl: 'https://relay.example/v1',
      apiKey: 'fixture-key',
      model: 'fixture-model',
    })
    const frames =
      api === 'openai-responses'
        ? [
            { type: 'response.created', response: { id: 'response-fixture' } },
            {
              type: 'response.output_item.added',
              output_index: 0,
              item: { id: 'message-fixture', type: 'message', role: 'assistant', content: [] },
            },
            {
              type: 'response.output_text.delta',
              output_index: 0,
              content_index: 0,
              delta: 'finished',
            },
            {
              type: 'response.output_item.done',
              output_index: 0,
              item: {
                id: 'message-fixture',
                type: 'message',
                role: 'assistant',
                content: [{ type: 'output_text', text: 'finished', annotations: [] }],
              },
            },
            {
              type: 'response.completed',
              response: {
                id: 'response-fixture',
                status: 'completed',
                output: [
                  {
                    id: 'message-fixture',
                    type: 'message',
                    role: 'assistant',
                    content: [{ type: 'output_text', text: 'finished', annotations: [] }],
                  },
                ],
                usage: { input_tokens: 2, output_tokens: 3, total_tokens: 5 },
              },
            },
          ]
            .map((event) => `data: ${JSON.stringify(event)}\n\n`)
            .join('')
        : `data: ${JSON.stringify({
            id: 'completion-fixture',
            choices: [{ index: 0, delta: { content: 'finished' }, finish_reason: 'stop' }],
          })}\n\ndata: ${JSON.stringify({
            id: 'completion-fixture',
            choices: [],
            usage: { prompt_tokens: 2, completion_tokens: 3, total_tokens: 5 },
          })}\n\ndata: [DONE]\n\n`
    const source = unfinishedHttpResponse(t, frames)
    const result = await runtime.modelRuntime.completeSimple(
      runtime.modelRuntime.getModel('terminal-frame', 'fixture-model'),
      { messages: [{ role: 'user', content: 'fixture', timestamp: 0 }] },
      { maxTokens: 16, fetch: async () => source.response, maxRetries: 0 },
    )
    assert.equal(result.stopReason, 'stop', result.errorMessage)
    assert.equal(result.content.find((part) => part.type === 'text')?.text, 'finished')
    assert.equal(result.usage.totalTokens, 5)
    if (api === 'openai-responses') {
      for (const terminal of [
        {
          type: 'response.incomplete',
          response: { status: 'incomplete', incomplete_details: { reason: 'max_output_tokens' } },
        },
        {
          type: 'response.failed',
          response: {
            status: 'failed',
            error: { code: 'fixture_failure', message: 'fixture failed' },
          },
        },
      ]) {
        const source = unfinishedHttpResponse(t, responseFrames('partial', terminal))
        const message = await runtime.modelRuntime.completeSimple(
          runtime.modelRuntime.getModel('terminal-frame', 'fixture-model'),
          { messages: [{ role: 'user', content: 'fixture', timestamp: 0 }] },
          { maxTokens: 16, fetch: async () => source.response, maxRetries: 0 },
        )
        assert.equal(
          message.stopReason,
          terminal.type === 'response.incomplete' ? 'length' : 'error',
        )
        if (terminal.type === 'response.failed')
          assert.match(message.errorMessage, /fixture_failure/)
      }
      const message = await runtime.modelRuntime.completeSimple(
        runtime.modelRuntime.getModel('terminal-frame', 'fixture-model'),
        { messages: [{ role: 'user', content: 'fixture', timestamp: 0 }] },
        {
          maxTokens: 16,
          fetch: async () =>
            new Response('data: [DONE]\n\n', { headers: { 'Content-Type': 'text/event-stream' } }),
          maxRetries: 0,
        },
      )
      assert.equal(message.stopReason, 'error')
      assert.match(message.errorMessage, /before a terminal response event/)
    }
  })
}

test('OpenAI Responses relay fills omitted function-call arguments from deltas', async () => {
  const events = [
    {
      type: 'response.function_call_arguments.delta',
      output_index: 2,
      delta: '{"path":"README.md"}',
    },
    {
      type: 'response.function_call_arguments.done',
      output_index: 2,
    },
  ]
  const source = events
    .map((event) => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`)
    .join('')
  const bytes = new TextEncoder().encode(source)
  const response = await createOpenAIRequestFetch(
    async () =>
      new Response(
        new ReadableStream({
          start(controller) {
            controller.enqueue(bytes.slice(0, 19))
            controller.enqueue(bytes.slice(19))
            controller.close()
          },
        }),
        { headers: { 'Content-Type': 'text/event-stream' } },
      ),
    { normalizeResponses: true },
  )('https://relay.example/v1/responses', { headers: {} })

  const body = await response.text()
  const doneData = body
    .split('\n')
    .filter((line) => line.startsWith('data:'))
    .map((line) => JSON.parse(line.slice(5)))
    .find((event) => event.type === 'response.function_call_arguments.done')
  assert.ok(doneData)
  assert.equal(doneData.arguments, '{"path":"README.md"}')
})

test('OpenAI Responses Runtime accepts relay tool-call done events without arguments', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-responses-relay-stream-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  await runtime.createProvider({
    id: 'responses-relay',
    name: 'Responses relay',
    api: 'openai-responses',
    baseUrl: 'https://relay.example/v1',
    apiKey: 'test-key',
    model: 'relay-model',
  })

  const argumentsText = '{"path":"README.md"}'
  const events = [
    { type: 'response.created', response: { id: 'resp_1' } },
    {
      type: 'response.output_item.added',
      output_index: 0,
      item: {
        id: 'fc_1',
        call_id: 'call_1',
        type: 'function_call',
        name: 'read',
        arguments: '',
      },
    },
    { type: 'response.function_call_arguments.delta', output_index: 0, delta: argumentsText },
    { type: 'response.function_call_arguments.done', output_index: 0 },
    {
      type: 'response.output_item.done',
      output_index: 0,
      item: {
        id: 'fc_1',
        call_id: 'call_1',
        type: 'function_call',
        name: 'read',
        arguments: argumentsText,
      },
    },
    {
      type: 'response.completed',
      response: { id: 'resp_1', status: 'completed', output: [] },
    },
  ]
  const result = await runtime.modelRuntime.completeSimple(
    runtime.modelRuntime.getModel('responses-relay', 'relay-model'),
    { messages: [{ role: 'user', content: 'Read the README', timestamp: Date.now() }] },
    {
      maxTokens: 32,
      fetch: async () =>
        new Response(events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join(''), {
          headers: { 'Content-Type': 'text/event-stream' },
        }),
    },
  )

  assert.equal(result.stopReason, 'toolUse')
  assert.deepEqual(result.content.find((block) => block.type === 'toolCall').arguments, {
    path: 'README.md',
  })
})

for (const kind of ['record', 'tuples', 'headers', 'request']) {
  test(`OpenAI header filtering preserves transport options and supports ${kind}`, async () => {
    const values = {
      'X-Stainless-Lang': 'js',
      'x-stainless-future-field': 'diagnostic',
      Authorization: 'Bearer test-key',
      'User-Agent': 'configured-client',
      'OpenAI-Project': 'test-project',
      'X-Custom': 'keep',
      'Content-Type': 'application/json',
    }
    const headers =
      kind === 'tuples' ? Object.entries(values) : kind === 'headers' ? new Headers(values) : values
    const controller = new AbortController()
    const body = '{"model":"test"}'
    const init = { method: 'POST', headers, body, signal: controller.signal, duplex: 'half' }
    const input =
      kind === 'request'
        ? new Request('https://relay.example/v1', init)
        : 'https://relay.example/v1'
    const response = new Response('ok')
    const wrapped = createOpenAIRequestFetch(async (actualInput, actualInit) => {
      assert.equal(actualInput, input)
      assert.deepEqual(
        [...actualInit.headers.keys()].filter((name) => name.startsWith('x-stainless-')),
        [],
      )
      for (const [name, value] of Object.entries(values)) {
        if (!name.toLowerCase().startsWith('x-stainless-'))
          assert.equal(actualInit.headers.get(name), value)
      }
      if (kind !== 'request') {
        assert.equal(actualInit.signal, controller.signal)
        assert.equal(actualInit.body, body)
        assert.equal(actualInit.duplex, 'half')
      } else {
        assert.equal(await actualInput.text(), body)
      }
      return response
    })
    assert.equal(await wrapped(input, kind === 'request' ? undefined : init), response)
    assert.equal(new Headers(headers).get('x-stainless-lang'), 'js')
  })
}

test('OpenAI header filtering respects Request init overrides and propagates cancellation', async () => {
  const input = new Request('https://relay.example/v1', {
    headers: { 'X-Stainless-Lang': 'js', 'X-Replaced': 'old' },
  })
  const controller = new AbortController()
  controller.abort()
  const wrapped = createOpenAIRequestFetch(async (_, init) => {
    assert.equal(init.headers.get('x-replaced'), null)
    assert.equal(init.headers.get('x-custom'), 'new')
    init.signal.throwIfAborted()
  })
  await assert.rejects(
    wrapped(input, { headers: { 'X-Custom': 'new' }, signal: controller.signal }),
    { name: 'AbortError' },
  )
})

for (const api of ['openai-completions', 'openai-responses', 'anthropic-messages']) {
  for (const official of [true, false]) {
    test(`${api} SDK requests ${official ? 'official' : 'relay'} apply only the OpenAI filter`, async (t) => {
      const directory = await mkdtemp(join(tmpdir(), 'pisper-header-test-'))
      const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
      t.after(async () => {
        await runtime.dispose()
        await rm(directory, { recursive: true, force: true })
      })
      await runtime.init()
      const baseUrl = official
        ? api === 'anthropic-messages'
          ? 'https://api.anthropic.com'
          : 'https://api.openai.com/v1'
        : 'https://relay.example/v1'
      await runtime.createProvider({
        id: 'header-test',
        name: 'Header Test',
        api,
        baseUrl,
        apiKey: 'test-key',
        model: 'test-model',
      })
      const model = runtime.modelRuntime.getModel('header-test', 'test-model')
      const context = { messages: [{ role: 'user', content: 'Test', timestamp: 0 }] }
      for (const method of ['stream', 'streamSimple', 'complete', 'completeSimple']) {
        let calls = 0
        const result = runtime.modelRuntime[method](model, context, {
          maxTokens: 16,
          headers: { 'X-Custom': 'keep', 'User-Agent': 'test-client' },
          fetch: async (input, init) => {
            calls++
            assert.ok(String(input).startsWith(baseUrl))
            const headers = new Headers(init.headers)
            assert.equal(headers.get('x-custom'), 'keep')
            assert.equal(headers.get('user-agent'), 'test-client')
            const diagnostics = [...headers.keys()].filter((name) =>
              name.startsWith('x-stainless-'),
            )
            if (api === 'anthropic-messages') {
              assert.ok(diagnostics.length > 0)
              assert.equal(headers.get('x-api-key'), 'test-key')
            } else {
              assert.deepEqual(diagnostics, [])
              assert.equal(headers.get('authorization'), 'Bearer test-key')
            }
            assert.equal(JSON.parse(init.body).model, 'test-model')
            return new Response(JSON.stringify({ error: { message: 'captured' } }), {
              status: 400,
              headers: { 'Content-Type': 'application/json' },
            })
          },
        })
        const message = await (method.startsWith('stream') ? result.result() : result)
        assert.equal(calls, 1)
        assert.equal(message.stopReason, 'error')
        assert.match(message.errorMessage, /captured/)
      }
    })
  }
}

test('Azure OpenAI SDK also removes diagnostics without changing API key authentication', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-azure-header-test-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  const template = runtime.modelRuntime
    .getModels('azure')
    .find((model) => model.api === 'azure-openai-responses')
  assert.ok(template)
  let captured
  const result = await runtime.modelRuntime.completeSimple(
    { ...template, baseUrl: 'https://test-resource.openai.azure.com/openai/v1' },
    { messages: [{ role: 'user', content: 'Test', timestamp: 0 }] },
    {
      apiKey: 'test-key',
      maxTokens: 16,
      env: {},
      fetch: async (_, init) => {
        captured = new Headers(init.headers)
        return new Response(JSON.stringify({ error: { message: 'captured' } }), {
          status: 400,
          headers: { 'Content-Type': 'application/json' },
        })
      },
    },
  )
  assert.match(result.errorMessage, /captured/)
  assert.equal(captured.get('api-key'), 'test-key')
  assert.deepEqual(
    [...captured.keys()].filter((name) => name.startsWith('x-stainless-')),
    [],
  )
})

test('visual OpenAI SDK sends no Stainless headers and retains configured headers', async (t) => {
  let captured
  const server = createServer(async (request, response) => {
    for await (const _ of request) {
      // 消费请求体后再响应，避免测试清理与上传并发。
    }
    captured = request.headers
    response.writeHead(200, { 'Content-Type': 'application/json' })
    response.end(JSON.stringify({ data: [{ b64_json: Buffer.from('image').toString('base64') }] }))
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  t.after(async () => {
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
  })
  await generateOpenAICompatible(
    {
      id: 'gpt-image-2',
      apiKey: 'test-key',
      baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
      headers: { 'X-Custom': 'keep', 'User-Agent': 'test-client' },
    },
    { kind: 'image', prompt: 'test' },
  )
  assert.deepEqual(
    Object.keys(captured).filter((name) => name.startsWith('x-stainless-')),
    [],
  )
  assert.equal(captured.authorization, 'Bearer test-key')
  assert.equal(captured['x-custom'], 'keep')
  assert.equal(captured['user-agent'], 'test-client')
})
