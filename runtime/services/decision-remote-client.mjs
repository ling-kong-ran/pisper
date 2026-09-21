// Jev 远端决策客户端：封装 TypeSafe 官方接口（POST /v1/systemone）与
// OpenRouter 转发接口（POST {base}/alpha/decisions，注意没有 /v1）。
// 两种协议请求体一致：{ model, state, questions }，响应按问题 id 回填类型化答案。
// 本模块只做协议与校验，配置持久化和本地模型编排在 decision-service.mjs。

/** @typedef {{ type: 'noul', instructions: string, criteria?: { true?: string, false?: string } }} NoulQuestion */
/** @typedef {{ type: 'choice', instructions: string, options: string[] }} ChoiceQuestion */
/** @typedef {{ type: 'score', instructions: string, options: string[] }} ScoreQuestion */
/** @typedef {NoulQuestion | ChoiceQuestion | ScoreQuestion} DecisionQuestionBody */
/** @typedef {DecisionQuestionBody & { id: string }} DecisionQuestion */

/**
 * @typedef {{ defaultBaseUrl: string, path: string, defaultModelId: string }} RemoteProviderPreset
 */

/**
 * @typedef {{ provider: string, baseUrl: string, modelId: string, apiKey: string }} RemoteConfig
 */

/**
 * @typedef {{ answers: Record<string, object>, model: string | null,
 *   usage: { inputTokens: number, costUsd: number | null } }} RemoteDecisionResult
 */

/** @type {Readonly<Record<string, RemoteProviderPreset>>} */
export const REMOTE_PROVIDERS = Object.freeze({
  // TypeSafe 官方接口。文档：https://docs.typesafe.ai（early access，需要控制台发放权限）
  typesafe: Object.freeze({
    defaultBaseUrl: 'https://api.typesafe.ai',
    path: '/v1/systemone',
    defaultModelId: 'jev-1.13.0',
  }),
  // OpenRouter 的 decisions 是独立协议，不在 /v1 之下（/v1/alpha/decisions 会 404）。
  openrouter: Object.freeze({
    defaultBaseUrl: 'https://openrouter.ai/api',
    path: '/alpha/decisions',
    defaultModelId: 'typesafe/jev-1.13',
  }),
  // 自定义/中转：用户自填 baseUrl，协议路径按 openrouter 形态（/alpha/decisions）拼接，
  // 也允许 baseUrl 直接写完整接口地址（以 /systemone 或 /decisions 结尾时原样使用）。
  custom: Object.freeze({
    defaultBaseUrl: '',
    path: '/alpha/decisions',
    defaultModelId: 'typesafe/jev-1.13',
  }),
})

export class DecisionError extends Error {
  /**
   * @param {string} code 稳定机器可读错误码
   * @param {string} message
   * @param {{ statusCode?: number, retryable?: boolean }} [options]
   */
  constructor(code, message, { statusCode = 400, retryable = false } = {}) {
    super(message)
    this.name = 'DecisionError'
    this.code = code
    this.statusCode = statusCode
    this.retryable = retryable
  }
}

/** @type {Record<string, string>} */
const FAILURE_MESSAGES = {
  config_missing: '尚未配置远端 Jev API 密钥或地址。',
  auth: 'Jev API 认证失败，请检查 API 密钥与账户权限。',
  rate_limited: 'Jev API 请求过于频繁，请稍后重试。',
  overloaded: 'Jev API 暂时过载，请稍后重试。',
  state_too_large: '输入超出 Jev 上下文限制（state 与最长问题合计约 32k token）。',
  invalid: 'Jev API 拒绝了请求，请检查问题定义。',
  network: '无法连接 Jev API。',
  timeout: 'Jev API 请求超时。',
  aborted: '请求已取消。',
  bad_response: 'Jev API 返回了无法解析的响应。',
}

/**
 * @param {string} code
 * @param {{ statusCode?: number, retryable?: boolean }} [extra]
 */
export function remoteFailure(code, extra = {}) {
  return new DecisionError(code, FAILURE_MESSAGES[code] ?? FAILURE_MESSAGES.network, extra)
}

