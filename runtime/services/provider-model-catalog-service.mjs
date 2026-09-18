// Provider 模型目录服务：把发现的模型目录（discovery 结果）同步进模型配置，
// 并维护能力元数据（上下文窗口/思考等级/输入类型）。
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { PiDevModelMetadataService } from './pi-dev-model-metadata.mjs'

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

// 推断上下文窗口：无任何元数据时的默认回退。
// 注意：此函数是优先级链的最后一级（用户配置 > pi.dev 落盘数据 > 内置元数据 > 此函数），
// 不再保留任何按型号硬编码的特例（曾有 gpt-5.6=272k，已过时且错误）；
// 精确窗口一律来自 pi.dev 落盘文件与内置元数据表，避免硬编码随模型迭代腐烂。
export function inferredContextWindow(modelId, fallback = 200_000) {
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
      explicitContextWindow !== undefined && Number(explicitContextWindow)
        ? Number(explicitContextWindow)
        : Number(remoteMetadata?.contextWindow) ||
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
  configuredApi,
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
      // 同 URL 多 Key 合并时，模型保留发现它的连接归属，运行时据此选择正确凭据。
      pisperAuthProvider: candidate.authProvider || providerId,
      pisperAuthKeyId: candidate.authKeyId || '',
    }
  }
  return {
    id: candidate.id,
    name: candidate.name || candidate.id,
    api: configuredApi || entry.api || template?.api || 'openai-responses',
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
    contextWindow:
      explicitContextWindow !== undefined && Number(explicitContextWindow)
        ? Number(explicitContextWindow)
        : Number(remoteMetadata?.contextWindow) || inferredContextWindow(candidate.id),
    maxTokens: Number(remoteMetadata?.maxTokens) || template?.maxTokens || 128_000,
    headers: template?.headers ? { ...template.headers } : undefined,
    thinkingLevelMap:
      candidate.kind === 'chat'
        ? { ...(remoteMetadata?.thinkingLevelMap || DEFAULT_THINKING_LEVEL_MAP) }
        : undefined,
    pisperKind: candidate.kind || 'chat',
    // 新模型没有当前连接的原生定义时，使用模型目录来源对应的 Key。
    pisperAuthProvider: candidate.authProvider || providerId,
    pisperAuthKeyId: candidate.authKeyId || '',
  }
}

export class ProviderModelCatalogService {
  constructor({ path, metadata = null, piDevCachePath = null }) {
    this.path = path
    this.metadata = metadata
    this.piDevMetadata = piDevCachePath
      ? new PiDevModelMetadataService({ cachePath: piDevCachePath })
      : null
    this.state = { providers: {} }
    this.configuredBaseUrls = new Map()
    this.configuredApis = new Map()
    this.configuredHeaders = new Map()
    this.configuredProviderTypes = new Map()
    this.writeQueue = Promise.resolve()
  }

  async init() {
    this.state = await readJson(this.path, { providers: {} })
    this.state.providers ||= {}
    // 初始化 pi.dev 元数据服务
    if (this.piDevMetadata) {
      await this.piDevMetadata.init()
    }
  }

  async dispose() {
    await this.piDevMetadata?.dispose()
    await this.writeQueue
  }

  isCurrent(providerId, baseUrl) {
    const entry = this.state.providers?.[providerId]
    return Boolean(entry && normalizedBaseUrl(entry.baseUrl) === normalizedBaseUrl(baseUrl))
  }

  get(providerId) {
    return this.state.providers?.[providerId] || null
  }

