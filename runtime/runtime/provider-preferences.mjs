// Provider 偏好与配置管理：负责模型运行时装配、Provider 配置（连接/密钥/模型）读写、
// 模型发现与目录同步、默认模型对账、会话模型/思考等级切换，以及配置迁移与安全校验。
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { ModelRuntime } from './pi-coding-agent.mjs'
import { inferModelKind } from '../services/visual-generation/index.mjs'
import { redactSecretText } from '../security/secret-redaction.mjs'
import { applyPisperSystemPrompt } from '../prompts/pisper-system-prompt.mjs'

// 内置 Provider 标识与展示名；其余自定义 Provider 由用户配置。
const KNOWN_PROVIDERS = [
  'openai',
  'openai-codex',
  'anthropic',
  'google',
  'deepseek',
  'xai',
  'openrouter',
  'kimi-coding',
  'zai-coding-cn',
]
const PROVIDER_LABELS = {
  openai: 'OpenAI',
  'openai-codex': 'OpenAI Codex',
  anthropic: 'Anthropic',
  google: 'Google',
  deepseek: 'DeepSeek',
  xai: 'xAI',
  openrouter: 'OpenRouter',
  'kimi-coding': 'Kimi Code',
  'zai-coding-cn': 'GLM',
}
// 官方 OAuth Provider 的合法主机：检测自定义端点时以此为准。
const OFFICIAL_OAUTH_HOSTS = {
  'openai-codex': 'chatgpt.com',
  anthropic: 'api.anthropic.com',
}

// 检测 Provider 是否配置了非官方端点：官方 OAuth Provider 若被指向自定义 Base URL，
// 登录凭据可能被外发到第三方中转站，导入配置前必须拦截。
function hasNonOfficialOAuthEndpoint(providerId, overlay) {
  const expectedHost = OFFICIAL_OAUTH_HOSTS[providerId]
  if (!expectedHost) return true
  const values = [
    overlay?.baseUrl,
    ...(Array.isArray(overlay?.models) ? overlay.models.map((model) => model?.baseUrl) : []),
  ].filter(Boolean)
  return values.some((value) => {
    try {
      const url = new URL(value)
      return url.protocol !== 'https:' || url.hostname !== expectedHost || Boolean(url.port)
    } catch {
      return true
    }
  })
}

// 支持的 API 协议与各 Provider 默认端点。
const PROVIDER_API_IDS = new Set([
  'openai-responses',
  'openai-completions',
  'anthropic-messages',
  'google-generative-ai',
])
const PROVIDER_DEFAULT_BASE_URLS = {
  openai: 'https://api.openai.com/v1',
  'openai-codex': 'https://chatgpt.com/backend-api',
  anthropic: 'https://api.anthropic.com/v1',
  google: 'https://generativelanguage.googleapis.com/v1beta',
  deepseek: 'https://api.deepseek.com',
  xai: 'https://api.x.ai/v1',
  openrouter: 'https://openrouter.ai/api/v1',
  'kimi-coding': 'https://api.kimi.com/coding/',
  'zai-coding-cn': 'https://open.bigmodel.cn/api/paas/v4',
}

// 模型排序权重：用于在 Provider 内优先推荐新模型（无启发式时按 reasoning 区分）。
function modelRank(provider, model) {
  const id = model.id.toLowerCase()
  if ((provider === 'openai' || provider === 'openai-codex') && id.startsWith('gpt-5')) return 100
  if (provider === 'anthropic' && /claude-(opus|sonnet)-4/.test(id)) return 100
  if (provider === 'google' && /gemini-(3|2\.5)/.test(id)) return 100
  if (provider === 'deepseek' && /reasoner|chat/.test(id)) return 90
  if (provider === 'kimi-coding') {
    if (id === 'k3') return 120
    if (id === 'kimi-for-coding-highspeed') return 115
    if (id === 'kimi-for-coding' || id === 'k2p7') return 110
    if (id.includes('k2-thinking')) return 100
  }
  if (provider === 'zai-coding-cn') {
    if (id === 'glm-5.2') return 120
    if (id === 'glm-5.1') return 110
    if (id.includes('glm-5-turbo')) return 105
    if (id === 'glm-4.7') return 100
    if (id.includes('glm-4.7-flash')) return 90
  }
  return model.reasoning ? 50 : 10
}

// 自定义 Provider ID 规范化：小写、去非法字符、限长。
function providerProfileId(value) {
  return String(value || '')
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 60)
}

// 从凭据对象/字符串中提取密钥明文。
function credentialSecret(credential) {
  if (typeof credential === 'string') return credential.trim()
  if (!credential || typeof credential !== 'object') return ''
  return String(
    credential.key || credential.apiKey || credential.token || credential.accessToken || '',
  ).trim()
}

// 解析已配置密钥：显式凭据优先；其次支持 $ENV 环境变量引用。
function configuredProviderSecret(credential, providerConfig) {
  const stored = credentialSecret(credential)
  if (stored) return stored
  const reference = String(providerConfig?.apiKey || '').trim()
  if (reference.startsWith('$')) return String(process.env[reference.slice(1)] || '').trim()
  return reference
}

// Base URL 归一化（去尾部斜杠、小写），用于端点比较。
function normalizedProviderBaseUrl(value) {
  return String(value || '')
    .trim()
    .replace(/\/+$/, '')
    .toLowerCase()
}

function sameBaseUrl(left, right) {
  return normalizedProviderBaseUrl(left) === normalizedProviderBaseUrl(right)
}

function hasHeader(headers, expectedName) {
  const expected = String(expectedName || '').toLowerCase()
  return Object.keys(headers || {}).some((name) => name.toLowerCase() === expected)
}

function usesCustomProviderEndpoint(providerId, providerConfig) {
  const baseUrl = String(providerConfig?.baseUrl || '').trim()
  if (!baseUrl) return false
  const officialBaseUrl = PROVIDER_DEFAULT_BASE_URLS[providerId]
  return !officialBaseUrl || !sameBaseUrl(baseUrl, officialBaseUrl)
}

// 自定义 Provider 端点（与官方默认不同）时附加 Pisper 的 User-Agent。
function providerHeaders(providerId, providerConfig, userAgent, modelHeaders = {}) {
  const headers = { ...(providerConfig?.headers || {}), ...(modelHeaders || {}) }
  if (usesCustomProviderEndpoint(providerId, providerConfig) && !hasHeader(headers, 'user-agent'))
    headers['User-Agent'] = userAgent
  return headers
}

// 由配置推断 Provider 类型：模型全为视觉模型则视为 visual，否则 chat。
function inferredProviderType(providerConfig) {
  const models = Array.isArray(providerConfig?.models) ? providerConfig.models : []
  if (!models.length) return 'chat'
  return models.every((model) => inferModelKind(model.id, model.kind) !== 'chat')
    ? 'visual'
    : 'chat'
}

