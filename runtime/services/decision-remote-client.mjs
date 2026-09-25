export { normalizeDecideInput } from './decision-contract.mjs'
export { MAX_REMOTE_ESTIMATED_TOKENS } from './decision-jev-adapter.mjs'
// 兼容旧调用方；新服务使用协议中立的注册表和决策契约。
export { DecisionError, remoteFailure } from './decision-errors.mjs'
export {
  estimateTokens,
  MAX_STATE_CHARS,
  MAX_QUESTIONS,
  MAX_INSTRUCTIONS_CHARS,
  MAX_OPTION_LABEL_CHARS,
  MAX_CHOICE_OPTIONS,
  MIN_SCORE_LEVELS,
  MAX_SCORE_LEVELS,
} from './decision-input.mjs'
export { DECISION_PROVIDER_CATALOG as REMOTE_PROVIDERS } from '../../shared/decision-provider-catalog.mjs'
export {
  toRemoteQuestions,
  assertWithinRemoteLimit,
  remoteEndpoint,
} from './decision-jev-adapter.mjs'
export { normalizeRemoteResponse } from './decision-response.mjs'
import { defaultDecisionRegistry } from './decision-backends.mjs'
import { toModelRequest, toPublicResult } from './decision-contract.mjs'
/** @typedef {import('./decision-contract.mjs').PublicQuestion} DecisionQuestionBody */
/** @typedef {import('./decision-registry.mjs').RemoteConfig} RemoteConfig */
/** @typedef {import('./decision-response.mjs').RemoteDecisionResult} RemoteDecisionResult */
/**
 * @param {RemoteConfig} config
 * @param {{state: string, questions: Record<string, DecisionQuestionBody>}} input
 * @param {import('./decision-registry.mjs').DecisionContext} [context]
 */
export async function callRemoteDecisions(config, input, context = {}) {
  return toPublicResult(
    await defaultDecisionRegistry.decide(config, toModelRequest(input), context),
  )
}
