import assert from 'node:assert/strict'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'

function response() {
  return {
    status: 0,
    headersSent: false,
    writableEnded: false,
    destroyed: false,
    endCount: 0,
    writeHead(status, headers) {
      this.status = status
      this.headers = headers
      this.headersSent = true
    },
    end() {
      this.writableEnded = true
      this.endCount += 1
    },
  }
}

function request() {
  return { method: 'GET', async *[Symbol.asyncIterator]() {} }
}

test('非 SSE API 错误保留 4xx、区分 5xx，未标注错误兼容返回 400', async () => {
  const clientHandler = createApiHandler({
    async getConfig() {
      const error = new Error('bad request')
      error.status = 422
      throw error
    },
  })
  const client = response()
  await clientHandler(request(), client, new URL('http://localhost/api/config'))
  assert.equal(client.status, 422)

  const serverHandler = createApiHandler({
    async getConfig() {
      const error = new Error('upstream failure')
      error.statusCode = 503
      throw error
    },
  })
  const server = response()
  await serverHandler(request(), server, new URL('http://localhost/api/config'))
  assert.equal(server.status, 503)

  const unknownHandler = createApiHandler({
    async getConfig() {
      throw new Error('unexpected')
    },
  })
  const unknown = response()
  await unknownHandler(request(), unknown, new URL('http://localhost/api/config'))
  assert.equal(unknown.status, 400)
})

test('非 SSE 错误在 headersSent 后不会重复响应', async () => {
  const handler = createApiHandler({
    async getConfig() {
      throw new Error('late failure')
    },
  })
  const output = response()
  output.headersSent = true
  await handler(request(), output, new URL('http://localhost/api/config'))
  assert.equal(output.endCount, 0)
  assert.equal(output.status, 0)
})