// 视觉模型“占用”键：同一端点 + 同一模型 ID + 同一 kind 被视为同一个视觉模型。
function visualModelClaimKey(baseUrl, modelId, kind) {
  return [normalizedProviderBaseUrl(baseUrl), String(modelId || '').toLowerCase(), kind].join('\0')
}

// 收集各视觉 Provider 的模型占用：用于避免同一视觉模型被多个 Provider 重复列出。
function dedicatedVisualModelClaims(modelsJson, appConfig) {
  const claims = new Map()
  const disabled = new Set(appConfig.disabledProviders || [])
  for (const [providerId, provider] of Object.entries(modelsJson.providers || {})) {
    if (disabled.has(providerId)) continue
    const type = appConfig.providerTypes?.[providerId] || inferredProviderType(provider)
    if (type !== 'visual') continue
    for (const model of provider.models || []) {
      const kind = inferModelKind(model.id, model.kind)
      if (kind === 'chat') continue
      const baseUrl =
        model.baseUrl || provider.baseUrl || PROVIDER_DEFAULT_BASE_URLS[providerId] || ''
      if (!baseUrl) continue
      const key = visualModelClaimKey(baseUrl, model.id, kind)
      const providerIds = claims.get(key) || new Set()
      providerIds.add(providerId)
      claims.set(key, providerIds)
    }
  }
  return claims
}

function claimedByOtherVisualProvider(claims, providerId, baseUrl, modelId, kind) {
  const providerIds = claims.get(visualModelClaimKey(baseUrl, modelId, kind))
  return Boolean(providerIds && [...providerIds].some((id) => id !== providerId))
}

function claimedByOtherVisualProviderAnyKind(claims, providerId, baseUrl, modelId) {
  const prefix = [normalizedProviderBaseUrl(baseUrl), String(modelId || '').toLowerCase(), ''].join(
    '\0',
  )
  for (const [key, providerIds] of claims) {
    if (!key.startsWith(prefix)) continue
    if ([...providerIds].some((id) => id !== providerId)) return true
  }
  return false
}

// 扩展思考等级表（按强度升序），用于把请求等级收敛到模型可用等级。
const EXTENDED_THINKING_LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']

// 把请求的思考等级收敛到模型支持列表：先向上找再向下找，找不到回退到第一个。
export function clampThinkingLevelToAvailable(availableLevels, requested) {
  const levels = Array.isArray(availableLevels) ? availableLevels : []
  const current = String(requested || '')
  if (!levels.length) return current || 'off'
  if (levels.includes(current)) return current
  const requestedIndex = EXTENDED_THINKING_LEVELS.indexOf(current)
  if (requestedIndex === -1) return levels[0]
  for (let index = requestedIndex; index < EXTENDED_THINKING_LEVELS.length; index += 1) {
    const candidate = EXTENDED_THINKING_LEVELS[index]
    if (levels.includes(candidate)) return candidate
  }
  for (let index = requestedIndex - 1; index >= 0; index -= 1) {
    const candidate = EXTENDED_THINKING_LEVELS[index]
    if (levels.includes(candidate)) return candidate
  }
  return levels[0]
}

// 对齐会话思考等级：当前等级不在可用列表时收敛并写回会话。
export function reconcileSessionThinkingLevel(session) {
  const availableLevels = session.getAvailableThinkingLevels()
  const current = String(session.thinkingLevel || '')
  if (!availableLevels.length || availableLevels.includes(current)) {
    return { availableLevels, thinkingLevel: current, changed: false }
  }
  const thinkingLevel = clampThinkingLevelToAvailable(availableLevels, current)
  if (typeof session.setThinkingLevel === 'function') session.setThinkingLevel(thinkingLevel)
  return { availableLevels, thinkingLevel, changed: thinkingLevel !== current }
}

// 会话思考状态（含可用等级与支持性说明），供前端展示。
function sessionThinkingState(session) {
  const availableLevels = session.getAvailableThinkingLevels()
  const supported = availableLevels.length > 0
  const thinkingLevel = clampThinkingLevelToAvailable(availableLevels, session.thinkingLevel)
  return {
    thinkingLevel,
    availableLevels,
    status: supported ? 'supported' : 'unsupported',
    message: supported ? '' : 'The current model does not expose configurable thinking levels.',
    model: session.model ? `${session.model.provider}/${session.model.id}` : '',
  }
}

// ProviderPreferences：模型运行时/Provider 配置的读写中枢，所有配置变更后
// 都会重载模型运行时并使会话运行时失效（下次会话按新配置重建）。
export class ProviderPreferences {
  constructor({
    authPath,
    modelsPath,
    appConfigPath,
    providerUserAgent,
    providerDiscovery,
    providerModelDiscovery,
    providerModelCatalog,
    modelMetadata,
    getModelRuntime,
    setModelRuntime,
    getSettingsManager,
    getSession,
    isSessionRunActive,
    contextUsage,
    invalidateProjection,
    invalidateSessionRuntimes,
    reloadModelRuntime,
    getConfig,
    getProviderDiscovery,
    discoverProviderModels,
    reconcileDefaultModel,
    providerState,
  }) {
    this.authPath = authPath
    this.modelsPath = modelsPath
    this.appConfigPath = appConfigPath
    this.providerUserAgent = providerUserAgent
    this.providerDiscovery = providerDiscovery
    this.providerModelDiscovery = providerModelDiscovery
    this.providerModelCatalog = providerModelCatalog
    this.modelMetadata = modelMetadata
    this.getModelRuntime = getModelRuntime
    this.setModelRuntime = setModelRuntime
    this.getSettingsManager = getSettingsManager
    this.getSession = getSession
    this.isSessionRunActive =
      isSessionRunActive || ((_id, value) => Boolean(value?.session?.isStreaming))
    this.contextUsage = contextUsage
    this.invalidateProjection = invalidateProjection
    this.invalidateSessionRuntimes = invalidateSessionRuntimes
    this.reloadModelRuntime = reloadModelRuntime
    this.getConfigFacade = getConfig
    this.getProviderDiscoveryFacade = getProviderDiscovery
    this.discoverProviderModelsFacade = discoverProviderModels
    this.reconcileDefaultModelFacade = reconcileDefaultModel
    this.providerState = providerState
  }

