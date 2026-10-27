// 输出上限续跑：模型因 max output tokens 停在 stopReason "length" 时，Pi 只对
// 「输出量低于模型 maxTokens」的可恢复截断做一次压缩重试。真正打满输出上限时
// _checkCompaction 返回 false，agent 循环结束，任务停在半截回复上。
// 这里在 Pi 明确不再重试之后补一轮隐藏续跑，而不是再终止任务。
const OUTPUT_LIMIT_CONTINUATION_PATCH = Symbol('pisper.output-limit-continuation')

export const OUTPUT_LIMIT_CONTINUATION_MARKER = '[Pisper internal output limit continuation]'
// 连续截断上限：避免模型每次都写满输出上限时无限续跑。
export const MAX_OUTPUT_LIMIT_CONTINUATIONS = 3

export function isOutputLimitContinuationMessage(content) {
  return String(content || '').startsWith(OUTPUT_LIMIT_CONTINUATION_MARKER)
}

export function outputLimitContinuationPrompt(
  attempt,
  maxAttempts = MAX_OUTPUT_LIMIT_CONTINUATIONS,
) {
  return `${OUTPUT_LIMIT_CONTINUATION_MARKER}
The previous assistant response was cut off because it reached the model's maximum output token limit (${attempt}/${maxAttempts}).
Continue exactly where that response stopped. Finish the interrupted content or tool call, then keep working on the original task.
Keep each next response shorter: split large writes and long explanations into smaller steps.
Do not repeat work that already completed, and do not treat this continuation as a new user request.`
}

function messageText(message) {
  const content = message?.content
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content
    .filter((part) => part?.type === 'text')
    .map((part) => part.text || '')
    .join('')
}

function enqueueOutputLimitContinuation(session, attempt) {
  const agent = session?.agent
  if (typeof agent?.followUp !== 'function') return false
  // 用户或目标延续已经排队时，本轮会由既有队列继续，不再叠一条截断续跑。
  if (typeof agent.hasQueuedMessages === 'function' && agent.hasQueuedMessages()) return false
  agent.followUp({
    role: 'user',
    content: [{ type: 'text', text: outputLimitContinuationPrompt(attempt) }],
    timestamp: Date.now(),
  })
  return true
}

// 给会话装上输出上限续跑。只拦截回合收尾的压缩检查；
// 回合中的压缩检查和发送前检查不能插入续跑。
export function installOutputLimitContinuation(session) {
  if (!session || session[OUTPUT_LIMIT_CONTINUATION_PATCH]) return session
  const state = { consecutive: 0, postAgentRuns: 0 }

  // Pi 1.4 的收尾流程不再显式传 true；仅在真实回合收尾内识别默认参数，
  // 不能放宽发送前和工具回合中的压缩检查。
  if (typeof session._handlePostAgentRun === 'function') {
    const handlePostAgentRun = session._handlePostAgentRun.bind(session)
    session._handlePostAgentRun = async function handlePostAgentRunWithOutputLimit(...args) {
      state.postAgentRuns += 1
      try {
        return await handlePostAgentRun(...args)
      } finally {
        state.postAgentRuns -= 1
      }
    }
  }

  if (typeof session.subscribe === 'function') {
    session.subscribe((event) => {
      if (event?.type !== 'message_start' || event.message?.role !== 'user') return
      if (!isOutputLimitContinuationMessage(messageText(event.message))) state.consecutive = 0
    })
  }

  if (typeof session._checkCompaction === 'function') {
    const checkCompaction = session._checkCompaction.bind(session)
    session._checkCompaction = async function checkCompactionWithOutputLimitContinuation(...args) {
      const result = await checkCompaction(...args)
      // 兼容旧版显式标志和新版收尾上下文；提前压缩和发送前检查必须保持原行为。
      if (args[1] !== true && !(args[1] === undefined && state.postAgentRuns > 0)) return result
      const message = args[0]
      if (message?.stopReason !== 'length') {
        state.consecutive = 0
        return result
      }
      // Pi 已经决定压缩并重试可恢复截断时，不要再塞一条续跑提示。
      if (result) return result
      if (state.consecutive >= MAX_OUTPUT_LIMIT_CONTINUATIONS) return false
      const attempt = state.consecutive + 1
      if (!enqueueOutputLimitContinuation(session, attempt)) return false
      state.consecutive = attempt
      return true
    }
  }

  session[OUTPUT_LIMIT_CONTINUATION_PATCH] = true
  return session
}
