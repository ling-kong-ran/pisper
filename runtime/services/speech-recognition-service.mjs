import { randomUUID } from 'node:crypto'
import { access, mkdir, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { formatSpeechTerms, speechHotwords } from '../../shared/speech-terms.mjs'

const SAMPLE_RATE = 16_000
const MAX_SAMPLES = SAMPLE_RATE * 10 * 60
const MODEL_FILES = [
  'encoder.int8.onnx',
  'decoder.onnx',
  'joiner.int8.onnx',
  'tokens.txt',
  'bpe.model',
]
// 空闲 30s 后释放识别器引用，让原生模型内存可被 GC 回收，避免模型常驻。
const RECOGNIZER_IDLE_UNLOAD_MS = 30_000
// 流式会话 10 分钟无活动自动取消，防止前端异常退出导致 stream 泄漏。
const SESSION_TTL_MS = 10 * 60_000
const MAX_SESSIONS = 4

async function hasModelFiles(directory) {
  try {
    await Promise.all(MODEL_FILES.map((file) => access(join(directory, file))))
    return true
  } catch {
    return false
  }
}

function assertSamples(samples) {
  if (!(samples instanceof Float32Array) || samples.length === 0) throw new Error('语音数据为空。')
}

export class SpeechRecognitionService {
  constructor({
    packagedModelDir = '',
    modelDir = process.env.PISPER_SPEECH_MODEL_DIR,
    hotwordsDir = '',
    bpeVocabPath = '',
    // 测试注入假原生模块，避免依赖真实 sherpa-onnx。
    nativeModule = null,
    idleUnloadMs = RECOGNIZER_IDLE_UNLOAD_MS,
  } = {}) {
    this.packagedModelDir = packagedModelDir ? resolve(packagedModelDir) : ''
    this.configuredModelDir = modelDir ? resolve(modelDir) : ''
    this.hotwordsDir = hotwordsDir ? resolve(hotwordsDir) : ''
    this.bpeVocabPath = bpeVocabPath ? resolve(bpeVocabPath) : ''
    this.hotwordsKey = ''
    this.injectedNativeModule = nativeModule
    this.nativeModule = null
    this.recognizer = null
    this.operation = Promise.resolve()
    this.sessions = new Map()
    this.idleUnloadMs = idleUnloadMs
    this.idleTimer = null
  }

  async resolveModelDir() {
    const candidates = [this.configuredModelDir, this.packagedModelDir].filter(Boolean)
    for (const candidate of candidates) {
      if (await hasModelFiles(candidate)) return candidate
    }
    throw new Error('语音模型尚未安装，请先下载语音识别模型。')
  }

  async loadRecognizer(terms = []) {
    const hotwords = speechHotwords(terms)
    if (this.recognizer && this.hotwordsKey === hotwords) return this.recognizer
    if (this.recognizer) {
      if ([...this.sessions.values()].some((session) => session.stream)) {
        throw new Error('另一个语音会话正在使用不同的项目术语，请结束后重试。')
      }
      // 保持单一识别器，不能为每个项目常驻一份模型。
      this.unloadRecognizer()
    }
    const modelDir = await this.resolveModelDir()
    const bpeVocab = this.bpeVocabPath || join(modelDir, 'bpe.vocab')
    const supportsHotwords =
      hotwords &&
      this.hotwordsDir &&
      (await access(bpeVocab).then(
        () => true,
        () => false,
      ))
    let hotwordsFile = ''
    if (supportsHotwords) {
      await mkdir(this.hotwordsDir, { recursive: true })
      hotwordsFile = join(this.hotwordsDir, 'active-terms.txt')
      await writeFile(hotwordsFile, `${hotwords}\n`, 'utf8')
    }
    if (!this.nativeModule) {
      try {
        this.nativeModule = this.injectedNativeModule || (await import('sherpa-onnx-node'))
      } catch (error) {
        throw new Error(
          `当前平台没有可用的 sherpa-onnx 原生运行时：${error instanceof Error ? error.message : String(error)}`,
        )
      }
    }
    const OnlineRecognizer = this.nativeModule.OnlineRecognizer
    if (typeof OnlineRecognizer !== 'function') throw new Error('sherpa-onnx 原生运行时接口无效。')
    this.recognizer = new OnlineRecognizer({
      featConfig: { sampleRate: SAMPLE_RATE, featureDim: 80 },
      modelConfig: {
        transducer: {
          encoder: join(modelDir, 'encoder.int8.onnx'),
          decoder: join(modelDir, 'decoder.onnx'),
          joiner: join(modelDir, 'joiner.int8.onnx'),
        },
        tokens: join(modelDir, 'tokens.txt'),
        ...(supportsHotwords ? { modelingUnit: 'bpe', bpeVocab } : {}),
        numThreads: 1,
        provider: 'cpu',
      },
      decodingMethod: supportsHotwords ? 'modified_beam_search' : 'greedy_search',
      ...(supportsHotwords ? { maxActivePaths: 2, hotwordsFile, hotwordsScore: 1.5 } : {}),
    })
    this.hotwordsKey = hotwords
    return this.recognizer
  }

  // sherpa-onnx-node 没有显式 free API，置空引用后由 GC 回收原生模型内存。
  unloadRecognizer() {
    this.recognizer = null
    if (typeof globalThis.gc === 'function') globalThis.gc()
  }

  // 每次识别活动后刷新空闲计时；存在活跃流式会话时不能卸载。
  markActive() {
    if (this.idleTimer) {
      clearTimeout(this.idleTimer)
      this.idleTimer = null
    }
  }

  scheduleIdleUnload() {
    if (this.sessions.size > 0 || this.idleUnloadMs <= 0) return
    this.markActive()
    this.idleTimer = setTimeout(() => {
      this.idleTimer = null
      if (this.sessions.size === 0) this.unloadRecognizer()
    }, this.idleUnloadMs)
    this.idleTimer.unref?.()
  }

  enqueue(task) {
    const run = this.operation.then(task)
    this.operation = run.catch(() => {})
    return run
  }

  decodeReady(recognizer, stream) {
    while (recognizer.isReady(stream)) recognizer.decode(stream)
    return String(recognizer.getResult(stream)?.text || '').trim()
  }

  transcribe(samples, { terms = [] } = {}) {
    assertSamples(samples)
    return this.enqueue(async () => {
      this.markActive()
      try {
        if (samples.length > MAX_SAMPLES) throw new Error('单次语音输入不能超过 10 分钟。')
        const recognizer = await this.loadRecognizer(terms)
        const stream = recognizer.createStream()
        stream.acceptWaveform({ samples, sampleRate: SAMPLE_RATE })
        // X-ASR 需要尾部上下文；补齐静音可避免按键松开时丢掉最后一个词。
        stream.acceptWaveform({ samples: new Float32Array(SAMPLE_RATE), sampleRate: SAMPLE_RATE })
        stream.inputFinished()
        return formatSpeechTerms(this.decodeReady(recognizer, stream), terms)
      } finally {
        this.scheduleIdleUnload()
      }
    })
  }

  async startSession({ terms = [] } = {}) {
    if (this.sessions.size >= MAX_SESSIONS) throw new Error('语音识别会话过多，请稍后重试。')
    this.markActive()
    const id = randomUUID()
    const session = { stream: null, totalSamples: 0, lastActive: Date.now(), terms: [...terms] }
    this.sessions.set(id, session)
    // 先加载识别器并创建 stream，失败时不留下空会话。
    try {
      await this.enqueue(async () => {
        const recognizer = await this.loadRecognizer(session.terms)
        session.stream = recognizer.createStream()
      })
    } catch (error) {
      this.sessions.delete(id)
      throw error
    }
    return { id }
  }

  getSession(id) {
    const session = this.sessions.get(String(id || ''))
    if (!session?.stream) throw new Error('语音识别会话不存在或已结束。')
    return session
  }

  acceptChunk(id, samples) {
    return this.enqueue(() => {
      assertSamples(samples)
      const session = this.getSession(id)
      this.markActive()
      session.lastActive = Date.now()
      session.totalSamples += samples.length
      if (session.totalSamples > MAX_SAMPLES) throw new Error('单次语音输入不能超过 10 分钟。')
      if (!this.recognizer) throw new Error('语音识别器已卸载，请重新开始录音。')
      session.stream.acceptWaveform({ samples, sampleRate: SAMPLE_RATE })
      return { text: this.decodeReady(this.recognizer, session.stream) }
    })
  }

  finishSession(id) {
    return this.enqueue(() => {
      const session = this.getSession(id)
      this.markActive()
      try {
        if (!this.recognizer) throw new Error('语音识别器已卸载，请重新开始录音。')
        if (session.totalSamples > 0) {
          session.stream.acceptWaveform({
            samples: new Float32Array(SAMPLE_RATE),
            sampleRate: SAMPLE_RATE,
          })
        }
        session.stream.inputFinished()
        const text = formatSpeechTerms(
          this.decodeReady(this.recognizer, session.stream),
          session.terms,
        )
        if (!text) throw new Error('未识别到语音内容。')
        return { text }
      } finally {
        this.sessions.delete(id)
        this.scheduleIdleUnload()
      }
    })
  }

  async cancelSession(id) {
    const session = this.sessions.get(String(id || ''))
    if (!session) return { ok: true }
    this.sessions.delete(id)
    this.scheduleIdleUnload()
    return { ok: true }
  }

  sweepExpiredSessions(now = Date.now()) {
    for (const [id, session] of this.sessions) {
      if (now - session.lastActive > SESSION_TTL_MS) this.sessions.delete(id)
    }
  }
}

export { MAX_SAMPLES, RECOGNIZER_IDLE_UNLOAD_MS, SAMPLE_RATE, SESSION_TTL_MS }