  // 重载模型运行时：读取 models.json 的覆盖配置（Base URL/Header/上下文窗口等）
  // 装饰进引擎运行时，然后重建 ModelRuntime。
  async reload() {
    const modelsJson = await readJson(this.modelsPath, { providers: {} })
    const modelRuntime = await ModelRuntime.create({
      authPath: this.authPath,
      modelsPath: this.modelsPath,
      allowModelNetwork: false,
    })
    const configuredBaseUrls = {}
    const configuredHeaders = {}
    const configuredContextWindows = {}
    const configuredInputs = {}
    const configuredReasoning = {}
    for (const provider of modelRuntime.getProviders()) {
      const overlay = modelsJson.providers?.[provider.id] || {}
      configuredBaseUrls[provider.id] =
        overlay.baseUrl ||
        PROVIDER_DEFAULT_BASE_URLS[provider.id] ||
        modelRuntime.getModels(provider.id)[0]?.baseUrl ||
        ''
      if (usesCustomProviderEndpoint(provider.id, overlay)) {
        configuredHeaders[provider.id] = providerHeaders(
          provider.id,
          overlay,
          this.providerUserAgent,
        )
      }
      for (const model of overlay.models || []) {
        if (Number(model?.contextWindow) > 0) {
          configuredContextWindows[`${provider.id}:${model.id}`] = Number(model.contextWindow)
        }
        if (Array.isArray(model?.input)) {
          configuredInputs[`${provider.id}:${model.id}`] = model.input
        }
        const configuredModel = overlay.models?.find((entry) => entry?.id === model.id)
        if (configuredModel) {
          configuredReasoning[`${provider.id}:${model.id}`] =
            typeof configuredModel.reasoning === 'boolean' ? configuredModel.reasoning : null
        }
      }
    }
    this.providerModelCatalog.decorateRuntime(
      modelRuntime,
      configuredBaseUrls,
      configuredHeaders,
      configuredContextWindows,
      configuredInputs,
      configuredReasoning,
    )
    this.setModelRuntime(modelRuntime)
    this.invalidateProjection('', { allUsage: true })
  }

  // 切换会话模型：校验运行状态/停用 Provider，切换后重对齐思考等级与系统提示。
  async setSessionModel(id, provider, modelId) {
    const value = await this.getSession(id)
    if (this.isSessionRunActive?.(id, value) ?? value.session.isStreaming) {
      throw new Error('当前会话正在运行，请完成或停止后再切换模型。')
    }
    const appConfig = await readJson(this.appConfigPath, {
      toolMode: 'full',
      disabledProviders: [],
    })
    if ((appConfig.disabledProviders || []).includes(String(provider || ''))) {
      throw new Error('该 Provider 当前未启用。')
    }
    await this.modelMetadata.ensure(modelId)
    const model = this.getModelRuntime().getModel(String(provider || ''), String(modelId || ''))
    if (!model) throw new Error('指定的模型不存在。')
    const settingsManager = this.getSettingsManager()
    const settings = settingsManager.getGlobalSettings()
    const defaultProvider = settings.defaultProvider
    const defaultModel = settings.defaultModel
    const defaultThinkingLevel = settings.defaultThinkingLevel
    try {
      await value.session.setModel(model)
      applyPisperSystemPrompt(value.session, model)
      reconcileSessionThinkingLevel(value.session)
    } finally {
      if (defaultProvider && defaultModel) {
        settingsManager.setDefaultModelAndProvider(defaultProvider, defaultModel)
      }
      if (defaultThinkingLevel) settingsManager.setDefaultThinkingLevel(defaultThinkingLevel)
    }
    value.modified = new Date().toISOString()
    this.invalidateProjection(id)
    const thinking = sessionThinkingState(value.session)
    return {
      id: value.session.sessionId,
      model: `${model.provider}/${model.id}`,
      provider: model.provider,
      modelId: model.id,
      thinkingLevel: thinking.thinkingLevel,
      availableThinkingLevels: thinking.availableLevels,
      thinkingStatus: thinking.status,
      thinkingMessage: thinking.message,
      contextUsage: this.contextUsage(value.session),
    }
  }

  async getSessionThinkingState(id) {
    const value = await this.getSession(id)
    return { id: value.session.sessionId, ...sessionThinkingState(value.session) }
  }

  async setSessionThinkingLevel(id, level) {
    const value = await this.getSession(id)
    if (this.isSessionRunActive?.(id, value) ?? value.session.isStreaming) {
      throw new Error('当前会话正在运行，请完成或停止后再切换思考等级。')
    }
    const requested = String(level || '')
    const availableLevels = value.session.getAvailableThinkingLevels()
    if (!availableLevels.includes(requested)) {
      throw new Error('当前模型不支持该思考等级。')
    }
    const settingsManager = this.getSettingsManager()
    const defaultThinkingLevel =
      settingsManager.getGlobalSettings().defaultThinkingLevel || 'medium'
    try {
      value.session.setThinkingLevel(requested)
    } finally {
      settingsManager.setDefaultThinkingLevel(defaultThinkingLevel)
    }
    value.modified = new Date().toISOString()
    this.invalidateProjection(id, { transcript: false, activity: true, usage: true })
    return { id: value.session.sessionId, ...sessionThinkingState(value.session) }
  }

  // 默认模型解析：从全局设置取默认 Provider/模型。
  resolveDefaultModel() {
    const settings = this.getSettingsManager()?.getGlobalSettings?.() || {}
    const provider = settings.defaultProvider
    const modelId = settings.defaultModel
    const modelRuntime = this.getModelRuntime()
    if (!provider || !modelId || !modelRuntime?.getModel) return null
    return modelRuntime.getModel(String(provider), String(modelId)) || null
  }

  // 发现外部 Provider 配置（CLI 登录/文件导入等），标注已导入/冲突状态。
  async getProviderDiscovery() {
    const modelRuntime = this.getModelRuntime()
    const [discovery, credentials, modelsJson, appConfig] = await Promise.all([
      this.providerDiscovery.discover(),
      readJson(this.authPath, {}),
      readJson(this.modelsPath, { providers: {} }),
      readJson(this.appConfigPath, { providerImports: {} }),
    ])
    return {
      ...discovery,
      providers: discovery.providers.map((provider) => {
        const importedFingerprint = appConfig.providerImports?.[provider.id]?.fingerprint
        const imported =
          importedFingerprint === provider.fingerprint &&
          (provider.kind === 'authentication'
            ? Boolean(credentials[provider.providerId])
            : Boolean(modelsJson.providers?.[provider.providerId]))
        const conflict =
          provider.kind === 'authentication'
            ? Boolean(credentials[provider.providerId]) && !imported
            : Boolean(modelsJson.providers?.[provider.providerId]) && !imported
        return {
          ...provider,
          configured:
            Boolean(credentials[provider.providerId]) ||
            modelRuntime.hasConfiguredAuth(provider.providerId),
          imported,
          conflict,
        }
      }),
    }
  }

