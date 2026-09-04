import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { SpeechRecognitionService } from '../services/speech-recognition-service.mjs'

function createFakeNative() {
  const stats = { created: 0, decodes: 0 }
  class FakeStream {
    constructor() {
      this.samples = 0
      this.ready = false
    }

    acceptWaveform({ samples }) {
      this.samples += samples.length
      this.ready = true
    }

    inputFinished() {
      // 模拟真实 sherpa：输入结束后尾部仍可能有待解码帧。
      this.ready = true
    }
  }
  class FakeOnlineRecognizer {
    constructor() {
      stats.created += 1
    }

    createStream() {
      return new FakeStream()
    }

    isReady(stream) {
      return stream.ready
    }

    decode(stream) {
      stream.ready = false
      stats.decodes += 1
    }

    getResult(stream) {
      return { text: stream.samples ? `共${stream.samples}样本` : '' }
    }
  }
  return { native: { OnlineRecognizer: FakeOnlineRecognizer }, stats }
}

async function createModelDir(t) {
  const dir = await mkdtemp(join(tmpdir(), 'pisper-speech-model-'))
  await writeFile(join(dir, 'model.int8.onnx'), 'fake')
  await writeFile(join(dir, 'tokens.txt'), 'fake')
  t.after(() => rm(dir, { recursive: true, force: true }))
  return dir
}

async function settle(ms = 20) {
  await new Promise((resolve) => setTimeout(resolve, ms))
}

test('一次性转写保持可用并在空闲后卸载识别器', async (t) => {
  const modelDir = await createModelDir(t)
  const { native, stats } = createFakeNative()
  const service = new SpeechRecognitionService({ modelDir, nativeModule: native, idleUnloadMs: 5 })
  const text = await service.transcribe(new Float32Array(320))
  assert.equal(text, '共320样本')
  assert.equal(stats.created, 1)
  assert.ok(service.recognizer, '转写期间识别器应存活')
  await settle()
  assert.equal(service.recognizer, null, '空闲超时后应释放识别器引用')
})

test('流式会话增量返回部分文本，finish 汇总并卸载', async (t) => {
  const modelDir = await createModelDir(t)
  const { native, stats } = createFakeNative()
  const service = new SpeechRecognitionService({ modelDir, nativeModule: native, idleUnloadMs: 5 })

  const { id } = await service.startSession()
  const partial = await service.acceptChunk(id, new Float32Array(160))
  assert.equal(partial.text, '共160样本')
  const partial2 = await service.acceptChunk(id, new Float32Array(160))
  assert.equal(partial2.text, '共320样本')

  // 流式会话存活期间不允许空闲卸载。
  await settle()
  assert.ok(service.recognizer, '存在活跃会话时识别器不应被卸载')

  const final = await service.finishSession(id)
  assert.equal(final.text, '共320样本')
  assert.equal(stats.decodes, 3)
  await settle()
  assert.equal(service.recognizer, null, '会话结束后空闲超时应卸载')

  await assert.rejects(() => service.acceptChunk(id, new Float32Array(10)), /会话不存在/)
})

test('cancel 移除会话且静默幂等', async (t) => {
  const modelDir = await createModelDir(t)
  const { native } = createFakeNative()
  const service = new SpeechRecognitionService({ modelDir, nativeModule: native })

  const { id } = await service.startSession()
  await service.acceptChunk(id, new Float32Array(160))
  assert.deepEqual(await service.cancelSession(id), { ok: true })
  assert.deepEqual(await service.cancelSession(id), { ok: true })
  assert.equal(service.sessions.size, 0)
  await assert.rejects(() => service.finishSession(id), /会话不存在/)
})

test('会话总量超过 10 分钟限制时拒绝继续喂入', async (t) => {
  const modelDir = await createModelDir(t)
  const { native } = createFakeNative()
  const service = new SpeechRecognitionService({ modelDir, nativeModule: native })
  const { id } = await service.startSession()
  // 直接把累计量撑到上限边界，避免构造巨大数组。
  service.getSession(id).totalSamples = 16_000 * 10 * 60
  await assert.rejects(() => service.acceptChunk(id, new Float32Array(1)), /10 分钟/)
})

test('过期会话被清扫，未知会话与空数据被拒绝', async (t) => {
  const modelDir = await createModelDir(t)
  const { native } = createFakeNative()
  const service = new SpeechRecognitionService({ modelDir, nativeModule: native })
  const { id } = await service.startSession()
  service.getSession(id).lastActive = Date.now() - 11 * 60_000
  service.sweepExpiredSessions()
  assert.equal(service.sessions.size, 0)
  await assert.rejects(() => service.acceptChunk('missing', new Float32Array(1)), /会话不存在/)
  await assert.rejects(() => service.acceptChunk(id, new Float32Array(0)), /语音数据为空/)
})

test('模型缺失时给出明确错误', async () => {
  const service = new SpeechRecognitionService({ modelDir: '/nonexistent', nativeModule: null })
  await assert.rejects(() => service.transcribe(new Float32Array(1)), /语音模型尚未随 Runtime 提供/)
})
