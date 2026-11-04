// Bot 对话这一轮的来由（谁叫起来的、交回了哪些结果），以及整理记忆、复盘用的无头请求
// （不建会话、不写会话文件、不带工具）。
import { ATTACHMENT_MARKER, textFromContent } from './stream-projection.mjs'
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

const TIDY_INSTRUCTION = [
  'You keep a small memory for an assistant. Tidy the entries below:',
  '- merge entries that say the same thing into one (keep the clearest wording, use the id of one of them);',
  '- drop entries that are clearly outdated or contradicted by a newer entry;',
  '- keep everything else unchanged. Never invent new facts, never add entries.',
  'Respond with JSON only: {"kept":[{"id":"<existing id>","text":"<final text>"}]}.',
].join('\n')

// 一次不带上下文、不带工具的无头请求（整理记忆、复盘）：出错或取消时 text 为 null。
export async function requestBotText({ modelRuntime, model, text: prompt, signal }) {
  if (!model) throw new Error('没有可用模型。')
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

// 每日整理记忆：一次无头请求，回来的清单只认已有的 id（服务里再校验一次）。
export async function requestBotMemoryTidy({ modelRuntime, model, entries, signal }) {
  const listing = entries.map((entry) => `${entry.id}: ${entry.text}`).join('\n')
  const result = await requestBotText({
    modelRuntime,
    model,
    text: `${TIDY_INSTRUCTION}\n\n${listing}`,
    signal,
  })
  return {
    kept: result.text === null ? null : parseTidy(result.text, entries),
    usage: result.usage,
  }
}

// 解析整理结果：只认已有 id、不重复；解析不出来或一条都不剩（多半是出错）时当作不整理。
export function parseTidy(raw, entries) {
  const source = String(raw || '')
  const start = source.indexOf('{')
  const end = source.lastIndexOf('}')
  if (start < 0 || end <= start) return null
  let parsed
  try {
    parsed = JSON.parse(source.slice(start, end + 1))
  } catch {
    return null
  }
  const known = new Set(entries.map((entry) => entry.id))
  const seen = new Set()
  const kept = []
  for (const item of Array.isArray(parsed?.kept) ? parsed.kept : []) {
    const id = String(item?.id || '')
    if (!known.has(id) || seen.has(id)) continue
    seen.add(id)
    kept.push({ id, text: String(item?.text ?? '') })
  }
  return kept.length ? kept : null
}
