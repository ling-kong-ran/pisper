import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

const DEFAULT_ENDPOINT = 'https://models.dev/api.json'
const MAX_RESPONSE_BYTES = 1024 * 1024

const TEXT_IMAGE_INPUT = Object.freeze(['text', 'image'])

function metadata(id, contextWindow, maxTokens, input) {
  return Object.freeze({ id, contextWindow, ...(maxTokens ? { maxTokens } : {}), ...(input ? { input } : {}) })
}

export const BUNDLED_MODEL_METADATA = Object.freeze({
  'gpt-5.6-luna': metadata('gpt-5.6-luna', 272_000, null, TEXT_IMAGE_INPUT),
  'gpt-5.6-sol': metadata('gpt-5.6-sol', 272_000, null, TEXT_IMAGE_INPUT),
  'gpt-5.6-terra': metadata('gpt-5.6-terra', 272_000, null, TEXT_IMAGE_INPUT),

  'claude-3-5-sonnet-latest': metadata('claude-3-5-sonnet-latest', 200_000, 8_192),
  'claude-3-7-sonnet-latest': metadata('claude-3-7-sonnet-latest', 200_000, 64_000),
  'claude-haiku-4-5': metadata('claude-haiku-4-5', 200_000, 64_000),
  'claude-haiku-4-5-20251001': metadata('claude-haiku-4-5-20251001', 200_000, 64_000),
  'claude-opus-4-1': metadata('claude-opus-4-1', 200_000, 32_000),
  'claude-opus-4-1-20250805': metadata('claude-opus-4-1-20250805', 200_000, 32_000),
  'claude-opus-4-5': metadata('claude-opus-4-5', 200_000, 64_000),
  'claude-opus-4-5-20251101': metadata('claude-opus-4-5-20251101', 200_000, 64_000),
  'claude-opus-4-6': metadata('claude-opus-4-6', 1_000_000, 128_000),
  'claude-sonnet-4-5': metadata('claude-sonnet-4-5', 1_000_000, 64_000),
  'claude-sonnet-4-5-20250929': metadata('claude-sonnet-4-5-20250929', 1_000_000, 64_000),
  'claude-sonnet-4-6': metadata('claude-sonnet-4-6', 1_000_000, 128_000),

  'glm-4.5-air': metadata('glm-4.5-air', 131_072, 98_304),
  'glm-4.7': metadata('glm-4.7', 204_800, 131_072),
  'glm-4.7-flash': metadata('glm-4.7-flash', 204_800, 131_072),
  'glm-5-turbo': metadata('glm-5-turbo', 200_000, 131_072),
  'glm-5.1': metadata('glm-5.1', 200_000, 131_072),
  'glm-5.2': metadata('glm-5.2', 1_000_000, 131_072),
  'glm-5v-turbo': metadata('glm-5v-turbo', 200_000, 131_072),

  'deepseek-chat': metadata('deepseek-chat', 128_000),
  'deepseek-reasoner': metadata('deepseek-reasoner', 128_000),
  'deepseek-v4-flash': metadata('deepseek-v4-flash', 1_000_000, 384_000),
  'deepseek-v4-pro': metadata('deepseek-v4-pro', 1_000_000, 384_000),

  k2p7: metadata('k2p7', 262_144, 32_768),
  k3: metadata('k3', 1_048_576, 131_072),
  'kimi-for-coding': metadata('kimi-for-coding', 262_144, 32_768),
  'kimi-for-coding-highspeed': metadata('kimi-for-coding-highspeed', 262_144, 32_768),
  'kimi-k2-thinking': metadata('kimi-k2-thinking', 262_144, 32_768),

  'gemini-2.0-flash': metadata('gemini-2.0-flash', 1_048_576, 8_192),
  'gemini-2.0-flash-lite': metadata('gemini-2.0-flash-lite', 1_048_576, 8_192),
  'gemini-2.5-flash': metadata('gemini-2.5-flash', 1_048_576, 65_536),
  'gemini-2.5-flash-lite': metadata('gemini-2.5-flash-lite', 1_048_576, 65_536),
  'gemini-2.5-pro': metadata('gemini-2.5-pro', 1_048_576, 65_536),
  'gemini-3-flash-preview': metadata('gemini-3-flash-preview', 1_048_576, 65_536),
  'gemini-3-pro-preview': metadata('gemini-3-pro-preview', 1_048_576, 65_536),
  'gemini-3.1-flash-lite': metadata('gemini-3.1-flash-lite', 1_048_576, 65_536),
  'gemini-3.1-flash-lite-preview': metadata('gemini-3.1-flash-lite-preview', 1_048_576, 65_536),
  'gemini-3.1-pro-preview': metadata('gemini-3.1-pro-preview', 1_048_576, 65_536),
  'gemini-3.1-pro-preview-customtools': metadata('gemini-3.1-pro-preview-customtools', 1_048_576, 65_536),
  'gemini-3.5-flash': metadata('gemini-3.5-flash', 1_048_576, 65_536),
  'gemini-flash-latest': metadata('gemini-flash-latest', 1_048_576, 65_536),
  'gemini-flash-lite-latest': metadata('gemini-flash-lite-latest', 1_048_576, 65_536),
})

