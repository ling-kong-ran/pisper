import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createOpenAIRequestFetch } from '../services/openai-request-transport.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { generateOpenAICompatible } from '../services/visual-generation/openai-compatible.mjs'

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
  const template = runtime.modelRuntime.getModels('azure-openai-responses')[0]
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