/**
 * 粗估 token：中文按 1.05 token/字，其余按 0.5 token/字符。
 * 只用于本地拦截超限输入（官方硬限为 state + 最长问题 ≤ 32k token），不参与计费。
 * 系数偏保守（高估），宁可提前拒绝也不要用真实请求撞 422。
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

/** 官方硬限 32k（state + 最长问题），留约 4k 给估算误差。 */
export const MAX_REMOTE_ESTIMATED_TOKENS = 28_000

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
 * 返回按 id 键控的映射，供远端协议与本地引擎直接使用。
 * @param {unknown} input
 * @returns {{ state: string, questions: Record<string, DecisionQuestionBody> }}
 */
export function normalizeDecideInput(input) {
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
  if (!state.trim()) throw new DecisionError('invalid', 'state 不能为空。')
  if (state.length > MAX_STATE_CHARS) {
    throw new DecisionError(
      'state_too_large',
      /** @type {string} */ (FAILURE_MESSAGES.state_too_large),
    )
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

    if (rawQuestion.type === 'noul') {
      /** @type {NoulQuestion} */
      const entry = { type: 'noul', instructions }
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
      const options = rawQuestion.options.map((option) => String(option ?? '').trim())
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

    throw new DecisionError('invalid', `问题「${id}」的 type 必须是 noul / choice / score。`)
  })

  return { state, questions: normalized }
}

/**
 * 归一化后的问题定义转远端协议的 questions 映射。
 * @param {Record<string, DecisionQuestionBody>} questions
 * @returns {Record<string, object>}
 */
export function toRemoteQuestions(questions) {
  /** @type {Record<string, object>} */
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
      // score 的 criteria 是按档位顺序排列的数组。
      remote[id] = {
        type: 'score',
        instructions: question.instructions,
        criteria: question.options,
      }
    }
  }
  return remote
}

/**
 * 本地拦截：state + 最长问题定义的超限估算，避免用真实请求撞 422。
 * @param {string} state
 * @param {Record<string, DecisionQuestionBody>} questions
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
  const provider = REMOTE_PROVIDERS[config.provider] ? config.provider : 'custom'
  const preset = /** @type {RemoteProviderPreset} */ (REMOTE_PROVIDERS[provider])
  const baseUrl = String(config.baseUrl || preset.defaultBaseUrl || '').replace(/\/+$/, '')
  if (!baseUrl || !/^https:\/\//i.test(baseUrl)) {
    throw remoteFailure('config_missing')
  }
  // 允许 baseUrl 直接写完整接口地址，便于中转站透传。
  if (/\/(systemone|decisions)$/i.test(baseUrl)) return baseUrl
  return `${baseUrl}${preset.path}`
}

/**
 * @param {number} ms
 * @param {AbortSignal | undefined} signal
 */
function sleep(ms, signal) {
  return /** @type {Promise<void>} */ (
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => resolve(), ms)
      const onAbort = () => {
        clearTimeout(timer)
        reject(remoteFailure('aborted', { retryable: false }))
      }
      if (signal?.aborted) return onAbort()
      signal?.addEventListener('abort', onAbort, { once: true })
    })
  )
}

const MAX_RETRIES = 3 // 429/529/5xx 的退避重试次数（不含首次）
const DEFAULT_TIMEOUT_MS = 120_000

/**
 * 调用远端 Jev 决策接口。
 * @param {RemoteConfig} config
 * @param {{ state: string, questions: Record<string, DecisionQuestionBody> }} input normalizeDecideInput 的输出
 * @param {{ signal?: AbortSignal, fetchImpl?: typeof fetch, timeoutMs?: number }} [ctx]
 * @returns {Promise<RemoteDecisionResult>}
 */
