// 决策服务：统一对外提供「状态 + 类型化问题 → 类型化答案 + 概率」能力。
// 通过注册表选择协议适配器；服务拥有配置、审批规则和在途调用生命周期。
// 配置持久化在 <dataDir>/decisions/config.json（0600，API 密钥不回传）。

import { redactSecretValue } from '../security/secret-redaction.mjs'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { DecisionError, remoteFailure } from './decision-errors.mjs'
import { estimateTokens, normalizeModelInput } from './decision-input.mjs'
import { toModelRequest, toPublicResult, normalizeDecideInput } from './decision-contract.mjs'
import { defaultDecisionRegistry } from './decision-backends.mjs'

/** @typedef {import('./decision-registry.mjs').RemoteConfig} RemoteConfig */
/** @typedef {typeof defaultDecisionRegistry} DecisionRegistry */
/** @typedef {{provider: string, modelId: string, endpoint: string, policyId: string}} ApprovalBinding */
/**
 * @typedef {{ version: number, remote: RemoteConfig, approvalBinding: ApprovalBinding | null,
 *   delegate: { enabled: boolean, allowThreshold: number, verifyActions: boolean } }} DecisionConfig
 */

const CONFIG_VERSION = 1

const DEFAULT_DELEGATE = Object.freeze({
  enabled: false,
  // 置信度足够高才自动批准；其余一律继承会话权限模式（回落人工审批）。
  allowThreshold: 0.9,
  // computer-use 动作后验证：act_ui 的语义预期交给决策模型判断。
  verifyActions: false,
})

const DEFAULT_CONFIG = Object.freeze({
  version: CONFIG_VERSION,
  remote: {
    provider: 'typesafe',
    baseUrl: '',
    modelId: '',
    apiKey: '',
  },
  delegate: DEFAULT_DELEGATE,
})

/**
 * @param {unknown} value
 * @returns {value is Record<string, unknown>}
 */
function isPlainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

/**
 * @param {Record<string, unknown>} raw
 * @param {DecisionRegistry} registry
 * @returns {RemoteConfig}
 */
function normalizeRemoteConfig(raw, registry) {
  const provider = typeof raw.provider === 'string' ? raw.provider : DEFAULT_CONFIG.remote.provider
  const preset = registry.provider(provider)
  const baseUrl = typeof raw.baseUrl === 'string' ? raw.baseUrl.trim() : ''
  const modelId = typeof raw.modelId === 'string' ? raw.modelId.trim() : ''
  const apiKey = typeof raw.apiKey === 'string' ? raw.apiKey.trim() : ''
  return {
    provider,
    baseUrl,
    modelId: modelId || preset.defaultModelId,
    apiKey,
  }
}

/**
 * 归一化委派配置；strict 模式（用户输入）对非法阈值报错，非 strict（读旧配置）回退默认。
 * @param {Record<string, unknown>} [raw]
 * @param {{ strict?: boolean }} [options]
 */
function normalizeDelegateConfig(raw = {}, { strict = false } = {}) {
  const enabled = raw.enabled === true
  const verifyActions = raw.verifyActions === true
  const allow = Number(raw.allowThreshold)
  const allowValid = Number.isFinite(allow) && allow >= 0.5 && allow <= 1
  if (strict && raw.allowThreshold !== undefined && !allowValid) {
    throw new DecisionError('invalid', '批准阈值必须在 0.5–1 之间。')
  }
  const allowThreshold = allowValid ? allow : DEFAULT_DELEGATE.allowThreshold
  return { enabled, allowThreshold, verifyActions }
}

/** @param {unknown} raw @returns {ApprovalBinding | null} */
function readApprovalBinding(raw) {
  if (
    !isPlainObject(raw) ||
    typeof raw.provider !== 'string' ||
    typeof raw.modelId !== 'string' ||
    typeof raw.endpoint !== 'string' ||
    typeof raw.policyId !== 'string'
  )
    return null
  return {
    provider: raw.provider,
    modelId: raw.modelId,
    endpoint: raw.endpoint,
    policyId: raw.policyId,
  }
}
/** @param {ApprovalBinding | null} a @param {ApprovalBinding | null} b */
function sameApprovalBinding(a, b) {
  return (
    a !== null &&
    b !== null &&
    a.provider === b.provider &&
    a.modelId === b.modelId &&
    a.endpoint === b.endpoint &&
    a.policyId === b.policyId
  )
}

