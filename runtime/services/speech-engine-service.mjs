import { fork } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { resolve } from 'node:path'

const WORKER_URL = new URL('../workers/speech-inference-worker.mjs', import.meta.url)
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i
const MAX_SAMPLES = 16_000 * 600
const MESSAGES = {
  invalid: 'Invalid speech input.',
  busy: 'Speech engine is busy. Please try again after the current operation.',
  missing: 'Speech model is not installed or failed verification.',
  session: 'Speech recognition session does not exist or has ended.',
  limit: 'Speech input or output exceeds the supported limit.',
  cancelled: 'Speech synthesis was cancelled.',
  disposed: 'Speech engine is disposed.',
  timeout: 'Speech inference timed out.',
  worker: 'Speech inference process stopped unexpectedly.',
  inference: 'Speech inference failed.',
  config: 'Speech model configuration is invalid.',
}

export function speechEngineError(code) {
  const error = new Error(MESSAGES[code] || MESSAGES.inference)
  error.code = Object.hasOwn(MESSAGES, code) ? code : 'inference'
  error.statusCode = code === 'busy' ? 409 : code === 'session' ? 404 : 400
  return error
}

export function validateSpeechText(text, maxTextLength = 400) {
  if (typeof text !== 'string' || !text.trim() || !text.isWellFormed()) {
    throw speechEngineError('invalid')
  }
  if (text.length > maxTextLength) throw speechEngineError('limit')
}

function validateAudio(result, maxOutputSeconds) {
  const { wav, sampleRate, durationMs } = result || {}
  if (
    !Buffer.isBuffer(wav) ||
    !Number.isInteger(sampleRate) ||
    sampleRate < 8000 ||
    sampleRate > 48000 ||
    !Number.isFinite(durationMs) ||
    durationMs <= 0 ||
    durationMs > maxOutputSeconds * 1000 ||
    wav.length < 46 ||
    wav.length % 2 !== 0 ||
    wav.length > 44 + sampleRate * maxOutputSeconds * 2 ||
    wav.toString('ascii', 0, 4) !== 'RIFF' ||
    wav.readUInt32LE(4) !== wav.length - 8 ||
    wav.toString('ascii', 8, 16) !== 'WAVEfmt ' ||
    wav.readUInt32LE(16) !== 16 ||
    wav.readUInt16LE(20) !== 1 ||
    wav.readUInt16LE(22) !== 1 ||
    wav.readUInt32LE(24) !== sampleRate ||
    wav.readUInt32LE(28) !== sampleRate * 2 ||
    wav.readUInt16LE(32) !== 2 ||
    wav.readUInt16LE(34) !== 16 ||
    wav.toString('ascii', 36, 40) !== 'data' ||
    wav.readUInt32LE(40) !== wav.length - 44 ||
    Math.abs(durationMs - ((wav.length - 44) / 2 / sampleRate) * 1000) > 0.001
  )
    throw speechEngineError('limit')
}

function assertSamples(samples) {
  if (!(samples instanceof Float32Array) || !samples.length) throw speechEngineError('invalid')
  if (samples.length > MAX_SAMPLES) throw speechEngineError('limit')
  for (const sample of samples) {
    if (!Number.isFinite(sample)) throw speechEngineError('invalid')
  }
}

// 只继承原生库加载和系统临时目录所需的环境，避免把提供商密钥交给模型进程。
export function speechWorkerEnvironment(env = process.env) {
  const allowed = new Set([
    'PATH',
    'SYSTEMROOT',
    'WINDIR',
    'SYSTEMDRIVE',
    'TEMP',
    'TMP',
    'TMPDIR',
    'HOME',
    'USERPROFILE',
    'LOCALAPPDATA',
    'APPDATA',
    'LANG',
    'LC_ALL',
    'LD_LIBRARY_PATH',
    'DYLD_LIBRARY_PATH',
    'DYLD_FALLBACK_LIBRARY_PATH',
    'PISPER_APP_ROOT',
  ])
  return Object.fromEntries(Object.entries(env).filter(([key]) => allowed.has(key.toUpperCase())))
}

