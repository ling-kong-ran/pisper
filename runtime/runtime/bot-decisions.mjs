// Bot 对话这一轮的来由（谁叫起来的、交回了哪些结果），以及复盘用的无头请求
// （不建会话、不写会话文件、不带工具）。
import { ATTACHMENT_MARKER, textFromContent } from './stream-projection.mjs'
import { pisperError } from '../../shared/errors.mjs'
import { isAgentCompletionMessage } from '../services/multi-agent-service.mjs'
import { parseBotMessageTag } from '../services/bots/bot-links.mjs'
import { isBotBriefMessage } from '../services/bots/bot-proactive.mjs'

// 这一轮是被什么叫起来的：看末尾回复之前的那几条用户消息。
// message：别的 Bot 说进来的话（来源标记只认附件上下文里的，用户正文里写的不算）；
// completedAgents：这一轮交回来的子 agent 结果（完成通知里的 id）；reply：这一轮最后说的话。
export function botTurnTrigger(messages) {
  let index = messages.length - 1
  const replyParts = []
  while (index >= 0 && messages[index]?.role !== 'user') {
    if (messages[index]?.role === 'assistant')
      replyParts.unshift(textFromContent(messages[index].content))
    index -= 1
  }
  let message = null
  let brief = false
  const completedAgents = []
  for (; index >= 0 && messages[index]?.role === 'user'; index -= 1) {
    const raw = textFromContent(messages[index].content)
    if (isBotBriefMessage(raw)) {
      brief = true
      continue
    }
    if (isAgentCompletionMessage(raw)) {
      for (const match of raw.matchAll(/\(([0-9a-f-]{36})\) has completed with status/g))
        completedAgents.push(match[1])
      continue
    }
    const at = raw.indexOf(ATTACHMENT_MARKER)
    if (!message && at >= 0) message = parseBotMessageTag(raw.slice(at))
  }
  // 最后一段有字的回复（工具调用之间的旁白不算）。
  return {
    message,
    brief,
    completedAgents,
    reply: (
      replyParts
        .map((part) => part.trim())
        .filter(Boolean)
        .at(-1) || ''
    ).trim(),
  }
}

// 一次不带上下文、不带工具的无头请求（复盘）：出错或取消时 text 为 null。
export async function requestBotText({ modelRuntime, model, text: prompt, signal }) {
  if (!model) throw pisperError('unavailable', 'model')
  const stream = modelRuntime.streamSimple(
    model,
    {
      systemPrompt: '',
      messages: [
        { role: 'user', content: [{ type: 'text', text: prompt }], timestamp: Date.now() },
      ],
      tools: [],
    },
    { signal },
  )
  let text = ''
  for await (const event of stream) if (event.type === 'text_delta') text += event.delta
  const message = await stream.result()
  if (message.stopReason === 'error' || message.stopReason === 'aborted')
    return { text: null, usage: message.usage || null }
  return { text, usage: message.usage || null }
}
