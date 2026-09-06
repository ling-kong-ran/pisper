import assert from 'node:assert/strict'
import test from 'node:test'
import {
  createVoiceEndpoint,
  createVoiceEndpointAdapter,
} from '../../src/features/chat/voice-endpoint.ts'

function fixture(options = {}) {
  const frames = []
  let destroyed = 0
  const endpoint = createVoiceEndpointAdapter(
    () => ({
      processFrame(frame) {
        assert.equal(frame.length, 320)
        assert.equal(frame.byteOffset, 0)
        assert.equal(frame.buffer.byteLength, 640)
        frames.push(Array.from(frame))
        return frame[0] > 0 ? 1 : 0
      },
      destroy() {
        destroyed += 1
      },
    }),
    options,
  )
  return {
    endpoint,
    frames,
    get destroyed() {
      return destroyed
    },
  }
}

test('arbitrary chunks preserve every sample with a single bounded independent PCM frame', () => {
  const f = fixture()
  const samples = Float32Array.from({ length: 320 * 4 + 17 }, (_, i) => (i % 17) / 20)
  for (let offset = 0; offset < samples.length; offset += 73)
    assert.equal(f.endpoint.acceptPcm(samples.subarray(offset, offset + 73)), false)
  assert.equal(f.frames.length, 4)
  assert.deepEqual(
    f.frames.flat(),
    Array.from(samples.slice(0, 1280), (value) => Math.trunc(value * 32767)),
  )
  f.endpoint.acceptPcm(new Float32Array(303))
  assert.equal(f.frames.length, 5)
  assert.deepEqual(
    f.frames[4].slice(0, 17),
    Array.from(samples.slice(1280), (value) => Math.trunc(value * 32767)),
  )
  f.endpoint.dispose()
})

test('confirmed onset and minimum voice require full hangover and emit end only once', () => {
  const f = fixture({ onsetMs: 40, minVoicedMs: 80, silenceMs: 100 })
  assert.equal(f.endpoint.acceptPcm(new Float32Array(1280).fill(0.5)), false)
  assert.equal(f.endpoint.hasSpeech, true)
  assert.equal(f.endpoint.acceptPcm(new Float32Array(1599)), false)
  assert.equal(f.endpoint.acceptPcm(new Float32Array(1)), true)
  assert.equal(f.endpoint.acceptPcm(new Float32Array(3200)), false)
  f.endpoint.reset()
  assert.equal(f.destroyed, 1)
  assert.equal(f.endpoint.hasSpeech, false)
  f.endpoint.acceptPcm(new Float32Array(1280).fill(0.5))
  assert.equal(f.endpoint.acceptPcm(new Float32Array(1600)), true)
  f.endpoint.dispose()
  f.endpoint.dispose()
  f.endpoint.reset()
  assert.equal(f.destroyed, 2)
  assert.equal(f.endpoint.acceptPcm(new Float32Array(320)), false)
})

test('silence and short clicks never confirm speech or trigger an endpoint', () => {
  const f = fixture({ onsetMs: 40, minVoicedMs: 100, silenceMs: 100 })
  assert.equal(f.endpoint.acceptPcm(new Float32Array(16000 * 60)), false)
  f.endpoint.acceptPcm(new Float32Array(640).fill(0.5))
  assert.equal(f.endpoint.hasSpeech, false)
  assert.equal(f.endpoint.acceptPcm(new Float32Array(16000)), false)
  assert.equal(f.endpoint.hasSpeech, false)
  f.endpoint.dispose()
})

test('invalid durations and libfvad classification failures surface', () => {
  for (const value of [0, -1, NaN, Infinity])
    assert.throws(() => fixture({ silenceMs: value }), /Invalid VAD/)
  const endpoint = createVoiceEndpointAdapter(() => ({ processFrame: () => -1, destroy() {} }))
  assert.throws(() => endpoint.acceptPcm(new Float32Array(320)), /processing failed/)
  endpoint.dispose()
})

test('actual WebRTC WASM rejects silence and endpoints a modulated voiced segment', async () => {
  const endpoint = await createVoiceEndpoint({ onsetMs: 40, minVoicedMs: 80, silenceMs: 200 })
  try {
    assert.equal(endpoint.acceptPcm(new Float32Array(16000)), false)
    assert.equal(endpoint.hasSpeech, false)
    const voice = Float32Array.from({ length: 16000 }, (_, index) => {
      const time = index / 16000
      const fundamental = 130 + 35 * Math.sin(time * 2 * Math.PI * 3)
      return (
        (0.55 + 0.3 * Math.sin(time * 2 * Math.PI * 5)) *
        (0.4 * Math.sin(2 * Math.PI * fundamental * time) +
          0.2 * Math.sin(2 * Math.PI * fundamental * time * 2) +
          0.1 * Math.sin(2 * Math.PI * fundamental * time * 3))
      )
    })
    let ends = Number(endpoint.acceptPcm(voice))
    assert.equal(endpoint.hasSpeech, true)
    ends += Number(endpoint.acceptPcm(new Float32Array(16000 * 2)))
    ends += Number(endpoint.acceptPcm(new Float32Array(16000)))
    assert.equal(ends, 1)
  } finally {
    endpoint.dispose()
  }
})
