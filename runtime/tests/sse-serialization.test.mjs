import assert from 'node:assert/strict'
import test from 'node:test'
import { replaceLoneSurrogates, sseSend } from '../http/response.mjs'

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
