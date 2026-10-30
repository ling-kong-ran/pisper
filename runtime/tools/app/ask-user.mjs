import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { QuestionInputError, normalizeQuestions } from '../../services/session-question-service.mjs'

// 提问工具（规格 8.4）：代理遇到需要用户拍板的分歧时，用 1–4 个带选项的问题问用户，
// 本次调用阻塞到用户回答、跳过或运行被停止。和 goal / plan 一样是内部运行时工具，不进插件目录；
// 没有人在场的运行（定时任务、工作流节点、子代理、外部 MCP 调用）不提供它。
export const ASK_USER_TOOL_NAME = 'ask_user'

const optionSchema = Type.Object({
  label: Type.String({ description: 'Short option text shown on a clickable button (≤60 chars).' }),
  description: Type.Optional(
    Type.String({ description: 'Optional one-line explanation of the option (≤200 chars).' }),
  ),
})

const questionSchema = Type.Object({
  header: Type.Optional(
    Type.String({
      description: 'Very short title for the question, e.g. "Auth method" (≤24 chars).',
    }),
  ),
  question: Type.String({
    description: 'The full question, ending with a question mark (≤400 chars).',
  }),
  options: Type.Array(optionSchema, {
    minItems: 2,
    maxItems: 6,
    description:
      '2–6 distinct choices. Do not add an "Other" option: the user can always answer in free text.',
  }),
  multiSelect: Type.Optional(
    Type.Boolean({ description: 'Allow selecting several options. Defaults to false.' }),
  ),
})

// 代理收到的结果：每题的已选项与自由文本；跳过或取消时明确告诉代理接下来该怎么做。
function resultText(questions, result) {
  if (result.outcome === 'skipped')
    return 'The user skipped these questions without answering. Decide on your own using your best judgement, or ask again in a different way if the decision truly needs them.'
  if (result.outcome === 'cancelled')
    return `The questions were cancelled before the user answered (${result.reason || 'run stopped'}). Do not assume any answer.`
  return JSON.stringify(
    {
      outcome: 'answered',
      answers: questions.map((question, index) => ({
        question: question.question,
        selected: result.answers[index]?.selected || [],
        ...(result.answers[index]?.other ? { other: result.answers[index].other } : {}),
      })),
    },
    null,
    2,
  )
}

export function createAskUserTool({ requestAnswers, isAvailable = () => true }) {
  return defineTool({
    name: ASK_USER_TOOL_NAME,
    label: 'Ask User',
    description:
      'Ask the user 1–4 multiple-choice questions and wait for the answers. Each question has 2–6 clickable options; the user may also answer in free text or skip.',
    promptSnippet:
      'Ask the user clickable multiple-choice questions when a decision genuinely needs them',
    promptGuidelines: [
      'Use ask_user only when you cannot proceed well without the user choosing between concrete alternatives (requirements, trade-offs, destructive choices). Do not use it for permission to run tools; approvals are handled separately.',
      'Batch related questions into one call (at most 4). Keep options short, distinct and mutually exclusive unless multiSelect is true. Put the recommended option first.',
      'The call blocks until the user answers, skips, or stops the run. If the user skips, decide on your own.',
    ],
    parameters: Type.Object({
      questions: Type.Array(questionSchema, { minItems: 1, maxItems: 4 }),
    }),
    async execute(toolCallId, params, signal) {
      if (!isAvailable())
        return {
          content: [
            {
              type: 'text',
              text: 'ask_user is not available in this run because no user is present to answer. Decide on your own.',
            },
          ],
          isError: true,
        }
      let questions
      try {
        questions = normalizeQuestions(params?.questions)
      } catch (error) {
        if (!(error instanceof QuestionInputError)) throw error
        return { content: [{ type: 'text', text: error.message }], isError: true }
      }
      const result = await requestAnswers({ toolCallId, questions, signal })
      return {
        content: [{ type: 'text', text: resultText(questions, result) }],
        details: {
          outcome: result.outcome,
          questions,
          ...(result.answers ? { answers: result.answers } : {}),
        },
      }
    },
  })
}