export class SpeechEngineService {
  constructor({
    catalog,
    modelDownloads,
    resourceDir = '',
    hotwordsDir = '',
    idleUnloadMs = 30_000,
    sessionTtlMs = 600_000,
    // 会话孤儿（页面刷新/热更新中途断开）只能等 TTL；周期 sweep 让它们在
    // 无新会话进入时也能过期，避免孤儿 ASR stream 永久持有模型。0 关闭（测试用）。
    sessionSweepMs = 60_000,
    startupTimeoutMs = 60_000,
    inferenceTimeoutMs = 120_000,
    shutdownGraceMs = 250,
    maxTextLength = 400,
    maxOutputSeconds = 45,
    numThreads = null,
    workerUrl = WORKER_URL,
    forkProcess = fork,
  } = {}) {
    if (
      !catalog?.models ||
      !modelDownloads ||
      !Number.isInteger(maxTextLength) ||
      maxTextLength < 1 ||
      maxTextLength > 400 ||
      !Number.isFinite(maxOutputSeconds) ||
      maxOutputSeconds <= 0 ||
      maxOutputSeconds > 45 ||
      (numThreads !== null && (!Number.isInteger(numThreads) || numThreads < 1 || numThreads > 16))
    )
      throw speechEngineError('config')
    this.catalog = structuredClone(catalog)
    this.modelDownloads = modelDownloads
    this.resourceDir = resolve(resourceDir)
    this.hotwordsDir = hotwordsDir ? resolve(hotwordsDir) : ''
    Object.assign(this, {
      idleUnloadMs,
      sessionTtlMs,
      sessionSweepMs,
      startupTimeoutMs,
      inferenceTimeoutMs,
      shutdownGraceMs,
      maxTextLength,
      maxOutputSeconds,
      numThreads,
      workerUrl,
      forkProcess,
    })
    this.worker = null
    // ASR/TTS 各自持有进程和串行队列，两类计算可并行，类内请求仍有序。
    this.workers = new Map()
    this.voiceSessions = new Map()
    this.warmupOperations = 0
    this.sessions = new Map()
    this.pending = new Map()
    this.operations = new Map(['asr', 'tts'].map((kind) => [kind, Promise.resolve()]))
    this.asrOperations = 0
    this.activeSpeech = null
    this.speechTasks = new Map()
    this.controllers = new Set()
    this.idleTimer = null
    this.idleGeneration = 0
    this.disposed = false
    this.disposePromise = null
    // 周期清理过期会话；unref 保证计时器不会拖住进程退出。
    this.sessionSweepTimer = null
    if (Number.isFinite(sessionSweepMs) && sessionSweepMs > 0) {
      this.sessionSweepTimer = setInterval(() => {
        if (!this.disposed) this.sweepExpiredSessions()
      }, sessionSweepMs)
      this.sessionSweepTimer.unref?.()
    }
  }

  model(kind) {
    const model = this.catalog.models.find(
      (item) => item.kind === kind && item.id === this.catalog.defaults?.[kind],
    )
    if (!model) throw speechEngineError('config')
    return model
  }

  enqueue(task, kind = 'asr') {
    const result = this.operations.get(kind).then(task)
    this.operations.set(
      kind,
      result.catch(() => {}),
    )
    return result
  }

  markActive() {
    this.idleGeneration += 1
    clearTimeout(this.idleTimer)
    this.idleTimer = null
  }

