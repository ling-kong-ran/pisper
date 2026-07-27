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
    writeHead(status) { this.status = status },
    end(body = '') { this.body = body },
  }
}

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

test('sandbox status, install, and session execution mode APIs delegate to the runtime', async () => {
  const calls = []
  const runtime = {
    async getSandboxStatus() {
      calls.push(['status'])
      return { supported: true, state: 'not-installed', platform: 'win32' }
    },
    async installLocalSandbox() {
      calls.push(['install'])
      return { supported: true, state: 'ready', platform: 'win32' }
    },
    async setSessionExecutionMode(id, mode) {
      calls.push(['mode', id, mode])
      return { id, executionMode: mode, permissionMode: mode === 'full-access' ? 'ignore' : 'auto' }
    },
  }
  const handler = createApiHandler(runtime)

  const statusResponse = response()
  assert.equal(await handler(request('GET'), statusResponse, new URL('http://localhost/api/sandbox/status')), true)
  assert.equal(statusResponse.status, 200)
  assert.equal(JSON.parse(statusResponse.body).state, 'not-installed')

  const installResponse = response()
  assert.equal(await handler(request('POST', {}), installResponse, new URL('http://localhost/api/sandbox/install')), true)
  assert.equal(installResponse.status, 200)
  assert.equal(JSON.parse(installResponse.body).state, 'ready')

  const modeResponse = response()
  assert.equal(await handler(request('PUT', { mode: 'workspace' }), modeResponse, new URL('http://localhost/api/sessions/session%201/execution-mode')), true)
  assert.equal(modeResponse.status, 200)
  assert.deepEqual(JSON.parse(modeResponse.body), { id: 'session 1', executionMode: 'workspace', permissionMode: 'auto' })
  assert.deepEqual(calls, [
    ['status'],
    ['install'],
    ['mode', 'session 1', 'workspace'],
  ])
})
