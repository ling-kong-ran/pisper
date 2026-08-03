import assert from 'node:assert/strict'
import test from 'node:test'
import { apiJson } from '../../src/lib/api.ts'
import { ApiError, DEFAULT_HTTP_TIMEOUT_MS, requestJson } from '../../src/lib/http.ts'

async function withFetch(fetchImplementation, callback) {
  const originalFetch = globalThis.fetch
  globalThis.fetch = fetchImplementation
  try {
    return await callback()
  } finally {
    globalThis.fetch = originalFetch
  }
}

test('fetch JSON client preserves request body and header compatibility', async () => {
  assert.equal(DEFAULT_HTTP_TIMEOUT_MS, 30_000)
  const requests = []
  await withFetch(
    async (path, options) => {
      requests.push({ path, options })
      return new Response(JSON.stringify({ ok: true }), {
        headers: { 'Content-Type': 'application/json' },
      })
    },
    async () => {
      assert.deepEqual(
        await requestJson('/api/data', {
          method: 'POST',
          data: { value: 1 },
          headers: { 'X-Request': 'data' },
        }),
        { ok: true },
      )
      assert.deepEqual(
        await apiJson('/api/body', {
          method: 'PATCH',
          body: JSON.stringify({ value: 2 }),
        }),
        { ok: true },
      )
      assert.deepEqual(
        await requestJson('/api/body-object', {
          method: 'PUT',
          body: { value: 3 },
        }),
        { ok: true },
      )
    },
  )

  assert.equal(requests[0].path, '/api/data')
  assert.equal(requests[0].options.body, '{"value":1}')
  assert.equal(requests[0].options.headers.get('Content-Type'), 'application/json')
  assert.equal(requests[0].options.headers.get('X-Request'), 'data')
  assert.equal(requests[1].options.body, '{"value":2}')
  assert.equal(requests[2].options.body, '{"value":3}')
  for (const request of requests) assert.ok(request.options.signal instanceof AbortSignal)
})

test('fetch JSON client handles JSON, text, 204, and empty success responses', async () => {
  const responses = [
    new Response(JSON.stringify({ value: 42 })),
    new Response('plain text'),
    new Response(null, { status: 204 }),
    new Response(null, { status: 200 }),
  ]
  await withFetch(
    async () => responses.shift(),
    async () => {
      assert.deepEqual(await requestJson('/json'), { value: 42 })
      assert.equal(await requestJson('/text'), 'plain text')
      assert.equal(await requestJson('/no-content'), undefined)
      assert.equal(await requestJson('/empty'), undefined)
    },
  )
})

test('fetch JSON client normalizes JSON and non-JSON error payloads', async () => {
  const responses = [
    new Response(JSON.stringify({ error: 'invalid request', code: 'invalid' }), {
      status: 422,
      headers: { 'Content-Type': 'application/json' },
    }),
    new Response('gateway unavailable', { status: 502 }),
  ]
  await withFetch(
    async () => responses.shift(),
    async () => {
      await assert.rejects(requestJson('/json-error'), (error) => {
        assert.ok(error instanceof ApiError)
        assert.equal(error.message, 'invalid request')
        assert.equal(error.status, 422)
        assert.deepEqual(error.data, { error: 'invalid request', code: 'invalid' })
        return true
      })
      await assert.rejects(requestJson('/text-error'), (error) => {
        assert.ok(error instanceof ApiError)
        assert.equal(error.message, 'gateway unavailable')
        assert.equal(error.status, 502)
        assert.deepEqual(error.data, { error: 'gateway unavailable' })
        return true
      })
    },
  )
})

test('fetch JSON client applies per-request timeout and external abort signals', async () => {
  const pendingFetch = async (_path, options) =>
    new Promise((_resolve, reject) => {
      options.signal.addEventListener('abort', () => reject(options.signal.reason), { once: true })
    })

  await withFetch(pendingFetch, async () => {
    await assert.rejects(requestJson('/timeout', { timeout: 5 }), (error) => {
      assert.ok(error instanceof ApiError)
      assert.equal(error.message, '请求超时 (5ms)')
      assert.equal(error.status, undefined)
      return true
    })

    const controller = new AbortController()
    const request = requestJson('/abort', { signal: controller.signal, timeout: 1_000 })
    controller.abort(new Error('caller stopped'))
    await assert.rejects(request, (error) => {
      assert.ok(error instanceof ApiError)
      assert.equal(error.message, 'caller stopped')
      assert.equal(error.status, undefined)
      return true
    })
  })
})

test('fetch JSON client normalizes network failures as ApiError', async () => {
  await withFetch(
    async () => {
      throw new TypeError('connection refused')
    },
    async () => {
      await assert.rejects(requestJson('/network'), (error) => {
        assert.ok(error instanceof ApiError)
        assert.equal(error.message, 'connection refused')
        assert.equal(error.status, undefined)
        assert.equal(error.data, undefined)
        return true
      })
    },
  )
})