export async function callRemoteDecisions(config, input, ctx = {}) {
  const apiKey = String(config.apiKey || '').trim()
  const modelId = String(config.modelId || '').trim()
  if (!apiKey || !modelId) throw remoteFailure('config_missing')
  const url = remoteEndpoint(config)
  assertWithinRemoteLimit(input.state, input.questions)

  const fetchImpl = ctx.fetchImpl ?? fetch
  const body = JSON.stringify({
    model: modelId,
    state: input.state,
    questions: toRemoteQuestions(input.questions),
  })

  /** @type {DecisionError | null} */
  let lastError = null
  for (let attempt = 0; attempt <= MAX_RETRIES; attempt += 1) {
    if (attempt > 0) {
      // 指数退避 + 抖动，仅用于 429/529/5xx；确定性 4xx 不重试。
      await sleep(
        Math.min(8000, 500 * 2 ** (attempt - 1)) + Math.floor(Math.random() * 250),
        ctx.signal,
      )
    }
    const timeout = AbortSignal.timeout(ctx.timeoutMs ?? DEFAULT_TIMEOUT_MS)
    const signal = ctx.signal ? AbortSignal.any([ctx.signal, timeout]) : timeout

    /** @type {Response} */
    let response
    try {
      response = await fetchImpl(url, {
        method: 'POST',
        headers: {
          Authorization: `Bearer ${apiKey}`,
          'Content-Type': 'application/json',
        },
        body,
        signal,
      })
    } catch (cause) {
      if (ctx.signal?.aborted) throw remoteFailure('aborted', { retryable: false })
      const timeoutError =
        typeof cause === 'object' &&
        cause !== null &&
        'name' in cause &&
        cause.name === 'TimeoutError'
      lastError = remoteFailure(timeoutError ? 'timeout' : 'network', {
        statusCode: 502,
        retryable: true,
      })
      continue
    }

    const text = await response.text().catch(() => '')
    /** @type {any} */
    let json = null
    try {
      json = text ? JSON.parse(text) : null
    } catch {
      // 网关 HTML 等非 JSON 响应按状态码走下面的错误分支。
    }

    if (response.ok && json && typeof json === 'object') {
      return normalizeRemoteResponse(json)
    }

    const status = response.status
    if (status === 401 || status === 402 || status === 403) {
      throw remoteFailure('auth', { statusCode: 401 })
    }
    if (status === 413 || status === 422) {
      const code = status === 413 ? 'state_too_large' : 'invalid'
      throw remoteFailure(code, { statusCode: status === 413 ? 413 : 400 })
    }
    if (status === 429) {
      lastError = remoteFailure('rate_limited', { statusCode: 429, retryable: true })
      continue
    }
    if (status === 529 || status >= 500) {
      lastError = remoteFailure('overloaded', { statusCode: 502, retryable: true })
      continue
    }
    throw remoteFailure('bad_response', { statusCode: 502 })
  }

  throw lastError ?? remoteFailure('network', { statusCode: 502, retryable: true })
}

/**
 * 远端响应归一化为统一答案形状：
 * noul → { type, noul }；choice → { type, choice, probabilities, confidence }；
 * score → { type, score, legend, probabilities, confidence }。
 * @param {any} payload
 * @returns {RemoteDecisionResult}
 */
export function normalizeRemoteResponse(payload) {
  const rawAnswers = payload?.answers
  if (!isPlainObject(rawAnswers)) throw remoteFailure('bad_response', { statusCode: 502 })
  /** @type {Record<string, object>} */
  const answers = {}
  for (const [id, answer] of Object.entries(rawAnswers)) {
    if (!isPlainObject(answer)) throw remoteFailure('bad_response', { statusCode: 502 })
    if (typeof answer.noul === 'number') {
      answers[id] = { type: 'noul', noul: clamp01(answer.noul) }
    } else if (typeof answer.choice === 'string') {
      answers[id] = {
        type: 'choice',
        choice: answer.choice,
        probabilities: isPlainObject(answer.probabilities) ? answer.probabilities : {},
        confidence: typeof answer.confidence === 'number' ? clamp01(answer.confidence) : null,
      }
    } else if (typeof answer.score === 'number') {
      answers[id] = {
        type: 'score',
        score: answer.score,
        legend: isPlainObject(answer.legend) ? answer.legend : null,
        probabilities: isPlainObject(answer.probabilities) ? answer.probabilities : {},
        confidence: typeof answer.confidence === 'number' ? clamp01(answer.confidence) : null,
      }
    } else {
      throw remoteFailure('bad_response', { statusCode: 502 })
    }
  }
  const usage = isPlainObject(payload.usage) ? payload.usage : {}
  return {
    answers,
    model: typeof payload.model === 'string' ? payload.model : null,
    usage: {
      inputTokens: Number(usage.input_tokens ?? usage.inputTokens ?? 0) || 0,
      costUsd: typeof usage.cost === 'number' ? usage.cost : null,
    },
  }
}

/**
 * @param {number} value
 */
function clamp01(value) {
  return Math.min(1, Math.max(0, value))
}
