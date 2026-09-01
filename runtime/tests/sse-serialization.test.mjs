import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import test, { mock } from 'node:test'
import {
  replaceLoneSurrogates,
  SSE_STALL_TIMEOUT_MS,
  sseSend,
  sseSendGuarded,
} from '../http/response.mjs'

test('replaceLoneSurrogates keeps valid surrogate pairs intact', () => {
  const pair = 'a\ud83d\ude00b'
  assert.equal(replaceLoneSurrogates(pair), pair)
  assert.equal(replaceLoneSurrogates(pair), 'a😀b')
})

test('replaceLoneSurrogates normalises lone high and low surrogates', () => {
  assert.equal(replaceLoneSurrogates('\ud83d'), '\ufffd')
  assert.equal(replaceLoneSurrogates('\ude00'), '\ufffd')
  assert.equal(replaceLoneSurrogates('x\ud83dy'), 'x\ufffdy')
  assert.equal(replaceLoneSurrogates('x\ude00y'), 'x\ufffdy')
})

test('replaceLoneSurrogates repairs a mix without touching plain text', () => {
  assert.equal(replaceLoneSurrogates('前\ud83d后\ud83d\ude00末\ude00'), '前\ufffd后😀末\ufffd')
  assert.equal(replaceLoneSurrogates('plain ascii'), 'plain ascii')
  assert.equal(replaceLoneSurrogates(''), '')
})

test('sseSend emits strict JSON even when payloads contain lone surrogates', () => {
  const chunks = []
  const res = {
    write(chunk) {
      chunks.push(chunk)
    },
  }
  sseSend(res, 'text_delta', { delta: '\ud83d' })
  const frame = chunks.join('')
  assert.ok(frame.startsWith('event: text_delta\ndata: '))
  const jsonText = frame.slice('event: text_delta\ndata: '.length).replace(/\n+$/, '')
  assert.deepEqual(JSON.parse(jsonText), { delta: '\ufffd' })
})

test('sseSend reports backpressure so callers can disconnect slow consumers', () => {
  const res = {
    write() {
      return false
    },
  }
  assert.equal(sseSend(res, 'snapshot', { text: 'working' }), false)
})

test('sseSendGuarded tolerates transient backpressure from healthy clients', () => {
  const res = {
    writableLength: 32 * 1024,
    destroyed: false,
    write() {
      return false
    },
    once() {},
    destroy() {
      this.destroyed = true
    },
  }
  // 瞬时背压（缓冲远低于上限）不断连，帧仍被视为已接收。
  assert.equal(sseSendGuarded(res, 'snapshot', { text: 'working' }), true)
  assert.equal(res.destroyed, false)
})

test('sseSendGuarded destroys the connection only after a drain stall timeout', () => {
  mock.timers.enable({ apis: ['setTimeout'] })
  try {
    const res = {
      writableLength: 64 * 1024,
      destroyed: false,
      write() {
        return false
      },
      once() {},
      destroy() {
        this.destroyed = true
      },
    }
    assert.equal(sseSendGuarded(res, 'snapshot', { text: 'working' }), true)
    // 背压本身不断连；持续 30s 排不出去才判死销毁。
    assert.equal(res.destroyed, false)
    mock.timers.tick(SSE_STALL_TIMEOUT_MS + 1)
    assert.equal(res.destroyed, true)
  } finally {
    mock.timers.reset()
  }
})

test('sseSendGuarded clears the stall timer once the client drains', () => {
  mock.timers.enable({ apis: ['setTimeout'] })
  try {
    const listeners = new Map()
    const res = {
      writableLength: 64 * 1024,
      destroyed: false,
      write() {
        return false
      },
      once(event, callback) {
        listeners.set(event, callback)
      },
      destroy() {
        this.destroyed = true
      },
    }
    assert.equal(sseSendGuarded(res, 'snapshot', { text: 'working' }), true)
    // 客户端恢复排空：drain 解除判死计时，超时后连接仍存活。
    listeners.get('drain')?.()
    mock.timers.tick(SSE_STALL_TIMEOUT_MS + 1)
    assert.equal(res.destroyed, false)
  } finally {
    mock.timers.reset()
  }
})

test('sseSendGuarded removes paired close listeners after every drain cycle', () => {
  class BackpressuredResponse extends EventEmitter {
    writableLength = 64 * 1024
    destroyed = false
    writableEnded = false

    write() {
      return false
    }
  }

  const res = new BackpressuredResponse()
  for (let cycle = 0; cycle < 20; cycle += 1) {
    assert.equal(sseSendGuarded(res, 'agent_update', { cycle }), true)
    assert.equal(res.listenerCount('close'), 1)
    res.emit('drain')
    assert.equal(res.listenerCount('close'), 0)
    assert.equal(res.listenerCount('drain'), 0)
  }
})

test('sseSendGuarded skips destroyed or ended responses', () => {
  const res = {
    writableLength: 0,
    destroyed: true,
    writes: 0,
    write() {
      this.writes += 1
      return true
    },
  }
  assert.equal(sseSendGuarded(res, 'snapshot', { text: 'working' }), false)
  assert.equal(res.writes, 0)
})

test('sseSend keeps valid surrogate pairs as parseable JSON', () => {
  const chunks = []
  const res = {
    write(chunk) {
      chunks.push(chunk)
    },
  }
  sseSend(res, 'text_delta', { delta: '你好😀' })
  const frame = chunks.join('')
  const jsonText = frame.slice('event: text_delta\ndata: '.length).replace(/\n+$/, '')
  assert.deepEqual(JSON.parse(jsonText), { delta: '你好😀' })
})
