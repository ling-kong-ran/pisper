import assert from 'node:assert/strict'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'

function request(method, body) {
  return {
    method,
    async *[Symbol.asyncIterator]() {
      if (body !== undefined) yield Buffer.from(JSON.stringify(body))
    },
  }
}

function response() {
  return {
    status: 0,
    body: '',
    destroyed: false,
    writableEnded: false,
    writeHead(status) { this.status = status },
    flushHeaders() {},
    write(body = '') { this.body += body },
    end(body = '') {
      this.body += body
      this.writableEnded = true
    },
  }
}

test('app update API distinguishes upstream failures from invalid requests', async () => {
  const updates = {
    async check() {
      throw new Error('GitHub commit 比较失败：HTTP 403')
    },
  }
  const handler = createApiHandler({}, { updates })
  const output = response()

  assert.equal(
    await handler(request('GET'), output, new URL('http://localhost/api/app-update?refresh=1')),
    true,
  )
  assert.equal(output.status, 502)
  assert.deepEqual(JSON.parse(output.body), { error: 'GitHub commit 比较失败：HTTP 403' })
})

test('compaction preference APIs expose and update the threshold percentage', async () => {
  const calls = []
  const runtime = {
    getCompactionPreference() {
      calls.push(['get'])
      return { thresholdPercent: 80, minPercent: 50, maxPercent: 95 }
    },
    async updateCompactionPreference(input) {
      calls.push(['update', input.thresholdPercent])
      return { thresholdPercent: input.thresholdPercent, minPercent: 50, maxPercent: 95 }
    },
  }
  const handler = createApiHandler(runtime)

  const getResponse = response()
  assert.equal(await handler(request('GET'), getResponse, new URL('http://localhost/api/settings/compaction')), true)
  assert.deepEqual(JSON.parse(getResponse.body), { thresholdPercent: 80, minPercent: 50, maxPercent: 95 })

  const patchResponse = response()
  assert.equal(await handler(request('PATCH', { thresholdPercent: 75 }), patchResponse, new URL('http://localhost/api/settings/compaction')), true)
  assert.deepEqual(JSON.parse(patchResponse.body), { thresholdPercent: 75, minPercent: 50, maxPercent: 95 })
  assert.deepEqual(calls, [['get'], ['update', 75]])
})

test('chat API forwards explicit Tool requests as structured runtime input', async () => {
  const calls = []
  const runtime = {
    async streamPrompt(input) {
      calls.push(input)
      input.send('done', { text: 'complete' })
    },
  }
  const handler = createApiHandler(runtime)
  const output = response()

  assert.equal(await handler(request('POST', {
    sessionId: 'session-1',
    message: '/web_search Pisper release',
    requestedToolNames: ['web_search'],
  }), output, new URL('http://localhost/api/chat')), true)

  assert.equal(output.status, 200)
  assert.deepEqual(calls[0].requestedToolNames, ['web_search'])
  assert.match(output.body, /event: done/)
})

test('session creation forwards the CLI workspace to the shared runtime', async () => {
  const calls = []
  const runtime = {
    async createSession(name, cwd) {
      calls.push([name, cwd])
      return { id: 'session-1', name, cwd }
    },
  }
  const handler = createApiHandler(runtime)
  const output = response()

  assert.equal(await handler(
    request('POST', { name: 'CLI chat', cwd: 'E:\\code\\workspace' }),
    output,
    new URL('http://localhost/api/sessions'),
  ), true)
  assert.equal(output.status, 201)
  assert.deepEqual(calls, [['CLI chat', 'E:\\code\\workspace']])
})

test('session model and thinking APIs delegate current-session changes to the runtime', async () => {
  const calls = []
  const runtime = {
    async setSessionModel(id, provider, model) {
      calls.push(['model', id, provider, model])
      return { id, model: `${provider}/${model}` }
    },
    async getSessionThinkingState(id) {
      calls.push(['thinking-state', id])
      return { id, thinkingLevel: 'medium', availableLevels: ['off', 'medium', 'high'] }
    },
    async setSessionThinkingLevel(id, level) {
      calls.push(['thinking', id, level])
      return { id, thinkingLevel: level, availableLevels: ['off', 'medium', 'high'] }
    },
  }
  const handler = createApiHandler(runtime)

  const modelResponse = response()
  assert.equal(await handler(request('PUT', { provider: 'openai', model: 'gpt-5.6' }), modelResponse, new URL('http://localhost/api/sessions/session%201/model')), true)
  assert.deepEqual(JSON.parse(modelResponse.body), { id: 'session 1', model: 'openai/gpt-5.6' })

  const thinkingStateResponse = response()
  assert.equal(await handler(request('GET'), thinkingStateResponse, new URL('http://localhost/api/sessions/session%201/thinking-level')), true)
  assert.deepEqual(JSON.parse(thinkingStateResponse.body), { id: 'session 1', thinkingLevel: 'medium', availableLevels: ['off', 'medium', 'high'] })

  const thinkingResponse = response()
  assert.equal(await handler(request('PUT', { level: 'high' }), thinkingResponse, new URL('http://localhost/api/sessions/session%201/thinking-level')), true)
  assert.deepEqual(JSON.parse(thinkingResponse.body), { id: 'session 1', thinkingLevel: 'high', availableLevels: ['off', 'medium', 'high'] })
  assert.deepEqual(calls, [
    ['model', 'session 1', 'openai', 'gpt-5.6'],
    ['thinking-state', 'session 1'],
    ['thinking', 'session 1', 'high'],
  ])
})

test('session execution mode API delegates to the runtime', async () => {
  const calls = []
  const runtime = {
    async setSessionExecutionMode(id, mode) {
      calls.push(['mode', id, mode])
      return { id, executionMode: mode, permissionMode: mode === 'full-access' ? 'ignore' : 'auto' }
    },
  }
  const handler = createApiHandler(runtime)

  const modeResponse = response()
  assert.equal(await handler(request('PUT', { mode: 'workspace' }), modeResponse, new URL('http://localhost/api/sessions/session%201/execution-mode')), true)
  assert.equal(modeResponse.status, 200)
  assert.deepEqual(JSON.parse(modeResponse.body), { id: 'session 1', executionMode: 'workspace', permissionMode: 'auto' })
  assert.deepEqual(calls, [['mode', 'session 1', 'workspace']])
})
