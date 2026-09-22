import { remoteFailure } from './decision-errors.mjs'
/** @typedef {{answers: Record<string, DecisionAnswer>, model: string | null, usage: {inputTokens: number, costUsd: number | null}}} RemoteDecisionResult */

/** @typedef {{ type: 'noul', noul: number } | { type: 'choice', choice: string, probabilities: Record<string, number>, confidence: number | null } | { type: 'score', score: number, legend: Record<string, unknown> | null, probabilities: Record<string, number>, confidence: number | null }} DecisionAnswer */

/** @param {unknown} value @returns {value is Record<string, unknown>} */
function record(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}
/** @returns {never} */
function invalid() {
  throw remoteFailure('bad_response', { statusCode: 502 })
}
/** @param {unknown} value */
function probability(value) {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0 || value > 1) invalid()
  return value
}
/** @param {unknown} value @param {string[] | undefined} labels */
function probabilities(value, labels) {
  if (value === undefined) return {}
  if (!record(value)) invalid()
  return Object.fromEntries(
    Object.entries(value).map(([key, value]) => {
      if (labels && !labels.includes(key)) invalid()
      return [key, probability(value)]
    }),
  )
}

/**
 * SDK 的泛型只提供静态提示；远端答案仍须按本次问题验证，异常概率不得被修正成批准。
 * @param {unknown} payload
 * @param {Record<string, import('./decision-contract.mjs').PublicQuestion>} [questions]
 * @returns {RemoteDecisionResult}
 */
export function normalizeRemoteResponse(payload, questions) {
  if (!record(payload) || !record(payload.answers)) invalid()
  const rawAnswers = payload.answers
  if (
    questions &&
    (Object.keys(questions).length !== Object.keys(rawAnswers).length ||
      Object.keys(questions).some((id) => !Object.hasOwn(rawAnswers, id)))
  )
    invalid()
  /** @type {Record<string, DecisionAnswer>} */
  const answers = {}
  for (const [id, answer] of Object.entries(rawAnswers)) {
    if (!record(answer)) invalid()
    const kinds = ['noul', 'choice', 'score'].filter((kind) => Object.hasOwn(answer, kind))
    if (kinds.length !== 1) invalid()
    const kind = kinds[0]
    const question = questions?.[id]
    if ((answer.type !== undefined && answer.type !== kind) || (question && question.type !== kind))
      invalid()
    if (kind === 'noul') {
      answers[id] = { type: 'noul', noul: probability(answer.noul) }
      continue
    }
    const confidence = answer.confidence == null ? null : probability(answer.confidence)
    if (kind === 'choice') {
      if (
        typeof answer.choice !== 'string' ||
        (question?.type === 'choice' && !question.options.includes(answer.choice))
      )
        invalid()
      answers[id] = {
        type: 'choice',
        choice: answer.choice,
        confidence,
        probabilities: probabilities(
          answer.probabilities,
          question?.type === 'choice' ? question.options : undefined,
        ),
      }
    } else {
      if (
        typeof answer.score !== 'number' ||
        !Number.isFinite(answer.score) ||
        answer.score < 0 ||
        (question?.type === 'score' && answer.score > question.options.length - 1)
      )
        invalid()
      if (answer.legend != null && !record(answer.legend)) invalid()
      answers[id] = {
        type: 'score',
        score: answer.score,
        confidence,
        legend: record(answer.legend) ? answer.legend : null,
        probabilities: probabilities(
          answer.probabilities,
          question?.type === 'score'
            ? question.options.map((_, index) => String(index))
            : undefined,
        ),
      }
    }
  }
  const usage = record(payload.usage) ? payload.usage : {}
  const inputTokens = Number(usage.input_tokens ?? usage.inputTokens ?? 0)
  if (!Number.isSafeInteger(inputTokens) || inputTokens < 0) invalid()
  if (
    usage.cost != null &&
    (typeof usage.cost !== 'number' || !Number.isFinite(usage.cost) || usage.cost < 0)
  )
    invalid()
  return {
    answers,
    model: typeof payload.model === 'string' ? payload.model : null,
    usage: { inputTokens, costUsd: typeof usage.cost === 'number' ? usage.cost : null },
  }
}
