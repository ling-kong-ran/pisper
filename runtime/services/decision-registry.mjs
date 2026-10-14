// 注册表只装配供应商、协议与模型策略。实例无后台资源；在途调用由 DecisionService 持有。
import { DecisionError, remoteFailure } from './decision-errors.mjs'
import { validateModelResult } from './decision-contract.mjs'

/** @typedef {{provider: string, baseUrl: string, modelId: string, apiKey: string}} RemoteConfig */
/** @typedef {{signal?: AbortSignal, fetchImpl?: typeof fetch, timeoutMs?: number}} DecisionContext */
/** @typedef {{questionTypes: readonly string[], booleanProbability: boolean}} DecisionCapabilities */
/** @typedef {{id: string, capabilities: DecisionCapabilities, decide: (config: {endpoint: string, apiKey: string, modelId: string}, input: import('./decision-contract.mjs').ModelRequest, context: DecisionContext) => Promise<unknown>}} DecisionAdapter */
/** @typedef {{provider: string, modelId: string, approvalPolicyId?: string, capabilities?: DecisionCapabilities}} DecisionModelDefinition */
/** @typedef {import('../../shared/decision-provider-catalog.mjs').DecisionProviderDefinition} ProviderDefinition */

/** @param {string} base @param {ProviderDefinition} provider */
export function resolveDecisionEndpoint(base, provider) {
  const { path, endpointSuffixes = [] } = provider
  try {
    const url = new URL(base)
    if (url.protocol !== 'https:' || url.username || url.password || url.hash || url.search)
      throw remoteFailure('config_missing')
    // 完整接口由供应商预设的路径后缀识别；协议特例以元数据声明，注册表不识别具体 SDK 路径。
    const clean = base.replace(/\/+$/, '')
    return clean.endsWith(path) ||
      endpointSuffixes.some((suffix) => clean.toLowerCase().endsWith(suffix.toLowerCase()))
      ? clean
      : `${clean}${path}`
  } catch {
    throw remoteFailure('config_missing')
  }
}

/**
 * 新协议只注册适配器与供应商，不修改服务或审批调用方。
 * 注册来源必须是应用代码，不能从用户配置反序列化可执行实现。
 * @param {{adapters: readonly DecisionAdapter[], providers: Readonly<Record<string, ProviderDefinition>>, models?: readonly DecisionModelDefinition[]}} definitions
 */