  async sync(providerId, { baseUrl, api, models, modelKeyIds = {} }) {
    const cleanModels = [
      ...new Map(
        (models || [])
          .filter((model) => model?.id)
          .map((model) => {
            const id = String(model.id)
            const keyIds = [...new Set(modelKeyIds[id] || [])].filter(Boolean)
            return [
              id,
              {
                id,
                name: String(model.name || model.id),
                kind: ['chat', 'image', 'video'].includes(model.kind) ? model.kind : 'chat',
                ...(keyIds.length ? { keyIds } : {}),
              },
            ]
          }),
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
    configuredApis = {},
    configuredApiKeys = {},
    configuredProviderTypes = {},
  ) {
    this.configuredBaseUrls = new Map(
      Object.entries(configuredBaseUrls || {}).map(([id, url]) => [id, normalizedBaseUrl(url)]),
    )
    this.configuredApis = new Map(Object.entries(configuredApis || {}))
    const explicitContextWindows = new Map(Object.entries(configuredContextWindows || {}))
    const explicitInputs = new Map(Object.entries(configuredInputs || {}))
    const explicitReasoning = new Map(Object.entries(configuredReasoning || {}))
    this.configuredHeaders = new Map(Object.entries(configuredHeaders || {}))
    this.configuredProviderTypes = new Map(Object.entries(configuredProviderTypes || {}))
    const apiKeys = new Map(Object.entries(configuredApiKeys || {}))
    const rawGetModels = runtime.getModels.bind(runtime)
    const rawGetModel = runtime.getModel.bind(runtime)
    const rawGetAvailable = runtime.getAvailable.bind(runtime)
    const rawGetAvailableSnapshot = runtime.getAvailableSnapshot.bind(runtime)
    const runtimeCapabilities = runtimeCapabilityMetadata(rawGetModels())
    const effectiveMetadata = {
      get: (modelId) => {
        // 优先级：pi.dev 落盘数据 → 内置元数据 → runtime 能力。
        // pi.dev 数据是恒定参考数据：未命中不触发网络抓取，由后两级兜底。
        let piDevMeta = null
        if (this.piDevMetadata) {
          const contextWindow = this.piDevMetadata.getContextWindowSync(modelId)
          if (contextWindow) {
            piDevMeta = { contextWindow }
          }
        }
        return mergedMetadata(
          piDevMeta,
          mergedMetadata(
            this.metadata?.get(modelId),
            runtimeCapabilities.get(
              String(modelId || '')
                .trim()
                .toLowerCase(),
            )?.metadata,
          ),
        )
      },
    }

    const catalogEntries = (providerId) => {
      const configuredBaseUrl = this.configuredBaseUrls.get(providerId)
      if (!configuredBaseUrl) return []
      return (
        Object.entries(this.state.providers || {})
          // 目录共享严格按端点 URL：同一网关即使两个连接协议配置不同，也应复用已发现的模型 ID。
          .filter(([, entry]) => normalizedBaseUrl(entry?.baseUrl) === configuredBaseUrl)
          // 当前连接优先，其他同端点连接按 ID 稳定排序，模型 ID 冲突时可预测地选 Key。
          .sort(([left], [right]) => {
            if (left === providerId) return -1
            if (right === providerId) return 1
            return left.localeCompare(right)
          })
          .map(([id, entry]) => ({ id, entry }))
      )
    }
    const modelsForProvider = (providerId) => {
      const configuredApi = this.configuredApis.get(providerId)
      const raw = [...rawGetModels(providerId)].map((model) =>
        modelWithMetadata(
          configuredApi ? { ...model, api: configuredApi } : model,
          effectiveMetadata,
          explicitContextWindows.get(`${providerId}:${model.id}`),
          explicitInputs.get(`${providerId}:${model.id}`),
          explicitReasoning.get(`${providerId}:${model.id}`),
        ),
      )
      const entries = catalogEntries(providerId)
      const own = entries.find((item) => item.id === providerId)
      const existing = new Map(raw.map((model) => [model.id, model]))
      const models = []
      const seen = new Set()
      const append = (sourceProvider, entry, candidate) => {
        if (seen.has(candidate.id)) return
        seen.add(candidate.id)
        models.push(
          runtimeModel(
            providerId,
            entry,
            {
              ...candidate,
              authProvider: sourceProvider,
              authKeyId: Array.isArray(candidate.keyIds) ? candidate.keyIds[0] || '' : '',
            },
            existing.get(candidate.id),
            raw[0],
            effectiveMetadata,
            explicitContextWindows.get(`${providerId}:${candidate.id}`),
            explicitInputs.get(`${providerId}:${candidate.id}`),
            explicitReasoning.get(`${providerId}:${candidate.id}`),
            configuredApi,
          ),
        )
      }
      // 自身目录存在时仍保持“本 Key 已下线模型被移除”的原有同步语义；
      // 新连接无自身目录时先保留本地初始模型，再补同 URL 的已发现模型。
      if (own) {
        for (const candidate of own.entry.models || []) append(own.id, own.entry, candidate)
      } else {
        for (const model of raw) {
          seen.add(model.id)
          models.push({ ...model, pisperAuthProvider: providerId })
        }
      }
      const targetType = this.configuredProviderTypes.get(providerId) || 'chat'
      for (const source of entries) {
        if (source.id === providerId) continue
        const sourceType = this.configuredProviderTypes.get(source.id) || 'chat'
        for (const candidate of source.entry.models || []) {
          // 专用视觉连接占用图像/视频模型；同 URL 对话连接不能把它们重新列回聊天模型。
          if (targetType === 'chat' && sourceType === 'visual' && candidate.kind !== 'chat')
            continue
          append(source.id, source.entry, candidate)
        }
      }
      const providerHeaders = this.configuredHeaders.get(providerId)
      if (!providerHeaders || Object.keys(providerHeaders).length === 0) return models
      return models.map((model) => ({
        ...model,
        headers: { ...providerHeaders, ...(model.headers || {}) },
      }))
    }

    // Pi 会话经 getAuth(model) 取凭据；同端点合并进来的模型必须使用发现它的连接 Key。
    // 纯目录测试会传入最小 runtime，缺少 getAuth 时保留其原有契约。
    if (typeof runtime.getAuth === 'function') {
      const rawGetAuth = runtime.getAuth.bind(runtime)
      runtime.getAuth = async (providerOrModel, overrides = {}) => {
        if (typeof providerOrModel === 'string') return rawGetAuth(providerOrModel, overrides)
        const source = providerOrModel?.pisperAuthProvider || providerOrModel?.provider
        const key = providerOrModel?.pisperAuthKeyId
          ? apiKeys.get(source)?.[providerOrModel.pisperAuthKeyId]
          : ''
        if (
          source &&
          (source !== providerOrModel.provider || key) &&
          overrides.apiKey === undefined
        ) {
          const resolved = await rawGetAuth(source, key ? { ...overrides, apiKey: key } : overrides)
          if (resolved) return resolved
        }
        return rawGetAuth(providerOrModel, overrides)
      }
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
      if (model || catalogEntries(providerId).length) return model
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
