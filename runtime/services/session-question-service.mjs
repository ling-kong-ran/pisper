// 会话提问服务（ask_user，规格 8.4）：代理调用 ask_user 时登记一组待回答的问题，阻塞本次工具调用，
// 直到用户回答、跳过，或运行被停止 / 会话被释放。与审批服务同一套形状：待处理队列、短期已决缓存、
// 按会话挂载的事件发射器，以及给看板事件流的观察者（不依赖本轮是否挂着发射器）。
// 问题没有超时（与会话审批的产品语义一致：没人回答就一直等）；完全访问模式同样会问，问题不是权限。
import { randomUUID } from 'node:crypto'

export const QUESTION_LIMITS = Object.freeze({
  questions: 4,
  header: 24,
  question: 400,
  minOptions: 2,
  maxOptions: 6,
  label: 60,
  description: 200,
  other: 2000,
})
export const RESOLVED_QUESTION_TTL_MS = 5 * 60_000
export const MAX_RESOLVED_QUESTIONS = 256
const QUESTION_ID_PATTERN = /^q_[A-Za-z0-9-]{8,64}$/

// 输入校验错误：带稳定的 code，HTTP 层原样转成 400，工具层把消息交还给代理。
export class QuestionInputError extends Error {
  constructor(code, message) {
    super(message)
    this.name = 'QuestionInputError'
    this.code = code
  }
}

// eslint-disable-next-line no-control-regex
const CONTROL_PATTERN = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g

// 按码点截断并去掉控制字符，输出必须同时能被 JSON.parse 与 serde_json 解析。
function cleanText(value, limit) {
  if (typeof value !== 'string') return ''
  const cleaned = value.toWellFormed().replace(CONTROL_PATTERN, '').trim()
  const points = Array.from(cleaned)
  return points.length > limit ? points.slice(0, limit).join('') : cleaned
}

function record(value) {
  return value && typeof value === 'object' && !Array.isArray(value) ? value : {}
}

/**
 * 归一化代理传来的问题（不可信输入）。超长文本截断，结构不对直接报错，让代理改正后重试。
 * @param {unknown} input
 */
export function normalizeQuestions(input) {
  if (!Array.isArray(input) || input.length < 1)
    throw new QuestionInputError('questions_required', 'ask_user needs 1–4 questions.')
  if (input.length > QUESTION_LIMITS.questions)
    throw new QuestionInputError('too_many_questions', 'ask_user accepts at most 4 questions.')
  return input.map((raw, index) => {
    const item = record(raw)
    const question = cleanText(item.question, QUESTION_LIMITS.question)
    if (!question)
      throw new QuestionInputError('question_text_required', `Question ${index + 1} has no text.`)
    const header =
      cleanText(item.header, QUESTION_LIMITS.header) || cleanText(question, QUESTION_LIMITS.header)
    const rawOptions = Array.isArray(item.options) ? item.options : []
    if (
      rawOptions.length < QUESTION_LIMITS.minOptions ||
      rawOptions.length > QUESTION_LIMITS.maxOptions
    )
      throw new QuestionInputError(
        'option_count',
        `Question ${index + 1} needs 2–6 options (the user can always answer "Other" in free text).`,
      )
    const seen = new Set()
    const options = rawOptions.map((rawOption, optionIndex) => {
      const option = typeof rawOption === 'string' ? { label: rawOption } : record(rawOption)
      const label = cleanText(option.label, QUESTION_LIMITS.label)
      if (!label)
        throw new QuestionInputError(
          'option_label_required',
          `Question ${index + 1} option ${optionIndex + 1} has no label.`,
        )
      if (seen.has(label))
        throw new QuestionInputError(
          'duplicate_option',
          `Question ${index + 1} has duplicate option "${label}".`,
        )
      seen.add(label)
      const description = cleanText(option.description, QUESTION_LIMITS.description)
      return description ? { label, description } : { label }
    })
    return { header, question, options, multiSelect: item.multiSelect === true }
  })
}

/**
 * 校验用户的回答（来自 HTTP，不可信）。返回归一化后的回答，或抛 QuestionInputError。
 * 每个问题都必须有答案：至少选一项，或填写“其他”；单选题最多选一项。
 * @param {ReturnType<typeof normalizeQuestions>} questions
 * @param {unknown} input
 */
export function normalizeAnswers(questions, input) {
  const body = record(input)
  if (body.skip === true) return { skip: true }
  const answers = Array.isArray(body.answers) ? body.answers : null
  if (!answers || answers.length !== questions.length)
    throw new QuestionInputError('answers_mismatch', '回答数量与问题数量不一致。')
  return {
    skip: false,
    answers: answers.map((raw, index) => {
      const answer = record(raw)
      const question = questions[index]
      const labels = new Set(question.options.map((option) => option.label))
      const selectedInput = Array.isArray(answer.selected) ? answer.selected : []
      if (selectedInput.length > question.options.length)
        throw new QuestionInputError('invalid_selection', `第 ${index + 1} 题的选项无效。`)
      const selected = []
      for (const value of selectedInput) {
        if (typeof value !== 'string' || !labels.has(value) || selected.includes(value))
          throw new QuestionInputError('invalid_selection', `第 ${index + 1} 题的选项无效。`)
        selected.push(value)
      }
      if (!question.multiSelect && selected.length > 1)
        throw new QuestionInputError('single_select', `第 ${index + 1} 题只能选一项。`)
      if (
        typeof answer.other === 'string' &&
        Array.from(answer.other).length > QUESTION_LIMITS.other
      )
        throw new QuestionInputError('other_too_long', `第 ${index + 1} 题的补充回答太长。`)
      const other = cleanText(answer.other, QUESTION_LIMITS.other)
      if (!selected.length && !other)
        throw new QuestionInputError('answer_required', `第 ${index + 1} 题还没有回答。`)
      return other ? { selected, other } : { selected }
    }),
  }
}

