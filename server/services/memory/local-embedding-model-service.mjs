import { createHash } from 'node:crypto'
import { createRequire } from 'node:module'
import { mkdir, open, rename, rm, stat } from 'node:fs/promises'
import { basename, dirname, join } from 'node:path'
import { EmbeddingProvider } from './embedding-provider.mjs'
import { readJson, writeJsonAtomic } from '../../storage/json-file.mjs'

const require = createRequire(import.meta.url)
const HUGGING_FACE_HOST = 'https://huggingface.co'
const HUGGING_FACE_MIRROR = 'https://hf-mirror.com'

function file(path, size, sha256) {
  return { path, size, sha256 }
}

export const LOCAL_EMBEDDING_MODELS = [
  {
    id: 'multilingual-minilm-l12-v2-q8',
    name: 'Multilingual MiniLM L12 v2',
    description: 'Chinese and multilingual semantic retrieval',
    repository: 'Xenova/paraphrase-multilingual-MiniLM-L12-v2',
    version: '2c4055b12046f11709e9df2c122e59ffbdc2f900',
    dimensions: 384,
    languages: ['zh', 'en', 'multilingual'],
    files: [
      file('config.json', 673, '05b570bff786faa5c4604152aa16f19f77ed6dfc31e47dd0f3dd987078693ac7'),
      file('tokenizer.json', 17_082_913, 'b60b6b43406a48bf3638526314f3d232d97058bc93472ff2de930d43686fa441'),
      file('tokenizer_config.json', 496, '3f5961b9ac86288cccdb97f32fb848d6187c78e1603958c53f3ea1f296b7d8a2'),
      file('special_tokens_map.json', 280, '06e405a36dfe4b9604f484f6a1e619af1a7f7d09e34a8555eb0b77b66318067f'),
      file('onnx/model_quantized.onnx', 118_308_126, '66fc00f5f29afcaff34092e1bdd20008ca3918265a82fb9695a551e510cc4ebc'),
    ],
  },
  {
    id: 'all-minilm-l6-v2-q8',
    name: 'All MiniLM L6 v2',
    description: 'Compact English semantic retrieval',
    repository: 'Xenova/all-MiniLM-L6-v2',
    version: '751bff37182d3f1213fa05d7196b954e230abad9',
    dimensions: 384,
    languages: ['en'],
    files: [
      file('config.json', 650, '7135149f7cffa1a573466c6e4d8423ed73b62fd2332c575bf738a0d033f70df7'),
      file('tokenizer.json', 711_661, 'da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0'),
      file('tokenizer_config.json', 366, '9261e7d79b44c8195c1cada2b453e55b00aeb81e907a6664974b4d7776172ab3'),
      file('special_tokens_map.json', 125, 'b6d346be366a7d1d48332dbc9fdf3bf8960b5d879522b7799ddba59e76237ee3'),
      file('onnx/model_quantized.onnx', 22_972_370, 'afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1'),
    ],
  },
].map((model) => ({
  ...model,
  size: model.files.reduce((total, entry) => total + entry.size, 0),
}))

async function fileMatches(path, expected) {
  try {
    return (await stat(path)).size === expected.size
  } catch {
    return false
  }
}

async function modelInstalled(modelsDir, model) {
  for (const entry of model.files) {
    if (!await fileMatches(join(modelsDir, model.id, entry.path), entry)) return false
  }
  return true
}

class FastEmbedProvider extends EmbeddingProvider {
  constructor({ model, path }) {
    super({ id: 'local-fastembed', model: model.id, version: model.version, dimensions: model.dimensions })
    this.path = path
    this.extractorPromise = null
  }

  async extractor() {
    if (!this.extractorPromise) {
      this.extractorPromise = Promise.resolve().then(() => require('fastembed')).then(({ EmbeddingModel, FlagEmbedding }) => FlagEmbedding.init({
        model: EmbeddingModel.CUSTOM,
        modelAbsoluteDirPath: this.path,
        modelName: 'onnx/model_quantized.onnx',
        maxLength: 512,
        showDownloadProgress: false,
      })).catch((error) => {
        this.extractorPromise = null
        throw error
      })
    }
    return this.extractorPromise
  }

  async embed(texts) {
    const values = Array.isArray(texts) ? texts.map((text) => String(text || '')) : [String(texts || '')]
    if (!values.length) return []
    const extractor = await this.extractor()
    const vectors = []
    for await (const batch of extractor.embed(values, 16)) {
      for (const vector of batch) vectors.push(Float32Array.from(vector))
    }
    if (vectors.length !== values.length || vectors.some((vector) => vector.length !== this.dimensions)) {
      throw new Error('本地 embedding 模型返回了无效维度。')
    }
    return vectors
  }

  async dispose() {
    try {
      const extractor = await this.extractorPromise
      await extractor?.session?.release?.()
    } catch {}
    this.extractorPromise = null
  }
}

export class LocalEmbeddingModelService {
  constructor({ modelsDir, configPath, fetchImpl = globalThis.fetch, onChange, catalog = LOCAL_EMBEDDING_MODELS } = {}) {
    this.modelsDir = modelsDir
    this.configPath = configPath
    this.fetchImpl = fetchImpl
    this.onChange = onChange
    this.catalog = catalog
    this.downloads = new Map()
    this.provider = null
  }

  async init() {
    await mkdir(this.modelsDir, { recursive: true })
  }

