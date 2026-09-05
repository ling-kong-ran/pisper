import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import test, { mock } from 'node:test'
import {
  SSE_HEARTBEAT_INTERVAL_MS,
  startSseHeartbeat,
  stopSseHeartbeat,
} from '../http/response.mjs'

class FakeResponse extends EventEmitter {
  destroyed = false
  writableEnded = false
  writes = []
  writeResults = []

  write(chunk) {
    this.writes.push(chunk)
    return this.writeResults.shift() ?? true
  }
}

test('SSE heartbeat waits for drain after write backpressure', (t) => {
  const response = new FakeResponse()
  response.writeResults = [false, true]
  t.after(() => stopSseHeartbeat(response))

  mock.timers.enable({ apis: ['setInterval'] })
  t.after(() => mock.timers.reset())
  startSseHeartbeat(response)
  startSseHeartbeat(response)
  mock.timers.tick(SSE_HEARTBEAT_INTERVAL_MS)
  assert.equal(response.writes.length, 1)
  mock.timers.tick(SSE_HEARTBEAT_INTERVAL_MS)
  assert.equal(response.writes.length, 1)
  response.emit('drain')
  mock.timers.tick(SSE_HEARTBEAT_INTERVAL_MS)
  assert.equal(response.writes.length, 2)
})

test('SSE heartbeat stops on error and close without further writes', (t) => {
  const response = new FakeResponse()
  t.after(() => stopSseHeartbeat(response))

  mock.timers.enable({ apis: ['setInterval'] })
  t.after(() => mock.timers.reset())
  startSseHeartbeat(response)
  response.emit('error', new Error('socket failed'))
  mock.timers.tick(SSE_HEARTBEAT_INTERVAL_MS * 2)
  assert.equal(response.writes.length, 0)

  startSseHeartbeat(response)
  response.emit('close')
  mock.timers.tick(SSE_HEARTBEAT_INTERVAL_MS)
  assert.equal(response.writes.length, 0)
})