export function createDecisionRegistry({ adapters, providers, models = [] }) {
  /** @type {Map<string, DecisionAdapter>} */
  const implementations = new Map()
  for (const adapter of adapters) {
    if (!adapter.id || implementations.has(adapter.id))
      throw new Error('Duplicate decision protocol.')
    implementations.set(
      adapter.id,
      Object.freeze({
        ...adapter,
        capabilities: Object.freeze({
          ...adapter.capabilities,
          questionTypes: Object.freeze([...adapter.capabilities.questionTypes]),
        }),
      }),
    )
  }
  /** @type {Readonly<Record<string, Readonly<ProviderDefinition>>>} */
  const catalog = Object.freeze(
    Object.fromEntries(
      Object.entries(providers).map(([id, provider]) => {
        if (!implementations.has(provider.protocol)) throw new Error('Unknown decision protocol.')
        if (
          !provider.path.startsWith('/') ||
          provider.path.startsWith('//') ||
          /[?#]/.test(provider.path)
        )
          throw new Error('Invalid decision provider path.')
        return [
          id,
          Object.freeze({
            ...provider,
            endpointSuffixes: Object.freeze([...(provider.endpointSuffixes ?? [])]),
          }),
        ]
      }),
    ),
  )
  const modelCatalog = models.map((model) => {
    if (!Object.hasOwn(catalog, model.provider) || !model.modelId)
      throw new Error('Unknown decision model provider.')
    return Object.freeze({
      ...model,
      capabilities: model.capabilities
        ? Object.freeze({
            ...model.capabilities,
            questionTypes: Object.freeze([...model.capabilities.questionTypes]),
          })
        : undefined,
    })
  })
  const modelKeys = new Set(
    modelCatalog.map((model) => JSON.stringify([model.provider, model.modelId])),
  )
  if (modelKeys.size !== modelCatalog.length) throw new Error('Duplicate decision model.')
  /** @param {string} id */
  function provider(id) {
    if (!Object.hasOwn(catalog, id)) throw remoteFailure('unsupported_provider')
    const entry = catalog[id]
    if (!entry) throw remoteFailure('unsupported_provider')
    return entry
  }
  /** @param {RemoteConfig} config */
  function resolve(config) {
    const entry = provider(config.provider)
    const adapter = implementations.get(entry.protocol)
    if (!adapter) throw remoteFailure('unsupported_provider')
    const modelId = config.modelId.trim() || entry.defaultModelId
    const model = modelCatalog.find(
      (model) => model.provider === config.provider && model.modelId === modelId,
    )
    const capabilities = model?.capabilities ?? adapter.capabilities
    if (
      capabilities.questionTypes.some(
        (type) => !adapter.capabilities.questionTypes.includes(type),
      ) ||
      (capabilities.booleanProbability && !adapter.capabilities.booleanProbability)
    )
      throw remoteFailure('unsupported_capability')
    const approvalPolicy = model?.approvalPolicyId ? { policyId: model.approvalPolicyId } : null
    return {
      adapter,
      modelId,
      approvalPolicy,
      capabilities,
      endpoint: resolveDecisionEndpoint(config.baseUrl || entry.defaultBaseUrl, entry),
    }
  }
  return Object.freeze({
    providers: catalog,
    provider,
    resolve,
    /**
     * @param {RemoteConfig} config
     * @param {import('./decision-contract.mjs').ModelRequest} input
     * @param {DecisionContext} [context]
     */
    async decide(config, input, context = {}) {
      if (context.signal?.aborted) throw remoteFailure('aborted')
      const selected = resolve(config)
      if (!config.apiKey || !selected.modelId) throw remoteFailure('config_missing')
      if (
        Object.values(input.questions).some(
          (question) => !selected.capabilities.questionTypes.includes(question.type),
        )
      )
        throw remoteFailure('unsupported_capability')
      const timeoutMs = context.timeoutMs ?? 120_000
      if (!Number.isInteger(timeoutMs) || timeoutMs <= 0 || timeoutMs > 2_147_483_647)
        throw remoteFailure('invalid')
      const deadline = new AbortController()
      const signal = context.signal
        ? AbortSignal.any([context.signal, deadline.signal])
        : deadline.signal
      const timer = setTimeout(() => deadline.abort(), timeoutMs)
      try {
        const raw = await selected.adapter.decide(
          { endpoint: selected.endpoint, apiKey: config.apiKey, modelId: selected.modelId },
          input,
          { ...context, signal, timeoutMs },
        )
        if (context.signal?.aborted) throw remoteFailure('aborted')
        if (deadline.signal.aborted)
          throw remoteFailure('timeout', { statusCode: 502, retryable: true })
        const result = validateModelResult(raw, input)
        // 不允许未声明概率能力的适配器通过返回 0/1 绕过能力约束。
        if (
          !selected.capabilities.booleanProbability &&
          Object.values(result.answers).some(
            (answer) => answer.type === 'boolean' && answer.probabilityTrue !== null,
          )
        )
          throw remoteFailure('bad_response', { statusCode: 502 })
        return result
      } catch (error) {
        if (context.signal?.aborted) throw remoteFailure('aborted')
        if (deadline.signal.aborted)
          throw remoteFailure('timeout', { statusCode: 502, retryable: true })
        // 领域错误只保留稳定码，不传播新 SDK 的任意异常文本或 cause。
        if (error instanceof DecisionError)
          throw remoteFailure(error.code, {
            statusCode: error.statusCode,
            retryable: error.retryable,
          })
        throw remoteFailure('bad_response', { statusCode: 502 })
      } finally {
        clearTimeout(timer)
      }
    },
  })
}