  scheduleIdle() {
    this.markActive()
    if (
      this.disposed ||
      this.sessions.size ||
      this.voiceSessions.size ||
      this.warmupOperations ||
      this.asrOperations ||
      this.speechTasks.size ||
      !this.workers.size ||
      this.idleUnloadMs <= 0
    )
      return
    const workers = [...this.workers.values()]
    const generation = this.idleGeneration
    this.idleTimer = setTimeout(() => {
      this.idleTimer = null
      // 回收进入模型所属队列，不能越过该模型的加载或推理任务。
      for (const worker of workers) {
        void this.enqueue(async () => {
          if (
            generation === this.idleGeneration &&
            !this.sessions.size &&
            !this.voiceSessions.size &&
            !this.warmupOperations &&
            !this.asrOperations &&
            !this.speechTasks.size &&
            this.workers.get(worker.kind) === worker
          )
            await this.stopWorker(worker)
        }, worker.kind).catch(() => {})
      }
    }, this.idleUnloadMs)
    this.idleTimer.unref?.()
  }

  checkTask(task) {
    if (this.disposed) throw speechEngineError('disposed')
    if (task.signal.aborted) throw task.signal.reason
  }

  async prepareWorker(kind, task) {
    this.checkTask(task)
    const cached = this.workers.get(kind)
    if (cached && !cached.stopping && !cached.exited) {
      this.worker = cached
      task.worker = cached
      return cached
    }
    if (cached) await this.stopWorker(cached)
    this.checkTask(task)
    const model = this.model(kind)
    let modelDir
    try {
      modelDir = await this.waitFor(
        this.modelDownloads.modelDirectory(model.id),
        task,
        this.startupTimeoutMs,
      )
    } catch (error) {
      if (error?.code === 'timeout' || task.signal.aborted || this.disposed) throw error
      throw speechEngineError('missing')
    }
    this.checkTask(task)
    let child
    try {
      child = this.forkProcess(this.workerUrl, ['--pisper-speech-worker'], {
        execArgv: [],
        serialization: 'advanced',
        windowsHide: true,
        stdio: ['ignore', 'ignore', 'ignore', 'ipc'],
        env: speechWorkerEnvironment(),
      })
    } catch {
      throw speechEngineError('worker')
    }
    const worker = { child, kind, stopping: false, exited: false, exitPromise: null }
    worker.exitPromise = new Promise((resolveExit) => {
      const exited = () => {
        if (worker.exited) return
        worker.exited = true
        clearTimeout(worker.killTimer)
        if (this.worker === worker) this.worker = null
        if (this.workers.get(kind) === worker) this.workers.delete(kind)
        for (const [id, session] of this.sessions) {
          if (session.worker === worker) this.sessions.delete(id)
        }
        this.rejectPending(worker, speechEngineError('worker'))
        resolveExit()
      }
      child.once('exit', exited)
      child.once('error', () => {
        this.rejectPending(worker, speechEngineError('worker'))
        if (!child.pid) exited()
        else void this.stopWorker(worker, speechEngineError('worker'), true)
      })
      child.on('message', (message) => this.receive(worker, message))
      child.once('disconnect', () => {
        if (!worker.exited) void this.stopWorker(worker, speechEngineError('worker'), true)
      })
    })
    this.worker = worker
    this.workers.set(kind, worker)
    task.worker = worker
    try {
      await this.rpc(
        worker,
        'init',
        {
          kind,
          model: {
            id: model.id,
            engine: model.engine,
            config: model.config,
            files: model.files?.map(({ path, bytes, sha256 }) => ({ path, bytes, sha256 })),
          },
          modelDir,
          hotwordsDir: this.hotwordsDir,
          resourceDir: this.resourceDir,
          maxTextLength: this.maxTextLength,
          maxOutputSeconds: this.maxOutputSeconds,
          numThreads: this.numThreads ?? model.config?.numThreads ?? 2,
        },
        this.startupTimeoutMs,
      )
      this.checkTask(task)
      return worker
    } catch (error) {
      await this.stopWorker(worker, error, true)
      throw error
    }
  }