export class DecisionService {
  /**
   * @param {{ dataDir: string, fetchImpl?: typeof fetch, registry?: DecisionRegistry }} options
   * fetchImpl 用于测试注入；生产环境走全局 fetch。
   */
  constructor({ dataDir, fetchImpl, registry = defaultDecisionRegistry }) {
    this.registry = registry
    if (!dataDir) throw new Error('DecisionService requires dataDir.')
    this.dir = join(dataDir, 'decisions')
    this.configPath = join(this.dir, 'config.json')
    this.fetchImpl = fetchImpl
    /** @type {Record<string, unknown> | null} */
    this.storedConfig = null
    this.disposed = false
    this.configRevision = 0
    this.configWrites = Promise.resolve()
    this.shutdownController = new AbortController()
    /** @type {Set<Promise<import('./decision-contract.mjs').ModelResult>>} */
    this.inFlight = new Set()
  }

  async init() {
    await mkdir(this.dir, { recursive: true })
    const stored = await readJson(this.configPath, null)
    this.storedConfig = isPlainObject(stored) && stored.version === CONFIG_VERSION ? stored : null
    return this
  }

  /**
   * @returns {DecisionConfig}
   */
  effectiveConfig() {
    const stored = isPlainObject(this.storedConfig) ? this.storedConfig : {}
    const storedDelegate = isPlainObject(stored.delegate) ? stored.delegate : {}
    const remote = normalizeRemoteConfig(
      isPlainObject(stored.remote) ? stored.remote : {},
      this.registry,
    )
    // v1 旧文件首次读取时绑定原供应商与型号；后续保存带出绑定，切换模型不会重新推断。
    const binding = Object.hasOwn(stored, 'approvalBinding')
      ? readApprovalBinding(stored.approvalBinding)
      : this.modelApprovalBinding(remote)
    return {
      version: CONFIG_VERSION,
      remote,
      delegate: normalizeDelegateConfig(storedDelegate),
      approvalBinding: binding,
    }
  }

  /** @param {RemoteConfig} remote @returns {ApprovalBinding | null} */
  modelApprovalBinding(remote) {
    try {
      const selected = this.registry.resolve(remote)
      if (
        !selected.approvalPolicy ||
        !selected.capabilities.booleanProbability ||
        !selected.capabilities.questionTypes.includes('boolean')
      )
        return null
      return {
        provider: remote.provider,
        modelId: selected.modelId,
        endpoint: selected.endpoint,
        policyId: selected.approvalPolicy.policyId,
      }
    } catch {
      return null
    }
  }

  /** @param {DecisionConfig} config @returns {'ready' | 'model_unverified' | 'threshold_required'} */
  approvalStatus(config) {
    const expected = this.modelApprovalBinding(config.remote)
    if (!expected) return 'model_unverified'
    return sameApprovalBinding(expected, config.approvalBinding) ? 'ready' : 'threshold_required'
  }

  /** 对外配置视图：API 密钥只回传 hasKey，绝不回传明文。 */
  publicConfig() {
    const config = this.effectiveConfig()
    const preset = this.registry.provider(config.remote.provider)
    return {
      remote: {
        provider: config.remote.provider,
        baseUrl: config.remote.baseUrl || preset.defaultBaseUrl,
        modelId: config.remote.modelId,
        hasKey: Boolean(config.remote.apiKey),
      },
      delegate: config.delegate,
      approval: { status: this.approvalStatus(config) },
    }
  }

  /**
   * @param {unknown} patch
   */
  async updateConfig(patch) {
    this.assertAlive()
    const pending = this.configWrites.then(() => this.saveConfig(patch))
    // 写入失败由该请求报告；队列继续服务下一次有效更新。
    this.configWrites = pending.then(
      () => {},
      () => {},
    )
    return pending
  }

