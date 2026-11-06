import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { QuestionInputError, normalizeQuestions } from '../../services/session-question-service.mjs'

// 提问工具（规格 8.4）：代理遇到需要用户拍板的分歧时，用 1–4 个带选项的问题问用户，
// 本次调用阻塞到用户回答、跳过或运行被停止。和 goal / plan 一样是内部运行时工具，不进插件目录；
// 没有人在场的运行（定时任务、工作流节点、子代理、外部 MCP 调用）不提供它。
export const ASK_USER_TOOL_NAME = 'ask_user'

const optionSchema = Type.Object({
  label: Type.String({ description: 'Button text (≤60 chars)' }),
  description: Type.Optional(Type.String({ description: 'One-line explanation (≤200 chars)' })),
})

// Bot 对话的提问工具（bot_ask）用同一份题目格式，卡片与回答的校验一致。
export const ASK_USER_QUESTION_SCHEMA = Type.Object({
  header: Type.Optional(
    Type.String({
      description: 'Short title (≤24 chars)',
    }),
  ),
  question: Type.String({
    description: 'Full question (≤400 chars)',
  }),
  options: Type.Array(optionSchema, {
    minItems: 2,
    maxItems: 6,
    description: 'Distinct choices. No "Other" option: the user can type an answer.',
  }),
  multiSelect: Type.Optional(Type.Boolean()),
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
      'Ask the user 1–4 multiple-choice questions and wait for the answers. They can also type an answer or skip.',
    promptGuidelines: [
      'Use ask_user only when you cannot proceed well without the user choosing between concrete alternatives (requirements, trade-offs, destructive choices), not for tool permission. Put related questions in one call and the recommended option first. If the user skips, decide on your own.',
    ],
    parameters: Type.Object({
      questions: Type.Array(ASK_USER_QUESTION_SCHEMA, { minItems: 1, maxItems: 4 }),
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