  waitFor(promise, task, timeoutMs) {
    return new Promise((resolveWait, reject) => {
      const finish = (callback, value) => {
        clearTimeout(timer)
        task.signal.removeEventListener('abort', abort)
        callback(value)
      }
      const abort = () => finish(reject, task.signal.reason)
      const timer = setTimeout(() => finish(reject, speechEngineError('timeout')), timeoutMs)
      task.signal.addEventListener('abort', abort, { once: true })
      Promise.resolve(promise).then(
        (value) => finish(resolveWait, value),
        (error) => finish(reject, error),
      )
      if (task.signal.aborted) abort()
    })
  }

  receive(worker, message) {
    const pending = this.pending.get(message?.id)
    if (!pending || pending.worker !== worker || worker.stopping) return
    this.pending.delete(message.id)
    clearTimeout(pending.timer)
    if (message.ok === true) pending.resolve(message.result)
    else pending.reject(speechEngineError(message?.error?.code))
  }

  rejectPending(worker, error) {
    for (const [id, pending] of this.pending) {
      if (pending.worker !== worker) continue
      clearTimeout(pending.timer)
      this.pending.delete(id)
      pending.reject(error)
    }
  }

  rpc(worker, method, params, timeoutMs = this.inferenceTimeoutMs) {
    if (worker.stopping || worker.exited) return Promise.reject(speechEngineError('worker'))
    const id = randomUUID()
    return new Promise((resolveRpc, reject) => {
      const timer = setTimeout(() => {
        void this.stopWorker(worker, speechEngineError('timeout'), true)
      }, timeoutMs)
      this.pending.set(id, { worker, resolve: resolveRpc, reject, timer })
      try {
        worker.child.send({ id, method, params }, (error) => {
          if (error) void this.stopWorker(worker, speechEngineError('worker'), true)
        })
      } catch {
        void this.stopWorker(worker, speechEngineError('worker'), true)
      }
    })
  }

  stopWorker(worker, error = speechEngineError('worker'), immediate = false) {
    if (worker.exited) return worker.exitPromise
    this.rejectPending(worker, error)
    const kill = () => {
      if (!worker.exited) worker.child.kill('SIGKILL')
    }
    if (!worker.stopping) {
      worker.stopping = true
      if (immediate) kill()
      else {
        try {
          worker.child.send({ method: 'shutdown' }, (sendError) => {
            if (sendError) kill()
          })
        } catch {
          kill()
        }
        worker.killTimer = setTimeout(kill, this.shutdownGraceMs)
      }
    } else if (immediate) kill()
    return worker.exitPromise
  }

