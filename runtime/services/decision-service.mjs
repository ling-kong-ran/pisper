// 决策服务：统一对外提供「状态 + 类型化问题 → 类型化答案 + 概率」能力。
// 远端后端：TypeSafe 官方 Jev API / OpenRouter decisions 接口 / 自定义中转
// （协议实现见 decision-remote-client.mjs）。
// 配置持久化在 <dataDir>/decisions/config.json（0600，API 密钥不回传）。

import { redactSecretValue } from '../security/secret-redaction.mjs'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import {
  DecisionError,
  REMOTE_PROVIDERS,
  callRemoteDecisions,
  estimateTokens,
  normalizeDecideInput,
} from './decision-remote-client.mjs'

/** @typedef {import('./decision-remote-client.mjs').RemoteConfig} RemoteConfig */

/**
 * @typedef {{ version: number, remote: RemoteConfig,
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
 * @param {Record<string, unknown>} [raw]
 * @returns {RemoteConfig}
 */
function normalizeRemoteConfig(raw = {}) {
  const provider =
    typeof raw.provider === 'string' && REMOTE_PROVIDERS[raw.provider]
      ? raw.provider
      : DEFAULT_CONFIG.remote.provider
  const preset = /** @type {{ defaultModelId: string }} */ (REMOTE_PROVIDERS[provider])
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

export class DecisionService {
  /**
   * @param {{ dataDir: string, fetchImpl?: typeof fetch }} options
   * fetchImpl 用于测试注入；生产环境走全局 fetch。
   */
  constructor({ dataDir, fetchImpl }) {
    if (!dataDir) throw new Error('DecisionService requires dataDir.')
    this.dir = join(dataDir, 'decisions')
    this.configPath = join(this.dir, 'config.json')
    this.fetchImpl = fetchImpl
    /** @type {Record<string, unknown> | null} */
    this.storedConfig = null
    this.disposed = false
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
    return {
      version: CONFIG_VERSION,
      remote: normalizeRemoteConfig(isPlainObject(stored.remote) ? stored.remote : {}),
      delegate: normalizeDelegateConfig(storedDelegate),
    }
  }

  /** 对外配置视图：API 密钥只回传 hasKey，绝不回传明文。 */
  publicConfig() {
    const config = this.effectiveConfig()
    const preset = /** @type {{ defaultBaseUrl: string }} */ (
      REMOTE_PROVIDERS[config.remote.provider]
    )
    return {
      remote: {
        provider: config.remote.provider,
        baseUrl: config.remote.baseUrl || preset.defaultBaseUrl,
        modelId: config.remote.modelId,
        hasKey: Boolean(config.remote.apiKey),
      },
      delegate: config.delegate,
    }
  }

  /**
   * @param {unknown} patch
   */
  async updateConfig(patch) {
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
      const remote = normalizeRemoteConfig(merged)
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
    }
    await mkdir(this.dir, { recursive: true })
    // 配置文件含 API 密钥：原子写入并限制为仅当前用户可读写。
    await writeJsonAtomic(this.configPath, current, { mode: 0o600 })
    this.storedConfig = /** @type {Record<string, unknown>} */ (/** @type {unknown} */ (current))
    return this.publicConfig()
  }

  /**
   * 统一决策入口：校验输入后调用远端 Jev API。
   * @param {unknown} input
   * @param {{ signal?: AbortSignal }} [options]
   */
  async decide(input, { signal } = {}) {
    this.assertAlive()
    const normalized = normalizeDecideInput(input)
    const config = this.effectiveConfig()
    const result = await callRemoteDecisions(config.remote, normalized, {
      signal,
      fetchImpl: this.fetchImpl,
    })
    return { backend: 'remote', ...result }
  }

  /**
   * 连通性测试：跑一个最小 noul 判断。
   * @param {{ signal?: AbortSignal }} [options]
   */
  async testConnection({ signal } = {}) {
    this.assertAlive()
    const config = this.effectiveConfig()
    const result = await callRemoteDecisions(
      config.remote,
      {
        state: 'The setup works.',
        questions: { ping: { type: 'noul', instructions: 'Is this a connectivity test?' } },
      },
      { signal, fetchImpl: this.fetchImpl },
    )
    return { backend: 'remote', ok: true, model: result.model, usage: result.usage }
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
    return config.delegate.verifyActions && Boolean(config.remote.apiKey)
  }

  /**
   * 动作后验证：给定动作预期与动作后的 UI 状态文本，判断是否达成预期。
   * 输入超限等错误直接上抛（不截断），由调用方降级为「验证不可用」。
   * @param {{ expectation: string, outcomeText: string }} input
   * @param {{ signal?: AbortSignal }} [options]
   * @returns {Promise<{ passed: boolean, probability: number }>}
   */
  async verifyActionOutcome({ expectation, outcomeText }, { signal } = {}) {
    const result = await this.decide(
      {
        state: `UI state after the action:\n${String(outcomeText)}`,
        questions: [
          {
            id: 'met',
            type: 'noul',
            instructions: `Does the UI state above satisfy this expectation: "${String(expectation)}"?`,
            criteria: {
              true: 'The expectation is clearly satisfied by the observed UI state',
              false: 'The expectation is not satisfied, or the state gives no evidence for it',
            },
          },
        ],
      },
      { signal },
    )
    const answer = result.answers?.met
    const probability =
      answer && 'noul' in answer && typeof answer.noul === 'number' ? answer.noul : null
    if (probability === null) throw new DecisionError('bad_response', '决策模型未返回有效判断。')
    return { passed: probability >= 0.5, probability }
  }

  /**
   * 工具调用审批委派判断：把待审批操作描述为 state，用 noul 询问
   * 「是否可以不再询问用户直接批准」。模型只能授予便利：
   * 置信度达标返回 approve，其余一律返回 ask，由调用方继承会话权限模式。
   * @param {{ toolName: string, risk?: string, reason?: string, args?: unknown }} call
   * @param {{ signal?: AbortSignal }} [options]
   * @returns {Promise<{ verdict: 'approve' | 'ask', probability?: number }>}
   */
  async judgeToolCall(call, { signal } = {}) {
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
    const result = await this.decide(
      {
        state,
        questions: [
          {
            id: 'approve',
            type: 'noul',
            instructions:
              'Is approving this tool call safe enough to proceed without asking the user, in the context of routine coding-agent work?',
            criteria: {
              true: 'Routine, reversible, or clearly scoped operation consistent with normal coding-agent tasks',
              false:
                'Potentially destructive, irreversible, wide-ranging, data-exfiltrating, or otherwise risky operation',
            },
          },
        ],
      },
      { signal },
    )
    const approveAnswer = result.answers?.approve
    const probability =
      approveAnswer && 'noul' in approveAnswer && typeof approveAnswer.noul === 'number'
        ? approveAnswer.noul
        : null
    if (probability === null) return { verdict: 'ask' }
    const { allowThreshold } = this.effectiveConfig().delegate
    if (probability >= allowThreshold) return { verdict: 'approve', probability }
    return { verdict: 'ask', probability }
  }

  assertAlive() {
    if (this.disposed) {
      throw new DecisionError('disposed', '决策服务已停止。', { statusCode: 503 })
    }
  }

  async dispose() {
    this.disposed = true
  }
}
