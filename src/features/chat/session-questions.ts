// @public 代理的提问（ask_user，规格 v2 第 8.4 节，能力 askUser）：Runtime 发来的待回答问题按不可信输入解析、限长，
// 会话页的问题卡、全部代理行内点选与侧栏原因共用这一份结构。一次提问带 1–4 个问题，每个问题 2–6 个选项，
// 都允许“其他”自由回答；单个问题且单选时点一下即提交。
export type QuestionOption = { label: string; description: string }
export type QuestionItem = {
  header: string
  question: string
  options: QuestionOption[]
  multiSelect: boolean
}
export type PendingQuestion = { id: string; questions: QuestionItem[]; createdAt: string }
export type QuestionAnswer = { selected: string[]; other?: string }

// 与 Runtime 的 QUESTION_LIMITS 一致：界面兜底截断，不信任超长字段。
export const QUESTION_UI_LIMITS = Object.freeze({
  questions: 4,
  header: 24,
  question: 400,
  options: 6,
  label: 60,
  description: 200,
  other: 2000,
})
const QUESTION_ID = /^q_[A-Za-z0-9-]{8,64}$/

function text(value: unknown, limit: number) {
  if (typeof value !== 'string') return ''
  const points = Array.from(value.trim())
  return points.length > limit ? points.slice(0, limit).join('') : points.join('')
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

function parseItem(raw: unknown): QuestionItem | null {
  const value = record(raw)
  if (!value) return null
  const question = text(value.question, QUESTION_UI_LIMITS.question)
  const options: QuestionOption[] = []
  const seen = new Set<string>()
  for (const entry of Array.isArray(value.options) ? value.options : []) {
    const option = record(entry)
    const label = text(option ? option.label : entry, QUESTION_UI_LIMITS.label)
    if (!label || seen.has(label)) continue
    seen.add(label)
    options.push({
      label,
      description: text(option?.description, QUESTION_UI_LIMITS.description),
    })
    if (options.length >= QUESTION_UI_LIMITS.options) break
  }
  if (!question || options.length < 2) return null
  return {
    header: text(value.header, QUESTION_UI_LIMITS.header),
    question,
    options,
    multiSelect: value.multiSelect === true,
  }
}

// 会话摘要里的 question（{ id, questions, createdAt, … }）与 /live、question_request 事件里的待回答问题同一形状。
export function parsePendingQuestion(raw: unknown): PendingQuestion | null {
  const value = record(raw)
  if (!value) return null
  const id =
    typeof value.id === 'string'
      ? value.id
      : typeof value.questionId === 'string'
        ? value.questionId
        : ''
  if (!QUESTION_ID.test(id)) return null
  const questions = (Array.isArray(value.questions) ? value.questions : [])
    .slice(0, QUESTION_UI_LIMITS.questions)
    .map(parseItem)
  if (!questions.length || questions.some((item) => !item)) return null
  const createdAt = typeof value.createdAt === 'string' ? value.createdAt : ''
  return { id, questions: questions as QuestionItem[], createdAt }
}

export function parsePendingQuestions(raw: unknown): PendingQuestion[] {
  if (!Array.isArray(raw)) return []
  const seen = new Set<string>()
  const list: PendingQuestion[] = []
  for (const entry of raw) {
    const question = parsePendingQuestion(entry)
    if (!question || seen.has(question.id)) continue
    seen.add(question.id)
    list.push(question)
  }
  return list.sort((left, right) => left.createdAt.localeCompare(right.createdAt))
}

// 全部代理行内点选（规格 3.3）：只有一题、单选、选项 ≤4 时，点选项即提交；其余“打开处理”。
export function inlineQuestionOptions(question: PendingQuestion | null): string[] | null {
  if (!question || question.questions.length !== 1) return null
  const [item] = question.questions
  if (item.multiSelect || item.options.length > 4) return null
  return item.options.map((option) => option.label)
}

// 每个问题都有答案（选了至少一项，或写了“其他”）才能提交。
export function answersComplete(question: PendingQuestion, answers: readonly QuestionAnswer[]) {
  return question.questions.every((_, index) => {
    const answer = answers[index]
    return Boolean(answer && (answer.selected.length > 0 || answer.other?.trim()))
  })
}