  async prepareSpeechSession(
    { requestId, kinds, hotwords = '', voiceId = this.catalog.defaults?.voice } = {},
    signal,
  ) {
    if (
      typeof requestId !== 'string' ||
      !UUID.test(requestId) ||
      !Array.isArray(kinds) ||
      kinds.length < 1 ||
      kinds.length > 2 ||
      new Set(kinds).size !== kinds.length ||
      kinds.some((kind) => !['asr', 'tts'].includes(kind)) ||
      typeof hotwords !== 'string' ||
      !hotwords.isWellFormed() ||
      Buffer.byteLength(hotwords) > 20 * 1024 ||
      /[:#@/\p{Cc}\p{Z}]/u.test(hotwords.replace(/[\n ]/g, '')) ||
      (hotwords &&
        (hotwords.split('\n').length > 128 ||
          hotwords.split('\n').some((term) => !term.trim() || term.length > 128))) ||
      !signal ||
      typeof signal.addEventListener !== 'function'
    )
      throw speechEngineError('invalid')
    if (kinds.includes('tts') && !this.model('tts').voices?.some((voice) => voice.id === voiceId))
      throw speechEngineError('invalid')
    if (this.disposed) throw speechEngineError('disposed')
    if (signal.aborted) throw speechEngineError('cancelled')
    if (this.voiceSessions.has(requestId) || this.voiceSessions.size >= 16)
      throw speechEngineError('busy')
    const requestedKinds = [...kinds]
    const controller = new AbortController()
    const task = { signal: controller.signal }
    const release = () => {
      if (this.voiceSessions.get(requestId)?.controller !== controller) return
      this.voiceSessions.delete(requestId)
      signal.removeEventListener('abort', release)
      controller.abort(speechEngineError('cancelled'))
      this.controllers.delete(controller)
      this.scheduleIdle()
    }
    // 连接代表语音模式的持有权；提前登记，预热期间退出也不能留下孤儿模型持有。
    this.voiceSessions.set(requestId, { controller, release })
    this.controllers.add(controller)
    signal.addEventListener('abort', release, { once: true })
    this.warmupOperations += 1
    this.markActive()
    try {
      // 加载进入各自队列，可跨模型并行，但不能与同模型的推理或回收交错。
      // 必须观察两项完成，某一项失败不能把另一项的迟到结果遗留在后台。
      const results = await Promise.allSettled(
        requestedKinds.map((kind) =>
          this.enqueue(async () => {
            const loading = { signal: task.signal }
            const worker = await this.prepareWorker(kind, loading)
            this.checkTask(loading)
            // ASR init 只建立服务对象，必须实际加载识别器，不能把空 worker 当预热成功。
            if (kind === 'asr')
              await this.rpc(worker, 'warmup', { terms: hotwords ? hotwords.split('\n') : [] })
            this.checkTask(loading)
          }, kind),
        ),
      )
      const failed = results.find((result) => result.status === 'rejected')
      if (failed) throw failed.reason
      this.checkTask(task)
      return { ready: true }
    } catch (error) {
      release()
      throw error
    } finally {
      this.warmupOperations -= 1
      this.scheduleIdle()
    }
  }

  asr(taskFn) {
    if (this.disposed) return Promise.reject(speechEngineError('disposed'))
    const controller = new AbortController()
    const task = { signal: controller.signal }
    this.controllers.add(controller)
    this.asrOperations += 1
    this.markActive()
    return this.enqueue(async () => {
      this.checkTask(task)
      return taskFn(task)
    }).finally(() => {
      this.controllers.delete(controller)
      this.asrOperations -= 1
      this.scheduleIdle()
    })
  }

  async transcribe(samples, { terms = [] } = {}) {
    assertSamples(samples)
    const snapshot = samples.slice()
    const options = structuredClone({ terms })
    return this.asr(async (task) => {
      const worker = await this.prepareWorker('asr', task)
      return this.rpc(worker, 'transcribe', { samples: snapshot, ...options })
    })
  }

  async startSession({ terms = [] } = {}) {
    const options = structuredClone({ terms })
    return this.asr(async (task) => {
      if (this.sessions.size >= 4) throw speechEngineError('busy')
      const worker = await this.prepareWorker('asr', task)
      const result = await this.rpc(worker, 'startSession', options)
      this.sessions.set(result.id, { worker, lastActive: Date.now(), totalSamples: 0 })
      return result
    })
  }

  session(id) {
    const session = this.sessions.get(id)
    if (!session || session.worker.exited || session.worker.stopping)
      throw speechEngineError('session')
    return session
  }

  async acceptChunk(id, samples) {
    assertSamples(samples)
    const snapshot = samples.slice()
    return this.asr(async () => {
      const session = this.session(id)
      session.lastActive = Date.now()
      session.totalSamples += snapshot.length
      try {
        if (session.totalSamples > MAX_SAMPLES) throw speechEngineError('limit')
        return await this.rpc(session.worker, 'acceptChunk', { id, samples: snapshot })
      } catch (error) {
        await this.clearSession(id, session)
        throw error
      }
    })
  }

  finishSession(id) {
    return this.asr(async () => {
      const session = this.session(id)
      try {
        return await this.rpc(session.worker, 'finishSession', { id })
      } finally {
        await this.clearSession(id, session)
      }
    })
  }

  async clearSession(id, session) {
    this.sessions.delete(id)
    if (!session.worker.stopping && !session.worker.exited) {
      try {
        await this.rpc(session.worker, 'cancelSession', { id })
      } catch {}
    }
  }

  cancelSession(id) {
    if (!this.sessions.has(id)) return Promise.resolve({ ok: true })
    return this.asr(async () => {
      const session = this.sessions.get(id)
      if (session) await this.clearSession(id, session)
      return { ok: true }
    })
  }

  sweepExpiredSessions(now = Date.now()) {
    for (const [id, session] of this.sessions) {
      if (!session.expiring && now - session.lastActive > this.sessionTtlMs) {
        session.expiring = true
        void this.cancelSession(id).catch(() => {})
      }
    }
  }

  async synthesize({ text, voiceId = this.catalog.defaults?.voice, requestId } = {}) {
    validateSpeechText(text, this.maxTextLength)
    if (typeof requestId !== 'string' || !UUID.test(requestId)) throw speechEngineError('invalid')
    const model = this.model('tts')
    const maxCodePoints = model.config?.maxTextCodePoints ?? this.maxTextLength
    if (!Number.isInteger(maxCodePoints) || maxCodePoints < 1 || maxCodePoints > 400)
      throw speechEngineError('config')
    if (Array.from(text).length > maxCodePoints) throw speechEngineError('limit')
    const voice = model.voices?.find((item) => item.id === voiceId)
    if (model.engine !== 'vits' || model.voices?.length !== 1) throw speechEngineError('config')
    if (!voice || voice.sid !== 0 || (voice.sourceId !== undefined && voice.sourceId !== 0))
      throw speechEngineError('invalid')
    if (this.disposed) throw speechEngineError('disposed')
    if (this.speechTasks.has(requestId) || this.speechTasks.size >= 16)
      throw speechEngineError('busy')
    const controller = new AbortController()
    const task = { requestId, signal: controller.signal, controller, worker: null }
    this.speechTasks.set(requestId, task)
    this.controllers.add(controller)
    this.markActive()
    task.completion = this.enqueue(async () => {
      this.checkTask(task)
      this.activeSpeech = task
      const worker = await this.prepareWorker('tts', task)
      task.worker = worker
      this.checkTask(task)
      try {
        const result = await this.rpc(worker, 'synthesize', { text, sid: voice.sid })
        this.checkTask(task)
        validateAudio(result, this.maxOutputSeconds)
        return result
      } catch (error) {
        await this.stopWorker(worker, error, true)
        throw error
      }
    }, 'tts').finally(() => {
      if (this.activeSpeech === task) this.activeSpeech = null
      if (this.speechTasks.get(requestId) === task) this.speechTasks.delete(requestId)
      this.controllers.delete(controller)
      this.scheduleIdle()
    })
    return task.completion
  }

  async cancelSpeech(requestId) {
    if (typeof requestId !== 'string' || !UUID.test(requestId)) throw speechEngineError('invalid')
    const task = this.speechTasks.get(requestId) ?? this.activeSpeech
    if (!task || task.requestId !== requestId) return { cancelled: false }
    task.controller.abort(speechEngineError('cancelled'))
    // 排队项只作废自己的凭证，不能终止前一片段正在使用的 TTS worker。
    if (task === this.activeSpeech) {
      if (task.worker) await this.stopWorker(task.worker, speechEngineError('cancelled'), true)
      await task.completion.catch(() => {})
    }
    return { cancelled: true }
  }

  dispose() {
    if (this.disposePromise) return this.disposePromise
    this.disposed = true
    this.markActive()
    clearInterval(this.sessionSweepTimer)
    this.sessionSweepTimer = null
    for (const session of this.voiceSessions.values()) session.release()
    for (const controller of this.controllers) controller.abort(speechEngineError('disposed'))
    this.disposePromise = (async () => {
      await Promise.all(
        [...this.workers.values()].map((worker) =>
          this.stopWorker(worker, speechEngineError('disposed'), true),
        ),
      )
      await Promise.all(this.operations.values())
      this.sessions.clear()
      this.speechTasks.clear()
    })()
    return this.disposePromise
  }
}
