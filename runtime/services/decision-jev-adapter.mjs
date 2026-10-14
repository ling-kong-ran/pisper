// TypeSafe decisions 协议适配；供应商与型号由注册表选择，不拥有审批规则或服务状态。
import { resolveDecisionEndpoint } from './decision-registry.mjs'
import { remoteFailure } from './decision-errors.mjs'
import { estimateTokens } from './decision-input.mjs'
import { requestDecisions } from './decision-transport.mjs'
import { normalizeRemoteResponse } from './decision-response.mjs'
import { toModelResult, toPublicQuestions } from './decision-contract.mjs'
import { DECISION_PROVIDER_CATALOG } from '../../shared/decision-provider-catalog.mjs'
// Jev 协议限制：32k 上下文预留估算余量，不向其他协议传播。
export const MAX_REMOTE_ESTIMATED_TOKENS = 28_000
/** @typedef {import('../../shared/decision-provider-catalog.mjs').DecisionProviderDefinition} RemoteProviderPreset */
/** @type {Readonly<Record<string, RemoteProviderPreset>>} */
const REMOTE_PROVIDERS = DECISION_PROVIDER_CATALOG
/**
 * 归一化后的问题定义转远端协议的 questions 映射。
 * @param {Record<string, import('./decision-contract.mjs').PublicQuestion>} questions
 * @returns {import('@typesafe-ai/sdk').Questions}
 */
export function toRemoteQuestions(questions) {
  /** @type {import('@typesafe-ai/sdk').Questions} */
  const remote = {}
  for (const [id, question] of Object.entries(questions)) {
    if (question.type === 'noul') {
      remote[id] = {
        type: 'noul',
        instructions: question.instructions,
        ...(question.criteria ? { criteria: question.criteria } : {}),
      }
    } else if (question.type === 'choice') {
      // 官方 choice 的 criteria 是 map<string, string|null>，null 表示该项无需说明。
      remote[id] = {
        type: 'choice',
        instructions: question.instructions,
        criteria: Object.fromEntries(question.options.map((option) => [option, null])),
      }
    } else {
      // score 的 criteria 是至少两个档位的元组；与 SDK 类型及边界校验一致。
      const [first, second, ...rest] = question.options
      if (first === undefined || second === undefined) throw remoteFailure('invalid')
      remote[id] = {
        type: 'score',
        instructions: question.instructions,
        criteria: [first, second, ...rest],
      }
    }
  }
  return remote
}

/**
 * 本地拦截：state + 最长问题定义的超限估算，避免用真实请求撞 422。
 * @param {string} state
 * @param {Record<string, import('./decision-contract.mjs').PublicQuestion>} questions
 */
export function assertWithinRemoteLimit(state, questions) {
  let longest = 0
  for (const question of Object.values(questions)) {
    const optionsText = 'options' in question && question.options ? question.options.join('') : ''
    const criteriaText =
      'criteria' in question && question.criteria ? Object.values(question.criteria).join('') : ''
    longest = Math.max(longest, estimateTokens(question.instructions + optionsText + criteriaText))
  }
  if (estimateTokens(state) + longest > MAX_REMOTE_ESTIMATED_TOKENS) {
    throw remoteFailure('state_too_large', { statusCode: 413 })
  }
}

/**
 * @param {{ provider: string, baseUrl?: string }} config
 */
export function remoteEndpoint(config) {
  if (!Object.hasOwn(REMOTE_PROVIDERS, config.provider)) throw remoteFailure('unsupported_provider')
  const preset = REMOTE_PROVIDERS[config.provider]
  return resolveDecisionEndpoint(config.baseUrl || preset.defaultBaseUrl, preset)
}

/** @type {import('./decision-registry.mjs').DecisionAdapter} */
export const jevDecisionAdapter = Object.freeze({
  id: 'typesafe-decisions',
  capabilities: Object.freeze({
    questionTypes: Object.freeze(['boolean', 'choice', 'score']),
    booleanProbability: true,
  }),
  async decide(config, input, context) {
    const apiKey = config.apiKey.trim()
    if (!apiKey || !config.modelId.trim()) throw remoteFailure('config_missing')
    const questions = toPublicQuestions(input.questions)
    assertWithinRemoteLimit(input.state, questions)
    const payload = await requestDecisions(
      {
        endpoint: config.endpoint,
        apiKey,
        modelId: config.modelId,
        state: input.state,
        questions: toRemoteQuestions(questions),
      },
      context,
    )
    return toModelResult(normalizeRemoteResponse(payload, questions))
  },
})
