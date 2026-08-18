// 视觉模型选择与目录：模型类型完全由用户显式选择，不再按 ID 猜测；
// 提供按 Provider 筛选可用图像/视频模型的能力。
import { readJson } from '../../storage/json-file.mjs'

// 不再按模型 ID 猜测用途：类型完全由用户在添加时显式选择。
// 保留函数签名以兼容既有调用点与历史配置（'auto' 或缺省一律按对话模型处理）。
// 模型用途推断：显式指定才返回对应类型，否则一律按对话模型处理。
export function inferModelKind(modelId, explicitKind = 'auto') {
  if (['chat', 'image', 'video'].includes(explicitKind)) return explicitKind
  return 'chat'
}

function credentialKey(credential) {
  if (typeof credential === 'string') return credential
  if (credential?.type === 'api_key') return credential.key || ''
  return credential?.key || credential?.token || credential?.access_token || ''
}

function defaultBaseUrl(providerId, api) {
  const value = `${providerId} ${api}`.toLowerCase()
  if (value.includes('google')) return 'https://generativelanguage.googleapis.com/v1beta'
  if (value.includes('xai') || value.includes('x-ai') || value.includes('grok'))
    return 'https://api.x.ai/v1'
  if (value.includes('openrouter')) return 'https://openrouter.ai/api/v1'
  if (value.includes('openai')) return 'https://api.openai.com/v1'
  return ''
}

function driverFor(model) {
  if (model.visualApi) return model.visualApi
  const value = `${model.providerId} ${model.api} ${model.baseUrl} ${model.id}`.toLowerCase()
  if (value.includes('google') || value.includes('generativelanguage.googleapis.com'))
    return model.kind === 'video' ? 'google-video' : 'google-image'
  if (value.includes('xai') || value.includes('x.ai') || value.includes('grok'))
    return model.kind === 'video' ? 'xai-video' : 'xai-image'
  if (value.includes('openrouter'))
    return model.kind === 'video' ? 'openai-video' : 'openrouter-image'
  return model.kind === 'video' ? 'openai-video' : 'openai-image'
}

function modelScore(model) {
  const id = model.id.toLowerCase()
  let score = 0
  if (/gpt-image-2|gpt-5\.4-image/.test(id)) score += 120
  else if (/gpt-image|gpt-\d.*image/.test(id)) score += 105
  if (/gemini-3|imagen-4/.test(id)) score += 100
  if (/grok-imagine/.test(id)) score += 95
  if (/sora-2-pro|veo-3\.1/.test(id)) score += 120
  else if (/sora-2|veo-3/.test(id)) score += 105
  if (model.providerId === 'openai') score += 8
  if (model.providerId === 'google') score += 7
  if (model.providerId === 'xai') score += 6
  return score
}

function normalizedBaseUrl(value) {
  return String(value || '')
    .trim()
    .replace(/\/+$/, '')
    .toLowerCase()
}

function isVisualProvider(providerId, provider, appConfig) {
  const explicitType = appConfig.providerTypes?.[providerId]
  if (explicitType) return explicitType === 'visual'
  const models = Array.isArray(provider.models) ? provider.models : []
  return (
    models.length > 0 && models.every((model) => inferModelKind(model.id, model.kind) !== 'chat')
  )
}

function duplicateKey(model) {
  return [normalizedBaseUrl(model.baseUrl), model.id.toLowerCase(), model.kind, model.driver].join(
    '\0',
  )
}

function duplicatePriority(model) {
  return (
    (model.visualProvider ? 10_000 : 0) + (model.configuredDefinition ? 1_000 : 0) + model.score
  )
}

function selectionPriority(model) {
  return model.score + (model.visualProvider ? 25 : 0) + (model.configuredDefinition ? 2 : 0)
}

function compareModels(left, right) {
  return selectionPriority(right) - selectionPriority(left) || left.name.localeCompare(right.name)
}

