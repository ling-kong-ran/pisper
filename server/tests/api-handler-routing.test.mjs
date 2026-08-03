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
    headers: {},
    body: '',
    destroyed: false,
    writableEnded: false,
    flushCount: 0,
    endCount: 0,
    writeHead(status, headers = {}) {
      this.status = status
      this.headers = headers
    },
    flushHeaders() {
      this.flushCount += 1
    },
    write(body = '') {
      this.body += body
    },
    end(body = '') {
      this.body += body
      this.writableEnded = true
      this.endCount += 1
    },
  }
}

test('API handler passes through non-API requests and returns the public JSON 404', async () => {
  const handler = createApiHandler({})
  const passThrough = response()
  const missing = response()

  assert.equal(await handler(request('GET'), passThrough, new URL('http://localhost/')), false)
  assert.equal(passThrough.status, 0)
  assert.equal(
    await handler(request('GET'), missing, new URL('http://localhost/api/not-registered')),
    true,
  )
  assert.equal(missing.status, 404)
  assert.deepEqual(JSON.parse(missing.body), { error: '接口不存在。' })
  assert.equal(missing.headers['Content-Type'], 'application/json; charset=utf-8')
  assert.equal(missing.headers['Cache-Control'], 'no-store')
})

test('API handler redacts secrets from asynchronous public errors', async () => {
  const secret = 'sk-example-secret-token-1234567890'
  const handler = createApiHandler({
    async getConfig() {
      throw new Error(`Provider failed, apiKey: ${secret}`)
    },
  })
  const output = response()

  await handler(request('GET'), output, new URL('http://localhost/api/config'))

  assert.equal(output.status, 400)
  assert.doesNotMatch(output.body, new RegExp(secret))
  assert.match(JSON.parse(output.body).error, /\[REDACTED SECRET\]/)
})

test('desktop service validation still precedes JSON body parsing', async () => {
  const handler = createApiHandler({})
  const output = response()
  const malformedRequest = {
    method: 'POST',
    async *[Symbol.asyncIterator]() {
      yield Buffer.from('{')
    },
  }

  await handler(malformedRequest, output, new URL('http://localhost/api/desktop-pet/enabled'))

  assert.equal(output.status, 400)
  assert.deepEqual(JSON.parse(output.body), { error: '桌面宠物服务尚未初始化。' })
})

test('API handler completes successful SSE responses exactly once', async () => {
  const handler = createApiHandler({
    async streamPrompt(input) {
      input.send('snapshot', { text: 'working' })
      input.send('done', { text: 'complete' })
    },
  })
  const output = response()

  await handler(
    request('POST', { sessionId: 'session-1', message: 'hello' }),
    output,
    new URL('http://localhost/api/chat'),
  )

  assert.equal(output.status, 200)
  assert.equal(output.headers['Content-Type'], 'text/event-stream; charset=utf-8')
  assert.equal(output.headers['Cache-Control'], 'no-cache, no-transform')
  assert.equal(output.headers.Connection, 'keep-alive')
  assert.equal(output.headers['X-Accel-Buffering'], 'no')
  assert.equal(output.flushCount, 1)
  assert.equal(output.endCount, 1)
  assert.match(output.body, /event: snapshot/)
  assert.match(output.body, /event: done/)
})

test('API handler emits a redacted terminal SSE error when streaming throws', async () => {
  const secret = 'sk-stream-secret-token-1234567890'
  const handler = createApiHandler({
    async streamPrompt(input) {
      input.send('snapshot', { text: 'working' })
      throw new Error(`stream failed, apiKey: ${secret}`)
    },
  })
  const output = response()

  await handler(
    request('POST', { sessionId: 'session-1', message: 'hello' }),
    output,
    new URL('http://localhost/api/chat'),
  )

  assert.equal(output.status, 200)
  assert.equal(output.endCount, 1)
  assert.match(output.body, /event: snapshot/)
  assert.match(output.body, /event: error/)
  assert.match(output.body, /\[REDACTED SECRET\]/)
  assert.doesNotMatch(output.body, new RegExp(secret))
})