  /** @param {unknown} patch */
  async saveConfig(patch) {
    this.assertAlive()
    if (!isPlainObject(patch)) throw new DecisionError('invalid', '配置必须是 JSON 对象。')
    for (const key of Object.keys(patch)) {
      if (key !== 'remote' && key !== 'delegate') {
        throw new DecisionError('invalid', `未知的配置字段：${key}`)
      }
    }
    const current = this.effectiveConfig()
    if (patch.remote !== undefined) {
      if (!isPlainObject(patch.remote)) {
        throw new DecisionError('invalid', 'remote 配置必须是对象。')
      }
      const patchRemote = patch.remote
      const merged = { ...current.remote, ...patchRemote }
      // 切换 provider 且未显式给 modelId 时，重置为新 provider 的默认模型，
      // 避免把上一家的模型 ID 发给新接口。
      if (
        patchRemote.provider !== undefined &&
        patchRemote.provider !== current.remote.provider &&
        patchRemote.modelId === undefined
      ) {
        merged.modelId = ''
      }
      const remote = normalizeRemoteConfig(merged, this.registry)
      // 空字符串表示「保持现有密钥」，避免前端回显时清空；null 表示清除。
      if (patchRemote.apiKey === '') remote.apiKey = current.remote.apiKey
      if (patchRemote.apiKey === null) remote.apiKey = ''
      current.remote = remote
    }
    if (patch.delegate !== undefined) {
      if (!isPlainObject(patch.delegate)) {
        throw new DecisionError('invalid', 'delegate 配置必须是对象。')
      }
      current.delegate = normalizeDelegateConfig(
        { ...current.delegate, ...patch.delegate },
        { strict: true },
      )
      if (patch.delegate.allowThreshold !== undefined)
        current.approvalBinding = this.modelApprovalBinding(current.remote)
    }
    await mkdir(this.dir, { recursive: true })
    // 配置文件含 API 密钥：原子写入并限制为仅当前用户可读写。
    await writeJsonAtomic(this.configPath, current, { mode: 0o600 })
    this.storedConfig = /** @type {Record<string, unknown>} */ (/** @type {unknown} */ (current))
    this.configRevision += 1
    return this.publicConfig()
  }

  /**
   * v1 决策入口：校验旧协议后转换到领域契约。
   * @param {unknown} input
   * @param {{ signal?: AbortSignal }} [options]
   */
  async decide(input, { signal } = {}) {
    this.assertAlive()
    const normalized = normalizeDecideInput(input)
    const result = await this.requestModel(toModelRequest(normalized), signal)
    return { backend: 'remote', ...toPublicResult(result) }
  }

  /**
   * 连通性测试：跑一个最小 noul 判断。
   * @param {{ signal?: AbortSignal }} [options]
   */
  async testConnection({ signal } = {}) {
    this.assertAlive()
    const result = await this.requestModel(
      {
        state: 'The setup works.',
        questions: { ping: this.connectionQuestion() },
      },
      signal,
    )
    return { backend: 'remote', ok: true, model: result.model, usage: result.usage }
  }

  /** @returns {import('./decision-contract.mjs').ModelQuestion} */
  connectionQuestion() {
    const { capabilities } = this.registry.resolve(this.effectiveConfig().remote)
    if (capabilities.questionTypes.includes('boolean'))
      return { type: 'boolean', instructions: 'Is this a connectivity test?' }
    if (capabilities.questionTypes.includes('choice'))
      return {
        type: 'choice',
        instructions: 'Select the purpose.',
        options: ['connectivity test', 'other'],
      }
    if (capabilities.questionTypes.includes('score'))
      return {
        type: 'score',
        instructions: 'Rate connectivity.',
        options: ['not connected', 'connected'],
      }
    throw remoteFailure('unsupported_capability')
  }

  /**
   * 生命周期只由服务持有；停止时取消并等待所有在途请求，不依赖页面是否仍挂载。
   * @param {import('./decision-contract.mjs').ModelRequest} input
   * @param {AbortSignal} [signal]
   */
  async requestModel(input, signal, remote = this.effectiveConfig().remote) {
    this.assertAlive()
    const normalized = normalizeModelInput({
      state: input.state,
      questions: Object.entries(input.questions).map(([id, question]) => ({ ...question, id })),
    })
    const request = this.registry.decide(remote, normalized, {
      signal: signal
        ? AbortSignal.any([signal, this.shutdownController.signal])
        : this.shutdownController.signal,
      fetchImpl: this.fetchImpl,
    })
    this.inFlight.add(request)
    try {
      return await request
    } finally {
      this.inFlight.delete(request)
    }
  }

  /**
   * 本地估算提示：返回 state 的粗估 token，供前端展示上下文占用。
   * @param {unknown} input
   */
  estimate(input) {
    const normalized = normalizeDecideInput(input)
    return { estimatedStateTokens: estimateTokens(normalized.state) }
  }

  /** 委派开关是否打开（不检查密钥；缺密钥时调用方走 decide 失败后回落）。 */
  delegationEnabled() {
    return this.effectiveConfig().delegate.enabled
  }

