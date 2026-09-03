import { access } from 'node:fs/promises'
import { join, resolve } from 'node:path'
const SAMPLE_RATE = 16_000
const MAX_SAMPLES = SAMPLE_RATE * 10 * 60
const MODEL_FILES = ['model.int8.onnx', 'tokens.txt']

async function hasModelFiles(directory) {
  try {
    await Promise.all(MODEL_FILES.map((file) => access(join(directory, file))))
    return true
  } catch {
    return false
  }
}

export class SpeechRecognitionService {
  constructor({ packagedModelDir = '', modelDir = process.env.PISPER_SPEECH_MODEL_DIR } = {}) {
    this.packagedModelDir = packagedModelDir ? resolve(packagedModelDir) : ''
    this.configuredModelDir = modelDir ? resolve(modelDir) : ''
    this.nativeModule = null
    this.recognizer = null
    this.operation = Promise.resolve()
  }

  async resolveModelDir() {
    const candidates = [this.configuredModelDir, this.packagedModelDir].filter(Boolean)
    for (const candidate of candidates) {
      if (await hasModelFiles(candidate)) return candidate
    }
    throw new Error(
      '语音模型尚未随 Runtime 提供，需要 model.int8.onnx 和 tokens.txt；请检查发布包或 PISPER_SPEECH_MODEL_DIR。',
    )
  }

  async loadRecognizer() {
    if (this.recognizer) return this.recognizer
    const modelDir = await this.resolveModelDir()
    try {
      this.nativeModule ||= await import('sherpa-onnx-node')
    } catch (error) {
      throw new Error(
        `当前平台没有可用的 sherpa-onnx 原生运行时：${error instanceof Error ? error.message : String(error)}`,
      )
    }
    const OnlineRecognizer = this.nativeModule.OnlineRecognizer
    if (typeof OnlineRecognizer !== 'function') throw new Error('sherpa-onnx 原生运行时接口无效。')
    this.recognizer = new OnlineRecognizer({
      featConfig: { sampleRate: SAMPLE_RATE, featureDim: 80 },
      modelConfig: {
        zipformer2Ctc: { model: join(modelDir, 'model.int8.onnx') },
        tokens: join(modelDir, 'tokens.txt'),
        numThreads: 1,
        provider: 'cpu',
      },
      decodingMethod: 'greedy_search',
    })
    return this.recognizer
  }

  transcribe(samples) {
    const task = this.operation.then(() => this.transcribeOnce(samples))
    this.operation = task.catch(() => {})
    return task
  }

  async transcribeOnce(samples) {
    if (!(samples instanceof Float32Array) || samples.length === 0)
      throw new Error('语音数据为空。')
    if (samples.length > MAX_SAMPLES) throw new Error('单次语音输入不能超过 10 分钟。')
    const recognizer = await this.loadRecognizer()
    const stream = recognizer.createStream()
    stream.acceptWaveform({ samples, sampleRate: SAMPLE_RATE })
    while (recognizer.isReady(stream)) recognizer.decode(stream)
    stream.inputFinished()
    while (recognizer.isReady(stream)) recognizer.decode(stream)
    return String(recognizer.getResult(stream)?.text || '').trim()
  }
}

export { SAMPLE_RATE }
