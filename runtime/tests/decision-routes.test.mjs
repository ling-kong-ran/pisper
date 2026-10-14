import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import test from 'node:test'
import { decisionRoutes } from '../http/routes/decisions.mjs'
import { DecisionError } from '../services/decision-errors.mjs'

for (const path of ['/api/decisions/test', '/api/decisions/decide']) {
  test(`${path} cancels disconnected clients and releases lifecycle listeners`, async () => {
    const req = new EventEmitter()
    const res = new EventEmitter()
    const started = Promise.withResolvers()
    let requestSignal
    const decide = (_input, { signal }) =>
      new Promise((_resolve, reject) => {
        requestSignal = signal
        signal.addEventListener('abort', () => reject(new DecisionError('aborted', 'cancelled')), {
          once: true,
        })
        started.resolve()
      })
    const route = decisionRoutes.find((route) => route.path === path)
    const pending = route.handler({
      req,
      res,
      services: { decisions: { decide, testConnection: (options) => decide(null, options) } },
      body: async () => ({ state: 'x', questions: [] }),
      json: () => assert.fail('disconnected client must not receive a response'),
      publicError: (error) => error.message,
    })
    await started.promise
    res.destroyed = true
    res.emit('close')
    await pending
    assert.equal(requestSignal.aborted, true)
    assert.equal(req.listenerCount('aborted'), 0)
    assert.equal(res.listenerCount('close'), 0)
  })
}

test('a decision request disconnected while parsing its body never starts inference', async () => {
  const req = new EventEmitter()
  const res = new EventEmitter()
  const body = Promise.withResolvers()
  const entered = Promise.withResolvers()
  const route = decisionRoutes.find((route) => route.path === '/api/decisions/decide')
  const pending = route.handler({
    req,
    res,
    services: { decisions: { decide: () => assert.fail('must not infer') } },
    body: () => {
      entered.resolve()
      return body.promise
    },
    json: () => assert.fail('must not respond'),
    publicError: (error) => error.message,
  })
  await entered.promise
  res.destroyed = true
  res.emit('close')
  body.resolve({ state: 'x', questions: [] })
  await pending
  assert.equal(res.listenerCount('close'), 0)
})
