// 决策领域契约与 v1 兼容转换。SDK 字段只在适配器出现；审批使用 boolean 的概率语义。
import { normalizeModelInput } from './decision-input.mjs'
import { remoteFailure } from './decision-errors.mjs'

/** @typedef {{type: 'noul', instructions: string, criteria?: {true?: string, false?: string}} | {type: 'choice', instructions: string, options: string[]} | {type: 'score', instructions: string, options: string[]}} PublicQuestion */
/** @typedef {{type: 'boolean', instructions: string, criteria?: {true?: string, false?: string}} | {type: 'choice' | 'score', instructions: string, options: string[]}} ModelQuestion */
/** @typedef {{state: string, questions: Record<string, ModelQuestion>}} ModelRequest */
/** @typedef {{type: 'boolean', value: boolean, probabilityTrue: number | null} | {type: 'choice', choice: string, probabilities: Record<string, number>, confidence: number | null} | {type: 'score', score: number, legend: Record<string, unknown> | null, probabilities: Record<string, number>, confidence: number | null}} ModelAnswer */
/** @typedef {{answers: Record<string, ModelAnswer>, model: string | null, usage: {inputTokens: number, costUsd: number | null}}} ModelResult */

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
/** @param {unknown} value @param {string[]} labels */
function distribution(value, labels) {
  if (!record(value)) invalid()
  return Object.fromEntries(
    Object.entries(value).map(([key, value]) => {
      if (!labels.includes(key)) invalid()
      return [key, probability(value)]
    }),
  )
}

/**
 * 所有适配器返回 unknown；统一校验避免新增实现绕开答案匹配与概率检查。
 * 不把布尔答案伪装成 0/1 概率，也不根据供应商的“confidence”推断概率。
 * @param {unknown} payload
 * @param {ModelRequest} input
 * @returns {ModelResult}
 */
export function validateModelResult(payload, input) {
  if (!record(payload) || !record(payload.answers) || !record(payload.usage)) invalid()
  const raw = payload.answers
  if (
    Object.keys(raw).length !== Object.keys(input.questions).length ||
    Object.keys(input.questions).some((id) => !Object.hasOwn(raw, id))
  )
    invalid()
  /** @type {Record<string, ModelAnswer>} */
  const answers = {}
  for (const [id, question] of Object.entries(input.questions)) {
    const answer = raw[id]
    if (!record(answer) || answer.type !== question.type) invalid()
    if (question.type === 'boolean') {
      if (typeof answer.value !== 'boolean') invalid()
      const p = answer.probabilityTrue === null ? null : probability(answer.probabilityTrue)
      if (p !== null && answer.value !== p >= 0.5) invalid()
      answers[id] = { type: 'boolean', value: answer.value, probabilityTrue: p }
    } else {
      const confidence = answer.confidence === null ? null : probability(answer.confidence)
      if (question.type === 'choice') {
        if (typeof answer.choice !== 'string' || !question.options.includes(answer.choice))
          invalid()
        answers[id] = {
          type: 'choice',
          choice: answer.choice,
          confidence,
          probabilities: distribution(answer.probabilities, question.options),
        }
      } else {
        if (
          typeof answer.score !== 'number' ||
          !Number.isFinite(answer.score) ||
          answer.score < 0 ||
          answer.score > question.options.length - 1
        )
          invalid()
        if (answer.legend !== null && !record(answer.legend)) invalid()
        answers[id] = {
          type: 'score',
          score: answer.score,
          confidence,
          legend: record(answer.legend) ? answer.legend : null,
          probabilities: distribution(
            answer.probabilities,
            question.options.map((_, i) => String(i)),
          ),
        }
      }
    }
  }
  const { inputTokens, costUsd } = payload.usage
  if (typeof inputTokens !== 'number' || !Number.isSafeInteger(inputTokens) || inputTokens < 0)
    invalid()
  if (costUsd !== null && (typeof costUsd !== 'number' || !Number.isFinite(costUsd) || costUsd < 0))
    invalid()
  if (payload.model !== null && typeof payload.model !== 'string') invalid()
  return { answers, model: payload.model, usage: { inputTokens, costUsd } }
}

/** @param {{state: string, questions: Record<string, PublicQuestion>}} input @returns {ModelRequest} */
export function toModelRequest(input) {
  return {
    state: input.state,
    questions: Object.fromEntries(
      Object.entries(input.questions).map(([id, question]) => [
        id,
        question.type === 'noul' ? { ...question, type: 'boolean' } : question,
      ]),
    ),
  }
}

/** @param {Record<string, ModelQuestion>} questions @returns {Record<string, PublicQuestion>} */
export function toPublicQuestions(questions) {
  return Object.fromEntries(
    Object.entries(questions).map(([id, question]) => {
      if (question.type === 'boolean') return [id, { ...question, type: 'noul' }]
      if (question.type === 'choice') return [id, { ...question, type: 'choice' }]
      return [id, { ...question, type: 'score' }]
    }),
  )
}

/** @param {import('./decision-response.mjs').RemoteDecisionResult} result @returns {ModelResult} */
export function toModelResult(result) {
  return {
    ...result,
    answers: Object.fromEntries(
      Object.entries(result.answers).map(([id, answer]) => [
        id,
        answer.type === 'noul'
          ? { type: 'boolean', value: answer.noul >= 0.5, probabilityTrue: answer.noul }
          : answer,
      ]),
    ),
  }
}

/** @param {ModelResult} result @returns {import('./decision-response.mjs').RemoteDecisionResult} */
export function toPublicResult(result) {
  return {
    ...result,
    answers: Object.fromEntries(
      Object.entries(result.answers).map(([id, answer]) => {
        if (answer.type !== 'boolean') return [id, answer]
        if (answer.probabilityTrue === null) throw remoteFailure('unsupported_capability')
        return [id, { type: 'noul', noul: answer.probabilityTrue }]
      }),
    ),
  }
}

/** @param {unknown} input */
export function normalizeDecideInput(input) {
  if (!record(input) || !Array.isArray(input.questions))
    return publicInput(normalizeModelInput(input))
  const questions = input.questions.map((question) => {
    if (!record(question)) return question
    // boolean 是内部名称，v1 只接受原有 noul 名称，避免悄悄扩展公共协议。
    if (question.type === 'boolean') throw remoteFailure('invalid')
    return { ...question, type: question.type === 'noul' ? 'boolean' : question.type }
  })
  return publicInput(normalizeModelInput({ ...input, questions }))
}
/** @param {ModelRequest} input */
function publicInput(input) {
  return { state: input.state, questions: toPublicQuestions(input.questions) }
}