export function isQuestionId(value) {
  return typeof value === 'string' && QUESTION_ID_PATTERN.test(value)
}

export class SessionQuestionService {
  constructor({ onEvent } = {}) {
    this.pending = new Map()
    this.resolved = new Map()
    this.emitters = new Map()
    // 看板事件流观察者：后台运行（没有挂前端流）的提问也要进“需要你”。
    this.onEvent = typeof onEvent === 'function' ? onEvent : null
  }

  attachEmitter(sessionId, emit) {
    if (emit) this.emitters.set(sessionId, emit)
  }

  detachEmitter(sessionId, emit) {
    if (!emit || this.emitters.get(sessionId) === emit) this.emitters.delete(sessionId)
  }

  emit(sessionId, event, data) {
    try {
      this.emitters.get(sessionId)?.(event, data)
    } catch {}
    try {
      this.onEvent?.(sessionId, event, data)
    } catch {}
  }

  pruneResolutions() {
    const cutoff = Date.now() - RESOLVED_QUESTION_TTL_MS
    for (const [id, value] of this.resolved) {
      if (new Date(value.resolvedAt).getTime() < cutoff) this.resolved.delete(id)
    }
  }

  rememberResolution(resolution) {
    this.pruneResolutions()
    this.resolved.set(resolution.questionId, resolution)
    while (this.resolved.size > MAX_RESOLVED_QUESTIONS)
      this.resolved.delete(this.resolved.keys().next().value)
  }

  /**
   * 登记一组问题并等待结果。questions 必须已经过 normalizeQuestions。
   * @returns {Promise<{ outcome: 'answered', answers: Array<{ selected: string[], other?: string }> } | { outcome: 'skipped' } | { outcome: 'cancelled', reason: string }>}
   */
  request({ sessionId, toolCallId = '', questions, signal }) {
    const questionId = `q_${randomUUID()}`
    const createdAt = new Date().toISOString()
    const publicQuestion = {
      id: questionId,
      questionId,
      sessionId,
      toolCallId: String(toolCallId || ''),
      questions,
      createdAt,
    }
    return new Promise((resolveQuestion) => {
      let settled = false
      const settle = (result) => {
        if (settled) return this.resolved.get(questionId)
        settled = true
        signal?.removeEventListener('abort', abort)
        this.pending.delete(questionId)
        const resolution = {
          id: questionId,
          questionId,
          sessionId,
          outcome: result.outcome,
          resolvedAt: new Date().toISOString(),
        }
        this.rememberResolution(resolution)
        this.emit(sessionId, 'question_resolved', resolution)
        resolveQuestion(result)
        return resolution
      }
      const abort = () => settle({ outcome: 'cancelled', reason: '运行已停止。' })
      if (signal?.aborted) {
        abort()
        return
      }
      signal?.addEventListener('abort', abort, { once: true })
      this.pending.set(questionId, { ...publicQuestion, settle })
      this.emit(sessionId, 'question_request', publicQuestion)
    })
  }

  /**
   * 回答或跳过。重复回答返回 alreadyResolved（HTTP 层转成 409 冲突）。
   * @param {string} sessionId @param {string} questionId @param {unknown} input
   */
  answer(sessionId, questionId, input) {
    const pending = this.pending.get(questionId)
    if (pending?.sessionId === sessionId) {
      const normalized = normalizeAnswers(pending.questions, input)
      const resolution = normalized.skip
        ? pending.settle({ outcome: 'skipped' })
        : pending.settle({ outcome: 'answered', answers: normalized.answers })
      return { found: true, alreadyResolved: false, ...resolution }
    }
    this.pruneResolutions()
    const previous = this.resolved.get(questionId)
    if (previous?.sessionId === sessionId)
      return { found: true, alreadyResolved: true, ...previous }
    return { found: false, alreadyResolved: false, id: questionId, questionId, sessionId }
  }

  // 停止运行、释放或删除会话：取消该会话全部待回答的问题。
  cancelSession(sessionId, reason = '运行已停止。') {
    let count = 0
    for (const question of [...this.pending.values()]) {
      if (question.sessionId !== sessionId) continue
      question.settle({ outcome: 'cancelled', reason })
      count += 1
    }
    return count
  }

  getPending(sessionId) {
    return [...this.pending.values()]
      .filter((question) => question.sessionId === sessionId)
      .map(({ settle: _settle, ...question }) => question)
  }

  dispose() {
    for (const question of [...this.pending.values()])
      question.settle({ outcome: 'cancelled', reason: '应用正在关闭。' })
    this.emitters.clear()
    this.resolved.clear()
  }
}

/**
 * 会话摘要与看板卡片里的 question 字段（只做加法）：取最早一组待回答的问题。
 * prompt / options / allowFreeText 对应规格 8.1 的单题形状，questions 是完整的 1–4 题。
 * @param {ReadonlyArray<{ id: string, questions: ReturnType<typeof normalizeQuestions>, createdAt: string }>} pending
 */
export function projectQuestionSummary(pending) {
  if (!Array.isArray(pending) || !pending.length) return null
  const first = [...pending].sort((left, right) => left.createdAt.localeCompare(right.createdAt))[0]
  const lead = first.questions[0]
  return {
    id: first.id,
    prompt: lead.question,
    header: lead.header,
    options: lead.options.map((option) => option.label),
    multiSelect: lead.multiSelect,
    allowFreeText: true,
    count: first.questions.length,
    createdAt: first.createdAt,
    questions: first.questions,
  }
}
