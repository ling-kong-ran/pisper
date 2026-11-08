import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { QuestionInputError, normalizeQuestions } from '../../services/session-question-service.mjs'
import { ASK_USER, ASK_USER_QUESTION } from '../../prompts/tools/ask-user.mjs'

// 提问工具（规格 8.4）：代理遇到需要用户拍板的分歧时，用 1–4 个带选项的问题问用户，
// 本次调用阻塞到用户回答、跳过或运行被停止。和 goal / plan 一样是内部运行时工具，不进插件目录；
// 没有人在场的运行（定时任务、工作流节点、子代理、外部 MCP 调用）不提供它。
export const ASK_USER_TOOL_NAME = 'ask_user'

const optionSchema = Type.Object({
  label: Type.String({ description: ASK_USER_QUESTION.optionLabel }),
  description: Type.Optional(Type.String({ description: ASK_USER_QUESTION.optionDescription })),
})

// Bot 对话的提问工具（bot_ask）用同一份题目格式，卡片与回答的校验一致。
export const ASK_USER_QUESTION_SCHEMA = Type.Object({
  header: Type.Optional(Type.String({ description: ASK_USER_QUESTION.header })),
  question: Type.String({ description: ASK_USER_QUESTION.question }),
  options: Type.Array(optionSchema, {
    minItems: 2,
    maxItems: 6,
    description: ASK_USER_QUESTION.options,
  }),
  multiSelect: Type.Optional(Type.Boolean()),
})

// 代理收到的结果：每题的已选项与自由文本；跳过或取消时明确告诉代理接下来该怎么做。
function resultText(questions, result) {
  if (result.outcome === 'skipped') return ASK_USER.skipped
  if (result.outcome === 'cancelled') return ASK_USER.cancelled(result.reason)
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
    description: ASK_USER.description,
    promptGuidelines: ASK_USER.guidelines,
    parameters: Type.Object({
      questions: Type.Array(ASK_USER_QUESTION_SCHEMA, { minItems: 1, maxItems: 4 }),
    }),
    async execute(toolCallId, params, signal) {
      if (!isAvailable())
        return {
          content: [{ type: 'text', text: ASK_USER.unavailable }],
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