  // 导入发现的 Provider：官方 OAuth Provider 带自定义端点时拒绝导入（防凭据外发）。
  async importDiscoveredProvider(discoveryId) {
    const loaded = await this.providerDiscovery.loadConfiguration(String(discoveryId || '').trim())
    const [credentials, modelsJson, appConfig] = await Promise.all([
      readJson(this.authPath, {}),
      readJson(this.modelsPath, { providers: {} }),
      readJson(this.appConfigPath, {
        toolMode: 'full',
        disabledProviders: [],
        providerImports: {},
      }),
    ])
    modelsJson.providers ||= {}
    const existingProvider = modelsJson.providers[loaded.providerId]
    if (
      loaded.kind === 'authentication' &&
      hasNonOfficialOAuthEndpoint(loaded.providerId, existingProvider)
    )
      throw new Error(
        '该官方 Provider 配置了自定义 Base URL。为避免登录凭据外发，请先为中转站使用独立 Provider ID。',
      )
    if (
      loaded.kind !== 'authentication' &&
      existingProvider &&
      JSON.stringify(existingProvider) !== JSON.stringify(loaded.providerConfig)
    )
      throw new Error('Pisper 已存在该 Provider 的模型配置，不会自动覆盖。')
    if (
      loaded.credential &&
      credentials[loaded.providerId] &&
      JSON.stringify(credentials[loaded.providerId]) !== JSON.stringify(loaded.credential)
    )
      throw new Error('Pisper 已存在该 Provider 的认证，不会自动覆盖。')

    if (loaded.kind !== 'authentication') {
      modelsJson.providers[loaded.providerId] = loaded.providerConfig
      await writeJsonAtomic(this.modelsPath, modelsJson)
    }
    if (loaded.credential && !credentials[loaded.providerId]) {
      credentials[loaded.providerId] = loaded.credential
      await writeJsonAtomic(this.authPath, credentials, { mode: 0o600 })
    }

    const disabledProviders = new Set(appConfig.disabledProviders || [])
    disabledProviders.delete(loaded.providerId)
    const providerImports = { ...(appConfig.providerImports || {}) }
    providerImports[String(discoveryId)] = {
      providerId: loaded.providerId,
      fingerprint: loaded.fingerprint,
      source: loaded.source,
    }
    await writeJsonAtomic(this.appConfigPath, {
      ...appConfig,
      disabledProviders: [...disabledProviders],
      providerImports,
    })

    await this.reloadModelRuntime()
    this.invalidateSessionRuntimes()
    return {
      kind: loaded.kind,
      providerId: loaded.providerId,
      selectedModel: loaded.selectedModel,
      config: await this.getConfigFacade(),
      discovery: await this.getProviderDiscoveryFacade(),
    }
  }

  // 完整配置视图：Provider 列表（含默认选中项）、工具模式、API Key 是否已配置。
  async getConfig() {
    const settings = this.getSettingsManager().getGlobalSettings()
    const modelRuntime = this.getModelRuntime()
    const appConfig = await readJson(this.appConfigPath, { toolMode: 'full' })
    const modelsJson = await readJson(this.modelsPath, { providers: {} })
    const credentials = await readJson(this.authPath, {})
    const runtimeProviders = modelRuntime.getProviders()
    const providerIds = [
      ...new Set([...KNOWN_PROVIDERS, ...Object.keys(modelsJson.providers || {})]),
    ]
    const disabledProviders = new Set(appConfig.disabledProviders || [])
    const visualClaims = dedicatedVisualModelClaims(modelsJson, appConfig)
    const providers = providerIds
      .map((id) => {
        const runtimeProvider = runtimeProviders.find((item) => item.id === id)
        const overlay = modelsJson.providers?.[id] || {}
        const overlayModels = Array.isArray(overlay.models) ? overlay.models : []
        const type = appConfig.providerTypes?.[id] || inferredProviderType(overlay)
        const models = modelRuntime
          .getModels(id)
          .map((model) => {
            const definition = overlayModels.find((item) => item.id === model.id)
            return {
              id: model.id,
              name: model.name || model.id,
              kind: inferModelKind(model.id, definition?.kind || model.pisperKind),
              reasoning: Boolean(model.reasoning),
              contextWindow: model.contextWindow || null,
              baseUrl: model.baseUrl || '',
              baseUrlOverride: definition?.baseUrl || '',
            }
          })
          .filter((model) => {
            if (type === 'visual') return model.kind !== 'chat'
            if (model.kind === 'chat') return true
            const baseUrl = model.baseUrl || overlay.baseUrl || PROVIDER_DEFAULT_BASE_URLS[id] || ''
            return !claimedByOtherVisualProvider(visualClaims, id, baseUrl, model.id, model.kind)
          })
          .sort(
            (left, right) =>
              modelRank(id, right) - modelRank(id, left) || left.name.localeCompare(right.name),
          )
        const chatModels = models.filter((model) => model.kind === 'chat')
        const preferredModel =
          appConfig.providerDefaultModels?.[id] ||
          (settings.defaultProvider === id ? settings.defaultModel : '')
        const defaultModel = chatModels.some((model) => model.id === preferredModel)
          ? preferredModel
          : chatModels[0]?.id || ''
        return {
          id,
          name: PROVIDER_LABELS[id] || overlay.name || runtimeProvider?.name || id,
          type,
          configured: Boolean(credentials[id]) || modelRuntime.hasConfiguredAuth(id),
          enabled: !disabledProviders.has(id),
          custom: !KNOWN_PROVIDERS.includes(id),
          api: overlay.api || modelRuntime.getModels(id)[0]?.api || 'openai-responses',
          baseUrl: overlay.baseUrl || PROVIDER_DEFAULT_BASE_URLS[id] || '',
          organization: overlay.headers?.['OpenAI-Organization'] || '',
          defaultModel,
          models,
        }
      })
      .filter((provider) => provider.models.length > 0 || KNOWN_PROVIDERS.includes(provider.id))

    const hasChatModel = (provider) =>
      provider.type !== 'visual' && provider.models.some((model) => model.kind === 'chat')
    const selectedProviderEntry =
      providers.find(
        (item) =>
          item.id === settings.defaultProvider &&
          item.enabled &&
          item.configured &&
          hasChatModel(item),
      ) ||
      providers.find((item) => item.enabled && item.configured && hasChatModel(item)) ||
      providers.find((item) => item.enabled && hasChatModel(item)) ||
      providers[0]
    const selectedProvider = selectedProviderEntry?.id || 'openai'
    const selectedModel = selectedProviderEntry?.defaultModel || ''
    return {
      provider: selectedProvider,
      model: selectedModel,
      defaultProvider: settings.defaultProvider || '',
      defaultModel: settings.defaultModel || '',
      thinkingLevel: settings.defaultThinkingLevel || 'medium',
      toolMode: appConfig.toolMode || 'full',
      providers,
      apiKeyConfigured: Boolean(credentials[selectedProvider]),
    }
  }

