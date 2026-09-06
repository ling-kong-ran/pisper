import { lstat, readdir } from 'node:fs/promises'
import { isAbsolute, join, parse, relative, resolve, sep } from 'node:path'
import { SpeechRecognitionService } from '../services/speech-recognition-service.mjs'
import { speechEngineError, validateSpeechText } from '../services/speech-engine-service.mjs'

function resourcePath(root, value) {
  if (typeof value !== 'string' || !value || isAbsolute(value) || /[,\0]/u.test(value)) {
    throw speechEngineError('config')
  }
  const target = resolve(root, value)
  const child = relative(resolve(root), target)
  if (!child || child === '..' || child.startsWith(`..${sep}`) || isAbsolute(child)) {
    throw speechEngineError('config')
  }
  return target
}

function modelResourcePath(root, value) {
  if (
    typeof value !== 'string' ||
    !value.isWellFormed() ||
    value.length > 512 ||
    !value
      .split('/')
      .every(
        (part) =>
          part &&
          part.length <= 255 &&
          part !== '.' &&
          part !== '..' &&
          !/[\\<>:"|?*,\p{Cc}\p{Cf}]/u.test(part) &&
          !/[. ]$/.test(part) &&
          !/^(con|prn|aux|nul|conin\$|conout\$|clock\$|com[0-9\u00b9\u00b2\u00b3]|lpt[0-9\u00b9\u00b2\u00b3])(?:\.|$)/i.test(
            part,
          ),
      )
  )
    throw speechEngineError('config')
  return resourcePath(root, value)
}

async function vitsResources(modelDir, model) {
  const config = model.config
  if (!config || !Array.isArray(model.files) || !model.files.length || model.files.length > 1024)
    throw speechEngineError('config')
  const allowedConfig = new Set([
    'model',
    'tokens',
    'lexicon',
    'dictDir',
    'numThreads',
    'maxTextCodePoints',
    'noiseScale',
    'noiseScaleW',
    'lengthScale',
    'ruleFsts',
  ])
  if (Object.keys(config).some((key) => !allowedConfig.has(key))) throw speechEngineError('config')
  const files = new Map()
  for (const file of model.files) {
    modelResourcePath(modelDir, file.path)
    if (files.has(file.path) || !Number.isSafeInteger(file.bytes) || file.bytes < 0)
      throw speechEngineError('config')
    files.set(file.path, file)
  }
  const resources = {
    noiseScale: config.noiseScale ?? 0.667,
    noiseScaleW: config.noiseScaleW ?? 0.8,
    lengthScale: config.lengthScale ?? 1,
  }
  for (const key of ['noiseScale', 'noiseScaleW', 'lengthScale']) {
    if (!Number.isFinite(resources[key]) || resources[key] <= 0) throw speechEngineError('config')
  }
  for (const key of ['model', 'tokens', 'lexicon', 'dictDir']) {
    resources[key] = modelResourcePath(modelDir, config[key])
    if (
      key === 'dictDir'
        ? files.has(config[key]) ||
          ![...files.keys()].some((path) => path.startsWith(`${config[key]}/`))
        : !files.has(config[key])
    )
      throw speechEngineError('config')
  }
  let ruleFsts
  if (config.ruleFsts !== undefined) {
    if (!Array.isArray(config.ruleFsts) || !config.ruleFsts.length || config.ruleFsts.length > 16)
      throw speechEngineError('config')
    ruleFsts = config.ruleFsts
      .map((path) => {
        const absolute = modelResourcePath(modelDir, path)
        if (!files.has(path)) throw speechEngineError('config')
        return absolute
      })
      .join(',')
  }
  // 父服务已验证整树摘要；构造原生对象前再检查祖先与整树，拒绝验证后替换的链接和额外文件。
  try {
    const root = resolve(modelDir)
    let current = parse(root).root
    for (const part of root.slice(current.length).split(sep).filter(Boolean)) {
      current = join(current, part)
      const stat = await lstat(current)
      if (stat.isSymbolicLink() || !stat.isDirectory()) throw speechEngineError('config')
    }
    const found = new Set()
    const inspect = async (directory, prefix = '') => {
      for (const entry of await readdir(directory)) {
        const path = prefix ? `${prefix}/${entry}` : entry
        const target = join(directory, entry)
        const stat = await lstat(target)
        if (stat.isSymbolicLink()) throw speechEngineError('config')
        if (stat.isDirectory() && [...files.keys()].some((file) => file.startsWith(`${path}/`))) {
          await inspect(target, path)
        } else if (
          !stat.isFile() ||
          stat.nlink !== 1 ||
          !(files.has(path) || path === '.installation.json') ||
          (files.has(path) && stat.size !== files.get(path).bytes)
        ) {
          throw speechEngineError('config')
        } else if (files.has(path)) found.add(path)
      }
    }
    await inspect(root)
    if (found.size !== files.size) throw speechEngineError('config')
  } catch {
    throw speechEngineError('config')
  }
  return { vits: resources, ...(ruleFsts ? { ruleFsts } : {}) }
}

export function encodeWav(samples, sampleRate, maxOutputSeconds = 45) {
  if (
    !(samples instanceof Float32Array) ||
    !samples.length ||
    !Number.isInteger(sampleRate) ||
    sampleRate < 8000 ||
    sampleRate > 48000 ||
    !Number.isFinite(maxOutputSeconds) ||
    maxOutputSeconds <= 0 ||
    maxOutputSeconds > 45 ||
    samples.length > sampleRate * maxOutputSeconds
  )
    throw speechEngineError('limit')
  const wav = Buffer.alloc(44 + samples.length * 2)
  wav.write('RIFF', 0)
  wav.writeUInt32LE(wav.length - 8, 4)
  wav.write('WAVEfmt ', 8)
  wav.writeUInt32LE(16, 16)
  wav.writeUInt16LE(1, 20)
  wav.writeUInt16LE(1, 22)
  wav.writeUInt32LE(sampleRate, 24)
  wav.writeUInt32LE(sampleRate * 2, 28)
  wav.writeUInt16LE(2, 32)
  wav.writeUInt16LE(16, 34)
  wav.write('data', 36)
  wav.writeUInt32LE(samples.length * 2, 40)
  for (let index = 0; index < samples.length; index += 1) {
    if (!Number.isFinite(samples[index])) throw speechEngineError('inference')
    const sample = Math.max(-1, Math.min(1, samples[index]))
    wav.writeInt16LE(Math.round(sample * (sample < 0 ? 32768 : 32767)), 44 + index * 2)
  }
  return { wav, sampleRate, durationMs: (samples.length / sampleRate) * 1000 }
}

// 工厂仅供独立 worker 和无大模型测试使用，父服务不加载原生推理库。
export function createSpeechInferenceHandler({ nativeModule = null } = {}) {
  let recognition = null
  let tts = null
  let settings = null
  return async function handle(method, params = {}) {
    if (method === 'init') {
      if (settings) throw speechEngineError('config')
      const {
        kind,
        model,
        modelDir,
        resourceDir,
        hotwordsDir,
        numThreads = model?.config?.numThreads ?? 2,
        maxTextLength = 400,
        maxOutputSeconds = 45,
      } = params
      if (
        !Number.isInteger(maxTextLength) ||
        maxTextLength < 1 ||
        maxTextLength > 400 ||
        !Number.isFinite(maxOutputSeconds) ||
        maxOutputSeconds <= 0 ||
        maxOutputSeconds > 45 ||
        !Number.isInteger(numThreads) ||
        numThreads < 1 ||
        numThreads > 16
      )
        throw speechEngineError('config')
      const config = model?.config || {}
      const maxCodePoints = config.maxTextCodePoints ?? maxTextLength
      if (!Number.isInteger(maxCodePoints) || maxCodePoints < 1 || maxCodePoints > 400)
        throw speechEngineError('config')
      if (kind === 'asr' && model.engine === 'online-transducer') {
        recognition = new SpeechRecognitionService({
          modelDir,
          hotwordsDir,
          bpeVocabPath: config.bpeVocabResource
            ? resourcePath(resourceDir, config.bpeVocabResource)
            : '',
          idleUnloadMs: 0,
          nativeModule,
        })
      } else if (kind === 'tts' && model.engine === 'vits') {
        const resources = await vitsResources(modelDir, model)
        const loaded = nativeModule || (await import('sherpa-onnx-node'))
        const native = loaded.default || loaded
        const ttsConfig = {
          model: {
            // 1.13.7 由 Melo 元数据启用内置 Jieba；dictDir 保留共享资源合同，但 addon 不读取该字段。
            vits: resources.vits,
            numThreads,
            provider: 'cpu',
          },
          ...(resources.ruleFsts ? { ruleFsts: resources.ruleFsts } : {}),
          maxNumSentences: 1,
        }
        tts =
          typeof native.OfflineTts?.createAsync === 'function'
            ? await native.OfflineTts.createAsync(ttsConfig)
            : new native.OfflineTts(ttsConfig)
      } else throw speechEngineError('config')
      settings = { kind, maxTextLength, maxOutputSeconds, maxCodePoints }
      return { ready: true }
    }
    if (!settings) throw speechEngineError('config')
    if (method === 'synthesize' && tts) {
      validateSpeechText(params.text, settings.maxTextLength)
      if (Array.from(params.text).length > settings.maxCodePoints) throw speechEngineError('limit')
      const sid = params.sid
      // VITS 单说话人模型可能报告 0 个可选说话人，仍合法使用 sid 0。
      if (sid !== 0 || !Number.isInteger(tts.numSpeakers) || tts.numSpeakers < 0) {
        throw speechEngineError('invalid')
      }
      if (!Number.isInteger(tts.sampleRate) || tts.sampleRate < 8000 || tts.sampleRate > 48000) {
        throw speechEngineError('inference')
      }
      let count = 0
      let exceeded = false
      const audio = await tts.generateAsync({
        text: params.text,
        sid,
        speed: 1,
        // 尽早停止后续句子生成；原生返回值仍须再次检查，不能信任回调已执行。
        onProgress: ({ samples }) => {
          count += samples?.length || 0
          if (count > tts.sampleRate * settings.maxOutputSeconds) exceeded = true
          return !exceeded
        },
      })
      if (exceeded) throw speechEngineError('limit')
      return encodeWav(audio.samples, audio.sampleRate, settings.maxOutputSeconds)
    }
    if (!recognition) throw speechEngineError('config')
    switch (method) {
      case 'transcribe':
        return recognition.transcribe(params.samples, { terms: params.terms })
      case 'startSession':
        return recognition.startSession({ terms: params.terms })
      case 'acceptChunk':
        return recognition.acceptChunk(params.id, params.samples)
      case 'finishSession':
        return recognition.finishSession(params.id)
      case 'cancelSession':
        return recognition.cancelSession(params.id)
      default:
        throw speechEngineError('invalid')
    }
  }
}

export function runSpeechInferenceWorker() {
  if (typeof process.send !== 'function' || !process.connected) {
    throw speechEngineError('worker')
  }
  const handle = createSpeechInferenceHandler()
  let operation = Promise.resolve()
  // 父进程断开时直接退出，即使原生异步任务仍占用线程池也不保留模型。
  process.once('disconnect', () => process.exit(0))
  process.on('message', (message) => {
    if (message?.method === 'shutdown') process.exit(0)
    if (typeof message?.id !== 'string') return
    operation = operation
      .then(async () => {
        let reply
        try {
          reply = { id: message.id, ok: true, result: await handle(message.method, message.params) }
        } catch (error) {
          reply = { id: message.id, ok: false, error: { code: error?.code || 'inference' } }
        }
        if (process.connected)
          process.send(reply, (error) => {
            if (error) process.exit(1)
          })
      })
      .catch(() => process.exit(1))
  })
}

if (process.argv.includes('--pisper-speech-worker')) runSpeechInferenceWorker()
