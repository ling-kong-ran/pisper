// Provider 模型目录服务：把发现的模型目录（discovery 结果）同步进模型配置，
// 并维护能力元数据（上下文窗口/思考等级/输入类型）。
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

const DEFAULT_THINKING_LEVEL_MAP = Object.freeze({ xhigh: null, max: null })

function normalizedBaseUrl(value) {
  return String(value || '')
    .trim()
    .replace(/\/+$/, '')
    .toLowerCase()
}

function zeroCost() {
  return { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }
}

// 推断上下文窗口：有特殊已知值时返回精确值，否则回退到默认。
export function inferredContextWindow(modelId, fallback = 200_000) {
  if (/^gpt-5\.6(?:-|$)/i.test(String(modelId || ''))) return 272_000
  return Number(fallback) || 200_000
}

function normalizedInput(value) {
  if (!Array.isArray(value)) return null
  const input = [...new Set(value.filter((item) => ['text', 'image'].includes(item)))]
  return input.includes('text') ? input : null
}

function runtimeCapabilityMetadata(models) {
  const capabilities = new Map()
  for (const model of models || []) {
    const id = String(model?.id || '')
      .trim()
      .toLowerCase()
    if (!id || typeof model.reasoning !== 'boolean') continue
    const candidate = {
      reasoning: model.reasoning,
      thinkingLevelMap: { ...DEFAULT_THINKING_LEVEL_MAP, ...(model.thinkingLevelMap || {}) },
    }
    const score =
      (model.thinkingLevelMap ? 100 + Object.keys(model.thinkingLevelMap).length : 0) +
      (model.reasoning ? 1 : 10)
    if (!capabilities.has(id) || score > capabilities.get(id).score)
      capabilities.set(id, { metadata: candidate, score })
  }
  return capabilities
}

function mergedMetadata(primary, fallback) {
  if (!primary) return fallback || null
  if (!fallback) return primary
  return {
    ...fallback,
    ...primary,
    thinkingLevelMap: primary.thinkingLevelMap || fallback.thinkingLevelMap,
  }
}

function modelWithMetadata(
  model,
  metadata,
  explicitContextWindow,
  explicitInput,
  explicitReasoning,
) {
  const remoteMetadata = metadata?.get(model.id)
  const metadataThinkingLevelMap = remoteMetadata?.thinkingLevelMap
  const modelThinkingLevelMap = model.thinkingLevelMap
  return {
    ...model,
    input: normalizedInput(explicitInput) ||
      normalizedInput(remoteMetadata?.input) ||
      normalizedInput(model.input) || ['text'],
    reasoning:
      typeof explicitReasoning === 'boolean'
        ? explicitReasoning
        : explicitReasoning === null
          ? (remoteMetadata?.reasoning ?? model.reasoning ?? true)
          : typeof model.reasoning === 'boolean'
            ? model.reasoning
            : (remoteMetadata?.reasoning ?? true),
    contextWindow:
      Number(explicitContextWindow) ||
      Number(remoteMetadata?.contextWindow) ||
      inferredContextWindow(model.id, model.contextWindow),
    maxTokens: Number(remoteMetadata?.maxTokens) || model.maxTokens,
    ...(metadataThinkingLevelMap || modelThinkingLevelMap
      ? {
          thinkingLevelMap: {
            ...(metadataThinkingLevelMap || {}),
            ...(modelThinkingLevelMap || {}),
          },
        }
      : {}),
  }
}

function runtimeModel(
  providerId,
  entry,
  candidate,
  existing,
  template,
  metadata,
  explicitContextWindow,
  explicitInput,
  explicitReasoning,
) {
  const remoteMetadata = metadata?.get(candidate.id)
  if (existing) {
    return {
      ...modelWithMetadata(
        existing,
        metadata,
        explicitContextWindow,
        explicitInput,
        explicitReasoning,
      ),
      name: candidate.name || existing.name,
      pisperKind: candidate.kind || 'chat',
    }
  }
  return {
    id: candidate.id,
    name: candidate.name || candidate.id,
    api: entry.api || template?.api || 'openai-responses',
    provider: providerId,
    baseUrl: entry.baseUrl || template?.baseUrl || '',
    reasoning:
      typeof explicitReasoning === 'boolean'
        ? explicitReasoning
        : explicitReasoning === null
          ? (remoteMetadata?.reasoning ?? candidate.kind === 'chat')
          : candidate.kind === 'chat' && (remoteMetadata?.reasoning ?? true),
    input: ['text', 'image'],
    cost: template?.cost || zeroCost(),
    contextWindow: Number(remoteMetadata?.contextWindow) || inferredContextWindow(candidate.id),
    maxTokens: Number(remoteMetadata?.maxTokens) || template?.maxTokens || 128_000,
    headers: template?.headers ? { ...template.headers } : undefined,
    thinkingLevelMap:
      candidate.kind === 'chat'
        ? { ...(remoteMetadata?.thinkingLevelMap || DEFAULT_THINKING_LEVEL_MAP) }
        : undefined,
    pisperKind: candidate.kind || 'chat',
  }
}