  // 保存配置：连接信息/模型定义/默认模型/工具模式一次性写入三个配置文件。
  async saveConfig(input, toolsFromConfig, toolPresets) {
    const provider = String(input.provider || '').trim()
    const model = String(input.model || '').trim()
    if (!provider) throw new Error('Provider 不能为空。')
    if (this.providerState.refreshPromise) {
      await this.providerState.refreshPromise.catch(() => {})
    }
    const currentAppConfig = await readJson(this.appConfigPath, {
      toolMode: 'full',
      disabledProviders: [],
    })
    const existingOverlay = await readJson(this.modelsPath, { providers: {} })
    const providerType =
      input.providerType === 'visual' || input.providerType === 'chat'
        ? input.providerType
        : currentAppConfig.providerTypes?.[provider] ||
          inferredProviderType(existingOverlay.providers?.[provider] || {})
    const disabledProviders = new Set(currentAppConfig.disabledProviders || [])
    if (input.enabled === true) disabledProviders.delete(provider)
    if (disabledProviders.has(provider) && input.setAsDefault !== false) {
      throw new Error('请先启用该 Provider，再将其设为默认配置。')
    }

    const providerOverlay = { ...(existingOverlay.providers?.[provider] || {}) }
    const baseUrl = String(input.baseUrl || '').trim()
    const modelBaseUrl = String(input.modelBaseUrl || '').trim()
    const organization = String(input.organization || '').trim()
    const requestedApi = PROVIDER_API_IDS.has(input.api) ? input.api : ''
    if (!baseUrl && !PROVIDER_DEFAULT_BASE_URLS[provider]) {
      throw new Error('请先填写 Provider Base URL，再保存。')
    }
    if (providerType !== 'visual' && !model) {
      throw new Error('请先选择一个对话模型，再保存。')
    }

    const modelRuntime = this.getModelRuntime()
    const credentials = await readJson(this.authPath, {})
    let apiKeyUpdated = false
    if (input.clearApiKey) {
      delete credentials[provider]
      apiKeyUpdated = true
    }
    if (typeof input.apiKey === 'string' && input.apiKey.trim()) {
      credentials[provider] = { type: 'api_key', key: input.apiKey.trim() }
      apiKeyUpdated = true
    }
    const hasAuthentication =
      Boolean(configuredProviderSecret(credentials[provider], providerOverlay)) ||
      modelRuntime.hasConfiguredAuth(provider)
    if (!hasAuthentication) {
      throw new Error('请先填写 API Key 或加载 Provider 认证，再保存。')
    }
    if (apiKeyUpdated) await writeJsonAtomic(this.authPath, credentials)

    const modelsJson = existingOverlay
    modelsJson.providers ||= {}
    if (requestedApi) {
      providerOverlay.api = requestedApi
      if (Array.isArray(providerOverlay.models)) {
        providerOverlay.models = providerOverlay.models.map((item) => ({
          ...item,
          api: requestedApi,
        }))
      }
    }
    const runtimeModel = model ? modelRuntime.getModel(provider, model) : null
    if (model && !runtimeModel) {
      providerOverlay.name ||= String(input.providerName || provider)
      providerOverlay.api ||= String(input.api || 'openai-responses')
      providerOverlay.models = Array.isArray(providerOverlay.models)
        ? [...providerOverlay.models]
        : []
      if (!providerOverlay.models.some((item) => item.id === model)) {
        providerOverlay.models.push({
          id: model,
          name: String(input.modelName || model),
          api: String(input.api || 'openai-responses'),
          kind: inferModelKind(model, input.modelKind),
          reasoning: input.reasoning !== false,
          input: ['text', 'image'],
          contextWindow: Number(input.contextWindow) || 200_000,
          maxTokens: Number(input.maxTokens) || 128_000,
        })
      }
    }
    const modelDefinitions = Array.isArray(providerOverlay.models)
      ? [...providerOverlay.models]
      : []
    const definitionIndex = model ? modelDefinitions.findIndex((item) => item.id === model) : -1
    if (model && (modelBaseUrl || definitionIndex >= 0)) {
      const definition =
        definitionIndex >= 0
          ? { ...modelDefinitions[definitionIndex] }
          : {
              id: model,
              name: runtimeModel?.name || String(input.modelName || model),
              api:
                requestedApi ||
                runtimeModel?.api ||
                String(input.api || providerOverlay.api || 'openai-responses'),
              kind: inferModelKind(model, input.modelKind),
              reasoning: runtimeModel?.reasoning ?? input.reasoning !== false,
              input: runtimeModel?.input || ['text', 'image'],
              contextWindow: runtimeModel?.contextWindow || Number(input.contextWindow) || 200_000,
              maxTokens: runtimeModel?.maxTokens || Number(input.maxTokens) || 128_000,
            }
      if (modelBaseUrl) definition.baseUrl = modelBaseUrl
      else delete definition.baseUrl
      definition.kind = inferModelKind(model, input.modelKind || definition.kind)
      if (definitionIndex >= 0) modelDefinitions[definitionIndex] = definition
      else modelDefinitions.push(definition)
      providerOverlay.models = modelDefinitions
    }
    if (baseUrl) providerOverlay.baseUrl = baseUrl
    else delete providerOverlay.baseUrl
    if (organization) {
      providerOverlay.headers = {
        ...(providerOverlay.headers || {}),
        'OpenAI-Organization': organization,
      }
    } else if (providerOverlay.headers) {
      delete providerOverlay.headers['OpenAI-Organization']
      if (Object.keys(providerOverlay.headers).length === 0) delete providerOverlay.headers
    }
    if (Object.keys(providerOverlay).length) modelsJson.providers[provider] = providerOverlay
    else delete modelsJson.providers[provider]
    await writeJsonAtomic(this.modelsPath, modelsJson)

    const settingsManager = this.getSettingsManager()
    const setAsDefault = input.setAsDefault !== false
    const defaultUpdated = setAsDefault && providerType !== 'visual' && Boolean(model)
    if (defaultUpdated) settingsManager.setDefaultModelAndProvider(provider, model)
    settingsManager.setDefaultThinkingLevel(input.thinkingLevel || 'medium')
    await settingsManager.flush()
    const errors = settingsManager.drainErrors()
    if (errors.length) throw errors[0].error

    const requestedToolMode = ['read-only', 'workspace', 'full', 'custom'].includes(input.toolMode)
      ? input.toolMode
      : 'full'
    await writeJsonAtomic(this.appConfigPath, {
      ...currentAppConfig,
      toolMode: requestedToolMode,
      enabledTools:
        requestedToolMode === 'custom'
          ? toolsFromConfig(currentAppConfig)
          : toolPresets[requestedToolMode],
      disabledProviders: [...disabledProviders],
      providerTypes: {
        ...(currentAppConfig.providerTypes || {}),
        [provider]: providerType,
      },
      providerDefaultModels:
        providerType === 'visual' || !model
          ? { ...(currentAppConfig.providerDefaultModels || {}) }
          : { ...(currentAppConfig.providerDefaultModels || {}), [provider]: model },
    })
    await this.reloadModelRuntime()
    this.invalidateSessionRuntimes()
    return { ...(await this.getConfigFacade()), apiKeyUpdated, defaultUpdated }
  }