  /**
   * 动作后验证是否可用：开关打开且已配置远端密钥。
   * 供 computer-use-verify 扩展决定是否执行验证调用。
   */
  actionVerificationEnabled() {
    const config = this.effectiveConfig()
    if (!config.delegate.verifyActions || !config.remote.apiKey) return false
    try {
      const { capabilities } = this.registry.resolve(config.remote)
      return capabilities.questionTypes.includes('boolean') && capabilities.booleanProbability
    } catch {
      return false
    }
  }

  /**
   * 动作后验证：给定动作预期与动作后的 UI 状态文本，判断是否达成预期。
   * 输入超限等错误直接上抛（不截断），由调用方降级为「验证不可用」。
   * @param {{ expectation: string, outcomeText: string }} input
   * @param {{ signal?: AbortSignal }} [options]
   * @returns {Promise<{ passed: boolean, probability: number }>}
   */
  async verifyActionOutcome({ expectation, outcomeText }, { signal } = {}) {
    const result = await this.requestModel(
      {
        state: `UI state after the action:\n${String(outcomeText)}`,
        questions: {
          met: {
            type: 'boolean',
            instructions: `Does the UI state above satisfy this expectation: "${String(expectation)}"?`,
            criteria: {
              true: 'The expectation is clearly satisfied by the observed UI state',
              false: 'The expectation is not satisfied, or the state gives no evidence for it',
            },
          },
        },
      },
      signal,
    )
    const answer = result.answers?.met
    const probability = answer?.type === 'boolean' ? answer.probabilityTrue : null
    if (probability === null) throw new DecisionError('bad_response', '决策模型未返回有效判断。')
    return { passed: probability >= 0.5, probability }
  }

  /**
   * 工具调用审批委派判断：把待审批操作描述为 state，用是非判断询问
   * 「是否可以不再询问用户直接批准」。模型只能授予便利：
   * 置信度达标返回 approve，其余一律返回 ask，由调用方继承会话权限模式。
   * @param {{ toolName: string, risk?: string, reason?: string, args?: unknown }} call
   * @param {{ signal?: AbortSignal }} [options]
   * @returns {Promise<{ verdict: 'approve' | 'ask', probability?: number }>}
   */
  async judgeToolCall(call, { signal } = {}) {
    this.assertAlive()
    const config = this.effectiveConfig()
    const revision = this.configRevision
    if (!config.delegate.enabled || this.approvalStatus(config) !== 'ready')
      return { verdict: 'ask' }
    // 不做任何截断：完整参数交给入口限长校验，超限直接驳回（state_too_large），
    // 调用方（权限服务）捕获后回落人工审批。
    const state = JSON.stringify({
      tool: String(call.toolName || ''),
      risk: String(call.risk || 'unknown'),
      approvalReason: String(call.reason || ''),
      arguments: call.args ?? null,
    })
    // 不发送脱敏后再自动批准：隐藏参数会改变风险语义，敏感调用直接交还人工。
    if (JSON.stringify(redactSecretValue(JSON.parse(state))) !== state) return { verdict: 'ask' }
    const result = await this.requestModel(
      {
        state,
        questions: {
          approve: {
            type: 'boolean',
            instructions:
              'Is approving this tool call safe enough to proceed without asking the user, in the context of routine coding-agent work?',
            criteria: {
              true: 'Routine, reversible, or clearly scoped operation consistent with normal coding-agent tasks',
              false:
                'Potentially destructive, irreversible, wide-ranging, data-exfiltrating, or otherwise risky operation',
            },
          },
        },
      },
      signal,
      config.remote,
    )
    const approveAnswer = result.answers?.approve
    const probability = approveAnswer?.type === 'boolean' ? approveAnswer.probabilityTrue : null
    if (probability === null) return { verdict: 'ask' }
    const current = this.effectiveConfig()
    if (
      revision !== this.configRevision ||
      this.approvalStatus(current) !== 'ready' ||
      !sameApprovalBinding(config.approvalBinding, current.approvalBinding) ||
      config.delegate.allowThreshold !== current.delegate.allowThreshold ||
      config.delegate.enabled !== current.delegate.enabled
    )
      return { verdict: 'ask' }
    if (probability >= config.delegate.allowThreshold) return { verdict: 'approve', probability }
    return { verdict: 'ask', probability }
  }

  assertAlive() {
    if (this.disposed) {
      throw new DecisionError('disposed', '决策服务已停止。', { statusCode: 503 })
    }
  }

  async dispose() {
    this.disposed = true
    this.shutdownController.abort()
    await Promise.allSettled([...this.inFlight])
    await this.configWrites
  }
}
