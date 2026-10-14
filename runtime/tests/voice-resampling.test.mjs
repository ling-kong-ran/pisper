import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'

const source = await readFile('src/features/chat/voice-input.ts', 'utf8')
const compiled = transformSync(`${source}\nexport { VOICE_WORKLET_JS }`, {
  loader: 'ts',
  format: 'cjs',
}).code
const module = { exports: {} }
runInNewContext(compiled, {
  module,
  exports: module.exports,
  require(name) {
    assert.ok(
      ['@/lib/http', '@/lib/api', '@/lib/abort-signal', '@shared/speech-terms.mjs'].includes(name),
    )
    return {}
  },
})
const workletSource = module.exports.VOICE_WORKLET_JS
assert.equal(typeof workletSource, 'string')

const targetSampleRate = 16_000
const durationSeconds = 10
const floatTolerance = 1e-7

function createProcessor(inputSampleRate) {
  let Processor
  const chunks = []
  runInNewContext(workletSource, {
    sampleRate: inputSampleRate,
    AudioWorkletProcessor: class {
      port = {
        postMessage(buffer, transfer) {
          assert.equal(transfer.length, 1)
          assert.equal(transfer[0], buffer)
          // 使用真实转移语义，避免消息发出后仍共享原始缓冲区掩盖错误。
          const received = structuredClone(buffer, { transfer: [buffer] })
          chunks.push(new Float32Array(received))
        },
      }
    },
    registerProcessor(name, constructor) {
      assert.equal(name, 'pisper-voice-input')
      assert.equal(Processor, undefined)
      Processor = constructor
    },
  })
  assert.equal(typeof Processor, 'function')
  return {
    processor: new Processor({ processorOptions: { targetSampleRate } }),
    chunks,
  }
}

function* blockLengths(mode) {
  if (mode === '128-frame') {
    while (true) yield 128
  }
  // 固定种子与边界长度使跨块错误可重复，不依赖机器上的随机状态。
  yield* [1, 2, 127, 128, 129, 320, 511]
  let seed = 0x5eed1234
  while (true) {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0
    yield 1 + (seed % 511)
  }
}

function linearReference(input, inputSampleRate) {
  // 直接从全局输出序号计算有理时间，不复制生产代码的累计相位或缓冲区裁剪。
  // 与现有合同一致：必须存在右侧插值样本；不补零、不外推、不强行刷新尾帧。
  const count = Math.ceil(((input.length - 1) * targetSampleRate) / inputSampleRate)
  return Float32Array.from({ length: count }, (_, outputIndex) => {
    const numerator = outputIndex * inputSampleRate
    const left = Math.floor(numerator / targetSampleRate)
    const fraction = (numerator % targetSampleRate) / targetSampleRate
    return input[left] * (1 - fraction) + input[left + 1] * fraction
  })
}

function capture(input, inputSampleRate, mode) {
  const { processor, chunks } = createProcessor(inputSampleRate)
  let offset = 0
  let blocks = 0
  const output = new Float32Array(128)
  for (const size of blockLengths(mode)) {
    if (offset === input.length) break
    const end = Math.min(offset + size, input.length)
    output.fill(1)
    assert.equal(processor.process([[input.subarray(offset, end)]], [[output]]), true)
    assert.ok(output.every((sample) => sample === 0))
    offset = end
    blocks += 1
  }
  const deliveredCount = chunks.reduce((count, chunk) => count + chunk.length, 0)
  // 尾部未满 320 的内部输出计入生成总数，但不伪装成已发送 PCM。
  const generated = new Float32Array(deliveredCount + processor.outputBuffer.length)
  let written = 0
  for (const chunk of chunks) {
    generated.set(chunk, written)
    written += chunk.length
  }
  generated.set(processor.outputBuffer, written)
  return { processor, chunks, generated, deliveredCount, blocks }
}

function differences(actual, expected) {
  let firstMismatch = null
  let maxAbsoluteError = 0
  for (let index = 0; index < Math.min(actual.length, expected.length); index += 1) {
    const error = Math.abs(actual[index] - expected[index])
    maxAbsoluteError = Math.max(maxAbsoluteError, error)
    if (error > floatTolerance && firstMismatch === null) {
      firstMismatch = { index, actual: actual[index], expected: expected[index], error }
    }
  }
  return { firstMismatch, maxAbsoluteError }
}

