import assert from 'node:assert/strict'
import { getEventListeners } from 'node:events'
import { createServer } from 'node:http'
import test from 'node:test'
import {
  callRemoteDecisions,
  normalizeDecideInput,
  normalizeRemoteResponse,
} from '../services/decision-remote-client.mjs'
import { MAX_DECISION_RESPONSE_BYTES } from '../services/decision-transport.mjs'

const config = {
  provider: 'typesafe',
  baseUrl: '',
  modelId: 'jev-1.13.0',
  apiKey: 'synthetic-test-key',
}
const input = normalizeDecideInput({
  state: 'A test',
  questions: [{ id: 'approve', type: 'noul', instructions: 'Is this safe?' }],
})

test('native fetch cancellation after headers exits cleanly without a cloned network response', async () => {
  const server = createServer((_req, res) => {
    res.writeHead(200, { 'Content-Type': 'application/json' })
    res.flushHeaders()
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const caller = new AbortController()
  try {
    await assert.rejects(
      callRemoteDecisions(config, input, {
        signal: caller.signal,
        fetchImpl: async (_url, init) => {
          const response = await fetch(`http://127.0.0.1:${server.address().port}`, init)
          setImmediate(() => caller.abort())
          return response
        },
      }),
      { code: 'aborted' },
    )
    await new Promise((resolve) => setImmediate(resolve))
  } finally {
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
  }
})

test('decision probabilities are rejected instead of clamped into automatic approval', () => {
  for (const noul of [-0.1, 1.1, NaN, Infinity, '1']) {
    assert.throws(
      () => normalizeRemoteResponse({ answers: { approve: { noul } } }, input.questions),
      { code: 'bad_response' },
    )
  }
  for (const answers of [
    {},
    { other: { noul: 1 } },
    { approve: { type: 'choice', noul: 1 } },
    { approve: { noul: 1, score: 1 } },
  ]) {
    assert.throws(() => normalizeRemoteResponse({ answers }, input.questions), {
      code: 'bad_response',
    })
  }
})

test('choice and score answers must match the requested labels and levels', () => {
  const choice = normalizeDecideInput({
    state: 'x',
    questions: [{ id: 'q', type: 'choice', instructions: '?', options: ['yes', 'no'] }],
  })
  for (const answer of [
    { choice: 'unknown' },
    { choice: 'yes', confidence: 2 },
    { choice: 'yes', probabilities: { yes: '0.9' } },
    { choice: 'yes', probabilities: { unknown: 0.9 } },
  ]) {
    assert.throws(() => normalizeRemoteResponse({ answers: { q: answer } }, choice.questions), {
      code: 'bad_response',
    })
  }
  const score = normalizeDecideInput({
    state: 'x',
    questions: [{ id: 'q', type: 'score', instructions: '?', options: ['low', 'high'] }],
  })
  for (const value of [-1, 2, NaN, Infinity]) {
    assert.throws(
      () => normalizeRemoteResponse({ answers: { q: { score: value } } }, score.questions),
      { code: 'bad_response' },
    )
  }
  assert.equal(
    normalizeRemoteResponse({ answers: { q: { score: 0.5 } } }, score.questions).answers.q.score,
    0.5,
  )
})

test('SDK adapter preserves official, OpenRouter and full relay endpoints', async () => {
  for (const [remote, endpoint] of [
    [config, 'https://api.typesafe.ai/v1/systemone'],
    [
      { ...config, provider: 'openrouter', baseUrl: '' },
      'https://openrouter.ai/api/alpha/decisions',
    ],
    [
      { ...config, provider: 'custom', baseUrl: 'https://relay.example/custom/decisions' },
      'https://relay.example/custom/decisions',
    ],
  ]) {
    const result = await callRemoteDecisions(remote, input, {
      fetchImpl: async (url, init) => {
        assert.equal(url, endpoint)
        assert.equal(init.redirect, 'error')
        assert.equal(new Headers(init.headers).get('Authorization'), `Bearer ${config.apiKey}`)
        assert.deepEqual(JSON.parse(init.body).questions, {
          approve: { type: 'noul', instructions: 'Is this safe?' },
        })
        return Response.json({ answers: { approve: { noul: 0.7 } } })
      },
    })
    assert.equal(result.answers.approve.noul, 0.7)
  }
})

test('already cancelled decisions never start a network request', async () => {
  await assert.rejects(
    callRemoteDecisions(config, input, {
      signal: AbortSignal.abort(),
      fetchImpl: () => assert.fail('must not fetch'),
    }),
    { code: 'aborted' },
  )
})

test('cancellation after headers interrupts response delivery and cleans the caller listener', async () => {
  const caller = new AbortController()
  const reading = Promise.withResolvers()
  let cancelled = false
  const request = callRemoteDecisions(config, input, {
    signal: caller.signal,
    fetchImpl: async () =>
      new Response(
        new ReadableStream({
          pull() {
            reading.resolve()
          },
          cancel() {
            cancelled = true
          },
        }),
      ),
  })
  const settled = assert.rejects(request, { code: 'aborted' })
  await reading.promise
  caller.abort()
  await settled
  assert.equal(cancelled, true)
  assert.equal(getEventListeners(caller.signal, 'abort').length, 0)
})

test('the overall timeout covers both body delivery and SDK retry backoff', async () => {
  for (const retrying of [false, true]) {
    let calls = 0
    await assert.rejects(
      callRemoteDecisions(config, input, {
        timeoutMs: 30,
        fetchImpl: async () => {
          calls += 1
          return retrying
            ? new Response('busy', { status: 429, headers: { 'Retry-After': '60' } })
            : new Response(new ReadableStream({ pull() {} }))
        },
      }),
      { code: 'timeout' },
    )
    assert.equal(calls, 1)
  }
})

test('empty retry headers on an empty response use backoff rather than an immediate retry', async () => {
  let calls = 0
  await assert.rejects(
    callRemoteDecisions(config, input, {
      timeoutMs: 30,
      fetchImpl: async () => {
        calls += 1
        return new Response(null, {
          status: 429,
          headers: { 'Retry-After': ' ', 'retry-after-ms': '' },
        })
      },
    }),
    { code: 'timeout' },
  )
  assert.equal(calls, 1)
})

test('SDK connection errors cannot expose credentials through their message or cause', async () => {
  await assert.rejects(
    callRemoteDecisions(config, input, {
      fetchImpl: async () => {
        throw new TypeError(`failed ${config.apiKey}`)
      },
    }),
    (error) => {
      assert.equal(error.code, 'network')
      assert.equal(error.cause, undefined)
      assert.ok(!error.message.includes(config.apiKey))
      return true
    },
  )
})

test('oversized decision responses stop reading without retrying or exposing their body', async () => {
  let calls = 0
  let cancelled = false
  await assert.rejects(
    callRemoteDecisions(config, input, {
      fetchImpl: async () => {
        calls += 1
        return new Response(
          new ReadableStream({
            start(controller) {
              controller.enqueue(new Uint8Array(MAX_DECISION_RESPONSE_BYTES + 1))
            },
            cancel() {
              cancelled = true
            },
          }),
        )
      },
    }),
    { code: 'bad_response' },
  )
  assert.equal(calls, 1)
  assert.equal(cancelled, true)
})

test('SDK error bodies and causes containing credentials never escape the decision boundary', async () => {
  for (const body of [config.apiKey, { error: { message: config.apiKey } }]) {
    await assert.rejects(
      callRemoteDecisions(config, input, {
        fetchImpl: async () => Response.json(body, { status: 401 }),
      }),
      (error) => {
        assert.equal(error.code, 'auth')
        assert.equal(error.cause, undefined)
        assert.ok(!JSON.stringify(error).includes(config.apiKey))
        assert.ok(!error.message.includes(config.apiKey))
        return true
      },
    )
  }
})