function positiveInteger(value) {
  const number = Number(value)
  return Number.isSafeInteger(number) && number > 0 ? number : null
}

function normalizedModels(payload) {
  const models = {}
  for (const provider of Object.values(payload || {})) {
    if (!provider?.models || typeof provider.models !== 'object') continue
    for (const [key, model] of Object.entries(provider.models)) {
      const id = String(model?.id || key || '').trim()
      const contextWindow = positiveInteger(model?.limit?.context ?? model?.contextWindow)
      if (!id || !contextWindow) continue
      const normalized = {
        id,
        contextWindow,
        maxTokens: positiveInteger(model?.limit?.output ?? model?.maxTokens),
      }
      const cacheKey = id.toLowerCase()
      if (!models[cacheKey] || contextWindow > models[cacheKey].contextWindow) models[cacheKey] = normalized
    }
  }
  return models
}

export class ModelMetadataService {
  constructor({ path, fetchImpl = globalThis.fetch, endpoint = DEFAULT_ENDPOINT } = {}) {
    this.path = path
    this.fetchImpl = fetchImpl
    this.endpoint = endpoint
    this.state = { version: 1, fetchedAt: 0, etag: '', models: {}, missing: [] }
    this.lookupPromise = null
  }

  async init() {
    const stored = await readJson(this.path, this.state)
    this.state = {
      version: 1,
      fetchedAt: positiveInteger(stored?.fetchedAt) || 0,
      etag: String(stored?.etag || ''),
      models: stored?.models && typeof stored.models === 'object' ? stored.models : {},
      missing: Array.isArray(stored?.missing) ? stored.missing.map((id) => String(id).toLowerCase()) : [],
    }
  }

  get(modelId) {
    const key = String(modelId || '').trim().toLowerCase()
    return this.state.models[key] || BUNDLED_MODEL_METADATA[key] || null
  }

  async ensure(modelId) {
    const key = String(modelId || '').trim().toLowerCase()
    if (!key || this.get(key)) return this.get(key)
    if (this.state.missing.includes(key)) return null
    if (this.lookupPromise) {
      await this.lookupPromise
      return this.get(key)
    }
    const pending = this.fetchDirectory(key).finally(() => {
      if (this.lookupPromise === pending) this.lookupPromise = null
    })
    this.lookupPromise = pending
    await pending
    return this.get(key)
  }

  async fetchDirectory(requestedModelId) {
    if (typeof this.fetchImpl !== 'function') return null
    try {
      const response = await this.fetchImpl(this.endpoint, {
        headers: { Accept: 'application/json' },
        signal: typeof AbortSignal?.timeout === 'function' ? AbortSignal.timeout(5_000) : undefined,
      })
      if (!response.ok) throw new Error(`HTTP ${response.status}`)
      const declaredSize = positiveInteger(response.headers?.get?.('content-length'))
      if (declaredSize && declaredSize > MAX_RESPONSE_BYTES) throw new Error('response too large')
      const text = await response.text()
      if (Buffer.byteLength(text) > MAX_RESPONSE_BYTES) throw new Error('response too large')
      const models = normalizedModels(JSON.parse(text))
      if (!Object.keys(models).length) throw new Error('empty model metadata')
      const missing = new Set(this.state.missing)
      if (!models[requestedModelId]) missing.add(requestedModelId)
      this.state = {
        version: 1,
        fetchedAt: Date.now(),
        etag: String(response.headers?.get?.('etag') || ''),
        models: { ...this.state.models, ...models },
        missing: [...missing],
      }
      await writeJsonAtomic(this.path, this.state)
      return this.get(requestedModelId)
    } catch {
      return null
    }
  }
}