  async config() {
    const appConfig = await readJson(this.configPath, {})
    return appConfig.memoryEmbedding || { enabled: false, modelId: '', source: 'huggingface' }
  }

  async saveConfig(memoryEmbedding) {
    const appConfig = await readJson(this.configPath, {})
    await writeJsonAtomic(this.configPath, { ...appConfig, memoryEmbedding })
  }

  async state(indexing = null) {
    const config = await this.config()
    const models = await Promise.all(this.catalog.map(async (model) => ({
      id: model.id,
      name: model.name,
      description: model.description,
      dimensions: model.dimensions,
      languages: model.languages,
      size: model.size,
      installed: await modelInstalled(this.modelsDir, model),
      downloading: this.downloads.get(model.id) || null,
    })))
    const selected = models.find((model) => model.id === config.modelId)
    return {
      enabled: config.enabled === true && Boolean(selected?.installed),
      selectedModelId: config.modelId || '',
      source: config.source === 'mirror' ? 'mirror' : 'huggingface',
      provider: 'local-fastembed',
      models,
      indexing,
    }
  }

  async getProvider() {
    const config = await this.config()
    if (config.enabled !== true) return null
    const model = this.catalog.find((item) => item.id === config.modelId) || null
    if (!model || !await modelInstalled(this.modelsDir, model)) return null
    if (this.provider?.model === model.id && this.provider?.version === model.version) return this.provider
    await this.provider?.dispose?.()
    this.provider = new FastEmbedProvider({ model, path: join(this.modelsDir, model.id) })
    return this.provider
  }

  async select(modelId, { enabled = true, source = 'huggingface' } = {}) {
    const model = this.catalog.find((item) => item.id === modelId) || null
    if (enabled && (!model || !await modelInstalled(this.modelsDir, model))) throw new Error('请先下载所选 embedding 模型。')
    await this.provider?.dispose?.()
    this.provider = null
    await this.saveConfig({ enabled: Boolean(enabled && model), modelId: model?.id || '', source: source === 'mirror' ? 'mirror' : 'huggingface' })
    await this.onChange?.()
    return this.state()
  }

  async download(modelId, { source = 'huggingface', activate = true } = {}) {
    const model = this.catalog.find((item) => item.id === modelId) || null
    if (!model) throw new Error('未知的本地 embedding 模型。')
    if (this.downloads.has(model.id)) throw new Error('该模型正在下载。')
    const baseUrl = source === 'mirror' ? HUGGING_FACE_MIRROR : HUGGING_FACE_HOST
    const totalBytes = model.size
    const progress = { status: 'downloading', receivedBytes: 0, totalBytes, percent: 0, file: '' }
    this.downloads.set(model.id, progress)
    const temporaryDir = join(this.modelsDir, `.${model.id}.${process.pid}.${Date.now()}.download`)
    try {
      await mkdir(temporaryDir, { recursive: true })
      for (const entry of model.files) {
        progress.file = basename(entry.path)
        const target = join(temporaryDir, entry.path)
        await mkdir(dirname(target), { recursive: true })
        const url = `${baseUrl}/${model.repository}/resolve/${model.version}/${entry.path}`
        const response = await this.fetchImpl(url, { redirect: 'follow', signal: AbortSignal.timeout(10 * 60_000) })
        if (!response.ok || !response.body?.getReader) throw new Error(`模型文件下载失败：HTTP ${response.status}`)
        const declared = Number(response.headers.get('content-length') || 0)
        if (declared && declared !== entry.size) throw new Error(`模型文件大小不匹配：${entry.path}`)
        const handle = await open(target, 'wx')
        const hash = createHash('sha256')
        let fileBytes = 0
        try {
          const reader = response.body.getReader()
          while (true) {
            const { done, value } = await reader.read()
            if (done) break
            const chunk = Buffer.from(value)
            fileBytes += chunk.length
            if (fileBytes > entry.size) throw new Error(`模型文件超过预期大小：${entry.path}`)
            hash.update(chunk)
            await handle.write(chunk)
            progress.receivedBytes += chunk.length
            progress.percent = Math.min(100, Math.round(progress.receivedBytes / totalBytes * 100))
          }
        } finally {
          await handle.close()
        }
        if (fileBytes !== entry.size || hash.digest('hex') !== entry.sha256) throw new Error(`模型文件 SHA-256 校验失败：${entry.path}`)
      }
      const destination = join(this.modelsDir, model.id)
      await rm(destination, { recursive: true, force: true })
      await rename(temporaryDir, destination)
      progress.status = 'ready'
      progress.percent = 100
      if (activate) await this.select(model.id, { enabled: true, source })
      return this.state()
    } catch (error) {
      progress.status = 'error'
      progress.error = error instanceof Error ? error.message : String(error)
      throw error
    } finally {
      await rm(temporaryDir, { recursive: true, force: true })
      setTimeout(() => this.downloads.delete(model.id), 2_000).unref?.()
    }
  }

  async remove(modelId) {
    const model = this.catalog.find((item) => item.id === modelId) || null
    if (!model) return false
    const config = await this.config()
    if (config.modelId === model.id) await this.select('', { enabled: false, source: config.source })
    await rm(join(this.modelsDir, model.id), { recursive: true, force: true })
    return true
  }

  async dispose() {
    await this.provider?.dispose?.()
    this.provider = null
  }
}