  // 设置 Provider 连接（API 协议 + Base URL + 可选密钥）。
  async setProviderConnection(id, input = {}) {
    const provider = String(id || '').trim()
    const api = String(input.api || '').trim()
    const baseUrl = String(input.baseUrl || '').trim()
    const apiKey = String(input.apiKey || '').trim()
    if (!provider) throw new Error('Provider 不能为空。')
    if (!PROVIDER_API_IDS.has(api)) throw new Error('Provider API 协议不受支持。')
    if (!baseUrl) throw new Error('Provider Base URL 不能为空。')
    if (baseUrl.length > 4_096) throw new Error('Provider Base URL 过长。')
    try {
      const parsed = new URL(baseUrl)
      if (
        !['http:', 'https:'].includes(parsed.protocol) ||
        parsed.username ||
        parsed.password ||
        parsed.hash
      ) {
        throw new Error()
      }
    } catch {
      throw new Error('Provider Base URL 必须是有效的 HTTP 或 HTTPS 地址。')
    }
    if (apiKey.length > 16_384) throw new Error('API Key 过长。')

    const config = await this.getConfigFacade()
    if (!config.providers.some((item) => item.id === provider)) {
      throw new Error('Provider 不存在。')
    }

    const modelsJson = await readJson(this.modelsPath, { providers: {} })
    modelsJson.providers ||= {}
    const providerOverlay = { ...(modelsJson.providers[provider] || {}), api }
    if (Array.isArray(providerOverlay.models)) {
      providerOverlay.models = providerOverlay.models.map((model) => ({ ...model, api }))
    }
    const officialBaseUrl = PROVIDER_DEFAULT_BASE_URLS[provider]
    if (officialBaseUrl && sameBaseUrl(baseUrl, officialBaseUrl)) {
      delete providerOverlay.baseUrl
    } else {
      providerOverlay.baseUrl = baseUrl
    }
    modelsJson.providers[provider] = providerOverlay
    await writeJsonAtomic(this.modelsPath, modelsJson)

    let apiKeyUpdated = false
    if (apiKey) {
      const credentials = await readJson(this.authPath, {})
      credentials[provider] = { type: 'api_key', key: apiKey }
      await writeJsonAtomic(this.authPath, credentials)
      apiKeyUpdated = true
    }

    await this.reloadModelRuntime()
    this.invalidateSessionRuntimes()
    return {
      ...(await this.getConfigFacade()),
      connectionUpdated: true,
      apiKeyUpdated,
      updatedProviderId: provider,
    }
  }

  // 仅更新 API Key。
  async setProviderApiKey(id, input = {}) {
    const provider = String(id || '').trim()
    const apiKey = String(input.apiKey || '').trim()
    if (!provider) throw new Error('Provider 不能为空。')
    if (!apiKey) throw new Error('API Key 不能为空。')
    if (apiKey.length > 16_384) throw new Error('API Key 过长。')

    const config = await this.getConfigFacade()
    if (!config.providers.some((item) => item.id === provider)) {
      throw new Error('Provider 不存在。')
    }
    const credentials = await readJson(this.authPath, {})
    credentials[provider] = { type: 'api_key', key: apiKey }
    await writeJsonAtomic(this.authPath, credentials)
    await this.reloadModelRuntime()
    this.invalidateSessionRuntimes()
    return {
      ...(await this.getConfigFacade()),
      apiKeyUpdated: true,
      updatedProviderId: provider,
    }
  }

  // 启用/停用 Provider：停用默认 Provider 时自动切换到其他已配置的 chat Provider。
  async setProviderEnabled(id, enabled) {
    const provider = String(id || '').trim()
    const modelRuntime = this.getModelRuntime()
    if (
      !modelRuntime.getProviders().some((item) => item.id === provider) &&
      !KNOWN_PROVIDERS.includes(provider)
    )
      throw new Error('Provider 不存在。')
    const appConfig = await readJson(this.appConfigPath, {
      toolMode: 'full',
      disabledProviders: [],
    })
    const disabled = new Set(appConfig.disabledProviders || [])
    if (enabled) disabled.delete(provider)
    else disabled.add(provider)

    const settingsManager = this.getSettingsManager()
    const settings = settingsManager.getGlobalSettings()
    if (!enabled && settings.defaultProvider === provider) {
      const credentials = await readJson(this.authPath, {})
      const providerTypes = appConfig.providerTypes || {}
      const modelsJson = await readJson(this.modelsPath, { providers: {} })
      const alternative = modelRuntime.getProviders().find((item) => {
        const type =
          providerTypes[item.id] || inferredProviderType(modelsJson.providers?.[item.id] || {})
        return (
          item.id !== provider &&
          type !== 'visual' &&
          !disabled.has(item.id) &&
          credentials[item.id] &&
          modelRuntime
            .getModels(item.id)
            .some((model) => inferModelKind(model.id, model.pisperKind) === 'chat')
        )
      })
      if (!alternative) throw new Error('至少需要保留一个已配置并启用的 Provider。')
      const alternativeModel = modelRuntime
        .getModels(alternative.id)
        .find((model) => inferModelKind(model.id, model.pisperKind) === 'chat')
      settingsManager.setDefaultModelAndProvider(alternative.id, alternativeModel.id)
      await settingsManager.flush()
    }
    await writeJsonAtomic(this.appConfigPath, {
      ...appConfig,
      disabledProviders: [...disabled],
    })
    this.invalidateSessionRuntimes()
    return this.getConfigFacade()
  }

