import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

function normalizedBaseUrl(value) {
  return String(value || '').trim().replace(/\/+$/, '').toLowerCase()
}

function zeroCost() {
  return { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }
}

function runtimeModel(providerId, entry, candidate, existing, template) {
  if (existing) return { ...existing, name: candidate.name || existing.name, pisperKind: candidate.kind || 'chat' }
  return {
    id: candidate.id,
    name: candidate.name || candidate.id,
    api: entry.api || template?.api || 'openai-responses',
    provider: providerId,
    baseUrl: entry.baseUrl || template?.baseUrl || '',
    reasoning: candidate.kind === 'chat',
    input: ['text', 'image'],
    cost: template?.cost || zeroCost(),
    contextWindow: template?.contextWindow || 200_000,
    maxTokens: template?.maxTokens || 128_000,
    headers: template?.headers ? { ...template.headers } : undefined,
    pisperKind: candidate.kind || 'chat',
  }
}

export class ProviderModelCatalogService {
  constructor({ path }) {
    this.path = path
    this.state = { providers: {} }
    this.configuredBaseUrls = new Map()
    this.configuredHeaders = new Map()
    this.writeQueue = Promise.resolve()
  }

  async init() {
    this.state = await readJson(this.path, { providers: {} })
    this.state.providers ||= {}
  }

  isCurrent(providerId, baseUrl) {
    const entry = this.state.providers?.[providerId]
    return Boolean(entry && normalizedBaseUrl(entry.baseUrl) === normalizedBaseUrl(baseUrl))
  }

  get(providerId) {
    return this.state.providers?.[providerId] || null
  }

  async sync(providerId, { baseUrl, api, models }) {
    const cleanModels = [...new Map((models || []).filter((model) => model?.id).map((model) => [String(model.id), {
      id: String(model.id),
      name: String(model.name || model.id),
      kind: ['chat', 'image', 'video'].includes(model.kind) ? model.kind : 'chat',
    }])).values()]
    if (!cleanModels.length) throw new Error('Provider 没有返回可同步的模型。')
    const previous = this.state.providers?.[providerId]
    const previousIds = new Set(previous?.models?.map((model) => model.id) || [])
    const nextIds = new Set(cleanModels.map((model) => model.id))
    const removedModelIds = [...previousIds].filter((id) => !nextIds.has(id))
    const addedModelIds = [...nextIds].filter((id) => !previousIds.has(id))
    const entry = {
      baseUrl: String(baseUrl || '').trim(),
      api: String(api || 'openai-responses').trim(),
      models: cleanModels,
      updatedAt: new Date().toISOString(),
    }
    this.writeQueue = this.writeQueue.catch(() => {}).then(async () => {
      this.state = { ...this.state, providers: { ...(this.state.providers || {}), [providerId]: entry } }
      await writeJsonAtomic(this.path, this.state)
    })
    await this.writeQueue
    return { entry, addedModelIds, removedModelIds }
  }

  async remove(providerId) {
    if (!this.state.providers?.[providerId]) return
    this.writeQueue = this.writeQueue.catch(() => {}).then(async () => {
      const providers = { ...(this.state.providers || {}) }
      delete providers[providerId]
      this.state = { ...this.state, providers }
      await writeJsonAtomic(this.path, this.state)
    })
    await this.writeQueue
  }

  decorateRuntime(runtime, configuredBaseUrls, configuredHeaders = {}) {
    this.configuredBaseUrls = new Map(Object.entries(configuredBaseUrls || {}).map(([id, url]) => [id, normalizedBaseUrl(url)]))
    this.configuredHeaders = new Map(Object.entries(configuredHeaders || {}))
    const rawGetModels = runtime.getModels.bind(runtime)
    const rawGetModel = runtime.getModel.bind(runtime)
    const rawGetAvailable = runtime.getAvailable.bind(runtime)
    const rawGetAvailableSnapshot = runtime.getAvailableSnapshot.bind(runtime)

    const catalogEntry = (providerId) => {
      const entry = this.state.providers?.[providerId]
      if (!entry) return null
      const configured = this.configuredBaseUrls.get(providerId)
      return configured && configured === normalizedBaseUrl(entry.baseUrl) ? entry : null
    }
    const modelsForProvider = (providerId) => {
      const raw = [...rawGetModels(providerId)]
      const entry = catalogEntry(providerId)
      const existing = new Map(raw.map((model) => [model.id, model]))
      const models = entry
        ? entry.models.map((candidate) => runtimeModel(providerId, entry, candidate, existing.get(candidate.id), raw[0]))
        : raw
      const providerHeaders = this.configuredHeaders.get(providerId)
      if (!providerHeaders || Object.keys(providerHeaders).length === 0) return models
      return models.map((model) => ({ ...model, headers: { ...providerHeaders, ...(model.headers || {}) } }))
    }

    runtime.getModels = (providerId) => {
      if (providerId) return modelsForProvider(providerId)
      const raw = [...rawGetModels()]
      const providerIds = new Set([...raw.map((model) => model.provider), ...Object.keys(this.state.providers || {})])
      return [...providerIds].flatMap((id) => modelsForProvider(id))
    }
    runtime.getModel = (providerId, modelId) => catalogEntry(providerId)
      ? modelsForProvider(providerId).find((model) => model.id === modelId)
      : rawGetModel(providerId, modelId)
    runtime.getAvailable = async (providerId) => {
      const raw = [...await rawGetAvailable(providerId)]
      const availableProviders = new Set(raw.map((model) => model.provider))
      if (providerId) return availableProviders.has(providerId) ? modelsForProvider(providerId) : []
      return runtime.getModels().filter((model) => availableProviders.has(model.provider))
    }
    runtime.getAvailableSnapshot = () => {
      const raw = [...rawGetAvailableSnapshot()]
      const availableProviders = new Set(raw.map((model) => model.provider))
      return runtime.getModels().filter((model) => availableProviders.has(model.provider))
    }
    return runtime
  }
}
