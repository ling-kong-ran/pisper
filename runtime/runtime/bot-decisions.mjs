// Bot 对话的“决定”请求：对话这一句说完后，拿同一份上下文快照再问一次“现在要做什么”，只要 JSON。
// 和旁问一样是一次无头请求：不建会话、不写会话文件、不带工具；系统提示与消息前缀与对话一致，便于命中提示缓存。
import { convertToLlm } from './pi-coding-agent.mjs'

export function botDecisionContext(session, instruction, now = Date.now()) {
  return {
    systemPrompt: session.systemPrompt || '',
    messages: [
      ...convertToLlm([...(session.messages || [])]),
      { role: 'user', content: [{ type: 'text', text: instruction }], timestamp: now },
    ],
    tools: [],
  }
}

const TIDY_INSTRUCTION = [
  'You keep a small memory for an assistant. Tidy the entries below:',
  '- merge entries that say the same thing into one (keep the clearest wording, use the id of one of them);',
  '- drop entries that are clearly outdated or contradicted by a newer entry;',
  '- keep everything else unchanged. Never invent new facts, never add entries.',
  'Respond with JSON only: {"kept":[{"id":"<existing id>","text":"<final text>"}]}.',
].join('\n')

// 每日整理记忆：一次无头请求，回来的清单只认已有的 id（服务里再校验一次）。
export async function requestBotMemoryTidy({ modelRuntime, model, entries, signal }) {
  if (!model) throw new Error('没有可用模型。')
  const listing = entries.map((entry) => `${entry.id}: ${entry.text}`).join('\n')
  const stream = modelRuntime.streamSimple(
    model,
    {
      systemPrompt: '',
      messages: [
        {
          role: 'user',
          content: [{ type: 'text', text: `${TIDY_INSTRUCTION}\n\n${listing}` }],
          timestamp: Date.now(),
        },
      ],
      tools: [],
    },
    { signal },
  )
  let text = ''
  for await (const event of stream) if (event.type === 'text_delta') text += event.delta
  const message = await stream.result()
  if (message.stopReason === 'error' || message.stopReason === 'aborted')
    return { kept: null, usage: message.usage || null }
  return { kept: parseTidy(text, entries), usage: message.usage || null }
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

/** @param {{ modelRuntime: any, session: any, instruction: string, signal?: AbortSignal }} input */
export async function requestBotDecisions({ modelRuntime, session, instruction, signal }) {
  const model = session.model
  if (!model) throw new Error('Bot 的对话还没有可用模型。')
  const stream = modelRuntime.streamSimple(model, botDecisionContext(session, instruction), {
    signal,
    sessionId: session.sessionId,
  })
  let text = ''
  for await (const event of stream) if (event.type === 'text_delta') text += event.delta
  const message = await stream.result()
  if (message.stopReason === 'aborted') throw new Error('决定请求已取消。')
  if (message.stopReason === 'error') throw new Error(message.errorMessage || '决定请求失败。')
  return { text, usage: message.usage || null }
}