function deduplicateModels(models) {
  const unique = new Map()
  for (const model of models) {
    const key = duplicateKey(model)
    const existing = unique.get(key)
    if (!existing || duplicatePriority(model) > duplicatePriority(existing)) unique.set(key, model)
  }
  return [...unique.values()]
}

function publicModel(model) {
  const {
    visualProvider: _visualProvider,
    configuredDefinition: _configuredDefinition,
    ...value
  } = model
  return value
}

export class VisualModelCatalog {
  constructor({ modelsPath, authPath, appConfigPath, getModelRuntime }) {
    this.modelsPath = modelsPath
    this.authPath = authPath
    this.appConfigPath = appConfigPath
    this.getModelRuntime = getModelRuntime
  }

  async candidates(kind) {
    const [modelsJson, credentials, appConfig] = await Promise.all([
      readJson(this.modelsPath, { providers: {} }),
      readJson(this.authPath, {}),
      readJson(this.appConfigPath, { disabledProviders: [], providerTypes: {} }),
    ])
    const disabled = new Set(appConfig.disabledProviders || [])
    const runtime = this.getModelRuntime?.()
    const providerIds = new Set([
      ...Object.keys(modelsJson.providers || {}),
      ...(runtime?.getProviders().map((provider) => provider.id) || []),
    ])
    const result = []
    for (const providerId of providerIds) {
      if (disabled.has(providerId)) continue
      const provider = modelsJson.providers?.[providerId] || {}
      const runtimeProvider = runtime?.getProvider(providerId)
      const apiKey = credentialKey(credentials[providerId])
      if (!apiKey) continue
      const runtimeModels = runtime?.getModels(providerId) || []
      const definitions = new Map((provider.models || []).map((model) => [model.id, model]))
      const modelIds = new Set([...definitions.keys(), ...runtimeModels.map((model) => model.id)])
      for (const modelId of modelIds) {
        const definition = definitions.get(modelId) || {}
        const runtimeModel = runtimeModels.find((model) => model.id === modelId)
        const modelKind = inferModelKind(modelId, definition.kind || runtimeModel?.pisperKind)
        if (modelKind !== kind) continue
        const api = definition.api || provider.api || runtimeModel?.api || ''
        const baseUrl = String(
          definition.baseUrl ||
            provider.baseUrl ||
            runtimeModel?.baseUrl ||
            defaultBaseUrl(providerId, api),
        ).replace(/\/+$/, '')
        if (!baseUrl) continue
        const value = {
          id: modelId,
          name: definition.name || runtimeModel?.name || modelId,
          providerId,
          providerName: provider.name || runtimeProvider?.name || providerId,
          api,
          kind: modelKind,
          baseUrl,
          apiKey,
          headers: {
            ...(provider.headers || {}),
            ...(runtimeModel?.headers || {}),
            ...(definition.headers || {}),
          },
          visualApi: definition.visualApi || provider.visualApi || '',
          visualProvider: isVisualProvider(providerId, provider, appConfig),
          configuredDefinition: definitions.has(modelId),
        }
        value.driver = driverFor(value)
        value.score = modelScore(value)
        result.push(value)
      }
    }
    return result
  }

  async list(kind) {
    const models = deduplicateModels(await this.candidates(kind))
    return models.sort(compareModels).map(publicModel)
  }

  async select(kind, requestedModel) {
    const candidates = await this.candidates(kind)
    if (!candidates.length)
      throw new Error(
        `没有已配置并启用的${kind === 'video' ? '视频' : '图像'}生成模型。请先在配置页添加视觉模型。`,
      )
    const requested = String(requestedModel || '')
      .trim()
      .toLowerCase()
    if (requested) {
      const qualified = candidates.find(
        (model) => `${model.providerId}/${model.id}`.toLowerCase() === requested,
      )
      if (qualified) return publicModel(qualified)
    }
    const models = deduplicateModels(candidates).sort(compareModels)
    if (!requested) return publicModel(models[0])
    const exact = models.find((model) => model.id.toLowerCase() === requested)
    if (!exact) throw new Error(`未找到已启用的视觉模型：${requestedModel}`)
    return publicModel(exact)
  }
}