  // 新建自定义 Provider（含初始模型与密钥）。
  async createProvider(input) {
    const id = providerProfileId(input.id || input.name)
    const name = String(input.name || '').trim()
    const api = String(input.api || 'openai-responses').trim()
    const baseUrl = String(input.baseUrl || '').trim()
    const modelId = String(input.model || '').trim()
    const providerType =
      input.providerType === 'visual' || inferModelKind(modelId, input.modelKind) !== 'chat'
        ? 'visual'
        : 'chat'
    if (!id || !name || !baseUrl || !modelId) {
      throw new Error('名称、Provider ID、Base URL 和初始模型不能为空。')
    }
    if (providerType === 'visual' && inferModelKind(modelId, input.modelKind) === 'chat') {
      throw new Error('视觉 Provider 的初始模型必须是图像或视频模型。')
    }
    const modelRuntime = this.getModelRuntime()
    if (modelRuntime.getProviders().some((item) => item.id === id) || KNOWN_PROVIDERS.includes(id))
      throw new Error('Provider ID 已存在，请使用不同的连接标识。')

    const modelsJson = await readJson(this.modelsPath, { providers: {} })
    modelsJson.providers ||= {}
    modelsJson.providers[id] = {
      name,
      api,
      baseUrl,
      models: [
        {
          id: modelId,
          name: String(input.modelName || modelId).trim() || modelId,
          api,
          kind: inferModelKind(modelId, input.modelKind),
          reasoning: input.reasoning !== false,
          input: ['text', 'image'],
          contextWindow: Number(input.contextWindow) || 200_000,
          maxTokens: Number(input.maxTokens) || 128_000,
        },
      ],
    }
    await writeJsonAtomic(this.modelsPath, modelsJson)

    const apiKey = String(input.apiKey || '').trim()
    if (apiKey) {
      const credentials = await readJson(this.authPath, {})
      credentials[id] = { type: 'api_key', key: apiKey }
      await writeJsonAtomic(this.authPath, credentials)
    }
    const appConfig = await readJson(this.appConfigPath, {
      toolMode: 'full',
      disabledProviders: [],
    })
    const disabled = new Set(appConfig.disabledProviders || [])
    if (input.enabled === false) disabled.add(id)
    else disabled.delete(id)
    await writeJsonAtomic(this.appConfigPath, {
      ...appConfig,
      disabledProviders: [...disabled],
      providerTypes: { ...(appConfig.providerTypes || {}), [id]: providerType },
      providerDefaultModels:
        providerType === 'visual'
          ? { ...(appConfig.providerDefaultModels || {}) }
          : { ...(appConfig.providerDefaultModels || {}), [id]: modelId },
    })
    await this.reloadModelRuntime()
    this.invalidateSessionRuntimes()
    return { ...(await this.getConfigFacade()), createdProviderId: id }
  }

  // 默认模型对账：把配置视图选中的默认模型写回全局设置。
  async reconcileDefaultModel() {
    const settingsManager = this.getSettingsManager()
    const settings = settingsManager.getGlobalSettings()
    const config = await this.getConfigFacade()
    if (
      config.provider &&
      config.model &&
      (settings.defaultProvider !== config.provider || settings.defaultModel !== config.model)
    ) {
      settingsManager.setDefaultModelAndProvider(config.provider, config.model)
      await settingsManager.flush()
    }
    return config
  }

  // 后台刷新各 Provider 的模型目录：跳过未配置密钥/未显式连接的 Provider，
  // 并发执行并去重（refreshPromise 保证同一时间只有一次刷新）。
  async refreshProviderModels() {
    if (this.providerState.refreshPromise) return this.providerState.refreshPromise
    const refresh = async () => {
      const modelRuntime = this.getModelRuntime()
      const [modelsJson, credentials, appConfig] = await Promise.all([
        readJson(this.modelsPath, { providers: {} }),
        readJson(this.authPath, {}),
        readJson(this.appConfigPath, { disabledProviders: [] }),
      ])
      const disabled = new Set(appConfig.disabledProviders || [])
      const providerIds = new Set([...KNOWN_PROVIDERS, ...Object.keys(modelsJson.providers || {})])
      const jobs = []
      for (const provider of providerIds) {
        if (disabled.has(provider) || provider === 'openai-codex') continue
        const overlay = modelsJson.providers?.[provider] || {}
        const baseUrl = String(overlay.baseUrl || PROVIDER_DEFAULT_BASE_URLS[provider] || '').trim()
        if (!baseUrl) continue
        const hasAuthentication =
          Boolean(configuredProviderSecret(credentials[provider], overlay)) ||
          modelRuntime.hasConfiguredAuth(provider)
        const isExplicitConnection = Boolean(overlay.baseUrl)
        if (!hasAuthentication && !isExplicitConnection) continue
        jobs.push(
          (async () => {
            try {
              const result = await this.discoverProviderModelsFacade(provider, {
                reconcile: false,
                includeConfig: false,
              })
              return {
                provider,
                ok: true,
                count: result.count,
                added: result.addedModelIds.length,
                removed: result.removedModelIds.length,
              }
            } catch (error) {
              return {
                provider,
                ok: false,
                error: redactSecretText(error instanceof Error ? error.message : String(error)),
              }
            }
          })(),
        )
      }
      const results = await Promise.all(jobs)
      return { results, config: await this.reconcileDefaultModelFacade() }
    }
    const pending = refresh().finally(() => {
      if (this.providerState.refreshPromise === pending) {
        this.providerState.refreshPromise = null
      }
    })
    this.providerState.refreshPromise = pending
    return pending
  }

  // 单 Provider 模型发现：拉取远程模型列表，与当前目录同步（新增/移除），
  // 仅在 Base URL 与已配置一致时才自动同步。
  async discoverProviderModels(providerId, input = {}) {
    const provider = String(providerId || '').trim()
    const modelRuntime = this.getModelRuntime()
    if (
      !modelRuntime.getProviders().some((item) => item.id === provider) &&
      !KNOWN_PROVIDERS.includes(provider)
    )
      throw new Error('Provider 不存在。')
    const [modelsJson, credentials, appConfig] = await Promise.all([
      readJson(this.modelsPath, { providers: {} }),
      readJson(this.authPath, {}),
      readJson(this.appConfigPath, { providerTypes: {} }),
    ])
    const overlay = modelsJson.providers?.[provider] || {}
    const runtimeModel = modelRuntime.getModels(provider)[0]
    const api = String(input.api || overlay.api || runtimeModel?.api || 'openai-responses').trim()
    const configuredBaseUrl = String(
      overlay.baseUrl || PROVIDER_DEFAULT_BASE_URLS[provider] || '',
    ).trim()
    const baseUrl = String(input.baseUrl || configuredBaseUrl || '').trim()
    if (!baseUrl) throw new Error('请先配置 Provider Base URL。')
    const apiKey =
      String(input.apiKey || '').trim() || configuredProviderSecret(credentials[provider], overlay)
    const discovered = await this.providerModelDiscovery.discover({
      api,
      baseUrl,
      apiKey,
      organization: String(
        input.organization || overlay.headers?.['OpenAI-Organization'] || '',
      ).trim(),
      headers: providerHeaders(provider, overlay, this.providerUserAgent),
    })
    const scope =
      input.providerType === 'visual' || input.providerType === 'chat'
        ? input.providerType
        : appConfig.providerTypes?.[provider] || inferredProviderType(overlay)
    const visualClaims = dedicatedVisualModelClaims(modelsJson, appConfig)
    const models =
      scope === 'visual'
        ? discovered.models
        : discovered.models.filter(
            (model) =>
              !claimedByOtherVisualProviderAnyKind(visualClaims, provider, baseUrl, model.id),
          )
    if (!models.length) throw new Error('Provider 没有返回可用的模型。')
    const result = { ...discovered, count: models.length, models, scope }
    const previousModelIds = new Set(modelRuntime.getModels(provider).map((model) => model.id))
    let sync = { addedModelIds: [], removedModelIds: [] }
    const synchronized = sameBaseUrl(baseUrl, configuredBaseUrl)
    if (synchronized) {
      await this.providerModelCatalog.sync(provider, {
        baseUrl,
        api,
        models: result.models,
      })
      const nextModelIds = new Set(result.models.map((model) => model.id))
      sync = {
        addedModelIds: [...nextModelIds].filter((id) => !previousModelIds.has(id)),
        removedModelIds: [...previousModelIds].filter((id) => !nextModelIds.has(id)),
      }
      if (input.reconcile !== false) await this.reconcileDefaultModelFacade()
    }
    const existing = new Set(modelRuntime.getModels(provider).map((model) => model.id))
    return {
      ...result,
      models: result.models.map((model) => ({ ...model, added: existing.has(model.id) })),
      synchronized,
      addedModelIds: sync.addedModelIds,
      removedModelIds: sync.removedModelIds,
      config: synchronized && input.includeConfig !== false ? await this.getConfigFacade() : null,
    }
  }

