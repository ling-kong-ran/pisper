// 旁问（/btw）：带着当前会话的完整上下文问一个临时问题，只发一次无头模型请求。
// 不建会话、不写会话文件、不改主会话的消息和状态，主会话运行中也能问；回答只回给提问的客户端。
// 系统提示词、工具列表和消息前缀与主会话保持一致，便于命中服务商的提示缓存；
// 工具定义只为保持缓存前缀，旁问本身不执行任何工具，因此不需要经过审批链路。
import { convertToLlm } from './pi-coding-agent.mjs'

export const SIDE_QUESTION_MAX_CHARS = 4000

const SIDE_QUESTION_INSTRUCTION = [
  'The user is asking a quick side question while the main task continues separately.',
  'Answer it directly from the conversation above and what you already know.',
  'Do not call any tools, do not continue or change the main task, and keep the answer brief.',
  "Reply in the user's language.",
].join(' ')

/** @param {unknown} value */
export function normalizeSideQuestion(value) {
  const question = typeof value === 'string' ? value.trim() : ''
  if (!question) throw new Error('旁问内容不能为空。')
  if (question.length > SIDE_QUESTION_MAX_CHARS)
    throw new Error(`旁问不能超过 ${SIDE_QUESTION_MAX_CHARS} 个字符。`)
  return question
}

// 只读快照：复制消息数组，之后主会话继续追加消息也不会影响这次请求。
export function sideQuestionContext(session, question, now = Date.now()) {
  const tools = Array.isArray(session.agent?.state?.tools) ? session.agent.state.tools : []
  return {
    systemPrompt: session.systemPrompt || '',
    messages: [
      ...convertToLlm([...(session.messages || [])]),
      {
        role: 'user',
        content: [
          {
            type: 'text',
            text: `<side_question>\n${question}\n</side_question>\n\n${SIDE_QUESTION_INSTRUCTION}`,
          },
        ],
        timestamp: now,
      },
    ],
    tools: tools.map(({ name, description, parameters }) => ({ name, description, parameters })),
  }
}

/**
 * @param {{ modelRuntime: any, session: any, question: string, signal?: AbortSignal, onDelta?: (text: string) => void }} input
 */
export async function streamSideQuestion({ modelRuntime, session, question, signal, onDelta }) {
  const model = session.model
  if (!model) throw new Error('当前会话还没有可用模型，请先选择模型。')
  const level = session.thinkingLevel
  const stream = modelRuntime.streamSimple(model, sideQuestionContext(session, question), {
    ...(level && level !== 'off' ? { reasoning: level } : {}),
    signal,
    // 与主会话同一个会话标识：按会话路由提示缓存的服务商可以复用前缀。
    sessionId: session.sessionId,
  })
  let text = ''
  let toolCallIgnored = false
  for await (const event of stream) {
    if (event.type === 'text_delta') {
      text += event.delta
      onDelta?.(event.delta)
    } else if (event.type === 'toolcall_start') {
      // 模型仍想调用工具时不执行，只把已经写出的文字交回去。
      toolCallIgnored = true
    }
  }
  const message = await stream.result()
  if (message.stopReason === 'aborted') throw new Error('旁问已取消。')
  if (message.stopReason === 'error') throw new Error(message.errorMessage || '旁问请求失败。')
  return { text, toolCallIgnored, usage: message.usage || null }
}