function assertPhase(actual, expected) {
  const { firstMismatch, maxAbsoluteError } = differences(actual, expected)
  assert.equal(
    firstMismatch,
    null,
    `global linear interpolation diverged: ${JSON.stringify({ firstMismatch, maxAbsoluteError })}`,
  )
}

test('linear reference uses global rational positions and requires right-hand support', () => {
  assert.deepEqual(
    linearReference(new Float32Array([0, 1, 2, 3, 4]), 48_000),
    new Float32Array([0, 3]),
  )
  assert.deepEqual(
    linearReference(new Float32Array([0, 1, 2, 3, 4]), 16_000),
    new Float32Array([0, 1, 2, 3]),
  )
  assert.deepEqual(
    linearReference(new Float32Array([0, 1, 2, 3, 4, 5, 6]), 44_100),
    new Float32Array([0, 2.75625, 5.5125]),
  )
})

for (const inputSampleRate of [48_000, 44_100, 16_000]) {
  for (const mode of ['128-frame', 'random-frame']) {
    const label = `${inputSampleRate}->${targetSampleRate} ${mode}`
    const length = inputSampleRate * durationSeconds
    const input = Float32Array.from({ length }, (_, index) => index / length)
    const reference = linearReference(input, inputSampleRate)
    const captured = capture(input, inputSampleRate, mode)
    const { processor, chunks, generated, deliveredCount, blocks } = captured

    test(`${label}: generated and delivered sample counts preserve the output rate`, (t) => {
      t.diagnostic(
        JSON.stringify({
          label,
          inputSamples: length,
          inputSeconds: durationSeconds,
          blocks,
          expectedGenerated: reference.length,
          actualGenerated: generated.length,
          expectedDelivered: Math.floor(reference.length / 320) * 320,
          actualDelivered: deliveredCount,
          pending: processor.outputBuffer.length,
          generatedSamplesPerInputSecond: generated.length / durationSeconds,
          ...differences(generated, reference),
        }),
      )
      assert.equal(generated.length, reference.length, 'generated count, including unposted tail')
      assert.equal(deliveredCount, Math.floor(reference.length / 320) * 320, 'posted count')
      assert.equal(processor.outputBuffer.length, reference.length % 320, 'unposted tail count')
    })

    test(`${label}: ramp preserves continuous phase across input and output boundaries`, () => {
      assertPhase(generated, reference)
    })

    test(`${label}: every generated sample remains finite and within the ramp range`, () => {
      assert.ok(generated.length > targetSampleRate * (durationSeconds - 1))
      assert.ok(generated.every((sample) => Number.isFinite(sample) && sample >= 0 && sample <= 1))
    })

    test(`${label}: every transferred PCM message contains exactly 320 float samples`, () => {
      assert.ok(chunks.length >= 499)
      for (const chunk of chunks) {
        assert.equal(chunk.length, 320)
        assert.equal(chunk.byteLength, 1280)
        assert.ok(chunk.every(Number.isFinite))
      }
      assert.ok(processor.outputBuffer.length < 320)
    })

    test(`${label}: known 997 Hz signal matches independent linear interpolation`, () => {
      const signal = Float32Array.from(
        { length },
        (_, index) => 0.5 * Math.sin((2 * Math.PI * 997 * index) / inputSampleRate),
      )
      const actual = capture(signal, inputSampleRate, mode).generated
      const expected = linearReference(signal, inputSampleRate)
      assertPhase(actual, expected)
      assert.equal(actual.length, expected.length)
    })
  }
}

test('48000->16000 retains the one-sample phase debt after a 128-frame quantum', () => {
  const { processor } = createProcessor(48_000)
  processor.process([[Float32Array.from({ length: 128 }, (_, index) => index)]], [])
  assert.equal(processor.outputBuffer.length, 43)
  assert.equal(processor.inputBuffer.length, 0)
  assert.equal(processor.readPosition, 1, 'the next global position is 129, not 128')
})