  // 手动添加模型（批量）：跳过已存在项，视觉 Provider 拒绝 chat 模型。
  async addProviderModels(providerId, inputs, { skipExisting = true } = {}) {
    const provider = String(providerId || '').trim()
    const modelRuntime = this.getModelRuntime()
    if (
      !modelRuntime.getProviders().some((item) => item.id === provider) &&
      !KNOWN_PROVIDERS.includes(provider)
    )
      throw new Error('Provider 不存在。')
    const models = Array.isArray(inputs) ? inputs : []
    if (!models.length) throw new Error('请至少选择一个模型。')
    if (models.length > 250) throw new Error('单次最多添加 250 个模型。')
    const [modelsJson, appConfig] = await Promise.all([
      readJson(this.modelsPath, { providers: {} }),
      readJson(this.appConfigPath, { providerTypes: {} }),
    ])
    modelsJson.providers ||= {}
    const overlay = { ...(modelsJson.providers[provider] || {}) }
    const providerType = appConfig.providerTypes?.[provider] || inferredProviderType(overlay)
    overlay.models = Array.isArray(overlay.models) ? [...overlay.models] : []
    const existing = new Set([
      ...overlay.models.map((item) => item.id),
      ...modelRuntime.getModels(provider).map((item) => item.id),
    ])
    const addedModelIds = []
    for (const input of models) {
      const modelId = String(input?.id || '').trim()
      if (!modelId) throw new Error('模型 ID 不能为空。')
      if (modelId.length > 240) throw new Error('模型 ID 过长。')
      const modelKind = inferModelKind(modelId, input.kind)
      if (providerType === 'visual' && modelKind === 'chat') {
        throw new Error('视觉 Provider 只能添加图像或视频模型。')
      }
      if (existing.has(modelId)) {
        if (!skipExisting) throw new Error('该模型已经存在。')
        continue
      }
      overlay.models.push({
        id: modelId,
        name: String(input.name || modelId).trim() || modelId,
        api: String(input.api || overlay.api || 'openai-responses'),
        kind: modelKind,
        ...(String(input.baseUrl || '').trim() ? { baseUrl: String(input.baseUrl).trim() } : {}),
        reasoning: input.reasoning !== false,
        input: ['text', 'image'],
        contextWindow: Number(input.contextWindow) || 200_000,
        maxTokens: Number(input.maxTokens) || 128_000,
      })
      existing.add(modelId)
      addedModelIds.push(modelId)
    }
    if (!addedModelIds.length) throw new Error('所选模型均已添加。')
    modelsJson.providers[provider] = overlay
    await writeJsonAtomic(this.modelsPath, modelsJson)
    const catalog = this.providerModelCatalog.get(provider)
    const providerBaseUrl = overlay.baseUrl || PROVIDER_DEFAULT_BASE_URLS[provider] || ''
    if (catalog && sameBaseUrl(catalog.baseUrl, providerBaseUrl)) {
      const added = models.filter((model) => addedModelIds.includes(String(model.id)))
      await this.providerModelCatalog.sync(provider, {
        baseUrl: catalog.baseUrl,
        api: catalog.api,
        models: [...catalog.models, ...added],
      })
    }
    await this.reloadModelRuntime()
    this.invalidateSessionRuntimes()
    return { ...(await this.getConfigFacade()), addedModelIds }
  }

  // 删除自定义 Provider：内置 Provider 不可删，同时清理密钥/配置/目录/默认设置。
  async deleteProvider(id) {
    const provider = String(id || '').trim()
    if (KNOWN_PROVIDERS.includes(provider)) {
      throw new Error('内置 Provider 不能删除，可以将其停用。')
    }
    const modelsJson = await readJson(this.modelsPath, { providers: {} })
    if (!modelsJson.providers?.[provider]) return null
    delete modelsJson.providers[provider]
    await writeJsonAtomic(this.modelsPath, modelsJson)
    const credentials = await readJson(this.authPath, {})
    delete credentials[provider]
    await writeJsonAtomic(this.authPath, credentials)
    const appConfig = await readJson(this.appConfigPath, {
      toolMode: 'full',
      disabledProviders: [],
    })
    appConfig.disabledProviders = (appConfig.disabledProviders || []).filter(
      (item) => item !== provider,
    )
    if (appConfig.providerTypes) delete appConfig.providerTypes[provider]
    if (appConfig.providerDefaultModels) delete appConfig.providerDefaultModels[provider]
    await writeJsonAtomic(this.appConfigPath, appConfig)
    await this.providerModelCatalog.remove(provider)
    const settingsManager = this.getSettingsManager()
    const settings = settingsManager.getGlobalSettings()
    const modelRuntime = this.getModelRuntime()
    if (settings.defaultProvider === provider) {
      const providerTypes = appConfig.providerTypes || {}
      const alternative = modelRuntime.getProviders().find((item) => {
        const type =
          providerTypes[item.id] || inferredProviderType(modelsJson.providers?.[item.id] || {})
        return (
          item.id !== provider &&
          type !== 'visual' &&
          credentials[item.id] &&
          modelRuntime
            .getModels(item.id)
            .some((model) => inferModelKind(model.id, model.pisperKind) === 'chat')
        )
      })
      if (alternative) {
        const alternativeModel = modelRuntime
          .getModels(alternative.id)
          .find((model) => inferModelKind(model.id, model.pisperKind) === 'chat')
        settingsManager.setDefaultModelAndProvider(alternative.id, alternativeModel.id)
        await settingsManager.flush()
      }
    }
    await this.reloadModelRuntime()
    this.invalidateSessionRuntimes()
    return this.getConfigFacade()
  }
}