export class ProviderModelCatalogService {
  constructor({ path, metadata = null }) {
    this.path = path
    this.metadata = metadata
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
    const cleanModels = [
      ...new Map(
        (models || [])
          .filter((model) => model?.id)
          .map((model) => [
            String(model.id),
            {
              id: String(model.id),
              name: String(model.name || model.id),
              kind: ['chat', 'image', 'video'].includes(model.kind) ? model.kind : 'chat',
            },
          ]),
      ).values(),
    ]
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
    this.writeQueue = this.writeQueue
      .catch(() => {})
      .then(async () => {
        this.state = {
          ...this.state,
          providers: { ...(this.state.providers || {}), [providerId]: entry },
        }
        await writeJsonAtomic(this.path, this.state)
      })
    await this.writeQueue
    return { entry, addedModelIds, removedModelIds }
  }

  async remove(providerId) {
    if (!this.state.providers?.[providerId]) return
    this.writeQueue = this.writeQueue
      .catch(() => {})
      .then(async () => {
        const providers = { ...(this.state.providers || {}) }
        delete providers[providerId]
        this.state = { ...this.state, providers }
        await writeJsonAtomic(this.path, this.state)
      })
    await this.writeQueue
  }

  decorateRuntime(
    runtime,
    configuredBaseUrls,
    configuredHeaders = {},
    configuredContextWindows = {},
    configuredInputs = {},
    configuredReasoning = {},
  ) {
    this.configuredBaseUrls = new Map(
      Object.entries(configuredBaseUrls || {}).map(([id, url]) => [id, normalizedBaseUrl(url)]),
    )
    const explicitContextWindows = new Map(Object.entries(configuredContextWindows || {}))
    const explicitInputs = new Map(Object.entries(configuredInputs || {}))
    const explicitReasoning = new Map(Object.entries(configuredReasoning || {}))
    this.configuredHeaders = new Map(Object.entries(configuredHeaders || {}))
    const rawGetModels = runtime.getModels.bind(runtime)
    const rawGetModel = runtime.getModel.bind(runtime)
    const rawGetAvailable = runtime.getAvailable.bind(runtime)
    const rawGetAvailableSnapshot = runtime.getAvailableSnapshot.bind(runtime)
    const runtimeCapabilities = runtimeCapabilityMetadata(rawGetModels())
    const effectiveMetadata = {
      get: (modelId) =>
        mergedMetadata(
          this.metadata?.get(modelId),
          runtimeCapabilities.get(
            String(modelId || '')
              .trim()
              .toLowerCase(),
          )?.metadata,
        ),
    }

    const catalogEntry = (providerId) => {
      const entry = this.state.providers?.[providerId]
      if (!entry) return null
      const configured = this.configuredBaseUrls.get(providerId)
      return configured && configured === normalizedBaseUrl(entry.baseUrl) ? entry : null
    }
    const modelsForProvider = (providerId) => {
      const raw = [...rawGetModels(providerId)].map((model) =>
        modelWithMetadata(
          model,
          effectiveMetadata,
          explicitContextWindows.get(`${providerId}:${model.id}`),
          explicitInputs.get(`${providerId}:${model.id}`),
          explicitReasoning.get(`${providerId}:${model.id}`),
        ),
      )
      const entry = catalogEntry(providerId)
      const existing = new Map(raw.map((model) => [model.id, model]))
      const models = entry
        ? entry.models.map((candidate) =>
            runtimeModel(
              providerId,
              entry,
              candidate,
              existing.get(candidate.id),
              raw[0],
              effectiveMetadata,
              explicitContextWindows.get(`${providerId}:${candidate.id}`),
              explicitInputs.get(`${providerId}:${candidate.id}`),
              explicitReasoning.get(`${providerId}:${candidate.id}`),
            ),
          )
        : raw
      const providerHeaders = this.configuredHeaders.get(providerId)
      if (!providerHeaders || Object.keys(providerHeaders).length === 0) return models
      return models.map((model) => ({
        ...model,
        headers: { ...providerHeaders, ...(model.headers || {}) },
      }))
    }

    runtime.getModels = (providerId) => {
      if (providerId) return modelsForProvider(providerId)
      const raw = [...rawGetModels()]
      const providerIds = new Set([
        ...raw.map((model) => model.provider),
        ...Object.keys(this.state.providers || {}),
      ])
      return [...providerIds].flatMap((id) => modelsForProvider(id))
    }
    runtime.getModel = (providerId, modelId) => {
      const model = modelsForProvider(providerId).find((item) => item.id === modelId)
      if (model || catalogEntry(providerId)) return model
      return rawGetModel(providerId, modelId)
    }
    runtime.getAvailable = async (providerId) => {
      const raw = [...(await rawGetAvailable(providerId))]
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
