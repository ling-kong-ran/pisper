// 决策领域输入边界；公共接口与协议适配器分别转换自身问题名称。
import { DecisionError, remoteFailure } from './decision-errors.mjs'

/** @typedef {{ type: 'boolean', instructions: string, criteria?: { true?: string, false?: string } }} BooleanQuestion */
/** @typedef {{ type: 'choice', instructions: string, options: string[] }} ChoiceQuestion */
/** @typedef {{ type: 'score', instructions: string, options: string[] }} ScoreQuestion */
/** @typedef {BooleanQuestion | ChoiceQuestion | ScoreQuestion} DecisionQuestionBody */
/** @typedef {DecisionQuestionBody & { id: string }} DecisionQuestion */

/**
 * 粗估 token：中文按 1.05 token/字，其余按 0.5 token/字符。
 * 供界面估算与适配器预检使用，不参与计费或承诺模型上下文容量。
 * 具体协议的上下文上限由适配器负责。
 */
const CJK_RE = /[\u2e80-\u9fff\uf900-\ufaff\uff00-\uffef\u3000-\u303f]/

/**
 * @param {string} text
 */
export function estimateTokens(text) {
  let cjk = 0
  let other = 0
  for (const ch of String(text)) {
    if (CJK_RE.test(ch)) cjk += 1
    else other += 1
  }
  return Math.ceil(cjk * 1.05 + other * 0.5)
}

export const MAX_STATE_CHARS = 200_000
export const MAX_QUESTIONS = 32
export const MAX_INSTRUCTIONS_CHARS = 2_000
export const MAX_OPTION_LABEL_CHARS = 200
export const MAX_CHOICE_OPTIONS = 255
export const MIN_SCORE_LEVELS = 2
export const MAX_SCORE_LEVELS = 10

const QUESTION_ID_RE = /^[A-Za-z][A-Za-z0-9_-]{0,63}$/

/**
 * @param {unknown} value
 * @returns {value is Record<string, unknown>}
 */
function isPlainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

/**
 * 校验并归一化一次 decide 请求。questions 以数组传入（可带 id），
 * 返回按 id 键控的领域问题；这里的数量和长度是应用入口上限。
 * @param {unknown} input
 * @returns {{ state: string, questions: Record<string, DecisionQuestionBody> }}
 */
export function normalizeModelInput(input) {
  if (!isPlainObject(input)) {
    throw new DecisionError('invalid', '请求必须是 JSON 对象。')
  }
  const rawState = input.state
  /** @type {string} */
  let state
  if (typeof rawState === 'string') {
    state = rawState
  } else {
    if (rawState === undefined || rawState === null) {
      throw new DecisionError('invalid', 'state 不能为空。')
    }
    try {
      state = JSON.stringify(rawState)
    } catch {
      throw new DecisionError('invalid', 'state 无法序列化为 JSON。')
    }
  }
  if (typeof state !== 'string' || !state.trim())
    throw new DecisionError('invalid', 'state 不能为空。')
  if (state.length > MAX_STATE_CHARS) {
    throw remoteFailure('state_too_large')
  }
  const rawQuestions = input.questions
  if (!Array.isArray(rawQuestions) || rawQuestions.length === 0) {
    throw new DecisionError('invalid', 'questions 必须是非空数组。')
  }
  if (rawQuestions.length > MAX_QUESTIONS) {
    throw new DecisionError('invalid', `一次最多 ${MAX_QUESTIONS} 个问题。`)
  }

  /** @type {Record<string, DecisionQuestionBody>} */
  const normalized = {}
  const usedIds = new Set()
  rawQuestions.forEach((rawQuestion, index) => {
    if (!isPlainObject(rawQuestion)) {
      throw new DecisionError('invalid', `第 ${index + 1} 个问题不是对象。`)
    }
    const id = typeof rawQuestion.id === 'string' && rawQuestion.id ? rawQuestion.id : `q${index}`
    if (!QUESTION_ID_RE.test(id)) {
      throw new DecisionError(
        'invalid',
        `问题 id「${id}」只能包含字母、数字、下划线和连字符，且以字母开头。`,
      )
    }
    if (usedIds.has(id)) throw new DecisionError('invalid', `问题 id「${id}」重复。`)
    usedIds.add(id)

    const instructions =
      typeof rawQuestion.instructions === 'string' ? rawQuestion.instructions.trim() : ''
    if (!instructions) throw new DecisionError('invalid', `问题「${id}」缺少 instructions。`)
    if (instructions.length > MAX_INSTRUCTIONS_CHARS) {
      throw new DecisionError('invalid', `问题「${id}」的 instructions 过长。`)
    }

    if (rawQuestion.type === 'boolean') {
      /** @type {BooleanQuestion} */
      const entry = { type: 'boolean', instructions }
      if (isPlainObject(rawQuestion.criteria)) {
        /** @type {{ true?: string, false?: string }} */
        const criteria = {}
        if (typeof rawQuestion.criteria.true === 'string' && rawQuestion.criteria.true.trim()) {
          criteria.true = rawQuestion.criteria.true.trim()
        }
        if (typeof rawQuestion.criteria.false === 'string' && rawQuestion.criteria.false.trim()) {
          criteria.false = rawQuestion.criteria.false.trim()
        }
        if (Object.keys(criteria).length) entry.criteria = criteria
      }
      normalized[id] = entry
      return
    }

    if (rawQuestion.type === 'choice' || rawQuestion.type === 'score') {
      if (!Array.isArray(rawQuestion.options)) {
        throw new DecisionError('invalid', `问题「${id}」缺少 options 数组。`)
      }
      if (rawQuestion.options.some((option) => typeof option !== 'string')) {
        throw new DecisionError('invalid', `问题「${id}」的选项必须是字符串。`)
      }
      const options = rawQuestion.options.map((option) => String(option).trim())
      if (options.some((option) => !option)) {
        throw new DecisionError('invalid', `问题「${id}」存在空选项。`)
      }
      if (new Set(options).size !== options.length) {
        throw new DecisionError('invalid', `问题「${id}」存在重复选项。`)
      }
      if (options.some((option) => option.length > MAX_OPTION_LABEL_CHARS)) {
        throw new DecisionError('invalid', `问题「${id}」的选项标签过长。`)
      }
      if (rawQuestion.type === 'choice') {
        if (options.length < 2 || options.length > MAX_CHOICE_OPTIONS) {
          throw new DecisionError(
            'invalid',
            `choice 问题「${id}」需要 2–${MAX_CHOICE_OPTIONS} 个选项。`,
          )
        }
        normalized[id] = { type: 'choice', instructions, options }
      } else {
        if (options.length < MIN_SCORE_LEVELS || options.length > MAX_SCORE_LEVELS) {
          throw new DecisionError(
            'invalid',
            `score 问题「${id}」需要 ${MIN_SCORE_LEVELS}–${MAX_SCORE_LEVELS} 个有序档位。`,
          )
        }
        normalized[id] = { type: 'score', instructions, options }
      }
      return
    }

    throw new DecisionError('invalid', `问题「${id}」的 type 必须是 boolean / choice / score。`)
  })

  return { state, questions: normalized }
}
